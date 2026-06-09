use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, Response, StatusCode};
use cgka_traits::engine::KeyPackage;
use cgka_traits::transport::{Timestamp, TransportEnvelope, TransportMessage, TransportSource};
use cgka_traits::{EpochId, GroupId, MemberId, MessageId};
use finitechat_server::{
    ClaimKeyPackageRequest, ErrorResponse, GroupSyncRequest, HealthResponse, HttpServerState,
    InboxSyncRequest, PublishKeyPackageResponse, PublishMessageRequest, http_router,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tower::ServiceExt;
use transport_http_server::{
    HTTP_SERVER_SOURCE, HttpCommitAdmission, HttpDeliveryPlane, HttpKeyPackageId,
    HttpKeyPackagePublication, HttpPublishReceipt, HttpPublishTarget, HttpSyncPage,
};

#[tokio::test]
async fn health_reports_ok() {
    let app = http_router(HttpServerState::default());

    let response = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/health")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);
    let body: HealthResponse = read_json(response).await;
    assert_eq!(body.status, "ok");
}

#[tokio::test]
async fn group_publish_syncs_pages_and_replays_exact_duplicates() {
    let app = http_router(HttpServerState::default());
    let group_id = group_id("route-group");
    let transport_group_id = b"route-transport-group".to_vec();
    let first = PublishMessageRequest {
        target: group_target(
            group_id.clone(),
            transport_group_id.clone(),
            Some(HttpCommitAdmission {
                source_epoch: EpochId(1),
            }),
        ),
        message: group_message("commit-1", transport_group_id.clone(), b"commit"),
        idempotency_key: None,
    };
    let second = PublishMessageRequest {
        target: group_target(group_id.clone(), transport_group_id.clone(), None),
        message: group_message("app-1", transport_group_id, b"app"),
        idempotency_key: None,
    };

    let response = post_json(app.clone(), "/messages", &first).await;
    assert_eq!(response.status(), StatusCode::OK);
    let first_receipt: HttpPublishReceipt = read_json(response).await;
    assert_eq!(first_receipt.seq, 1);
    assert_eq!(first_receipt.plane, HttpDeliveryPlane::Group);
    assert!(!first_receipt.duplicate);

    let response = post_json(app.clone(), "/messages", &first).await;
    assert_eq!(response.status(), StatusCode::OK);
    let replay_receipt: HttpPublishReceipt = read_json(response).await;
    assert_eq!(replay_receipt.seq, 1);
    assert!(replay_receipt.duplicate);

    let response = post_json(app.clone(), "/messages", &second).await;
    assert_eq!(response.status(), StatusCode::OK);
    let second_receipt: HttpPublishReceipt = read_json(response).await;
    assert_eq!(second_receipt.seq, 2);

    let response = post_json(
        app.clone(),
        "/sync/group",
        &GroupSyncRequest {
            group_id: group_id.clone(),
            after_seq: 0,
            limit: 1,
            requester: None,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let page: HttpSyncPage = read_json(response).await;
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].seq, 1);
    assert_eq!(page.entries[0].message.id, id("commit-1"));
    assert_eq!(page.next_after_seq, 1);
    assert!(page.has_more);

    let response = post_json(
        app,
        "/sync/group",
        &GroupSyncRequest {
            group_id,
            after_seq: page.next_after_seq,
            limit: 10,
            requester: None,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let page: HttpSyncPage = read_json(response).await;
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].seq, 2);
    assert_eq!(page.entries[0].message.id, id("app-1"));
    assert_eq!(page.next_after_seq, 2);
    assert!(!page.has_more);
}

#[tokio::test]
async fn same_epoch_second_commit_is_http_conflict() {
    let app = http_router(HttpServerState::default());
    let group_id = group_id("epoch-group");
    let transport_group_id = b"epoch-transport-group".to_vec();
    let first = PublishMessageRequest {
        target: group_target(
            group_id.clone(),
            transport_group_id.clone(),
            Some(HttpCommitAdmission {
                source_epoch: EpochId(7),
            }),
        ),
        message: group_message("commit-epoch-7-a", transport_group_id.clone(), b"commit-a"),
        idempotency_key: None,
    };
    let second = PublishMessageRequest {
        target: group_target(
            group_id,
            transport_group_id.clone(),
            Some(HttpCommitAdmission {
                source_epoch: EpochId(7),
            }),
        ),
        message: group_message("commit-epoch-7-b", transport_group_id, b"commit-b"),
        idempotency_key: None,
    };

    let response = post_json(app.clone(), "/messages", &first).await;
    assert_eq!(response.status(), StatusCode::OK);

    let response = post_json(app, "/messages", &second).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let error: ErrorResponse = read_json(response).await;
    assert_eq!(error.kind, "stale_epoch");
    assert!(error.error.contains("epoch:7"));
}

#[tokio::test]
async fn inbox_publish_and_sync_round_trips_welcome() {
    let app = http_router(HttpServerState::default());
    let recipient = member("recipient-device");
    let request = PublishMessageRequest {
        target: HttpPublishTarget::Inbox {
            recipient: recipient.clone(),
        },
        message: welcome_message("welcome-1", recipient.clone()),
        idempotency_key: None,
    };

    let response = post_json(app.clone(), "/messages", &request).await;
    assert_eq!(response.status(), StatusCode::OK);
    let receipt: HttpPublishReceipt = read_json(response).await;
    assert_eq!(receipt.seq, 1);
    assert_eq!(receipt.plane, HttpDeliveryPlane::Inbox);

    let response = post_json(
        app,
        "/sync/inbox",
        &InboxSyncRequest {
            recipient,
            after_seq: 0,
            limit: 10,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let page: HttpSyncPage = read_json(response).await;
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].message.id, id("welcome-1"));
    assert!(!page.has_more);
}

#[tokio::test]
async fn key_package_publish_and_claim_is_single_use() {
    let app = http_router(HttpServerState::default());
    let owner = member("alice-device");
    let key_package_id = HttpKeyPackageId::new(b"kp-route-1".to_vec());
    let publication = HttpKeyPackagePublication {
        key_package_id: key_package_id.clone(),
        owner: owner.clone(),
        key_package: KeyPackage::new(b"key-package-bytes".to_vec()),
    };

    let response = post_json(app.clone(), "/key-packages", &publication).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body: PublishKeyPackageResponse = read_json(response).await;
    assert!(body.published);

    let response = post_json(
        app.clone(),
        "/key-packages/claim",
        &ClaimKeyPackageRequest {
            owner: owner.clone(),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let claimed: Option<transport_http_server::HttpClaimedKeyPackage> = read_json(response).await;
    let claimed = claimed.expect("published KeyPackage can be claimed once");
    assert_eq!(claimed.key_package_id, key_package_id);
    assert_eq!(claimed.owner, owner);
    assert_eq!(claimed.key_package.bytes(), b"key-package-bytes");

    let response = post_json(
        app,
        "/key-packages/claim",
        &ClaimKeyPackageRequest {
            owner: member("alice-device"),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let claimed: Option<transport_http_server::HttpClaimedKeyPackage> = read_json(response).await;
    assert_eq!(claimed, None);
}

async fn post_json<T: Serialize>(app: Router, uri: &str, body: &T) -> Response<Body> {
    app.oneshot(
        Request::builder()
            .method(Method::POST)
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(body).expect("json body")))
            .expect("request"),
    )
    .await
    .expect("response")
}

async fn read_json<T: DeserializeOwned>(response: Response<Body>) -> T {
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body");
    serde_json::from_slice(&bytes).expect("json response")
}

fn id(label: &str) -> MessageId {
    MessageId::new(label.as_bytes().to_vec())
}

fn group_id(label: &str) -> GroupId {
    GroupId::new(label.as_bytes().to_vec())
}

fn member(label: &str) -> MemberId {
    MemberId::new(label.as_bytes().to_vec())
}

fn group_target(
    group_id: GroupId,
    transport_group_id: Vec<u8>,
    commit_admission: Option<HttpCommitAdmission>,
) -> HttpPublishTarget {
    HttpPublishTarget::Group {
        group_id,
        transport_group_id,
        commit_admission,
    }
}

fn group_message(
    message_id: &str,
    transport_group_id: Vec<u8>,
    payload: &[u8],
) -> TransportMessage {
    TransportMessage {
        id: id(message_id),
        payload: payload.to_vec(),
        timestamp: Timestamp(42),
        causal_deps: Vec::new(),
        source: TransportSource(HTTP_SERVER_SOURCE.to_owned()),
        envelope: TransportEnvelope::GroupMessage { transport_group_id },
    }
}

fn welcome_message(message_id: &str, recipient: MemberId) -> TransportMessage {
    TransportMessage {
        id: id(message_id),
        payload: b"welcome-bytes".to_vec(),
        timestamp: Timestamp(43),
        causal_deps: Vec::new(),
        source: TransportSource(HTTP_SERVER_SOURCE.to_owned()),
        envelope: TransportEnvelope::Welcome { recipient },
    }
}
