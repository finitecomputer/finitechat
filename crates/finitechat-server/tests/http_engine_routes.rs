use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, Response, StatusCode};
use cgka_conformance_simulator::client::decode_harness_app_payload;
use cgka_conformance_simulator::{ClientBuilder, HarnessClient, TransportBus};
use cgka_traits::engine::GroupEvent;
use cgka_traits::transport::{TransportEnvelope, TransportMessage};
use cgka_traits::{EpochId, GroupId};
use finitechat_server::{
    GroupSyncRequest, HttpServerState, InboxSyncRequest, PublishMessageRequest, http_router,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tower::ServiceExt;
use transport_http_server::{
    HttpCommitAdmission, HttpPublishReceipt, HttpPublishTarget, HttpSequence, HttpSyncPage,
};

#[tokio::test]
async fn http_routes_carry_real_marmot_invite_and_app_messages() {
    let bus = TransportBus::ordered();
    let app = http_router(HttpServerState::default());
    let mut alice = ClientBuilder::new(b"alice".to_vec()).attach(&bus);
    let mut bob = ClientBuilder::new(b"bob".to_vec()).attach(&bus);
    let mut carol = ClientBuilder::new(b"carol".to_vec()).attach(&bus);

    let bob_kp = bob.fresh_key_package().await;
    let (group_id, create_pending) = alice
        .create_group("finite-http-route-engine", vec![bob_kp], vec![])
        .await;
    let create_receipts =
        publish_drained_bus_messages_to_routes(&bus, app.clone(), &group_id, None).await;
    assert_eq!(create_receipts.len(), 1);
    alice.confirm(create_pending).await;

    deliver_http_inbox_to_client(app.clone(), &bus, &bob, 0).await;
    bob.tick().await;
    assert_eq!(bob.epoch(), EpochId(1));

    let carol_kp = carol.fresh_key_package().await;
    let invite_pending = alice.invite(vec![carol_kp]).await;
    let invite_receipts = publish_drained_bus_messages_to_routes(
        &bus,
        app.clone(),
        &group_id,
        Some(HttpCommitAdmission {
            source_epoch: EpochId(1),
        }),
    )
    .await;
    assert_eq!(invite_receipts.len(), 2);
    assert!(
        invite_receipts
            .iter()
            .any(|receipt| receipt.seq == 1 && !receipt.duplicate)
    );
    alice.confirm(invite_pending).await;

    let mut bob_group_cursor = 0;
    deliver_http_group_to_client(app.clone(), &bus, &bob, &group_id, &mut bob_group_cursor).await;
    bob.tick().await;
    assert_eq!(bob.epoch(), EpochId(2));
    assert!(bob.drain_events().iter().any(|event| {
        matches!(
            event,
            GroupEvent::EpochChanged {
                from: EpochId(1),
                to: EpochId(2),
                ..
            }
        )
    }));

    deliver_http_inbox_to_client(app.clone(), &bus, &carol, 0).await;
    carol.tick().await;
    assert_eq!(carol.epoch(), EpochId(2));
    carol.drain_events();

    alice
        .send_app_capture(b"hello over finite route".to_vec())
        .await;
    let app_receipts =
        publish_drained_bus_messages_to_routes(&bus, app.clone(), &group_id, None).await;
    assert_eq!(app_receipts.len(), 1);
    assert_eq!(app_receipts[0].seq, 2);

    let mut carol_group_cursor = 0;
    deliver_http_group_to_client(app.clone(), &bus, &bob, &group_id, &mut bob_group_cursor).await;
    deliver_http_group_to_client(app, &bus, &carol, &group_id, &mut carol_group_cursor).await;
    bob.tick().await;
    carol.tick().await;

    assert_eq!(
        received_payloads(&mut bob),
        vec![b"hello over finite route".to_vec()]
    );
    assert_eq!(
        received_payloads(&mut carol),
        vec![b"hello over finite route".to_vec()]
    );
}

async fn publish_drained_bus_messages_to_routes(
    bus: &TransportBus,
    app: Router,
    group_id: &GroupId,
    group_commit_admission: Option<HttpCommitAdmission>,
) -> Vec<HttpPublishReceipt> {
    let mut receipts = Vec::new();
    for message in drain_bus_queue(bus) {
        let target = match &message.envelope {
            TransportEnvelope::GroupMessage { transport_group_id } => HttpPublishTarget::Group {
                group_id: group_id.clone(),
                transport_group_id: transport_group_id.clone(),
                commit_admission: group_commit_admission,
            },
            TransportEnvelope::Welcome { recipient } => HttpPublishTarget::Inbox {
                recipient: recipient.clone(),
            },
        };
        let response = post_json(
            app.clone(),
            "/messages",
            &PublishMessageRequest { target, message },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        receipts.push(read_json(response).await);
    }
    receipts
}

fn drain_bus_queue(bus: &TransportBus) -> Vec<TransportMessage> {
    let mut drained = Vec::new();
    while bus.queued_len() > 0 {
        let message = bus
            .queued_messages()
            .into_iter()
            .next()
            .expect("queued_len reported a message");
        assert!(bus.drop_queued(0));
        drained.push(message);
    }
    drained
}

async fn deliver_http_inbox_to_client(
    app: Router,
    bus: &TransportBus,
    client: &HarnessClient,
    after_seq: HttpSequence,
) -> HttpSequence {
    let response = post_json(
        app,
        "/sync/inbox",
        &InboxSyncRequest {
            recipient: client.member_id(),
            after_seq,
            limit: 10,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let page: HttpSyncPage = read_json(response).await;
    for entry in &page.entries {
        bus.inject(client.bus_id, entry.message.clone());
    }
    page.next_after_seq
}

async fn deliver_http_group_to_client(
    app: Router,
    bus: &TransportBus,
    client: &HarnessClient,
    group_id: &GroupId,
    after_seq: &mut HttpSequence,
) {
    let response = post_json(
        app,
        "/sync/group",
        &GroupSyncRequest {
            group_id: group_id.clone(),
            after_seq: *after_seq,
            limit: 10,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let page: HttpSyncPage = read_json(response).await;
    for entry in &page.entries {
        bus.inject(client.bus_id, entry.message.clone());
    }
    *after_seq = page.next_after_seq;
}

fn received_payloads(client: &mut HarnessClient) -> Vec<Vec<u8>> {
    client
        .drain_events()
        .into_iter()
        .filter_map(|event| match event {
            GroupEvent::MessageReceived { payload, .. } => {
                Some(decode_harness_app_payload(&payload))
            }
            _ => None,
        })
        .collect()
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
