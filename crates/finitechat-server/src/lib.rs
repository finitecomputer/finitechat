use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use cgka_traits::{GroupId, MemberId};
use serde::{Deserialize, Serialize};
use transport_http_server::{
    HttpClaimedKeyPackage, HttpDeliveryService, HttpKeyPackagePublication, HttpPublishReceipt,
    HttpPublishTarget, HttpSequence, HttpServerError, HttpSyncPage,
};

#[derive(Clone, Debug, Default)]
pub struct HttpServerState {
    service: Arc<Mutex<HttpDeliveryService>>,
}

impl HttpServerState {
    pub fn new(service: HttpDeliveryService) -> Self {
        Self {
            service: Arc::new(Mutex::new(service)),
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
    let mut service = state.service.lock().expect("HTTP delivery service mutex");
    let receipt = service.publish(request.target, request.message)?;
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
    let mut service = state.service.lock().expect("HTTP delivery service mutex");
    service.publish_key_package(publication)?;
    Ok(Json(PublishKeyPackageResponse { published: true }))
}

async fn claim_key_package(
    State(state): State<HttpServerState>,
    Json(request): Json<ClaimKeyPackageRequest>,
) -> Result<Json<Option<HttpClaimedKeyPackage>>, ServerHttpError> {
    let mut service = state.service.lock().expect("HTTP delivery service mutex");
    let claimed = service.claim_key_package(&request.owner)?;
    Ok(Json(claimed))
}

#[derive(Debug)]
pub struct ServerHttpError(HttpServerError);

impl From<HttpServerError> for ServerHttpError {
    fn from(error: HttpServerError) -> Self {
        Self(error)
    }
}

impl IntoResponse for ServerHttpError {
    fn into_response(self) -> Response {
        let status = status_for_error(&self.0);
        let body = ErrorResponse {
            kind: kind_for_error(&self.0).to_owned(),
            error: self.0.to_string(),
        };
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
