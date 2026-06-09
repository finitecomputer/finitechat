use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use cgka_traits::{GroupId, MemberId};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use transport_http_server::{
    HttpClaimedKeyPackage, HttpDeliveryService, HttpKeyPackagePublication, HttpPublishReceipt,
    HttpPublishTarget, HttpSequence, HttpServerError, HttpSyncPage,
};

#[derive(Clone, Debug, Default)]
pub struct HttpServerState {
    service: Arc<Mutex<HttpDeliveryService>>,
    store: Option<Arc<SqliteHttpDeliveryStore>>,
}

impl HttpServerState {
    pub fn new(service: HttpDeliveryService) -> Self {
        Self {
            service: Arc::new(Mutex::new(service)),
            store: None,
        }
    }

    pub fn from_sqlite_path(path: impl AsRef<Path>) -> Result<Self, DurableStoreError> {
        let store = Arc::new(SqliteHttpDeliveryStore::open(path)?);
        let mut service = HttpDeliveryService::default();
        for operation in store.load_operations()? {
            replay_operation(&mut service, operation)?;
        }
        Ok(Self {
            service: Arc::new(Mutex::new(service)),
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
}

pub fn http_router(state: HttpServerState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/messages", post(publish_message))
        .route("/sync/group", post(sync_group))
        .route("/sync/inbox", post(sync_inbox))
        .route("/key-packages", post(publish_key_package))
        .route("/key-packages/claim", post(claim_key_package))
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
    let receipt = state.apply_mutation(|service| {
        let receipt = service.publish(request.target.clone(), request.message.clone())?;
        let operation = (!receipt.duplicate).then_some(PersistedOperation::PublishMessage {
            target: request.target,
            message: request.message,
        });
        Ok((receipt, operation))
    })?;
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum PersistedOperation {
    PublishMessage {
        target: HttpPublishTarget,
        message: cgka_traits::transport::TransportMessage,
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

    fn connection(&self) -> Result<Connection, rusqlite::Error> {
        Connection::open(&*self.path)
    }
}

fn replay_operation(
    service: &mut HttpDeliveryService,
    operation: PersistedOperation,
) -> Result<(), DurableStoreError> {
    match operation {
        PersistedOperation::PublishMessage { target, message } => {
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
    Store(DurableStoreError),
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
