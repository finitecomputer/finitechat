use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use cgka_traits::transport::{TransportEnvelope, TransportMessage};
use cgka_traits::{GroupId, MemberId, MessageId};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use transport_http_server::{
    HttpClaimedKeyPackage, HttpDeliveryService, HttpKeyPackagePublication, HttpPublishReceipt,
    HttpPublishTarget, HttpSequence, HttpServerError, HttpSyncPage, MAX_HTTP_SYNC_PAGE_ENTRIES,
};

#[derive(Clone, Debug, Default)]
pub struct HttpServerState {
    service: Arc<Mutex<HttpDeliveryService>>,
    publish_idempotency: Arc<Mutex<HashMap<String, PublishIdempotencyRecord>>>,
    welcome_claims: Arc<Mutex<HashMap<MessageId, WelcomeClaimRecord>>>,
    store: Option<Arc<SqliteHttpDeliveryStore>>,
}

impl HttpServerState {
    pub fn new(service: HttpDeliveryService) -> Self {
        Self {
            service: Arc::new(Mutex::new(service)),
            publish_idempotency: Arc::new(Mutex::new(HashMap::new())),
            welcome_claims: Arc::new(Mutex::new(HashMap::new())),
            store: None,
        }
    }

    pub fn from_sqlite_path(path: impl AsRef<Path>) -> Result<Self, DurableStoreError> {
        let store = Arc::new(SqliteHttpDeliveryStore::open(path)?);
        let mut service = HttpDeliveryService::default();
        for operation in store.load_operations()? {
            replay_operation(&mut service, operation)?;
        }
        let publish_idempotency = store.load_publish_idempotency()?;
        let welcome_claims = store.load_welcome_claims()?;
        Ok(Self {
            service: Arc::new(Mutex::new(service)),
            publish_idempotency: Arc::new(Mutex::new(publish_idempotency)),
            welcome_claims: Arc::new(Mutex::new(welcome_claims)),
            store: Some(store),
        })
    }

    fn apply_mutation<R>(
        &self,
        mutation: impl FnOnce(
            &mut HttpDeliveryService,
        ) -> Result<(R, Option<PersistedOperation>), HttpServerError>,
    ) -> Result<R, ServerHttpError> {
        let mut service = self.service.lock().expect("HTTP delivery service mutex");
        let Some(store) = &self.store else {
            let (result, _) = mutation(&mut service)?;
            return Ok(result);
        };

        let mut candidate = service.clone();
        let (result, operation) = mutation(&mut candidate)?;
        if let Some(operation) = operation {
            store.append_operation(&operation)?;
        }
        *service = candidate;
        Ok(result)
    }

    fn publish_message(
        &self,
        request: PublishMessageRequest,
    ) -> Result<HttpPublishReceipt, ServerHttpError> {
        let Some(idempotency_key) = request.idempotency_key.clone() else {
            return self.apply_mutation(|service| {
                let receipt = service.publish(request.target.clone(), request.message.clone())?;
                let operation =
                    (!receipt.duplicate).then_some(PersistedOperation::PublishMessage {
                        target: request.target,
                        message: request.message,
                        idempotency_key: None,
                    });
                Ok((receipt, operation))
            });
        };

        if idempotency_key.is_empty() {
            return Err(ServerHttpError::InvalidIdempotencyKey);
        }

        let fingerprint = PublishMessageFingerprint::from_request(&request);
        let mut service = self.service.lock().expect("HTTP delivery service mutex");
        let mut idempotency = self
            .publish_idempotency
            .lock()
            .expect("HTTP publish idempotency mutex");
        if let Some(record) = idempotency.get(&idempotency_key) {
            if record.fingerprint == fingerprint {
                return Ok(record.receipt.clone());
            }
            return Err(ServerHttpError::IdempotencyConflict { idempotency_key });
        }

        let mut candidate = service.clone();
        let receipt = candidate.publish(request.target.clone(), request.message.clone())?;
        let operation = (!receipt.duplicate).then_some(PersistedOperation::PublishMessage {
            target: request.target,
            message: request.message,
            idempotency_key: Some(idempotency_key.clone()),
        });
        let record = PublishIdempotencyRecord {
            fingerprint,
            receipt: receipt.clone(),
        };
        if let Some(store) = &self.store {
            store.append_publish_mutation(operation.as_ref(), Some((&idempotency_key, &record)))?;
        }
        *service = candidate;
        idempotency.insert(idempotency_key, record);
        Ok(receipt)
    }

    fn claim_welcomes(
        &self,
        request: ClaimWelcomesRequest,
    ) -> Result<Vec<HttpClaimedWelcome>, ServerHttpError> {
        if request.limit == 0 || request.limit > MAX_HTTP_SYNC_PAGE_ENTRIES {
            return Err(ServerHttpError::InvalidWelcomeClaimLimit {
                actual: request.limit,
                max: MAX_HTTP_SYNC_PAGE_ENTRIES,
            });
        }

        let service = self.service.lock().expect("HTTP delivery service mutex");
        let mut claims = self
            .welcome_claims
            .lock()
            .expect("HTTP welcome claims mutex");
        let mut claimed = Vec::new();
        let mut after_seq = 0;
        loop {
            let page =
                service.sync_inbox(&request.recipient, after_seq, MAX_HTTP_SYNC_PAGE_ENTRIES)?;
            for entry in page.entries {
                if claimed.len() >= request.limit {
                    break;
                }
                if !matches!(entry.message.envelope, TransportEnvelope::Welcome { .. }) {
                    continue;
                }
                if claims.contains_key(&entry.message.id) {
                    continue;
                }
                let record = WelcomeClaimRecord {
                    recipient: request.recipient.clone(),
                    seq: entry.seq,
                    message: entry.message,
                    state: WelcomeClaimState::Claimed,
                };
                if let Some(store) = &self.store {
                    store.upsert_welcome_claim(&record)?;
                }
                claims.insert(record.message.id.clone(), record.clone());
                claimed.push(record.into_claimed_welcome());
            }
            if claimed.len() >= request.limit || !page.has_more {
                break;
            }
            after_seq = page.next_after_seq;
        }
        Ok(claimed)
    }

    fn ack_welcome(
        &self,
        request: AckWelcomeRequest,
    ) -> Result<AckWelcomeResponse, ServerHttpError> {
        let mut claims = self
            .welcome_claims
            .lock()
            .expect("HTTP welcome claims mutex");
        let Some(record) = claims.get_mut(&request.message_id) else {
            return Err(ServerHttpError::WelcomeNotFound {
                message_id: request.message_id,
            });
        };
        let terminal_state = if request.activated {
            WelcomeClaimState::Acked
        } else {
            WelcomeClaimState::Failed
        };
        match (record.state, terminal_state) {
            (WelcomeClaimState::Claimed, _) => {
                record.state = terminal_state;
                if let Some(store) = &self.store {
                    store.upsert_welcome_claim(record)?;
                }
                Ok(AckWelcomeResponse { acked: true })
            }
            (current, wanted) if current == wanted => Ok(AckWelcomeResponse { acked: true }),
            (current, wanted) => Err(ServerHttpError::WelcomeAckConflict {
                message_id: request.message_id,
                current,
                wanted,
            }),
        }
    }
}

pub fn http_router(state: HttpServerState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/messages", post(publish_message))
        .route("/sync/group", post(sync_group))
        .route("/sync/inbox", post(sync_inbox))
        .route("/key-packages", post(publish_key_package))
        .route("/key-packages/claim", post(claim_key_package))
        .route("/welcomes/claim", post(claim_welcomes))
        .route("/welcomes/ack", post(ack_welcome))
        .with_state(state)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishMessageRequest {
    pub target: HttpPublishTarget,
    pub message: cgka_traits::transport::TransportMessage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupSyncRequest {
    pub group_id: GroupId,
    pub after_seq: HttpSequence,
    pub limit: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxSyncRequest {
    pub recipient: MemberId,
    pub after_seq: HttpSequence,
    pub limit: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimKeyPackageRequest {
    pub owner: MemberId,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimWelcomesRequest {
    pub recipient: MemberId,
    pub limit: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpClaimedWelcome {
    pub seq: HttpSequence,
    pub message: TransportMessage,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AckWelcomeRequest {
    pub message_id: MessageId,
    pub activated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AckWelcomeResponse {
    pub acked: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishKeyPackageResponse {
    pub published: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub kind: String,
    pub error: String,
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_owned(),
    })
}

async fn publish_message(
    State(state): State<HttpServerState>,
    Json(request): Json<PublishMessageRequest>,
) -> Result<Json<HttpPublishReceipt>, ServerHttpError> {
    let receipt = state.publish_message(request)?;
    Ok(Json(receipt))
}

async fn sync_group(
    State(state): State<HttpServerState>,
    Json(request): Json<GroupSyncRequest>,
) -> Result<Json<HttpSyncPage>, ServerHttpError> {
    let service = state.service.lock().expect("HTTP delivery service mutex");
    let page = service.sync_group(&request.group_id, request.after_seq, request.limit)?;
    Ok(Json(page))
}

async fn sync_inbox(
    State(state): State<HttpServerState>,
    Json(request): Json<InboxSyncRequest>,
) -> Result<Json<HttpSyncPage>, ServerHttpError> {
    let service = state.service.lock().expect("HTTP delivery service mutex");
    let page = service.sync_inbox(&request.recipient, request.after_seq, request.limit)?;
    Ok(Json(page))
}

async fn publish_key_package(
    State(state): State<HttpServerState>,
    Json(publication): Json<HttpKeyPackagePublication>,
) -> Result<Json<PublishKeyPackageResponse>, ServerHttpError> {
    state.apply_mutation(|service| {
        service.publish_key_package(publication.clone())?;
        Ok((
            PublishKeyPackageResponse { published: true },
            Some(PersistedOperation::PublishKeyPackage { publication }),
        ))
    })?;
    Ok(Json(PublishKeyPackageResponse { published: true }))
}

async fn claim_key_package(
    State(state): State<HttpServerState>,
    Json(request): Json<ClaimKeyPackageRequest>,
) -> Result<Json<Option<HttpClaimedKeyPackage>>, ServerHttpError> {
    let claimed = state.apply_mutation(|service| {
        let claimed = service.claim_key_package(&request.owner)?;
        let operation = claimed
            .is_some()
            .then_some(PersistedOperation::ClaimKeyPackage {
                owner: request.owner,
            });
        Ok((claimed, operation))
    })?;
    Ok(Json(claimed))
}

async fn claim_welcomes(
    State(state): State<HttpServerState>,
    Json(request): Json<ClaimWelcomesRequest>,
) -> Result<Json<Vec<HttpClaimedWelcome>>, ServerHttpError> {
    let claimed = state.claim_welcomes(request)?;
    Ok(Json(claimed))
}

async fn ack_welcome(
    State(state): State<HttpServerState>,
    Json(request): Json<AckWelcomeRequest>,
) -> Result<Json<AckWelcomeResponse>, ServerHttpError> {
    let acked = state.ack_welcome(request)?;
    Ok(Json(acked))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum PersistedOperation {
    PublishMessage {
        target: HttpPublishTarget,
        message: cgka_traits::transport::TransportMessage,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        idempotency_key: Option<String>,
    },
    PublishKeyPackage {
        publication: HttpKeyPackagePublication,
    },
    ClaimKeyPackage {
        owner: MemberId,
    },
}

impl PersistedOperation {
    fn kind(&self) -> &'static str {
        match self {
            Self::PublishMessage { .. } => "publish_message",
            Self::PublishKeyPackage { .. } => "publish_key_package",
            Self::ClaimKeyPackage { .. } => "claim_key_package",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct PublishMessageFingerprint {
    target: HttpPublishTarget,
    message: cgka_traits::transport::TransportMessage,
}

impl PublishMessageFingerprint {
    fn from_request(request: &PublishMessageRequest) -> Self {
        Self {
            target: request.target.clone(),
            message: request.message.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct PublishIdempotencyRecord {
    fingerprint: PublishMessageFingerprint,
    receipt: HttpPublishReceipt,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct WelcomeClaimRecord {
    recipient: MemberId,
    seq: HttpSequence,
    message: TransportMessage,
    state: WelcomeClaimState,
}

impl WelcomeClaimRecord {
    fn into_claimed_welcome(self) -> HttpClaimedWelcome {
        HttpClaimedWelcome {
            seq: self.seq,
            message: self.message,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WelcomeClaimState {
    Claimed,
    Acked,
    Failed,
}

#[derive(Clone, Debug)]
struct SqliteHttpDeliveryStore {
    path: Arc<PathBuf>,
}

impl SqliteHttpDeliveryStore {
    fn open(path: impl AsRef<Path>) -> Result<Self, rusqlite::Error> {
        let store = Self {
            path: Arc::new(path.as_ref().to_owned()),
        };
        let conn = store.connection()?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS http_delivery_ops (
                seq INTEGER PRIMARY KEY AUTOINCREMENT,
                kind TEXT NOT NULL,
                body_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS http_publish_idempotency (
                idempotency_key TEXT PRIMARY KEY,
                fingerprint_json TEXT NOT NULL,
                receipt_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS http_welcome_claims (
                message_id_json TEXT PRIMARY KEY,
                recipient_json TEXT NOT NULL,
                seq INTEGER NOT NULL,
                message_json TEXT NOT NULL,
                state_json TEXT NOT NULL
            );",
        )?;
        Ok(store)
    }

    fn append_operation(&self, operation: &PersistedOperation) -> Result<(), DurableStoreError> {
        let body_json = serde_json::to_string(operation)?;
        let conn = self.connection()?;
        conn.execute(
            "INSERT INTO http_delivery_ops (kind, body_json) VALUES (?1, ?2)",
            params![operation.kind(), body_json],
        )?;
        Ok(())
    }

    fn append_publish_mutation(
        &self,
        operation: Option<&PersistedOperation>,
        idempotency: Option<(&str, &PublishIdempotencyRecord)>,
    ) -> Result<(), DurableStoreError> {
        let mut conn = self.connection()?;
        let transaction = conn.transaction()?;
        if let Some(operation) = operation {
            transaction.execute(
                "INSERT INTO http_delivery_ops (kind, body_json) VALUES (?1, ?2)",
                params![operation.kind(), serde_json::to_string(operation)?],
            )?;
        }
        if let Some((idempotency_key, record)) = idempotency {
            transaction.execute(
                "INSERT INTO http_publish_idempotency (
                    idempotency_key,
                    fingerprint_json,
                    receipt_json
                ) VALUES (?1, ?2, ?3)",
                params![
                    idempotency_key,
                    serde_json::to_string(&record.fingerprint)?,
                    serde_json::to_string(&record.receipt)?,
                ],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    fn load_operations(&self) -> Result<Vec<PersistedOperation>, DurableStoreError> {
        let conn = self.connection()?;
        let mut statement =
            conn.prepare("SELECT body_json FROM http_delivery_ops ORDER BY seq ASC")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        let mut operations = Vec::new();
        for row in rows {
            operations.push(serde_json::from_str(&row?)?);
        }
        Ok(operations)
    }

    fn load_publish_idempotency(
        &self,
    ) -> Result<HashMap<String, PublishIdempotencyRecord>, DurableStoreError> {
        let conn = self.connection()?;
        let mut statement = conn.prepare(
            "SELECT idempotency_key, fingerprint_json, receipt_json FROM http_publish_idempotency",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut idempotency = HashMap::new();
        for row in rows {
            let (key, fingerprint_json, receipt_json) = row?;
            idempotency.insert(
                key,
                PublishIdempotencyRecord {
                    fingerprint: serde_json::from_str(&fingerprint_json)?,
                    receipt: serde_json::from_str(&receipt_json)?,
                },
            );
        }
        Ok(idempotency)
    }

    fn upsert_welcome_claim(&self, record: &WelcomeClaimRecord) -> Result<(), DurableStoreError> {
        let conn = self.connection()?;
        conn.execute(
            "INSERT INTO http_welcome_claims (
                message_id_json,
                recipient_json,
                seq,
                message_json,
                state_json
            ) VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(message_id_json) DO UPDATE SET
                recipient_json = excluded.recipient_json,
                seq = excluded.seq,
                message_json = excluded.message_json,
                state_json = excluded.state_json",
            params![
                serde_json::to_string(&record.message.id)?,
                serde_json::to_string(&record.recipient)?,
                record.seq,
                serde_json::to_string(&record.message)?,
                serde_json::to_string(&record.state)?,
            ],
        )?;
        Ok(())
    }

    fn load_welcome_claims(
        &self,
    ) -> Result<HashMap<MessageId, WelcomeClaimRecord>, DurableStoreError> {
        let conn = self.connection()?;
        let mut statement = conn.prepare(
            "SELECT message_id_json, recipient_json, seq, message_json, state_json
             FROM http_welcome_claims",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, u64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?;
        let mut claims = HashMap::new();
        for row in rows {
            let (message_id_json, recipient_json, seq, message_json, state_json) = row?;
            let message_id = serde_json::from_str(&message_id_json)?;
            claims.insert(
                message_id,
                WelcomeClaimRecord {
                    recipient: serde_json::from_str(&recipient_json)?,
                    seq,
                    message: serde_json::from_str(&message_json)?,
                    state: serde_json::from_str(&state_json)?,
                },
            );
        }
        Ok(claims)
    }

    fn connection(&self) -> Result<Connection, rusqlite::Error> {
        Connection::open(&*self.path)
    }
}

fn replay_operation(
    service: &mut HttpDeliveryService,
    operation: PersistedOperation,
) -> Result<(), DurableStoreError> {
    match operation {
        PersistedOperation::PublishMessage {
            target, message, ..
        } => {
            service.publish(target, message)?;
        }
        PersistedOperation::PublishKeyPackage { publication } => {
            service.publish_key_package(publication)?;
        }
        PersistedOperation::ClaimKeyPackage { owner } => {
            service.claim_key_package(&owner)?;
        }
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum DurableStoreError {
    #[error("SQLite delivery store error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("delivery store JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("persisted delivery operation failed replay: {0}")]
    Replay(#[from] HttpServerError),
}

#[derive(Debug)]
pub enum ServerHttpError {
    Delivery(HttpServerError),
    IdempotencyConflict {
        idempotency_key: String,
    },
    InvalidIdempotencyKey,
    InvalidWelcomeClaimLimit {
        actual: usize,
        max: usize,
    },
    Store(DurableStoreError),
    WelcomeAckConflict {
        message_id: MessageId,
        current: WelcomeClaimState,
        wanted: WelcomeClaimState,
    },
    WelcomeNotFound {
        message_id: MessageId,
    },
}

impl From<HttpServerError> for ServerHttpError {
    fn from(error: HttpServerError) -> Self {
        Self::Delivery(error)
    }
}

impl From<DurableStoreError> for ServerHttpError {
    fn from(error: DurableStoreError) -> Self {
        Self::Store(error)
    }
}

impl IntoResponse for ServerHttpError {
    fn into_response(self) -> Response {
        let (status, kind, error) = match self {
            Self::Delivery(error) => (
                status_for_error(&error),
                kind_for_error(&error).to_owned(),
                error.to_string(),
            ),
            Self::Store(error) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "delivery_store".to_owned(),
                error.to_string(),
            ),
            Self::IdempotencyConflict { idempotency_key } => (
                StatusCode::CONFLICT,
                "idempotency_conflict".to_owned(),
                format!("conflicting request for idempotency key '{idempotency_key}'"),
            ),
            Self::InvalidIdempotencyKey => (
                StatusCode::BAD_REQUEST,
                "invalid_idempotency_key".to_owned(),
                "idempotency key must not be empty".to_owned(),
            ),
            Self::InvalidWelcomeClaimLimit { actual, max } => (
                StatusCode::BAD_REQUEST,
                "invalid_welcome_claim_limit".to_owned(),
                format!("welcome claim limit must be between 1 and {max}, got {actual}"),
            ),
            Self::WelcomeAckConflict {
                message_id,
                current,
                wanted,
            } => (
                StatusCode::CONFLICT,
                "welcome_ack_conflict".to_owned(),
                format!("welcome {message_id} is already {current:?}; cannot ack as {wanted:?}"),
            ),
            Self::WelcomeNotFound { message_id } => (
                StatusCode::NOT_FOUND,
                "welcome_not_found".to_owned(),
                format!("welcome {message_id} was not claimed"),
            ),
        };
        let body = ErrorResponse { kind, error };
        (status, Json(body)).into_response()
    }
}

fn status_for_error(error: &HttpServerError) -> StatusCode {
    match error {
        HttpServerError::ConflictingMessageId { .. }
        | HttpServerError::StaleEpoch { .. }
        | HttpServerError::ConflictingKeyPackage { .. } => StatusCode::CONFLICT,
        HttpServerError::QueueFull { .. }
        | HttpServerError::GroupLimitExceeded { .. }
        | HttpServerError::InboxLimitExceeded { .. }
        | HttpServerError::KeyPackageInventoryFull { .. } => StatusCode::TOO_MANY_REQUESTS,
        HttpServerError::Empty { .. }
        | HttpServerError::TooLarge { .. }
        | HttpServerError::PublishTargetMismatch
        | HttpServerError::InvalidPageLimit { .. } => StatusCode::BAD_REQUEST,
    }
}

fn kind_for_error(error: &HttpServerError) -> &'static str {
    match error {
        HttpServerError::Empty { .. } => "empty",
        HttpServerError::TooLarge { .. } => "too_large",
        HttpServerError::PublishTargetMismatch => "publish_target_mismatch",
        HttpServerError::ConflictingMessageId { .. } => "conflicting_message_id",
        HttpServerError::StaleEpoch { .. } => "stale_epoch",
        HttpServerError::QueueFull { .. } => "queue_full",
        HttpServerError::GroupLimitExceeded { .. } => "group_limit_exceeded",
        HttpServerError::InboxLimitExceeded { .. } => "inbox_limit_exceeded",
        HttpServerError::InvalidPageLimit { .. } => "invalid_page_limit",
        HttpServerError::ConflictingKeyPackage { .. } => "conflicting_key_package",
        HttpServerError::KeyPackageInventoryFull { .. } => "key_package_inventory_full",
    }
}
