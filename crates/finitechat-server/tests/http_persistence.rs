use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, Response, StatusCode};
use cgka_traits::engine::KeyPackage;
use cgka_traits::transport::{Timestamp, TransportEnvelope, TransportMessage, TransportSource};
use cgka_traits::{EpochId, GroupId, MemberId, MessageId};
use finitechat_engine::{
    AccountRoomDevice, AccountRoomRecord, AppendEphemeralActivityRequest, AppendEventRequest,
    CommitAccepted, EphemeralActivityAccepted, EventAccepted, SubmitCommitRequest,
    UploadKeyPackageRequest, WelcomeRecord,
};
use finitechat_http::{
    AckWelcomeRequest, AckWelcomeResponse, BootstrapAccountRoomRequest,
    BootstrapAccountRoomResponse, ClaimKeyPackageRequest, ClaimKeyPackagesRequest,
    ClaimWelcomesRequest, ErrorResponse, ExpireKeyPackageLeaseRequest,
    ExpireKeyPackageLeaseResponse, FiniteAccountRoomCommitProjection, GetFanoutRequest,
    GroupSyncRequest, HttpClaimedWelcome, HttpFanoutPlan, HttpFanoutRoomPlan, HttpFanoutRoomStatus,
    HttpKeyPackageClaim, HttpKeyPackageInventory, InboxSyncRequest, KeyPackageInventoryRequest,
    ListAccountRoomDirectoryRequest, ListAccountRoomDirectoryResponse, MarkFanoutDoneRequest,
    MarkFanoutPreparedRequest, PublishMessageRequest, ReportInvalidCommitRequest,
    ReportInvalidCommitResponse, RevokeDeviceRequest, SaveAccountRoomRequest,
    SaveAccountRoomResponse, SaveFanoutRoomRequest,
};
use finitechat_proto::{
    DeviceRef, FiniteEnvelope, LogEntryKind, MAX_ACCOUNT_DEVICES_PER_ROOM,
    MAX_ENVELOPE_PAYLOAD_BYTES, MAX_EPHEMERAL_ACTIVITY_CACHE_ENTRIES_PER_ROUTE,
    MAX_IDEMPOTENCY_RECORDS_PER_ROOM_DEVICE, MembershipAddV1, MembershipDeltaV1,
    MembershipRemoveV1, RoomStatus, StagedWelcomeV1, WelcomeState,
};
use finitechat_server::{HttpServerState, http_router};
use rusqlite::{Connection, params};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tempfile::TempDir;
use tower::ServiceExt;
use transport_http_server::{
    HTTP_SERVER_SOURCE, HttpClaimedKeyPackage, HttpCommitAdmission, HttpDeliveryPlane,
    HttpKeyPackageId, HttpKeyPackagePublication, HttpPublishReceipt, HttpPublishTarget,
    HttpSyncPage, MAX_HTTP_SYNC_PAGE_ENTRIES,
};

#[tokio::test]
async fn sqlite_log_rebuilds_group_queue_and_duplicate_index_after_restart() {
    let temp = TempDir::new().expect("tempdir");
    let db_path = temp.path().join("delivery.sqlite3");
    let group_id = group_id("durable-group");
    let transport_group_id = b"durable-transport".to_vec();
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

    let app = persistent_app(&db_path);
    assert_eq!(
        post_json(app.clone(), "/messages", &first).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        post_json(app.clone(), "/messages", &second).await.status(),
        StatusCode::OK
    );

    let app = persistent_app(&db_path);
    let response = post_json(
        app.clone(),
        "/sync/group",
        &GroupSyncRequest {
            group_id: group_id.clone(),
            after_seq: 0,
            limit: 10,
            requester: None,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let page: HttpSyncPage = read_json(response).await;
    assert_eq!(page.entries.len(), 2);
    assert_eq!(page.entries[0].message.id, id("commit-1"));
    assert_eq!(page.entries[1].message.id, id("app-1"));
    assert_eq!(page.next_after_seq, 2);

    let response = post_json(app, "/messages", &first).await;
    assert_eq!(response.status(), StatusCode::OK);
    let receipt: HttpPublishReceipt = read_json(response).await;
    assert_eq!(receipt.seq, 1);
    assert!(receipt.duplicate);
}

#[tokio::test]
async fn sqlite_publish_idempotency_replays_original_receipt_after_restart() {
    let temp = TempDir::new().expect("tempdir");
    let db_path = temp.path().join("delivery.sqlite3");
    let group_id = group_id("idempotent-group");
    let transport_group_id = b"idempotent-transport".to_vec();
    let request = PublishMessageRequest {
        target: group_target(group_id.clone(), transport_group_id.clone(), None),
        message: group_message("idempotent-message", transport_group_id, b"first body"),
        idempotency_key: Some("idem-message-1".to_owned()),
    };

    let app = persistent_app(&db_path);
    let response = post_json(app, "/messages", &request).await;
    assert_eq!(response.status(), StatusCode::OK);
    let accepted: HttpPublishReceipt = read_json(response).await;
    assert_eq!(accepted.seq, 1);
    assert!(!accepted.duplicate);

    let app = persistent_app(&db_path);
    let response = post_json(app.clone(), "/messages", &request).await;
    assert_eq!(response.status(), StatusCode::OK);
    let replayed: HttpPublishReceipt = read_json(response).await;
    assert_eq!(replayed, accepted);
    assert!(!replayed.duplicate);

    let response = post_json(
        app.clone(),
        "/sync/group",
        &GroupSyncRequest {
            group_id,
            after_seq: 0,
            limit: 10,
            requester: None,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let page: HttpSyncPage = read_json(response).await;
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].message.id, id("idempotent-message"));
}

#[tokio::test]
async fn sqlite_publish_idempotency_rejects_same_key_with_different_body() {
    let temp = TempDir::new().expect("tempdir");
    let db_path = temp.path().join("delivery.sqlite3");
    let group_id = group_id("idempotency-conflict-group");
    let transport_group_id = b"idempotency-conflict-transport".to_vec();
    let first = PublishMessageRequest {
        target: group_target(group_id.clone(), transport_group_id.clone(), None),
        message: group_message(
            "idempotency-conflict-a",
            transport_group_id.clone(),
            b"first",
        ),
        idempotency_key: Some("idem-conflict".to_owned()),
    };
    let conflicting = PublishMessageRequest {
        target: group_target(group_id.clone(), transport_group_id.clone(), None),
        message: group_message("idempotency-conflict-b", transport_group_id, b"second"),
        idempotency_key: Some("idem-conflict".to_owned()),
    };

    let app = persistent_app(&db_path);
    assert_eq!(
        post_json(app, "/messages", &first).await.status(),
        StatusCode::OK
    );

    let app = persistent_app(&db_path);
    let response = post_json(app.clone(), "/messages", &conflicting).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let error: ErrorResponse = read_json(response).await;
    assert_eq!(error.kind, "idempotency_conflict");
    assert!(error.error.contains("idem-conflict"));

    let response = post_json(
        app.clone(),
        "/sync/group",
        &GroupSyncRequest {
            group_id,
            after_seq: 0,
            limit: 10,
            requester: None,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let page: HttpSyncPage = read_json(response).await;
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].message.id, id("idempotency-conflict-a"));
}

#[tokio::test]
async fn sqlite_log_rebuilds_commit_admission_after_restart() {
    let temp = TempDir::new().expect("tempdir");
    let db_path = temp.path().join("delivery.sqlite3");
    let group_id = group_id("epoch-durable-group");
    let transport_group_id = b"epoch-durable-transport".to_vec();
    let first = PublishMessageRequest {
        target: group_target(
            group_id.clone(),
            transport_group_id.clone(),
            Some(HttpCommitAdmission {
                source_epoch: EpochId(9),
            }),
        ),
        message: group_message("commit-epoch-9-a", transport_group_id.clone(), b"commit-a"),
        idempotency_key: None,
    };
    let second = PublishMessageRequest {
        target: group_target(
            group_id,
            transport_group_id.clone(),
            Some(HttpCommitAdmission {
                source_epoch: EpochId(9),
            }),
        ),
        message: group_message("commit-epoch-9-b", transport_group_id, b"commit-b"),
        idempotency_key: None,
    };

    let app = persistent_app(&db_path);
    assert_eq!(
        post_json(app, "/messages", &first).await.status(),
        StatusCode::OK
    );

    let app = persistent_app(&db_path);
    let response = post_json(app, "/messages", &second).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let error: ErrorResponse = read_json(response).await;
    assert_eq!(error.kind, "stale_epoch");
}

#[tokio::test]
async fn sqlite_log_rebuilds_key_package_claim_state_after_restart() {
    let temp = TempDir::new().expect("tempdir");
    let db_path = temp.path().join("delivery.sqlite3");
    let owner = member("durable-owner");
    let key_package_id = HttpKeyPackageId::new(b"durable-kp".to_vec());
    let publication = HttpKeyPackagePublication {
        key_package_id: key_package_id.clone(),
        owner: owner.clone(),
        key_package: KeyPackage::new(b"durable-key-package".to_vec()),
    };

    let app = persistent_app(&db_path);
    assert_eq!(
        post_json(app.clone(), "/key-packages", &publication)
            .await
            .status(),
        StatusCode::OK
    );
    let response = post_json(
        app.clone(),
        "/key-packages/claim",
        &ClaimKeyPackageRequest {
            owner: owner.clone(),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let claimed: Option<HttpClaimedKeyPackage> = read_json(response).await;
    assert_eq!(
        claimed
            .expect("claim before restart")
            .key_package_id
            .as_slice(),
        key_package_id.as_slice()
    );

    let app = persistent_app(&db_path);
    let response = post_json(
        app.clone(),
        "/key-packages/claim",
        &ClaimKeyPackageRequest { owner },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let claimed: Option<HttpClaimedKeyPackage> = read_json(response).await;
    assert_eq!(claimed, None);
}

#[tokio::test]
async fn sqlite_key_package_inventory_tracks_available_and_claimed_after_restart() {
    let temp = TempDir::new().expect("tempdir");
    let db_path = temp.path().join("delivery.sqlite3");
    let owner = member("inventory-owner");
    let first = key_package_publication("kp-inventory-a", owner.clone(), b"inventory-a");
    let second = key_package_publication("kp-inventory-b", owner.clone(), b"inventory-b");

    let app = persistent_app(&db_path);
    assert_eq!(
        post_json(app.clone(), "/key-packages", &first)
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        post_json(app.clone(), "/key-packages", &second)
            .await
            .status(),
        StatusCode::OK
    );
    assert_inventory(app.clone(), owner.clone(), 2, 0).await;

    let response = post_json(
        app.clone(),
        "/key-packages/claim",
        &ClaimKeyPackageRequest {
            owner: owner.clone(),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let claimed: Option<HttpClaimedKeyPackage> = read_json(response).await;
    assert_eq!(
        claimed
            .as_ref()
            .expect("first package claimed")
            .key_package_id
            .as_slice(),
        b"kp-inventory-a"
    );
    assert_inventory(app, owner.clone(), 1, 1).await;

    let app = persistent_app(&db_path);
    assert_inventory(app.clone(), owner.clone(), 1, 1).await;

    assert_eq!(
        post_json(app.clone(), "/key-packages", &first)
            .await
            .status(),
        StatusCode::OK
    );
    assert_inventory(app, owner, 1, 1).await;
}

#[tokio::test]
async fn sqlite_key_package_lease_expiry_and_reclaim_survives_restart_over_http() {
    let temp = TempDir::new().expect("tempdir");
    let db_path = temp.path().join("delivery.sqlite3");
    let owner = member("lease-owner");
    let key_package_id = HttpKeyPackageId::new(b"kp-lease-reclaim".to_vec());
    let publication = HttpKeyPackagePublication {
        key_package_id: key_package_id.clone(),
        owner: owner.clone(),
        key_package: KeyPackage::new(b"lease-reclaim-package".to_vec()),
    };

    let app = persistent_app(&db_path);
    assert_eq!(
        post_json(app.clone(), "/key-packages", &publication)
            .await
            .status(),
        StatusCode::OK
    );
    let response = post_json(
        app.clone(),
        "/key-packages/claim",
        &ClaimKeyPackageRequest {
            owner: owner.clone(),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let claimed: Option<HttpClaimedKeyPackage> = read_json(response).await;
    assert_eq!(
        claimed.as_ref().expect("first claim").key_package_id,
        key_package_id
    );
    assert_inventory(app.clone(), owner.clone(), 0, 1).await;
    let response = post_json(
        app.clone(),
        "/key-packages/leases/expire",
        &ExpireKeyPackageLeaseRequest {
            key_package_id: key_package_id.clone(),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let expired: ExpireKeyPackageLeaseResponse = read_json(response).await;
    assert!(expired.expired);
    assert_inventory(app.clone(), owner.clone(), 1, 0).await;

    let app = persistent_app(&db_path);
    assert_inventory(app.clone(), owner.clone(), 1, 0).await;
    let response = post_json(
        app.clone(),
        "/key-packages/claim",
        &ClaimKeyPackageRequest {
            owner: owner.clone(),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let reclaimed: Option<HttpClaimedKeyPackage> = read_json(response).await;
    let reclaimed = reclaimed.expect("reclaimed package");
    assert_eq!(reclaimed.key_package_id, key_package_id);
    assert_eq!(reclaimed.owner, owner);
    assert_eq!(reclaimed.key_package, publication.key_package);
    assert_inventory(app, member("lease-owner"), 0, 1).await;
}

#[tokio::test]
async fn sqlite_revoked_device_status_survives_restart_and_blocks_key_packages_over_http() {
    let temp = TempDir::new().expect("tempdir");
    let db_path = temp.path().join("delivery.sqlite3");
    let bob = DeviceRef::new("bob", "bob-phone");
    let owner = member_for_device(&bob);
    let first = key_package_publication("kp-revoked-bob-1", owner.clone(), b"revoked-one");
    let second = key_package_publication("kp-revoked-bob-2", owner.clone(), b"revoked-two");

    let app = persistent_app(&db_path);
    assert_eq!(
        post_json(app.clone(), "/key-packages", &first)
            .await
            .status(),
        StatusCode::OK
    );
    assert_inventory(app.clone(), owner.clone(), 1, 0).await;

    let response = post_json(
        app.clone(),
        "/devices/revoke",
        &RevokeDeviceRequest {
            device: bob.clone(),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let app = persistent_app(&db_path);
    let response = post_json(app.clone(), "/key-packages", &second).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let error: ErrorResponse = read_json(response).await;
    assert_eq!(error.kind, "device_revoked");

    let response = post_json(
        app.clone(),
        "/key-packages/claim",
        &ClaimKeyPackageRequest {
            owner: owner.clone(),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let error: ErrorResponse = read_json(response).await;
    assert_eq!(error.kind, "device_revoked");

    let response = post_json(
        app.clone(),
        "/key-packages/claims",
        &ClaimKeyPackagesRequest {
            owners: vec![owner.clone()],
            idempotency_key: Some("revoked-owner-batch".to_owned()),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let claims: Vec<HttpKeyPackageClaim> = read_json(response).await;
    assert_eq!(claims.len(), 1);
    assert_eq!(claims[0].owner, owner.clone());
    assert!(claims[0].claimed.is_none());
    assert_inventory(app, owner, 1, 0).await;
}

#[tokio::test]
async fn sqlite_revoked_device_blocks_welcome_activation_and_typed_routes_after_restart() {
    let temp = TempDir::new().expect("tempdir");
    let db_path = temp.path().join("delivery.sqlite3");
    let alice = DeviceRef::new("alice", "alice-laptop");
    let bob = DeviceRef::new("bob", "bob-phone");
    let pending_room_id = "room-revoked-pending".to_owned();
    let pending_mls_group_id = "mls-revoked-pending".to_owned();
    let active_room_id = "room-revoked-active".to_owned();
    let active_mls_group_id = "mls-revoked-active".to_owned();
    let target_room_id = "room-revoked-target".to_owned();
    let target_mls_group_id = "mls-revoked-target".to_owned();
    let pending_add = submit_add_device_request(
        &pending_room_id,
        &pending_mls_group_id,
        &alice,
        &bob,
        "welcome-revoked-pending",
        "commit-revoked-pending",
    );
    let active_add = submit_add_device_request(
        &active_room_id,
        &active_mls_group_id,
        &alice,
        &bob,
        "welcome-revoked-active",
        "commit-revoked-active",
    );

    let app = persistent_app(&db_path);
    for (room_id, mls_group_id) in [
        (&pending_room_id, &pending_mls_group_id),
        (&active_room_id, &active_mls_group_id),
    ] {
        let response = post_json(
            app.clone(),
            "/account-rooms/bootstrap",
            &BootstrapAccountRoomRequest {
                room_id: room_id.clone(),
                mls_group_id: mls_group_id.clone(),
                creator: alice.clone(),
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    publish_and_claim_key_package_for_add(&app, &pending_add).await;
    let response = post_json(app.clone(), "/commits", &pending_add).await;
    assert_eq!(response.status(), StatusCode::OK);
    let response = post_json(
        app.clone(),
        "/welcomes/claim",
        &ClaimWelcomesRequest {
            recipient: member_for_device(&bob),
            limit: 10,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let pending_claims: Vec<HttpClaimedWelcome> = read_json(response).await;
    assert_eq!(pending_claims.len(), 1);
    assert_eq!(pending_claims[0].message.id, id("welcome-revoked-pending"));

    publish_and_claim_key_package_for_add(&app, &active_add).await;
    let response = post_json(app.clone(), "/commits", &active_add).await;
    assert_eq!(response.status(), StatusCode::OK);
    let response = post_json(
        app.clone(),
        "/welcomes/claim",
        &ClaimWelcomesRequest {
            recipient: member_for_device(&bob),
            limit: 10,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let active_claims: Vec<HttpClaimedWelcome> = read_json(response).await;
    assert_eq!(active_claims.len(), 1);
    assert_eq!(active_claims[0].message.id, id("welcome-revoked-active"));
    let response = post_json(
        app.clone(),
        "/welcomes/ack",
        &AckWelcomeRequest {
            message_id: id("welcome-revoked-active"),
            activated: true,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    revoke_device(&app, &bob).await;

    let app = persistent_app(&db_path);
    let response = post_json(
        app.clone(),
        "/welcomes/claim",
        &ClaimWelcomesRequest {
            recipient: member_for_device(&bob),
            limit: 10,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let error: ErrorResponse = read_json(response).await;
    assert_eq!(error.kind, "device_revoked");

    let response = post_json(
        app.clone(),
        "/welcomes/ack",
        &AckWelcomeRequest {
            message_id: id("welcome-revoked-pending"),
            activated: true,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let error: ErrorResponse = read_json(response).await;
    assert_eq!(error.kind, "device_revoked");
    let page = account_room_page(&app, "bob").await;
    let pending_room = page
        .rooms
        .iter()
        .find(|room| room["room_id"].as_str() == Some(pending_room_id.as_str()))
        .expect("pending room");
    let pending_bob = pending_room["devices"]
        .as_array()
        .expect("devices")
        .iter()
        .find(|device| device["device"]["device_id"] == "bob-phone")
        .expect("pending Bob device");
    assert_eq!(pending_bob["active"], false);

    let response = post_json(
        app.clone(),
        "/events",
        &append_application_request(
            &active_room_id,
            &active_mls_group_id,
            &bob,
            1,
            b"revoked-send",
            "revoked-send-idempotency",
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let error: ErrorResponse = read_json(response).await;
    assert_eq!(error.kind, "device_revoked");

    let remove = submit_remove_device_request(
        &active_room_id,
        &active_mls_group_id,
        &bob,
        &alice,
        1,
        "revoked-commit-idempotency",
    );
    let response = post_json(app.clone(), "/commits", &remove).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let error: ErrorResponse = read_json(response).await;
    assert_eq!(error.kind, "device_revoked");

    let response = post_json(
        app.clone(),
        "/account-rooms/bootstrap",
        &BootstrapAccountRoomRequest {
            room_id: target_room_id.clone(),
            mls_group_id: target_mls_group_id.clone(),
            creator: alice.clone(),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let target_add = submit_add_device_request(
        &target_room_id,
        &target_mls_group_id,
        &alice,
        &bob,
        "welcome-revoked-target",
        "commit-revoked-target",
    );
    let response = post_json(app, "/commits", &target_add).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let error: ErrorResponse = read_json(response).await;
    assert_eq!(error.kind, "device_revoked");
}

#[tokio::test]
async fn sqlite_batch_key_package_claim_replays_exact_response_after_restart() {
    let temp = TempDir::new().expect("tempdir");
    let db_path = temp.path().join("delivery.sqlite3");
    let phone = member("alice-phone");
    let laptop = member("alice-laptop");
    let missing = member("alice-tablet");
    let other = member("bob-phone");

    let app = persistent_app(&db_path);
    for publication in [
        key_package_publication("kp-phone-1", phone.clone(), b"phone-one"),
        key_package_publication("kp-phone-2", phone.clone(), b"phone-two"),
        key_package_publication("kp-laptop-1", laptop.clone(), b"laptop-one"),
        key_package_publication("kp-other-1", other.clone(), b"other-one"),
    ] {
        assert_eq!(
            post_json(app.clone(), "/key-packages", &publication)
                .await
                .status(),
            StatusCode::OK
        );
    }

    let request = ClaimKeyPackagesRequest {
        owners: vec![laptop.clone(), phone.clone(), missing.clone()],
        idempotency_key: Some("fanout-claim-replay".to_owned()),
    };
    let response = post_json(app, "/key-packages/claims", &request).await;
    assert_eq!(response.status(), StatusCode::OK);
    let claimed: Vec<HttpKeyPackageClaim> = read_json(response).await;
    assert_eq!(claimed.len(), 3);
    assert_eq!(claimed[0].owner, laptop);
    assert_eq!(
        claimed[0]
            .claimed
            .as_ref()
            .expect("laptop claim")
            .key_package_id
            .as_slice(),
        b"kp-laptop-1"
    );
    assert_eq!(claimed[1].owner, phone.clone());
    assert_eq!(
        claimed[1]
            .claimed
            .as_ref()
            .expect("phone claim")
            .key_package_id
            .as_slice(),
        b"kp-phone-1"
    );
    assert_eq!(claimed[2].owner, missing);
    assert_eq!(claimed[2].claimed, None);

    let app = persistent_app(&db_path);
    let response = post_json(app.clone(), "/key-packages/claims", &request).await;
    assert_eq!(response.status(), StatusCode::OK);
    let replayed: Vec<HttpKeyPackageClaim> = read_json(response).await;
    assert_eq!(replayed, claimed);

    let response = post_json(
        app.clone(),
        "/key-packages/claim",
        &ClaimKeyPackageRequest {
            owner: phone.clone(),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let remaining_phone: Option<HttpClaimedKeyPackage> = read_json(response).await;
    assert_eq!(
        remaining_phone
            .expect("second phone package remains available")
            .key_package_id
            .as_slice(),
        b"kp-phone-2"
    );

    let response = post_json(
        app,
        "/key-packages/claim",
        &ClaimKeyPackageRequest { owner: other },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let other_claim: Option<HttpClaimedKeyPackage> = read_json(response).await;
    assert_eq!(
        other_claim
            .expect("other owner package remains available")
            .key_package_id
            .as_slice(),
        b"kp-other-1"
    );
}

#[tokio::test]
async fn sqlite_batch_key_package_claim_conflict_has_no_side_effects() {
    let temp = TempDir::new().expect("tempdir");
    let db_path = temp.path().join("delivery.sqlite3");
    let phone = member("conflict-phone");
    let laptop = member("conflict-laptop");

    let app = persistent_app(&db_path);
    for publication in [
        key_package_publication("kp-conflict-phone", phone.clone(), b"phone"),
        key_package_publication("kp-conflict-laptop", laptop.clone(), b"laptop"),
    ] {
        assert_eq!(
            post_json(app.clone(), "/key-packages", &publication)
                .await
                .status(),
            StatusCode::OK
        );
    }

    let first = ClaimKeyPackagesRequest {
        owners: vec![phone.clone()],
        idempotency_key: Some("fanout-conflict".to_owned()),
    };
    assert_eq!(
        post_json(app.clone(), "/key-packages/claims", &first)
            .await
            .status(),
        StatusCode::OK
    );

    let conflicting = ClaimKeyPackagesRequest {
        owners: vec![laptop.clone()],
        idempotency_key: Some("fanout-conflict".to_owned()),
    };
    let app = persistent_app(&db_path);
    let response = post_json(app.clone(), "/key-packages/claims", &conflicting).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let error: ErrorResponse = read_json(response).await;
    assert_eq!(error.kind, "idempotency_conflict");

    let response = post_json(
        app,
        "/key-packages/claim",
        &ClaimKeyPackageRequest { owner: laptop },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let laptop_claim: Option<HttpClaimedKeyPackage> = read_json(response).await;
    assert_eq!(
        laptop_claim
            .expect("conflict must not consume laptop package")
            .key_package_id
            .as_slice(),
        b"kp-conflict-laptop"
    );
}

#[tokio::test]
async fn sqlite_fanout_room_plan_survives_restart_and_reprepare() {
    let temp = TempDir::new().expect("tempdir");
    let db_path = temp.path().join("delivery.sqlite3");
    let fanout_id = "fanout-reprepare".to_owned();
    let room_id = group_id("fanout-room");
    let request = SaveFanoutRoomRequest {
        fanout_id: fanout_id.clone(),
        target_owner: member("alice-phone"),
        room: fanout_room_plan("fanout-room", "kp-fanout-1", "welcome-fanout-1", "link-1"),
    };

    let app = persistent_app(&db_path);
    let response = post_json(app, "/fanouts/rooms", &request).await;
    assert_eq!(response.status(), StatusCode::OK);
    let fanout: HttpFanoutPlan = read_json(response).await;
    assert_eq!(fanout.rooms.len(), 1);
    assert_eq!(fanout.rooms[0].status, HttpFanoutRoomStatus::Pending);

    let app = persistent_app(&db_path);
    let response = post_json(
        app.clone(),
        "/fanouts/rooms/prepared",
        &MarkFanoutPreparedRequest {
            fanout_id: fanout_id.clone(),
            room_id: room_id.clone(),
            prepared_message_id: id("prepared-loser"),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let fanout: HttpFanoutPlan = read_json(response).await;
    assert_eq!(
        fanout.rooms[0].status,
        HttpFanoutRoomStatus::Prepared {
            prepared_message_id: id("prepared-loser")
        }
    );

    let app = persistent_app(&db_path);
    let response = post_json(
        app.clone(),
        "/fanouts/rooms/prepared",
        &MarkFanoutPreparedRequest {
            fanout_id: fanout_id.clone(),
            room_id: room_id.clone(),
            prepared_message_id: id("prepared-retry"),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let fanout: HttpFanoutPlan = read_json(response).await;
    assert_eq!(
        fanout.rooms[0].status,
        HttpFanoutRoomStatus::Prepared {
            prepared_message_id: id("prepared-retry")
        }
    );

    let response = post_json(
        app.clone(),
        "/fanouts/rooms/done",
        &MarkFanoutDoneRequest {
            fanout_id: fanout_id.clone(),
            room_id: room_id.clone(),
            prepared_message_id: id("prepared-loser"),
            accepted_seq: 7,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let error: ErrorResponse = read_json(response).await;
    assert_eq!(error.kind, "fanout_conflict");

    let response = post_json(
        app,
        "/fanouts/rooms/done",
        &MarkFanoutDoneRequest {
            fanout_id: fanout_id.clone(),
            room_id,
            prepared_message_id: id("prepared-retry"),
            accepted_seq: 8,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let fanout: HttpFanoutPlan = read_json(response).await;
    assert_eq!(
        fanout.rooms[0].status,
        HttpFanoutRoomStatus::Done {
            prepared_message_id: id("prepared-retry"),
            accepted_seq: 8,
        }
    );

    let app = persistent_app(&db_path);
    let response = post_json(app, "/fanouts/get", &GetFanoutRequest { fanout_id }).await;
    assert_eq!(response.status(), StatusCode::OK);
    let fanout: Option<HttpFanoutPlan> = read_json(response).await;
    assert_eq!(
        fanout.expect("stored fanout").rooms[0].status,
        HttpFanoutRoomStatus::Done {
            prepared_message_id: id("prepared-retry"),
            accepted_seq: 8,
        }
    );
}

#[tokio::test]
async fn sqlite_fanout_room_plan_conflict_does_not_overwrite_existing_plan() {
    let temp = TempDir::new().expect("tempdir");
    let db_path = temp.path().join("delivery.sqlite3");
    let fanout_id = "fanout-conflict".to_owned();
    let initial = SaveFanoutRoomRequest {
        fanout_id: fanout_id.clone(),
        target_owner: member("alice-phone"),
        room: fanout_room_plan(
            "fanout-conflict-room",
            "kp-original",
            "welcome-original",
            "link",
        ),
    };
    let conflicting = SaveFanoutRoomRequest {
        fanout_id: fanout_id.clone(),
        target_owner: member("alice-phone"),
        room: fanout_room_plan(
            "fanout-conflict-room",
            "kp-other",
            "welcome-original",
            "link",
        ),
    };

    let app = persistent_app(&db_path);
    assert_eq!(
        post_json(app.clone(), "/fanouts/rooms", &initial)
            .await
            .status(),
        StatusCode::OK
    );
    let response = post_json(app, "/fanouts/rooms", &conflicting).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let error: ErrorResponse = read_json(response).await;
    assert_eq!(error.kind, "fanout_conflict");

    let app = persistent_app(&db_path);
    let response = post_json(app, "/fanouts/get", &GetFanoutRequest { fanout_id }).await;
    assert_eq!(response.status(), StatusCode::OK);
    let fanout: Option<HttpFanoutPlan> = read_json(response).await;
    assert_eq!(
        fanout.expect("stored fanout").rooms[0]
            .plan
            .key_package_id
            .as_slice(),
        b"kp-original"
    );
}

#[tokio::test]
async fn sqlite_account_room_directory_pages_and_survives_restart() {
    let temp = TempDir::new().expect("tempdir");
    let db_path = temp.path().join("delivery.sqlite3");
    let first_record = AccountRoomRecord {
        room_id: "room-a".to_owned(),
        mls_group_id: "mls-a".to_owned(),
        current_epoch: 1,
        last_seq: 7,
        status: RoomStatus::Open,
        devices: vec![
            AccountRoomDevice {
                device: DeviceRef {
                    account_id: "bob".to_owned(),
                    device_id: "bob-laptop".to_owned(),
                },
                active: true,
            },
            AccountRoomDevice {
                device: DeviceRef {
                    account_id: "alice".to_owned(),
                    device_id: "alice-laptop".to_owned(),
                },
                active: true,
            },
        ],
    };
    let first_expected = AccountRoomRecord {
        devices: vec![AccountRoomDevice {
            device: DeviceRef {
                account_id: "alice".to_owned(),
                device_id: "alice-laptop".to_owned(),
            },
            active: true,
        }],
        ..first_record.clone()
    };
    let second_record = AccountRoomRecord {
        room_id: "room-b".to_owned(),
        mls_group_id: "mls-b".to_owned(),
        current_epoch: 3,
        last_seq: 11,
        status: RoomStatus::Open,
        devices: vec![
            AccountRoomDevice {
                device: DeviceRef {
                    account_id: "alice".to_owned(),
                    device_id: "alice-laptop".to_owned(),
                },
                active: true,
            },
            AccountRoomDevice {
                device: DeviceRef {
                    account_id: "alice".to_owned(),
                    device_id: "alice-phone".to_owned(),
                },
                active: false,
            },
        ],
    };
    let first = SaveAccountRoomRequest {
        account_id: "alice".to_owned(),
        room_id: "room-a".to_owned(),
        record: serde_json::to_value(&first_record).expect("first record json"),
    };
    let second = SaveAccountRoomRequest {
        account_id: "alice".to_owned(),
        room_id: "room-b".to_owned(),
        record: serde_json::to_value(&second_record).expect("second record json"),
    };
    let wrong_account = SaveAccountRoomRequest {
        account_id: "alice".to_owned(),
        room_id: "room-wrong".to_owned(),
        record: serde_json::to_value(&AccountRoomRecord {
            room_id: "room-wrong".to_owned(),
            mls_group_id: "mls-wrong".to_owned(),
            current_epoch: 1,
            last_seq: 3,
            status: RoomStatus::Open,
            devices: vec![AccountRoomDevice {
                device: DeviceRef {
                    account_id: "bob".to_owned(),
                    device_id: "bob-laptop".to_owned(),
                },
                active: true,
            }],
        })
        .expect("wrong-account record json"),
    };

    let app = persistent_app(&db_path);
    let response = post_json(app.clone(), "/account-rooms", &second).await;
    assert_eq!(response.status(), StatusCode::OK);
    let saved: SaveAccountRoomResponse = read_json(response).await;
    assert!(saved.saved);
    assert_eq!(
        post_json(app.clone(), "/account-rooms", &first)
            .await
            .status(),
        StatusCode::OK
    );
    let response = post_json(app, "/account-rooms", &wrong_account).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let error: ErrorResponse = read_json(response).await;
    assert_eq!(error.kind, "invalid_account_room_request");

    let app = persistent_app(&db_path);
    let response = post_json(
        app.clone(),
        "/account-rooms/list",
        &ListAccountRoomDirectoryRequest {
            account_id: "alice".to_owned(),
            after_room_id: None,
            limit: 1,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let page: ListAccountRoomDirectoryResponse = read_json(response).await;
    assert_eq!(
        page.rooms,
        vec![serde_json::to_value(&first_expected).expect("first expected json")]
    );
    assert_eq!(page.next_after_room_id.as_deref(), Some("room-a"));
    assert!(page.has_more);

    let response = post_json(
        app,
        "/account-rooms/list",
        &ListAccountRoomDirectoryRequest {
            account_id: "alice".to_owned(),
            after_room_id: Some("room-a".to_owned()),
            limit: 10,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let page: ListAccountRoomDirectoryResponse = read_json(response).await;
    assert_eq!(
        page.rooms,
        vec![serde_json::to_value(&second_record).expect("second expected json")]
    );
    assert_eq!(page.next_after_room_id.as_deref(), Some("room-b"));
    assert!(!page.has_more);
}

#[tokio::test]
async fn sqlite_account_room_bootstrap_survives_restart_and_conflicts() {
    let temp = TempDir::new().expect("tempdir");
    let db_path = temp.path().join("delivery.sqlite3");
    let bootstrap = BootstrapAccountRoomRequest {
        room_id: "room-bootstrap".to_owned(),
        mls_group_id: "mls-bootstrap".to_owned(),
        creator: DeviceRef {
            account_id: "alice".to_owned(),
            device_id: "alice-laptop".to_owned(),
        },
    };

    let app = persistent_app(&db_path);
    let response = post_json(app, "/account-rooms/bootstrap", &bootstrap).await;
    assert_eq!(response.status(), StatusCode::OK);
    let bootstrapped: BootstrapAccountRoomResponse = read_json(response).await;
    assert!(bootstrapped.bootstrapped);

    let app = persistent_app(&db_path);
    let response = post_json(
        app.clone(),
        "/account-rooms/list",
        &ListAccountRoomDirectoryRequest {
            account_id: "alice".to_owned(),
            after_room_id: None,
            limit: 10,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let page: ListAccountRoomDirectoryResponse = read_json(response).await;
    assert_eq!(page.rooms.len(), 1);
    assert_eq!(page.rooms[0]["room_id"], "room-bootstrap");
    assert_eq!(page.rooms[0]["mls_group_id"], "mls-bootstrap");
    assert_eq!(page.rooms[0]["current_epoch"], 0);
    assert_eq!(page.rooms[0]["last_seq"], 0);
    assert_eq!(page.rooms[0]["devices"][0]["device"]["account_id"], "alice");
    assert_eq!(
        page.rooms[0]["devices"][0]["device"]["device_id"],
        "alice-laptop"
    );
    assert_eq!(page.rooms[0]["devices"][0]["active"], true);

    let response = post_json(app.clone(), "/account-rooms/bootstrap", &bootstrap).await;
    assert_eq!(response.status(), StatusCode::OK);
    let replayed: BootstrapAccountRoomResponse = read_json(response).await;
    assert!(!replayed.bootstrapped);

    let response = post_json(
        app,
        "/account-rooms/bootstrap",
        &BootstrapAccountRoomRequest {
            creator: DeviceRef {
                account_id: "alice".to_owned(),
                device_id: "alice-phone".to_owned(),
            },
            ..bootstrap
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let error: ErrorResponse = read_json(response).await;
    assert_eq!(error.kind, "account_room_bootstrap_conflict");
}

#[tokio::test]
async fn sqlite_account_room_bootstrap_rejects_raw_commit_history_without_membership_delta() {
    let temp = TempDir::new().expect("tempdir");
    let db_path = temp.path().join("delivery.sqlite3");
    let room_id = "room-bootstrap-raw-commit-rejected".to_owned();
    let mls_group_id = "mls-bootstrap-raw-commit-rejected".to_owned();
    let alice = DeviceRef::new("alice", "alice-laptop");
    let app = persistent_app(&db_path);

    let response = post_json(
        app.clone(),
        "/messages",
        &raw_commit_publish_request(
            &room_id,
            &mls_group_id,
            &alice,
            0,
            "raw-bootstrap-commit-without-delta",
            "raw-bootstrap-commit-without-delta-idempotency",
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let app = persistent_app(&db_path);
    let response = post_json(
        app,
        "/account-rooms/bootstrap",
        &BootstrapAccountRoomRequest {
            room_id,
            mls_group_id,
            creator: alice,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let error: ErrorResponse = read_json(response).await;
    assert_eq!(error.kind, "room_membership_conflict");
    assert!(
        error
            .error
            .contains("existing raw commit history to carry membership_delta")
    );
}

#[tokio::test]
async fn sqlite_submit_commit_route_publishes_room_entry_and_derives_membership_after_restart() {
    let temp = TempDir::new().expect("tempdir");
    let db_path = temp.path().join("delivery.sqlite3");
    let creator = DeviceRef::new("alice", "alice-laptop");
    let phone = DeviceRef::new("alice", "alice-phone");
    let room_id = "room-submit-commit-route".to_owned();
    let mls_group_id = "mls-submit-commit-route".to_owned();
    let welcome_id = "welcome-submit-commit-route".to_owned();
    let request = submit_add_device_request(
        &room_id,
        &mls_group_id,
        &creator,
        &phone,
        &welcome_id,
        "commit-route-idempotency",
    );
    let expected_message_id = request
        .envelope
        .message_id()
        .expect("commit envelope message id");

    let app = persistent_app(&db_path);
    let response = post_json(
        app.clone(),
        "/account-rooms/bootstrap",
        &BootstrapAccountRoomRequest {
            room_id: room_id.clone(),
            mls_group_id: mls_group_id.clone(),
            creator: creator.clone(),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    publish_and_claim_key_package_for_add(&app, &request).await;
    let response = post_json(app, "/commits", &request).await;
    assert_eq!(response.status(), StatusCode::OK);
    let accepted: CommitAccepted = read_json(response).await;
    assert_eq!(accepted.seq, 1);
    assert_eq!(accepted.message_id, expected_message_id);
    assert_eq!(accepted.released_welcomes, vec![welcome_id.clone()]);

    let app = persistent_app(&db_path);
    let response = post_json(app.clone(), "/commits", &request).await;
    assert_eq!(response.status(), StatusCode::OK);
    let replayed: CommitAccepted = read_json(response).await;
    assert_eq!(replayed, accepted);

    let response = post_json(
        app.clone(),
        "/sync/group",
        &GroupSyncRequest {
            group_id: group_id(&room_id),
            after_seq: 0,
            limit: 10,
            requester: None,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let group_page: HttpSyncPage = read_json(response).await;
    assert_eq!(group_page.entries.len(), 1);
    assert_eq!(group_page.entries[0].seq, accepted.seq);
    assert_eq!(group_page.entries[0].message.id, id(&accepted.message_id));
    let projection: FiniteAccountRoomCommitProjection =
        serde_json::from_slice(&group_page.entries[0].message.payload)
            .expect("commit projection payload");
    assert_eq!(projection.entry.message_id, accepted.message_id);
    assert_eq!(projection.entry.room_id, room_id);
    assert_eq!(projection.entry.kind, LogEntryKind::Commit);
    assert_eq!(projection.membership_delta, request.membership_delta);

    let recipient = member_for_device(&DeviceRef::new("alice", "alice-phone"));
    let response = post_json(
        app.clone(),
        "/sync/inbox",
        &InboxSyncRequest {
            recipient: recipient.clone(),
            after_seq: 0,
            limit: 10,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let inbox_page: HttpSyncPage = read_json(response).await;
    assert_eq!(inbox_page.entries.len(), 1);
    assert_eq!(inbox_page.entries[0].seq, 1);
    assert_eq!(inbox_page.entries[0].message.id, id(&welcome_id));
    let welcome: WelcomeRecord =
        serde_json::from_slice(&inbox_page.entries[0].message.payload).expect("welcome payload");
    assert_eq!(welcome.welcome_id, welcome_id);
    assert_eq!(welcome.commit_seq, accepted.seq);
    assert_eq!(welcome.recipient, DeviceRef::new("alice", "alice-phone"));
    assert_eq!(welcome.state, WelcomeState::Released);

    let response = post_json(
        app.clone(),
        "/account-rooms/list",
        &ListAccountRoomDirectoryRequest {
            account_id: "alice".to_owned(),
            after_room_id: None,
            limit: 10,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let page: ListAccountRoomDirectoryResponse = read_json(response).await;
    assert_eq!(page.rooms.len(), 1);
    assert_eq!(page.rooms[0]["current_epoch"], 1);
    assert_eq!(page.rooms[0]["last_seq"], accepted.seq);
    assert_eq!(page.rooms[0]["devices"][0]["active"], true);
    assert_eq!(
        page.rooms[0]["devices"][1]["device"]["device_id"],
        "alice-phone"
    );
    assert_eq!(page.rooms[0]["devices"][1]["active"], false);
}

#[tokio::test]
async fn sqlite_submit_commit_validates_and_consumes_claimed_key_package_after_restart() {
    let temp = TempDir::new().expect("tempdir");
    let db_path = temp.path().join("delivery.sqlite3");
    let creator = DeviceRef::new("alice", "alice-laptop");
    let phone = DeviceRef::new("alice", "alice-phone");
    let tablet = DeviceRef::new("alice", "alice-tablet");
    let room_id = "room-submit-key-package-lifecycle".to_owned();
    let mls_group_id = "mls-submit-key-package-lifecycle".to_owned();
    let request = submit_add_device_request(
        &room_id,
        &mls_group_id,
        &creator,
        &phone,
        "welcome-key-package-lifecycle-phone",
        "key-package-lifecycle-phone",
    );

    let app = persistent_app(&db_path);
    let response = post_json(
        app.clone(),
        "/account-rooms/bootstrap",
        &BootstrapAccountRoomRequest {
            room_id: room_id.clone(),
            mls_group_id: mls_group_id.clone(),
            creator: creator.clone(),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let response = post_json(app.clone(), "/commits", &request).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let error: ErrorResponse = read_json(response).await;
    assert_eq!(error.kind, "invalid_commit_request");
    assert!(error.error.contains("must be published and claimed"));
    assert_submit_commit_had_no_side_effects(&app, &room_id, &phone).await;

    publish_and_claim_key_package_for_add(&app, &request).await;
    let inventory = key_package_inventory_for_device(&app, &phone).await;
    assert_eq!(inventory.available, 0);
    assert_eq!(inventory.claimed, 1);

    let mut stale_ref = request.clone();
    stale_ref.membership_delta.adds[0].key_package_ref = "stale-ref".to_owned();
    let response = post_json(app.clone(), "/commits", &stale_ref).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let error: ErrorResponse = read_json(response).await;
    assert_eq!(error.kind, "invalid_commit_request");
    assert!(error.error.contains("metadata does not match"));
    assert_submit_commit_had_no_side_effects(&app, &room_id, &phone).await;
    let inventory = key_package_inventory_for_device(&app, &phone).await;
    assert_eq!(inventory.available, 0);
    assert_eq!(inventory.claimed, 1);

    let response = post_json(app.clone(), "/commits", &request).await;
    assert_eq!(response.status(), StatusCode::OK);
    let accepted: CommitAccepted = read_json(response).await;
    assert_eq!(accepted.seq, 1);
    assert_eq!(
        accepted.released_welcomes,
        vec!["welcome-key-package-lifecycle-phone".to_owned()]
    );
    let inventory = key_package_inventory_for_device(&app, &phone).await;
    assert_eq!(inventory.available, 0);
    assert_eq!(inventory.claimed, 0);

    let app = persistent_app(&db_path);
    let inventory = key_package_inventory_for_device(&app, &phone).await;
    assert_eq!(inventory.available, 0);
    assert_eq!(inventory.claimed, 0);
    let response = post_json(
        app.clone(),
        "/key-packages/leases/expire",
        &ExpireKeyPackageLeaseRequest {
            key_package_id: HttpKeyPackageId::new(
                request.membership_delta.adds[0]
                    .key_package_id
                    .as_bytes()
                    .to_vec(),
            ),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let error: ErrorResponse = read_json(response).await;
    assert_eq!(error.kind, "invalid_key_package_lease_request");
    assert!(error.error.contains("already consumed"));

    let mut reuse = submit_add_device_request_at_epoch_with_ids(
        &room_id,
        &mls_group_id,
        &creator,
        &tablet,
        1,
        "welcome-key-package-lifecycle-reuse",
        "key-package-lifecycle-reuse",
    );
    reuse.membership_delta.adds[0].key_package_id =
        request.membership_delta.adds[0].key_package_id.clone();
    reuse.membership_delta.adds[0].key_package_ref =
        request.membership_delta.adds[0].key_package_ref.clone();
    reuse.membership_delta.adds[0].key_package_hash =
        request.membership_delta.adds[0].key_package_hash.clone();
    let response = post_json(app.clone(), "/commits", &reuse).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let error: ErrorResponse = read_json(response).await;
    assert_eq!(error.kind, "invalid_commit_request");
    assert!(error.error.contains("already consumed"));

    let response = post_json(
        app,
        "/sync/inbox",
        &InboxSyncRequest {
            recipient: member_for_device(&tablet),
            after_seq: 0,
            limit: 10,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let page: HttpSyncPage = read_json(response).await;
    assert!(page.entries.is_empty());
}

#[tokio::test]
async fn sqlite_submit_commit_rejects_account_device_cap_before_side_effects() {
    let temp = TempDir::new().expect("tempdir");
    let db_path = temp.path().join("delivery.sqlite3");
    let creator = DeviceRef::new("alice", "alice-laptop");
    let room_id = "room-account-device-cap".to_owned();
    let mls_group_id = "mls-account-device-cap".to_owned();
    let app = persistent_app(&db_path);

    let response = post_json(
        app.clone(),
        "/account-rooms/bootstrap",
        &BootstrapAccountRoomRequest {
            room_id: room_id.clone(),
            mls_group_id: mls_group_id.clone(),
            creator: creator.clone(),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    for index in 0..(MAX_ACCOUNT_DEVICES_PER_ROOM - 1) {
        let device = DeviceRef::new("alice", format!("alice-extra-{index}"));
        let request = submit_add_device_request_at_epoch_with_ids(
            &room_id,
            &mls_group_id,
            &creator,
            &device,
            u64::from(index),
            &format!("welcome-account-cap-{index}"),
            &format!("commit-account-cap-{index}"),
        );
        publish_and_claim_key_package_for_add(&app, &request).await;
        let response = post_json(app.clone(), "/commits", &request).await;
        assert_eq!(response.status(), StatusCode::OK);
        let accepted: CommitAccepted = read_json(response).await;
        assert_eq!(accepted.seq, u64::from(index) + 1);
    }

    let overflow = DeviceRef::new("alice", "alice-extra-overflow");
    let overflow_request = submit_add_device_request_at_epoch_with_ids(
        &room_id,
        &mls_group_id,
        &creator,
        &overflow,
        u64::from(MAX_ACCOUNT_DEVICES_PER_ROOM - 1),
        "welcome-account-cap-overflow",
        "commit-account-cap-overflow",
    );
    publish_and_claim_key_package_for_add(&app, &overflow_request).await;
    let response = post_json(app.clone(), "/commits", &overflow_request).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let error: ErrorResponse = read_json(response).await;
    assert_eq!(error.kind, "invalid_commit_request");
    assert!(error.error.contains("room.devices_per_account"));

    let app = persistent_app(&db_path);
    let response = post_json(
        app.clone(),
        "/sync/group",
        &GroupSyncRequest {
            group_id: group_id(&room_id),
            after_seq: u64::from(MAX_ACCOUNT_DEVICES_PER_ROOM - 1),
            limit: 10,
            requester: Some(member_for_device(&creator)),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let page: HttpSyncPage = read_json(response).await;
    assert!(page.entries.is_empty());
    assert_eq!(
        page.next_after_seq,
        u64::from(MAX_ACCOUNT_DEVICES_PER_ROOM - 1)
    );

    let page = account_room_page(&app, "alice").await;
    assert_eq!(page.rooms.len(), 1);
    assert_eq!(
        page.rooms[0]["devices"].as_array().expect("devices").len(),
        MAX_ACCOUNT_DEVICES_PER_ROOM as usize
    );
    assert!(
        !page.rooms[0]["devices"]
            .as_array()
            .expect("devices")
            .iter()
            .any(|device| device["device"]["device_id"] == "alice-extra-overflow")
    );

    let response = post_json(
        app.clone(),
        "/sync/inbox",
        &InboxSyncRequest {
            recipient: member_for_device(&overflow),
            after_seq: 0,
            limit: 10,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let page: HttpSyncPage = read_json(response).await;
    assert!(page.entries.is_empty());

    let inventory = key_package_inventory_for_device(&app, &overflow).await;
    assert_eq!(inventory.available, 0);
    assert_eq!(inventory.claimed, 1);
}

#[tokio::test]
async fn sqlite_submit_commit_rejects_duplicate_pending_device_before_side_effects() {
    let temp = TempDir::new().expect("tempdir");
    let db_path = temp.path().join("delivery.sqlite3");
    let creator = DeviceRef::new("alice", "alice-laptop");
    let bob = DeviceRef::new("bob", "bob-phone");
    let room_id = "room-duplicate-pending-add".to_owned();
    let mls_group_id = "mls-duplicate-pending-add".to_owned();
    let app = persistent_app(&db_path);

    let response = post_json(
        app.clone(),
        "/account-rooms/bootstrap",
        &BootstrapAccountRoomRequest {
            room_id: room_id.clone(),
            mls_group_id: mls_group_id.clone(),
            creator: creator.clone(),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let add_bob = submit_add_device_request(
        &room_id,
        &mls_group_id,
        &creator,
        &bob,
        "welcome-duplicate-pending-bob",
        "commit-duplicate-pending-bob",
    );
    publish_and_claim_key_package_for_add(&app, &add_bob).await;
    let response = post_json(app.clone(), "/commits", &add_bob).await;
    assert_eq!(response.status(), StatusCode::OK);
    let accepted: CommitAccepted = read_json(response).await;
    assert_eq!(accepted.seq, 1);

    let duplicate = submit_add_device_request_at_epoch_with_ids(
        &room_id,
        &mls_group_id,
        &creator,
        &bob,
        1,
        "welcome-duplicate-pending-bob-retry",
        "commit-duplicate-pending-bob-retry",
    );
    publish_and_claim_key_package_for_add(&app, &duplicate).await;
    let response = post_json(app.clone(), "/commits", &duplicate).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let error: ErrorResponse = read_json(response).await;
    assert_eq!(error.kind, "invalid_commit_request");
    assert!(error.error.contains("already current or pending"));

    let app = persistent_app(&db_path);
    let response = post_json(
        app.clone(),
        "/sync/group",
        &GroupSyncRequest {
            group_id: group_id(&room_id),
            after_seq: accepted.seq,
            limit: 10,
            requester: Some(member_for_device(&creator)),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let page: HttpSyncPage = read_json(response).await;
    assert!(page.entries.is_empty());
    assert_eq!(page.next_after_seq, accepted.seq);

    let response = post_json(
        app.clone(),
        "/sync/inbox",
        &InboxSyncRequest {
            recipient: member_for_device(&bob),
            after_seq: 0,
            limit: 10,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let page: HttpSyncPage = read_json(response).await;
    assert_eq!(page.entries.len(), 1);
    assert_eq!(
        page.entries[0].message.id,
        id("welcome-duplicate-pending-bob")
    );

    let account_page = account_room_page(&app, "bob").await;
    assert_eq!(account_page.rooms.len(), 1);
    let devices = account_page.rooms[0]["devices"]
        .as_array()
        .expect("devices");
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0]["device"]["device_id"], "bob-phone");
    assert_eq!(devices[0]["active"], false);

    let inventory = key_package_inventory_for_device(&app, &bob).await;
    assert_eq!(inventory.available, 0);
    assert_eq!(inventory.claimed, 1);
}

#[tokio::test]
async fn sqlite_welcome_not_released_before_accepted_commit_over_http() {
    let temp = TempDir::new().expect("tempdir");
    let db_path = temp.path().join("delivery.sqlite3");
    let creator = DeviceRef::new("alice", "alice-laptop");
    let phone = DeviceRef::new("alice", "alice-phone");
    let room_id = "room-welcome-release-coupling".to_owned();
    let mls_group_id = "mls-welcome-release-coupling".to_owned();
    let welcome_id = "welcome-release-coupling-phone".to_owned();
    let request = submit_add_device_request(
        &room_id,
        &mls_group_id,
        &creator,
        &phone,
        &welcome_id,
        "welcome-release-coupling",
    );
    let mut rejected = request.clone();
    rejected.membership_delta.adds[0].key_package_hash = "wrong-hash".to_owned();

    let app = persistent_app(&db_path);
    let response = post_json(
        app.clone(),
        "/account-rooms/bootstrap",
        &BootstrapAccountRoomRequest {
            room_id: room_id.clone(),
            mls_group_id: mls_group_id.clone(),
            creator: creator.clone(),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    publish_and_claim_key_package_for_add(&app, &request).await;

    let response = post_json(app.clone(), "/commits", &rejected).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let error: ErrorResponse = read_json(response).await;
    assert_eq!(error.kind, "invalid_commit_request");
    assert!(error.error.contains("metadata does not match"));
    assert_submit_commit_had_no_side_effects(&app, &room_id, &phone).await;

    let app = persistent_app(&db_path);
    assert_submit_commit_had_no_side_effects(&app, &room_id, &phone).await;

    let response = post_json(app.clone(), "/commits", &request).await;
    assert_eq!(response.status(), StatusCode::OK);
    let accepted: CommitAccepted = read_json(response).await;
    assert_eq!(accepted.seq, 1);
    assert_eq!(accepted.released_welcomes, vec![welcome_id.clone()]);

    let response = post_json(
        app,
        "/sync/inbox",
        &InboxSyncRequest {
            recipient: member_for_device(&phone),
            after_seq: 0,
            limit: 10,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let page: HttpSyncPage = read_json(response).await;
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].message.id, id(&welcome_id));
    let welcome: WelcomeRecord =
        serde_json::from_slice(&page.entries[0].message.payload).expect("welcome payload");
    assert_eq!(welcome.state, WelcomeState::Released);
}

#[tokio::test]
async fn sqlite_submit_commit_replay_repairs_projection_after_partial_durable_publish() {
    let temp = TempDir::new().expect("tempdir");
    let db_path = temp.path().join("delivery.sqlite3");
    let creator = DeviceRef::new("alice", "alice-laptop");
    let phone = DeviceRef::new("alice", "alice-phone");
    let room_id = "room-submit-partial-replay".to_owned();
    let mls_group_id = "mls-submit-partial-replay".to_owned();
    let welcome_id = "welcome-submit-partial-replay".to_owned();
    let request = submit_add_device_request(
        &room_id,
        &mls_group_id,
        &creator,
        &phone,
        &welcome_id,
        "partial-replay-idempotency",
    );
    let message_id = request
        .envelope
        .message_id()
        .expect("commit envelope message id");
    let commit_publish = commit_publish_request_for_test(&request, &message_id);

    let app = persistent_app(&db_path);
    let response = post_json(
        app,
        "/account-rooms/bootstrap",
        &BootstrapAccountRoomRequest {
            room_id: room_id.clone(),
            mls_group_id,
            creator,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    // Model a process interruption after the commit publish/idempotency rows are
    // durable but before the finite projection writes run.
    insert_durable_commit_publish_without_projection(&db_path, &commit_publish, 1);

    let app = persistent_app(&db_path);
    let response = post_json(
        app.clone(),
        "/account-rooms/list",
        &ListAccountRoomDirectoryRequest {
            account_id: "alice".to_owned(),
            after_room_id: None,
            limit: 10,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let before_retry: ListAccountRoomDirectoryResponse = read_json(response).await;
    assert_eq!(before_retry.rooms.len(), 1);
    assert_eq!(before_retry.rooms[0]["current_epoch"], 0);

    let response = post_json(app.clone(), "/commits", &request).await;
    assert_eq!(response.status(), StatusCode::OK);
    let accepted: CommitAccepted = read_json(response).await;
    assert_eq!(accepted.seq, 1);
    assert_eq!(accepted.message_id, message_id);
    assert_eq!(accepted.released_welcomes, vec![welcome_id.clone()]);

    let app = persistent_app(&db_path);
    let response = post_json(app.clone(), "/commits", &request).await;
    assert_eq!(response.status(), StatusCode::OK);
    let replayed: CommitAccepted = read_json(response).await;
    assert_eq!(replayed, accepted);

    let response = post_json(
        app.clone(),
        "/account-rooms/list",
        &ListAccountRoomDirectoryRequest {
            account_id: "alice".to_owned(),
            after_room_id: None,
            limit: 10,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let after_retry: ListAccountRoomDirectoryResponse = read_json(response).await;
    assert_eq!(after_retry.rooms.len(), 1);
    assert_eq!(after_retry.rooms[0]["current_epoch"], 1);
    assert_eq!(after_retry.rooms[0]["last_seq"], accepted.seq);
    assert_eq!(
        after_retry.rooms[0]["devices"][1]["device"]["device_id"],
        "alice-phone"
    );
    assert_eq!(after_retry.rooms[0]["devices"][1]["active"], false);

    let response = post_json(
        app,
        "/sync/inbox",
        &InboxSyncRequest {
            recipient: member_for_device(&DeviceRef::new("alice", "alice-phone")),
            after_seq: 0,
            limit: 10,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let inbox_page: HttpSyncPage = read_json(response).await;
    assert_eq!(inbox_page.entries.len(), 1);
    assert_eq!(inbox_page.entries[0].message.id, id(&welcome_id));
}

#[tokio::test]
async fn sqlite_rejected_submit_commit_replays_rejection_after_restart() {
    let temp = TempDir::new().expect("tempdir");
    let db_path = temp.path().join("delivery.sqlite3");
    let creator = DeviceRef::new("alice", "alice-laptop");
    let phone = DeviceRef::new("alice", "alice-phone");
    let tablet = DeviceRef::new("alice", "alice-tablet");
    let room_id = "room-rejected-submit-replay".to_owned();
    let mls_group_id = "mls-rejected-submit-replay".to_owned();
    let winner = submit_add_device_request(
        &room_id,
        &mls_group_id,
        &creator,
        &phone,
        "welcome-rejected-submit-phone",
        "rejected-submit-winner",
    );
    let loser = submit_add_device_request(
        &room_id,
        &mls_group_id,
        &creator,
        &tablet,
        "welcome-rejected-submit-tablet",
        "rejected-submit-loser",
    );

    let app = persistent_app(&db_path);
    let response = post_json(
        app.clone(),
        "/account-rooms/bootstrap",
        &BootstrapAccountRoomRequest {
            room_id: room_id.clone(),
            mls_group_id: mls_group_id.clone(),
            creator: creator.clone(),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    publish_and_claim_key_package_for_add(&app, &winner).await;
    let response = post_json(app.clone(), "/commits", &winner).await;
    assert_eq!(response.status(), StatusCode::OK);
    let accepted: CommitAccepted = read_json(response).await;
    assert_eq!(accepted.seq, 1);

    let response = post_json(app, "/commits", &loser).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let first_error: ErrorResponse = read_json(response).await;
    assert_eq!(first_error.kind, "invalid_commit_request");
    assert!(
        first_error
            .error
            .contains("commit expected epoch 0 does not match room epoch 1")
    );

    let app = persistent_app(&db_path);
    let response = post_json(app.clone(), "/commits", &loser).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let replayed_error: ErrorResponse = read_json(response).await;
    assert_eq!(replayed_error, first_error);

    let response = post_json(
        app.clone(),
        "/sync/group",
        &GroupSyncRequest {
            group_id: group_id(&room_id),
            after_seq: 0,
            limit: 10,
            requester: None,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let page: HttpSyncPage = read_json(response).await;
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].seq, accepted.seq);

    let response = post_json(
        app.clone(),
        "/sync/inbox",
        &InboxSyncRequest {
            recipient: member_for_device(&tablet),
            after_seq: 0,
            limit: 10,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let inbox_page: HttpSyncPage = read_json(response).await;
    assert!(inbox_page.entries.is_empty());

    let response = post_json(
        app.clone(),
        "/account-rooms/list",
        &ListAccountRoomDirectoryRequest {
            account_id: "alice".to_owned(),
            after_room_id: None,
            limit: 10,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let page: ListAccountRoomDirectoryResponse = read_json(response).await;
    assert_eq!(page.rooms.len(), 1);
    assert_eq!(page.rooms[0]["current_epoch"], 1);
    assert_eq!(page.rooms[0]["last_seq"], accepted.seq);
    assert_eq!(
        page.rooms[0]["devices"].as_array().expect("devices").len(),
        2
    );
    assert!(
        !page.rooms[0]["devices"]
            .as_array()
            .expect("devices")
            .iter()
            .any(|device| device["device"]["device_id"] == "alice-tablet")
    );
}

#[tokio::test]
async fn sqlite_submit_commit_crash_matrix_rolls_back_and_retry_converges() {
    for crash_point in HttpSubmitCommitCrashPoint::ALL {
        let temp = TempDir::new().expect("tempdir");
        let db_path = temp.path().join("delivery.sqlite3");
        let creator = DeviceRef::new("alice", "alice-laptop");
        let phone = DeviceRef::new("alice", "alice-phone");
        let tablet = DeviceRef::new("alice", "alice-tablet");
        let room_id = "room-http-crash-matrix".to_owned();
        let mls_group_id = "mls-http-crash-matrix".to_owned();
        let first = submit_add_device_request(
            &room_id,
            &mls_group_id,
            &creator,
            &phone,
            "welcome-http-crash-phone",
            "http-crash-first",
        );
        let crash_request = submit_add_device_request_at_epoch_with_ids(
            &room_id,
            &mls_group_id,
            &creator,
            &tablet,
            1,
            "welcome-http-crash-tablet",
            "http-crash-matrix-commit",
        );

        let app = persistent_app(&db_path);
        let response = post_json(
            app.clone(),
            "/account-rooms/bootstrap",
            &BootstrapAccountRoomRequest {
                room_id: room_id.clone(),
                mls_group_id: mls_group_id.clone(),
                creator: creator.clone(),
            },
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        publish_and_claim_key_package_for_add(&app, &first).await;
        let response = post_json(app.clone(), "/commits", &first).await;
        assert_eq!(response.status(), StatusCode::OK);
        let first_accepted: CommitAccepted = read_json(response).await;
        assert_eq!(first_accepted.seq, 1);

        publish_and_claim_key_package_for_add(&app, &crash_request).await;
        install_http_submit_commit_crash_trigger(&db_path, crash_point);
        let response = post_json(app, "/commits", &crash_request).await;
        assert_eq!(
            response.status(),
            StatusCode::INTERNAL_SERVER_ERROR,
            "expected SQLite crash response at {crash_point:?}"
        );
        let error: ErrorResponse = read_json(response).await;
        assert_eq!(error.kind, "delivery_store");
        clear_http_submit_commit_crash_triggers(&db_path);

        let app = persistent_app(&db_path);
        assert_http_crash_commit_rolled_back(&app, &room_id, &tablet, first_accepted.seq).await;

        let response = post_json(app.clone(), "/commits", &crash_request).await;
        assert_eq!(response.status(), StatusCode::OK);
        let accepted: CommitAccepted = read_json(response).await;
        assert_eq!(accepted.seq, 2);
        assert_eq!(
            accepted.released_welcomes,
            vec!["welcome-http-crash-tablet".to_owned()]
        );

        let response = post_json(app.clone(), "/commits", &crash_request).await;
        assert_eq!(response.status(), StatusCode::OK);
        let replayed: CommitAccepted = read_json(response).await;
        assert_eq!(replayed, accepted);

        assert_http_crash_commit_converged(&app, &room_id, &tablet, accepted.seq).await;
    }
}

#[tokio::test]
async fn sqlite_raw_message_commit_projection_compatibility_survives_restart() {
    let temp = TempDir::new().expect("tempdir");
    let db_path = temp.path().join("delivery.sqlite3");
    let creator = DeviceRef::new("alice", "alice-laptop");
    let phone = DeviceRef::new("alice", "alice-phone");
    let room_id = "room-raw-projection-compat".to_owned();
    let mls_group_id = "mls-raw-projection-compat".to_owned();
    let request = submit_add_device_request(
        &room_id,
        &mls_group_id,
        &creator,
        &phone,
        "welcome-raw-projection-compat",
        "raw-projection-idempotency",
    );
    let message_id = request
        .envelope
        .message_id()
        .expect("commit envelope message id");
    let entry = finitechat_proto::RoomLogEntry {
        room_id: room_id.clone(),
        seq: 0,
        message_id: message_id.clone(),
        sender: creator.clone(),
        kind: LogEntryKind::Commit,
        epoch: request.expected_epoch,
        envelope: request.envelope.clone(),
        idempotency_key: request.idempotency_key.clone(),
    };
    let payload = serde_json::to_vec(&FiniteAccountRoomCommitProjection {
        entry,
        membership_delta: request.membership_delta.clone(),
    })
    .expect("projection payload");
    let transport_group_id = room_id.as_bytes().to_vec();

    let app = persistent_app(&db_path);
    let response = post_json(
        app.clone(),
        "/account-rooms/bootstrap",
        &BootstrapAccountRoomRequest {
            room_id: room_id.clone(),
            mls_group_id,
            creator,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let response = post_json(
        app,
        "/messages",
        &PublishMessageRequest {
            target: group_target(
                group_id(&room_id),
                transport_group_id.clone(),
                Some(HttpCommitAdmission {
                    source_epoch: EpochId(0),
                }),
            ),
            message: group_message(&message_id, transport_group_id, &payload),
            idempotency_key: Some("raw-projection-message-idempotency".to_owned()),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let app = persistent_app(&db_path);
    let response = post_json(
        app,
        "/account-rooms/list",
        &ListAccountRoomDirectoryRequest {
            account_id: "alice".to_owned(),
            after_room_id: None,
            limit: 10,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let page: ListAccountRoomDirectoryResponse = read_json(response).await;
    assert_eq!(page.rooms.len(), 1);
    assert_eq!(page.rooms[0]["room_id"], room_id);
    assert_eq!(page.rooms[0]["current_epoch"], 1);
    assert_eq!(
        page.rooms[0]["devices"][1]["device"]["device_id"],
        "alice-phone"
    );
    assert_eq!(page.rooms[0]["devices"][1]["active"], false);
}

#[tokio::test]
async fn submit_commit_route_rejects_missing_staged_welcome_before_side_effects() {
    let temp = TempDir::new().expect("tempdir");
    let db_path = temp.path().join("delivery.sqlite3");
    let room_id = "room-submit-missing-welcome".to_owned();
    let mut request = submit_add_device_request(
        &room_id,
        "mls-submit-missing-welcome",
        &DeviceRef::new("alice", "alice-laptop"),
        &DeviceRef::new("alice", "alice-phone"),
        "welcome-submit-missing-welcome",
        "missing-welcome-idempotency",
    );
    request.staged_welcomes.clear();

    let app = persistent_app(&db_path);
    let response = post_json(app.clone(), "/commits", &request).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let error: ErrorResponse = read_json(response).await;
    assert_eq!(error.kind, "invalid_commit_request");

    let app = persistent_app(&db_path);
    let response = post_json(
        app,
        "/sync/group",
        &GroupSyncRequest {
            group_id: group_id(&room_id),
            after_seq: 0,
            limit: 10,
            requester: None,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let page: HttpSyncPage = read_json(response).await;
    assert!(page.entries.is_empty());
}

#[tokio::test]
async fn sqlite_group_sync_filters_by_persisted_room_membership_projection() {
    let temp = TempDir::new().expect("tempdir");
    let db_path = temp.path().join("delivery.sqlite3");
    let room_id = "room-filtered-membership-sync".to_owned();
    let mls_group_id = "mls-filtered-membership-sync".to_owned();
    let alice = DeviceRef::new("alice", "alice-laptop");
    let bob = DeviceRef::new("bob", "bob-phone");
    let carol = DeviceRef::new("carol", "carol-phone");
    let app = persistent_app(&db_path);

    let response = post_json(
        app.clone(),
        "/account-rooms/bootstrap",
        &BootstrapAccountRoomRequest {
            room_id: room_id.clone(),
            mls_group_id: mls_group_id.clone(),
            creator: alice.clone(),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let response = post_json(
        app.clone(),
        "/events",
        &append_application_request(
            &room_id,
            &mls_group_id,
            &alice,
            0,
            b"hidden",
            "app-before-bob-idempotency",
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let hidden_acceptance: EventAccepted = read_json(response).await;
    assert_eq!(hidden_acceptance.seq, 1);

    let response = post_json(
        app.clone(),
        "/sync/group",
        &GroupSyncRequest {
            group_id: group_id(&room_id),
            after_seq: 0,
            limit: 10,
            requester: Some(member_for_device(&bob)),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let bob_hidden_page: HttpSyncPage = read_json(response).await;
    assert!(bob_hidden_page.entries.is_empty());
    assert_eq!(bob_hidden_page.next_after_seq, hidden_acceptance.seq);

    let request = submit_add_device_request(
        &room_id,
        &mls_group_id,
        &alice,
        &bob,
        "welcome-filtered-bob",
        "commit-filtered-bob",
    );
    let commit_message_id = request.envelope.message_id().expect("commit message id");
    publish_and_claim_key_package_for_add(&app, &request).await;
    let response = post_json(app.clone(), "/commits", &request).await;
    assert_eq!(response.status(), StatusCode::OK);
    let accepted: CommitAccepted = read_json(response).await;
    assert_eq!(accepted.seq, 2);

    let response = post_json(
        app.clone(),
        "/events",
        &append_application_request(
            &room_id,
            &mls_group_id,
            &bob,
            1,
            b"pending-send",
            "bob-pending-send-idempotency",
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let error: ErrorResponse = read_json(response).await;
    assert_eq!(error.kind, "sender_not_active");

    let mut pending_commit = submit_add_device_request(
        &room_id,
        &mls_group_id,
        &bob,
        &carol,
        "welcome-filtered-carol",
        "bob-pending-commit-idempotency",
    );
    pending_commit.expected_epoch = 1;
    pending_commit.envelope.epoch = 1;
    let pending_commit_message_id = pending_commit
        .envelope
        .message_id()
        .expect("pending commit message id");
    pending_commit.membership_delta.base_epoch = 1;
    pending_commit.membership_delta.post_commit_epoch = 2;
    pending_commit.membership_delta.commit_message_id = pending_commit_message_id;
    let response = post_json(app.clone(), "/commits", &pending_commit).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let error: ErrorResponse = read_json(response).await;
    assert_eq!(error.kind, "sender_not_active");

    let response = post_json(
        app,
        "/events",
        &append_application_request(
            &room_id,
            &mls_group_id,
            &alice,
            1,
            b"visible",
            "app-after-bob-idempotency",
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let visible_acceptance: EventAccepted = read_json(response).await;
    assert_eq!(visible_acceptance.seq, 3);

    let app = persistent_app(&db_path);
    let response = post_json(
        app.clone(),
        "/sync/group",
        &GroupSyncRequest {
            group_id: group_id(&room_id),
            after_seq: bob_hidden_page.next_after_seq,
            limit: 10,
            requester: Some(member_for_device(&bob)),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let bob_visible_page: HttpSyncPage = read_json(response).await;
    assert_eq!(bob_visible_page.entries.len(), 2);
    assert_eq!(
        bob_visible_page.entries[0].message.id.as_slice(),
        commit_message_id.as_bytes()
    );
    assert_eq!(
        bob_visible_page.entries[1].message.id.as_slice(),
        visible_acceptance.message_id.as_bytes()
    );
    assert_eq!(bob_visible_page.next_after_seq, visible_acceptance.seq);

    let response = post_json(
        app.clone(),
        "/welcomes/claim",
        &ClaimWelcomesRequest {
            recipient: member_for_device(&bob),
            limit: 10,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let claimed: Vec<HttpClaimedWelcome> = read_json(response).await;
    assert_eq!(claimed.len(), 1);

    let response = post_json(
        app,
        "/welcomes/ack",
        &AckWelcomeRequest {
            message_id: id("welcome-filtered-bob"),
            activated: true,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let app = persistent_app(&db_path);
    let response = post_json(
        app.clone(),
        "/events",
        &append_application_request(
            &room_id,
            &mls_group_id,
            &bob,
            1,
            b"activated-send",
            "bob-activated-send-idempotency",
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let bob_acceptance: EventAccepted = read_json(response).await;
    assert_eq!(bob_acceptance.seq, 4);

    let response = post_json(
        app.clone(),
        "/messages",
        &raw_commit_publish_request(
            &room_id,
            &mls_group_id,
            &alice,
            1,
            "raw-commit-without-membership-delta",
            "raw-commit-without-membership-delta-idempotency",
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let error: ErrorResponse = read_json(response).await;
    assert_eq!(error.kind, "invalid_raw_commit_import");

    let app = persistent_app(&db_path);
    let response = post_json(
        app,
        "/sync/group",
        &GroupSyncRequest {
            group_id: group_id(&room_id),
            after_seq: 0,
            limit: 10,
            requester: Some(member_for_device(&carol)),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let carol_page: HttpSyncPage = read_json(response).await;
    assert!(carol_page.entries.is_empty());
    assert_eq!(carol_page.next_after_seq, bob_acceptance.seq);
}

#[tokio::test]
async fn sqlite_removed_device_syncs_through_removal_and_cannot_send_over_http() {
    let temp = TempDir::new().expect("tempdir");
    let db_path = temp.path().join("delivery.sqlite3");
    let room_id = "room-removed-device-sync".to_owned();
    let mls_group_id = "mls-removed-device-sync".to_owned();
    let alice = DeviceRef::new("alice", "alice-laptop");
    let bob = DeviceRef::new("bob", "bob-phone");
    let add_bob = submit_add_device_request(
        &room_id,
        &mls_group_id,
        &alice,
        &bob,
        "welcome-removed-sync-bob",
        "add-removed-sync-bob",
    );
    let app = persistent_app(&db_path);

    let response = post_json(
        app.clone(),
        "/account-rooms/bootstrap",
        &BootstrapAccountRoomRequest {
            room_id: room_id.clone(),
            mls_group_id: mls_group_id.clone(),
            creator: alice.clone(),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    publish_and_claim_key_package_for_add(&app, &add_bob).await;
    let response = post_json(app.clone(), "/commits", &add_bob).await;
    assert_eq!(response.status(), StatusCode::OK);
    let add_acceptance: CommitAccepted = read_json(response).await;
    assert_eq!(add_acceptance.seq, 1);

    let response = post_json(
        app.clone(),
        "/welcomes/claim",
        &ClaimWelcomesRequest {
            recipient: member_for_device(&bob),
            limit: 10,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let claimed: Vec<HttpClaimedWelcome> = read_json(response).await;
    assert_eq!(claimed.len(), 1);
    let response = post_json(
        app.clone(),
        "/welcomes/ack",
        &AckWelcomeRequest {
            message_id: id("welcome-removed-sync-bob"),
            activated: true,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let remove_bob =
        submit_remove_device_request(&room_id, &mls_group_id, &alice, &bob, 1, "remove-sync-bob");
    let remove_message_id = remove_bob.envelope.message_id().expect("remove message id");
    let response = post_json(app.clone(), "/commits", &remove_bob).await;
    assert_eq!(response.status(), StatusCode::OK);
    let removal: CommitAccepted = read_json(response).await;
    assert_eq!(removal.seq, 2);
    assert_eq!(removal.message_id, remove_message_id);

    let app = persistent_app(&db_path);
    let response = post_json(
        app.clone(),
        "/sync/group",
        &GroupSyncRequest {
            group_id: group_id(&room_id),
            after_seq: add_acceptance.seq,
            limit: 10,
            requester: Some(member_for_device(&bob)),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let bob_page: HttpSyncPage = read_json(response).await;
    assert_eq!(bob_page.entries.len(), 1);
    assert_eq!(bob_page.entries[0].seq, removal.seq);
    assert_eq!(
        bob_page.entries[0].message.id.as_slice(),
        remove_message_id.as_bytes()
    );
    assert_eq!(bob_page.next_after_seq, removal.seq);
    assert!(!bob_page.has_more);

    let response = post_json(
        app.clone(),
        "/events",
        &append_application_request(
            &room_id,
            &mls_group_id,
            &alice,
            2,
            b"after removal",
            "alice-after-remove-idempotency",
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let after_removal: EventAccepted = read_json(response).await;
    assert_eq!(after_removal.seq, 3);

    let response = post_json(
        app.clone(),
        "/events",
        &append_application_request(
            &room_id,
            &mls_group_id,
            &bob,
            2,
            b"stale send",
            "bob-stale-send-idempotency",
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let error: ErrorResponse = read_json(response).await;
    assert_eq!(error.kind, "sender_not_active");

    let stale_commit =
        submit_remove_device_request(&room_id, &mls_group_id, &bob, &alice, 2, "bob-stale-commit");
    let response = post_json(app.clone(), "/commits", &stale_commit).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let error: ErrorResponse = read_json(response).await;
    assert_eq!(error.kind, "sender_not_active");

    let response = post_json(
        app,
        "/sync/group",
        &GroupSyncRequest {
            group_id: group_id(&room_id),
            after_seq: removal.seq,
            limit: 10,
            requester: Some(member_for_device(&bob)),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let hidden_after_removal: HttpSyncPage = read_json(response).await;
    assert!(hidden_after_removal.entries.is_empty());
    assert_eq!(hidden_after_removal.next_after_seq, after_removal.seq);
    assert!(!hidden_after_removal.has_more);
}

#[tokio::test]
async fn sqlite_typed_event_rejects_oversized_payload_without_persisting_log() {
    let temp = TempDir::new().expect("tempdir");
    let db_path = temp.path().join("delivery.sqlite3");
    let room_id = "room-event-oversized".to_owned();
    let mls_group_id = "mls-event-oversized".to_owned();
    let alice = DeviceRef::new("alice", "alice-laptop");
    let app = persistent_app(&db_path);

    let response = post_json(
        app.clone(),
        "/account-rooms/bootstrap",
        &BootstrapAccountRoomRequest {
            room_id: room_id.clone(),
            mls_group_id: mls_group_id.clone(),
            creator: alice.clone(),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let oversized = vec![0; MAX_ENVELOPE_PAYLOAD_BYTES as usize + 1];
    let response = post_json(
        app.clone(),
        "/events",
        &append_application_request(
            &room_id,
            &mls_group_id,
            &alice,
            0,
            &oversized,
            "oversized-event-idempotency",
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let error: ErrorResponse = read_json(response).await;
    assert_eq!(error.kind, "invalid_event_request");

    let response = post_json(
        app.clone(),
        "/sync/group",
        &GroupSyncRequest {
            group_id: group_id(&room_id),
            after_seq: 0,
            limit: 10,
            requester: None,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let page: HttpSyncPage = read_json(response).await;
    assert!(page.entries.is_empty());
    assert_eq!(page.next_after_seq, 0);

    let app = persistent_app(&db_path);
    let response = post_json(
        app,
        "/sync/group",
        &GroupSyncRequest {
            group_id: group_id(&room_id),
            after_seq: 0,
            limit: 10,
            requester: None,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let page: HttpSyncPage = read_json(response).await;
    assert!(page.entries.is_empty());
    assert_eq!(page.next_after_seq, 0);
}

#[tokio::test]
async fn sqlite_typed_event_duplicate_message_id_with_new_idempotency_key_conflicts() {
    let temp = TempDir::new().expect("tempdir");
    let db_path = temp.path().join("delivery.sqlite3");
    let room_id = "room-event-duplicate-message-id".to_owned();
    let mls_group_id = "mls-event-duplicate-message-id".to_owned();
    let alice = DeviceRef::new("alice", "alice-laptop");
    let app = persistent_app(&db_path);

    let response = post_json(
        app.clone(),
        "/account-rooms/bootstrap",
        &BootstrapAccountRoomRequest {
            room_id: room_id.clone(),
            mls_group_id: mls_group_id.clone(),
            creator: alice.clone(),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let first = append_application_request(
        &room_id,
        &mls_group_id,
        &alice,
        0,
        b"same ciphertext",
        "first-event-idempotency",
    );
    let duplicate = AppendEventRequest {
        idempotency_key: "second-event-idempotency".to_owned(),
        ..first.clone()
    };
    let message_id = first.envelope.message_id().expect("event message id");

    let response = post_json(app.clone(), "/events", &first).await;
    assert_eq!(response.status(), StatusCode::OK);
    let accepted: EventAccepted = read_json(response).await;
    assert_eq!(accepted.seq, 1);
    assert_eq!(accepted.message_id, message_id);

    let response = post_json(app.clone(), "/events", &first).await;
    assert_eq!(response.status(), StatusCode::OK);
    let replayed: EventAccepted = read_json(response).await;
    assert_eq!(replayed, accepted);

    let response = post_json(app.clone(), "/events", &duplicate).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let error: ErrorResponse = read_json(response).await;
    assert_eq!(error.kind, "duplicate_message_id");

    let app = persistent_app(&db_path);
    let response = post_json(app.clone(), "/events", &first).await;
    assert_eq!(response.status(), StatusCode::OK);
    let replayed_after_restart: EventAccepted = read_json(response).await;
    assert_eq!(replayed_after_restart, accepted);

    let response = post_json(app.clone(), "/events", &duplicate).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let error: ErrorResponse = read_json(response).await;
    assert_eq!(error.kind, "duplicate_message_id");

    let response = post_json(
        app,
        "/sync/group",
        &GroupSyncRequest {
            group_id: group_id(&room_id),
            after_seq: 0,
            limit: 10,
            requester: None,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let page: HttpSyncPage = read_json(response).await;
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.next_after_seq, 1);
}

#[tokio::test]
async fn sqlite_typed_event_idempotency_capacity_rejects_new_keys_but_replays_after_restart() {
    let temp = TempDir::new().expect("tempdir");
    let db_path = temp.path().join("delivery.sqlite3");
    let room_id = "room-event-idempotency-capacity".to_owned();
    let mls_group_id = "mls-event-idempotency-capacity".to_owned();
    let alice = DeviceRef::new("alice", "alice-laptop");
    let app = persistent_app(&db_path);

    let response = post_json(
        app.clone(),
        "/account-rooms/bootstrap",
        &BootstrapAccountRoomRequest {
            room_id: room_id.clone(),
            mls_group_id: mls_group_id.clone(),
            creator: alice.clone(),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    drop(app);
    for index in 0..(MAX_IDEMPOTENCY_RECORDS_PER_ROOM_DEVICE - 1) {
        let request = append_application_request(
            &room_id,
            &mls_group_id,
            &alice,
            0,
            format!("seeded-capacity-event-{index}").as_bytes(),
            &format!("seeded-capacity-event-{index}"),
        );
        let message_id = request.envelope.message_id().expect("seeded message id");
        let publish = event_publish_request_for_test(&request, &message_id);
        insert_durable_publish_idempotency_only(&db_path, &publish, u64::from(index) + 1);
    }

    let app = persistent_app(&db_path);
    let final_request = append_application_request(
        &room_id,
        &mls_group_id,
        &alice,
        0,
        b"capacity-event-final",
        "capacity-event-final",
    );
    let response = post_json(app.clone(), "/events", &final_request).await;
    assert_eq!(response.status(), StatusCode::OK);
    let final_accepted: EventAccepted = read_json(response).await;
    assert_eq!(final_accepted.seq, 1);

    let overflow_request = append_application_request(
        &room_id,
        &mls_group_id,
        &alice,
        0,
        b"capacity-event-overflow",
        "capacity-event-overflow",
    );
    let response = post_json(app.clone(), "/events", &overflow_request).await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let error: ErrorResponse = read_json(response).await;
    assert_eq!(error.kind, "idempotency_capacity_exceeded");

    let response = post_json(app.clone(), "/events", &final_request).await;
    assert_eq!(response.status(), StatusCode::OK);
    let replayed: EventAccepted = read_json(response).await;
    assert_eq!(replayed, final_accepted);

    let app = persistent_app(&db_path);
    let response = post_json(app.clone(), "/events", &final_request).await;
    assert_eq!(response.status(), StatusCode::OK);
    let replayed_after_restart: EventAccepted = read_json(response).await;
    assert_eq!(replayed_after_restart, final_accepted);

    let response = post_json(app.clone(), "/events", &overflow_request).await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let error: ErrorResponse = read_json(response).await;
    assert_eq!(error.kind, "idempotency_capacity_exceeded");

    let response = post_json(
        app,
        "/sync/group",
        &GroupSyncRequest {
            group_id: group_id(&room_id),
            after_seq: 0,
            limit: 10,
            requester: Some(member_for_device(&alice)),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let page: HttpSyncPage = read_json(response).await;
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].seq, 1);
    assert_eq!(page.next_after_seq, 1);
    assert!(!page.has_more);
}

#[tokio::test]
async fn sqlite_typed_event_sync_returns_bounded_pages_after_restart() {
    let temp = TempDir::new().expect("tempdir");
    let db_path = temp.path().join("delivery.sqlite3");
    let room_id = "room-event-bounded-sync".to_owned();
    let mls_group_id = "mls-event-bounded-sync".to_owned();
    let alice = DeviceRef::new("alice", "alice-laptop");
    let app = persistent_app(&db_path);

    let response = post_json(
        app.clone(),
        "/account-rooms/bootstrap",
        &BootstrapAccountRoomRequest {
            room_id: room_id.clone(),
            mls_group_id: mls_group_id.clone(),
            creator: alice.clone(),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    for index in 0..=MAX_HTTP_SYNC_PAGE_ENTRIES {
        let response = post_json(
            app.clone(),
            "/events",
            &append_application_request(
                &room_id,
                &mls_group_id,
                &alice,
                0,
                format!("small-{index}").as_bytes(),
                &format!("bounded-event-{index}"),
            ),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let accepted: EventAccepted = read_json(response).await;
        assert_eq!(accepted.seq, (index as u64) + 1);
    }

    let response = post_json(
        app.clone(),
        "/sync/group",
        &GroupSyncRequest {
            group_id: group_id(&room_id),
            after_seq: 0,
            limit: MAX_HTTP_SYNC_PAGE_ENTRIES,
            requester: Some(member_for_device(&alice)),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let first_page: HttpSyncPage = read_json(response).await;
    assert_eq!(first_page.entries.len(), MAX_HTTP_SYNC_PAGE_ENTRIES);
    assert_eq!(first_page.entries.first().unwrap().seq, 1);
    assert_eq!(
        first_page.entries.last().unwrap().seq,
        MAX_HTTP_SYNC_PAGE_ENTRIES as u64
    );
    assert_eq!(first_page.next_after_seq, MAX_HTTP_SYNC_PAGE_ENTRIES as u64);
    assert!(first_page.has_more);

    let app = persistent_app(&db_path);
    let response = post_json(
        app,
        "/sync/group",
        &GroupSyncRequest {
            group_id: group_id(&room_id),
            after_seq: first_page.next_after_seq,
            limit: MAX_HTTP_SYNC_PAGE_ENTRIES,
            requester: Some(member_for_device(&alice)),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let second_page: HttpSyncPage = read_json(response).await;
    assert_eq!(second_page.entries.len(), 1);
    assert_eq!(
        second_page.entries[0].seq,
        (MAX_HTTP_SYNC_PAGE_ENTRIES as u64) + 1
    );
    assert_eq!(
        second_page.next_after_seq,
        (MAX_HTTP_SYNC_PAGE_ENTRIES as u64) + 1
    );
    assert!(!second_page.has_more);
}

#[tokio::test]
async fn sqlite_ephemeral_activity_over_http_does_not_persist_or_advance_sequence() {
    let temp = TempDir::new().expect("tempdir");
    let db_path = temp.path().join("delivery.sqlite3");
    let room_id = "room-ephemeral-activity-volatile".to_owned();
    let mls_group_id = "mls-ephemeral-activity-volatile".to_owned();
    let alice = DeviceRef::new("alice", "alice-laptop");
    let app = persistent_app(&db_path);

    let response = post_json(
        app.clone(),
        "/account-rooms/bootstrap",
        &BootstrapAccountRoomRequest {
            room_id: room_id.clone(),
            mls_group_id: mls_group_id.clone(),
            creator: alice.clone(),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let request = ephemeral_activity_request(
        &room_id,
        &mls_group_id,
        &alice,
        0,
        Some("topic-activity"),
        1_000,
    );
    let response = post_json(app.clone(), "/activities", &request).await;
    assert_eq!(response.status(), StatusCode::OK);
    let accepted: EphemeralActivityAccepted = read_json(response).await;
    assert_eq!(accepted.cached_events_for_route, 1);
    assert_eq!(
        accepted.route_key,
        finitechat_engine::ephemeral_activity_route_key(&room_id, Some("topic-activity"), &alice)
    );

    let response = post_json(
        app.clone(),
        "/sync/group",
        &GroupSyncRequest {
            group_id: group_id(&room_id),
            after_seq: 0,
            limit: 10,
            requester: None,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let page: HttpSyncPage = read_json(response).await;
    assert!(page.entries.is_empty());
    assert_eq!(page.next_after_seq, 0);

    let app = persistent_app(&db_path);
    let response = post_json(
        app.clone(),
        "/sync/group",
        &GroupSyncRequest {
            group_id: group_id(&room_id),
            after_seq: 0,
            limit: 10,
            requester: None,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let page: HttpSyncPage = read_json(response).await;
    assert!(page.entries.is_empty());
    assert_eq!(page.next_after_seq, 0);

    let response = post_json(app, "/activities", &request).await;
    assert_eq!(response.status(), StatusCode::OK);
    let accepted: EphemeralActivityAccepted = read_json(response).await;
    assert_eq!(accepted.cached_events_for_route, 1);
}

#[tokio::test]
async fn sqlite_ephemeral_activity_over_http_authorizes_members_and_bounds_cache() {
    let temp = TempDir::new().expect("tempdir");
    let db_path = temp.path().join("delivery.sqlite3");
    let room_id = "room-ephemeral-activity-auth".to_owned();
    let mls_group_id = "mls-ephemeral-activity-auth".to_owned();
    let alice = DeviceRef::new("alice", "alice-laptop");
    let bob = DeviceRef::new("bob", "bob-phone");
    let add_bob = submit_add_device_request(
        &room_id,
        &mls_group_id,
        &alice,
        &bob,
        "welcome-ephemeral-bob",
        "commit-ephemeral-bob",
    );
    let app = persistent_app(&db_path);

    let response = post_json(
        app.clone(),
        "/account-rooms/bootstrap",
        &BootstrapAccountRoomRequest {
            room_id: room_id.clone(),
            mls_group_id: mls_group_id.clone(),
            creator: alice.clone(),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    publish_and_claim_key_package_for_add(&app, &add_bob).await;
    let response = post_json(app.clone(), "/commits", &add_bob).await;
    assert_eq!(response.status(), StatusCode::OK);

    let pending = ephemeral_activity_request(&room_id, &mls_group_id, &bob, 1, None, 1_000);
    let response = post_json(app.clone(), "/activities", &pending).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let error: ErrorResponse = read_json(response).await;
    assert_eq!(error.kind, "sender_not_active");

    let wrong_epoch = ephemeral_activity_request(&room_id, &mls_group_id, &alice, 0, None, 1_000);
    let response = post_json(app.clone(), "/activities", &wrong_epoch).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let error: ErrorResponse = read_json(response).await;
    assert_eq!(error.kind, "invalid_activity_request");

    let expired = AppendEphemeralActivityRequest {
        expires_at_ms: 1_000,
        ..ephemeral_activity_request(&room_id, &mls_group_id, &alice, 1, None, 1_000)
    };
    let response = post_json(app.clone(), "/activities", &expired).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let error: ErrorResponse = read_json(response).await;
    assert_eq!(error.kind, "invalid_activity_request");

    let response = post_json(
        app.clone(),
        "/welcomes/claim",
        &ClaimWelcomesRequest {
            recipient: member_for_device(&bob),
            limit: 10,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let claimed: Vec<HttpClaimedWelcome> = read_json(response).await;
    assert_eq!(claimed.len(), 1);
    let response = post_json(
        app.clone(),
        "/welcomes/ack",
        &AckWelcomeRequest {
            message_id: id("welcome-ephemeral-bob"),
            activated: true,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    for index in 0..=MAX_EPHEMERAL_ACTIVITY_CACHE_ENTRIES_PER_ROUTE {
        let mut request = ephemeral_activity_request(
            &room_id,
            &mls_group_id,
            &bob,
            1,
            Some("topic-route"),
            2_000 + u64::from(index),
        );
        request.payload = vec![0xff, index as u8];
        let response = post_json(app.clone(), "/activities", &request).await;
        assert_eq!(response.status(), StatusCode::OK);
        let accepted: EphemeralActivityAccepted = read_json(response).await;
        assert_eq!(
            accepted.cached_events_for_route,
            (index + 1).min(MAX_EPHEMERAL_ACTIVITY_CACHE_ENTRIES_PER_ROUTE)
        );
    }

    revoke_device(&app, &bob).await;
    let response = post_json(
        app.clone(),
        "/activities",
        &ephemeral_activity_request(&room_id, &mls_group_id, &bob, 1, Some("topic-route"), 3_000),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let error: ErrorResponse = read_json(response).await;
    assert_eq!(error.kind, "device_revoked");

    let response = post_json(
        app,
        "/sync/group",
        &GroupSyncRequest {
            group_id: group_id(&room_id),
            after_seq: 0,
            limit: 10,
            requester: None,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let page: HttpSyncPage = read_json(response).await;
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.next_after_seq, 1);
}

#[tokio::test]
async fn sqlite_invalid_commit_report_blocks_typed_mutations_after_restart() {
    let temp = TempDir::new().expect("tempdir");
    let db_path = temp.path().join("delivery.sqlite3");
    let alice = DeviceRef::new("alice", "alice-laptop");
    let bob = DeviceRef::new("bob", "bob-phone");
    let carol = DeviceRef::new("carol", "carol-phone");
    let room_id = "room-invalid-commit-report".to_owned();
    let mls_group_id = "mls-invalid-commit-report".to_owned();
    let request = submit_add_device_request(
        &room_id,
        &mls_group_id,
        &alice,
        &bob,
        "welcome-invalid-report-bob",
        "invalid-report-add-bob",
    );

    let app = persistent_app(&db_path);
    let response = post_json(
        app.clone(),
        "/account-rooms/bootstrap",
        &BootstrapAccountRoomRequest {
            room_id: room_id.clone(),
            mls_group_id: mls_group_id.clone(),
            creator: alice.clone(),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    publish_and_claim_key_package_for_add(&app, &request).await;
    let response = post_json(app.clone(), "/commits", &request).await;
    assert_eq!(response.status(), StatusCode::OK);
    let accepted: CommitAccepted = read_json(response).await;

    let response = post_json(
        app.clone(),
        "/rooms/report-invalid-commit",
        &ReportInvalidCommitRequest {
            room_id: room_id.clone(),
            reporter: carol.clone(),
            offending_seq: accepted.seq,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let error: ErrorResponse = read_json(response).await;
    assert_eq!(error.kind, "reporter_not_in_interval");

    let response = post_json(
        app,
        "/rooms/report-invalid-commit",
        &ReportInvalidCommitRequest {
            room_id: room_id.clone(),
            reporter: alice.clone(),
            offending_seq: accepted.seq,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let reported: ReportInvalidCommitResponse = read_json(response).await;
    assert!(reported.reported);

    let app = persistent_app(&db_path);
    let response = post_json(
        app.clone(),
        "/account-rooms/list",
        &ListAccountRoomDirectoryRequest {
            account_id: "alice".to_owned(),
            after_room_id: None,
            limit: 10,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let page: ListAccountRoomDirectoryResponse = read_json(response).await;
    assert_eq!(page.rooms.len(), 1);
    assert_eq!(page.rooms[0]["status"], "needs_repair");

    let response = post_json(
        app.clone(),
        "/events",
        &append_application_request(
            &room_id,
            &mls_group_id,
            &alice,
            1,
            b"blocked",
            "invalid-report-blocked-event",
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let error: ErrorResponse = read_json(response).await;
    assert_eq!(error.kind, "room_not_open");

    let blocked_commit =
        submit_add_device_request_at_epoch(&room_id, &mls_group_id, &alice, &carol, 1);
    let response = post_json(app, "/commits", &blocked_commit).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let error: ErrorResponse = read_json(response).await;
    assert_eq!(error.kind, "room_not_open");
}

#[tokio::test]
async fn sqlite_welcome_activation_marks_account_room_device_active_after_restart() {
    let temp = TempDir::new().expect("tempdir");
    let db_path = temp.path().join("delivery.sqlite3");
    let creator = DeviceRef {
        account_id: "alice".to_owned(),
        device_id: "alice-laptop".to_owned(),
    };
    let phone = DeviceRef {
        account_id: "alice".to_owned(),
        device_id: "alice-phone".to_owned(),
    };
    let room_id = "room-welcome-activation".to_owned();
    let mls_group_id = "mls-welcome-activation".to_owned();
    let app = persistent_app(&db_path);

    let response = post_json(
        app.clone(),
        "/account-rooms",
        &SaveAccountRoomRequest {
            account_id: "alice".to_owned(),
            room_id: room_id.clone(),
            record: serde_json::to_value(&AccountRoomRecord {
                room_id: room_id.clone(),
                mls_group_id,
                current_epoch: 2,
                last_seq: 7,
                status: RoomStatus::Open,
                devices: vec![
                    AccountRoomDevice {
                        device: creator,
                        active: true,
                    },
                    AccountRoomDevice {
                        device: phone.clone(),
                        active: false,
                    },
                ],
            })
            .expect("account-room record json"),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let recipient = member_for_device(&phone);
    let welcome_record = WelcomeRecord {
        welcome_id: "welcome-phone-activation".to_owned(),
        room_id: room_id.clone(),
        commit_seq: 7,
        recipient: phone.clone(),
        sender: DeviceRef {
            account_id: "alice".to_owned(),
            device_id: "alice-laptop".to_owned(),
        },
        key_package_id: "kp-phone-activation".to_owned(),
        join_epoch: 2,
        state: WelcomeState::Released,
        lease_token: Some("lease-phone-activation".to_owned()),
        welcome_payload: b"welcome-bytes".to_vec(),
        ratchet_tree_payload: b"ratchet-tree".to_vec(),
    };
    let welcome_payload = serde_json::to_vec(&welcome_record).expect("welcome record json");
    let response = post_json(
        app.clone(),
        "/messages",
        &PublishMessageRequest {
            target: HttpPublishTarget::Inbox {
                recipient: recipient.clone(),
            },
            message: welcome_message(
                "welcome-phone-activation",
                recipient.clone(),
                &welcome_payload,
            ),
            idempotency_key: None,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let response = post_json(
        app.clone(),
        "/welcomes/claim",
        &ClaimWelcomesRequest {
            recipient: recipient.clone(),
            limit: 10,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let claimed: Vec<HttpClaimedWelcome> = read_json(response).await;
    assert_eq!(claimed.len(), 1);

    let response = post_json(
        app,
        "/welcomes/ack",
        &AckWelcomeRequest {
            message_id: id("welcome-phone-activation"),
            activated: true,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let app = persistent_app(&db_path);
    let response = post_json(
        app.clone(),
        "/account-rooms/list",
        &ListAccountRoomDirectoryRequest {
            account_id: "alice".to_owned(),
            after_room_id: None,
            limit: 10,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let page: ListAccountRoomDirectoryResponse = read_json(response).await;
    assert_eq!(page.rooms.len(), 1);
    assert_eq!(
        page.rooms[0]["devices"][1]["device"]["device_id"],
        "alice-phone"
    );
    assert_eq!(page.rooms[0]["devices"][1]["active"], true);

    let response = post_json(
        app,
        "/welcomes/ack",
        &AckWelcomeRequest {
            message_id: id("welcome-phone-activation"),
            activated: true,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn sqlite_welcome_claim_survives_restart_before_ack() {
    let temp = TempDir::new().expect("tempdir");
    let db_path = temp.path().join("delivery.sqlite3");
    let recipient = member("welcome-recipient");
    let welcome = PublishMessageRequest {
        target: HttpPublishTarget::Inbox {
            recipient: recipient.clone(),
        },
        message: welcome_message("welcome-restart", recipient.clone(), b"welcome-bytes"),
        idempotency_key: Some("idem-welcome-restart".to_owned()),
    };

    let app = persistent_app(&db_path);
    assert_eq!(
        post_json(app.clone(), "/messages", &welcome).await.status(),
        StatusCode::OK
    );

    let response = post_json(
        app.clone(),
        "/welcomes/claim",
        &ClaimWelcomesRequest {
            recipient: recipient.clone(),
            limit: 10,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let claimed: Vec<HttpClaimedWelcome> = read_json(response).await;
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].seq, 1);
    assert_eq!(claimed[0].message.id, id("welcome-restart"));

    let response = post_json(
        app,
        "/welcomes/claim",
        &ClaimWelcomesRequest {
            recipient: recipient.clone(),
            limit: 10,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let duplicate_claim: Vec<HttpClaimedWelcome> = read_json(response).await;
    assert!(duplicate_claim.is_empty());

    let app = persistent_app(&db_path);
    let response = post_json(
        app.clone(),
        "/welcomes/claim",
        &ClaimWelcomesRequest {
            recipient,
            limit: 10,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let after_restart_claim: Vec<HttpClaimedWelcome> = read_json(response).await;
    assert!(after_restart_claim.is_empty());

    let response = post_json(
        app.clone(),
        "/welcomes/ack",
        &AckWelcomeRequest {
            message_id: id("welcome-restart"),
            activated: true,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let acked: AckWelcomeResponse = read_json(response).await;
    assert!(acked.acked);

    let app = persistent_app(&db_path);
    let response = post_json(
        app,
        "/welcomes/ack",
        &AckWelcomeRequest {
            message_id: id("welcome-restart"),
            activated: true,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let acked: AckWelcomeResponse = read_json(response).await;
    assert!(acked.acked);
}

#[tokio::test]
async fn sqlite_welcome_failed_ack_is_terminal_after_restart() {
    let temp = TempDir::new().expect("tempdir");
    let db_path = temp.path().join("delivery.sqlite3");
    let recipient = member("failed-welcome-recipient");
    let welcome = PublishMessageRequest {
        target: HttpPublishTarget::Inbox {
            recipient: recipient.clone(),
        },
        message: welcome_message("welcome-failed", recipient.clone(), b"welcome-bytes"),
        idempotency_key: None,
    };

    let app = persistent_app(&db_path);
    assert_eq!(
        post_json(app.clone(), "/messages", &welcome).await.status(),
        StatusCode::OK
    );
    let response = post_json(
        app.clone(),
        "/welcomes/claim",
        &ClaimWelcomesRequest {
            recipient: recipient.clone(),
            limit: 10,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let claimed: Vec<HttpClaimedWelcome> = read_json(response).await;
    assert_eq!(claimed.len(), 1);

    let response = post_json(
        app,
        "/welcomes/ack",
        &AckWelcomeRequest {
            message_id: id("welcome-failed"),
            activated: false,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let app = persistent_app(&db_path);
    let response = post_json(
        app.clone(),
        "/welcomes/claim",
        &ClaimWelcomesRequest {
            recipient,
            limit: 10,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let claimed: Vec<HttpClaimedWelcome> = read_json(response).await;
    assert!(claimed.is_empty());

    let response = post_json(
        app,
        "/welcomes/ack",
        &AckWelcomeRequest {
            message_id: id("welcome-failed"),
            activated: true,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    let error: ErrorResponse = read_json(response).await;
    assert_eq!(error.kind, "welcome_ack_conflict");
}

fn persistent_app(path: &std::path::Path) -> Router {
    http_router(HttpServerState::from_sqlite_path(path).expect("persistent server state"))
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

async fn assert_inventory(app: Router, owner: MemberId, available: u32, claimed: u32) {
    let response = post_json(
        app,
        "/key-packages/inventory",
        &KeyPackageInventoryRequest {
            owner: owner.clone(),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let inventory: HttpKeyPackageInventory = read_json(response).await;
    assert_eq!(inventory.owner, owner);
    assert_eq!(inventory.available, available);
    assert_eq!(inventory.claimed, claimed);
}

async fn revoke_device(app: &Router, device: &DeviceRef) {
    let response = post_json(
        app.clone(),
        "/devices/revoke",
        &RevokeDeviceRequest {
            device: device.clone(),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
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

fn member_for_device(device: &DeviceRef) -> MemberId {
    MemberId::new(serde_json::to_vec(device).expect("device member id json"))
}

fn submit_add_device_request(
    room_id: &str,
    mls_group_id: &str,
    sender: &DeviceRef,
    added: &DeviceRef,
    welcome_id: &str,
    idempotency_key: &str,
) -> SubmitCommitRequest {
    let envelope = FiniteEnvelope {
        room_id: room_id.to_owned(),
        mls_group_id: mls_group_id.to_owned(),
        epoch: 0,
        sender: sender.clone(),
        kind: LogEntryKind::Commit,
        payload: b"commit-add-device".to_vec(),
    };
    let commit_message_id = envelope.message_id().expect("commit message id");
    let key_package_id = format!("key-package-{welcome_id}");
    SubmitCommitRequest {
        room_id: room_id.to_owned(),
        sender: sender.clone(),
        expected_epoch: 0,
        envelope,
        membership_delta: MembershipDeltaV1 {
            base_epoch: 0,
            post_commit_epoch: 1,
            commit_message_id,
            adds: vec![MembershipAddV1 {
                device: added.clone(),
                key_package_id: key_package_id.clone(),
                key_package_ref: format!("key-package-ref-{welcome_id}"),
                key_package_hash: format!("key-package-hash-{welcome_id}"),
                welcome_id: welcome_id.to_owned(),
            }],
            removes: Vec::new(),
        },
        staged_welcomes: vec![StagedWelcomeV1 {
            welcome_id: welcome_id.to_owned(),
            welcome_payload: b"welcome-add-device".to_vec(),
            ratchet_tree_payload: b"ratchet-tree-add-device".to_vec(),
        }],
        idempotency_key: idempotency_key.to_owned(),
    }
}

fn submit_remove_device_request(
    room_id: &str,
    mls_group_id: &str,
    sender: &DeviceRef,
    removed: &DeviceRef,
    epoch: u64,
    idempotency_key: &str,
) -> SubmitCommitRequest {
    let envelope = FiniteEnvelope {
        room_id: room_id.to_owned(),
        mls_group_id: mls_group_id.to_owned(),
        epoch,
        sender: sender.clone(),
        kind: LogEntryKind::Commit,
        payload: format!("commit-remove-{idempotency_key}").into_bytes(),
    };
    let commit_message_id = envelope.message_id().expect("commit message id");
    SubmitCommitRequest {
        room_id: room_id.to_owned(),
        sender: sender.clone(),
        expected_epoch: epoch,
        envelope,
        membership_delta: MembershipDeltaV1 {
            base_epoch: epoch,
            post_commit_epoch: epoch + 1,
            commit_message_id,
            adds: Vec::new(),
            removes: vec![MembershipRemoveV1 {
                device: removed.clone(),
                removed_leaf_index: 1,
            }],
        },
        staged_welcomes: Vec::new(),
        idempotency_key: idempotency_key.to_owned(),
    }
}

fn submit_add_device_request_at_epoch(
    room_id: &str,
    mls_group_id: &str,
    sender: &DeviceRef,
    added: &DeviceRef,
    epoch: u64,
) -> SubmitCommitRequest {
    let welcome_id = format!("welcome-{room_id}-{epoch}");
    submit_add_device_request_at_epoch_with_ids(
        room_id,
        mls_group_id,
        sender,
        added,
        epoch,
        &welcome_id,
        &format!("commit-{room_id}-{epoch}"),
    )
}

fn submit_add_device_request_at_epoch_with_ids(
    room_id: &str,
    mls_group_id: &str,
    sender: &DeviceRef,
    added: &DeviceRef,
    epoch: u64,
    welcome_id: &str,
    idempotency_key: &str,
) -> SubmitCommitRequest {
    let mut request = submit_add_device_request(
        room_id,
        mls_group_id,
        sender,
        added,
        welcome_id,
        idempotency_key,
    );
    request.expected_epoch = epoch;
    request.envelope.epoch = epoch;
    let commit_message_id = request.envelope.message_id().expect("commit message id");
    request.membership_delta.base_epoch = epoch;
    request.membership_delta.post_commit_epoch = epoch + 1;
    request.membership_delta.commit_message_id = commit_message_id;
    request
}

async fn publish_and_claim_key_package_for_add(app: &Router, request: &SubmitCommitRequest) {
    let add = request
        .membership_delta
        .adds
        .first()
        .expect("add-device request has one add");
    let upload = UploadKeyPackageRequest {
        key_package_id: add.key_package_id.clone(),
        owner: add.device.clone(),
        key_package_ref: add.key_package_ref.clone(),
        key_package_hash: add.key_package_hash.clone(),
        key_package_payload: format!("payload-{}", add.key_package_id).into_bytes(),
    };
    let publication = HttpKeyPackagePublication {
        key_package_id: HttpKeyPackageId::new(upload.key_package_id.as_bytes().to_vec()),
        owner: member_for_device(&upload.owner),
        key_package: KeyPackage::new(serde_json::to_vec(&upload).expect("upload json")),
    };
    let response = post_json(app.clone(), "/key-packages", &publication).await;
    assert_eq!(response.status(), StatusCode::OK);

    let response = post_json(
        app.clone(),
        "/key-packages/claim",
        &ClaimKeyPackageRequest {
            owner: member_for_device(&upload.owner),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let claimed: Option<HttpClaimedKeyPackage> = read_json(response).await;
    let claimed = claimed.expect("claimed KeyPackage");
    assert_eq!(claimed.key_package_id, publication.key_package_id);
    assert_eq!(claimed.owner, publication.owner);
}

async fn key_package_inventory_for_device(
    app: &Router,
    owner: &DeviceRef,
) -> HttpKeyPackageInventory {
    let response = post_json(
        app.clone(),
        "/key-packages/inventory",
        &KeyPackageInventoryRequest {
            owner: member_for_device(owner),
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    read_json(response).await
}

async fn assert_submit_commit_had_no_side_effects(app: &Router, room_id: &str, added: &DeviceRef) {
    let response = post_json(
        app.clone(),
        "/sync/group",
        &GroupSyncRequest {
            group_id: group_id(room_id),
            after_seq: 0,
            limit: 10,
            requester: None,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let page: HttpSyncPage = read_json(response).await;
    assert!(page.entries.is_empty());

    let response = post_json(
        app.clone(),
        "/sync/inbox",
        &InboxSyncRequest {
            recipient: member_for_device(added),
            after_seq: 0,
            limit: 10,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let page: HttpSyncPage = read_json(response).await;
    assert!(page.entries.is_empty());
}

#[derive(Clone, Copy, Debug)]
enum HttpSubmitCommitCrashPoint {
    CommitDeliveryOperation,
    CommitIdempotencyRecord,
    WelcomeDeliveryOperation,
    WelcomeIdempotencyRecord,
    AccountRoomProjection,
    RoomMembershipProjection,
    KeyPackageConsumedProjection,
}

impl HttpSubmitCommitCrashPoint {
    const ALL: [Self; 7] = [
        Self::CommitDeliveryOperation,
        Self::CommitIdempotencyRecord,
        Self::WelcomeDeliveryOperation,
        Self::WelcomeIdempotencyRecord,
        Self::AccountRoomProjection,
        Self::RoomMembershipProjection,
        Self::KeyPackageConsumedProjection,
    ];

    fn trigger_sql(self) -> &'static str {
        match self {
            Self::CommitDeliveryOperation => {
                r#"
                CREATE TRIGGER finitechat_http_test_crash_after_commit_delivery
                AFTER INSERT ON http_delivery_ops
                WHEN NEW.kind = 'publish_message'
                  AND NEW.body_json LIKE '%http-crash-matrix-commit%'
                BEGIN
                  SELECT RAISE(ROLLBACK, 'finitechat http test crash after commit delivery');
                END;
                "#
            }
            Self::CommitIdempotencyRecord => {
                r#"
                CREATE TRIGGER finitechat_http_test_crash_after_commit_idempotency
                AFTER INSERT ON http_publish_idempotency
                WHEN NEW.idempotency_key = 'commit:room-http-crash-matrix:http-crash-matrix-commit'
                BEGIN
                  SELECT RAISE(ROLLBACK, 'finitechat http test crash after commit idempotency');
                END;
                "#
            }
            Self::WelcomeDeliveryOperation => {
                r#"
                CREATE TRIGGER finitechat_http_test_crash_after_welcome_delivery
                AFTER INSERT ON http_delivery_ops
                WHEN NEW.kind = 'publish_message'
                  AND NEW.body_json LIKE '%welcome-http-crash-tablet%'
                BEGIN
                  SELECT RAISE(ROLLBACK, 'finitechat http test crash after welcome delivery');
                END;
                "#
            }
            Self::WelcomeIdempotencyRecord => {
                r#"
                CREATE TRIGGER finitechat_http_test_crash_after_welcome_idempotency
                AFTER INSERT ON http_publish_idempotency
                WHEN NEW.idempotency_key = 'welcome:welcome-http-crash-tablet'
                BEGIN
                  SELECT RAISE(ROLLBACK, 'finitechat http test crash after welcome idempotency');
                END;
                "#
            }
            Self::AccountRoomProjection => {
                r#"
                CREATE TRIGGER finitechat_http_test_crash_after_account_room_projection
                AFTER UPDATE OF record_json ON http_account_rooms
                WHEN NEW.room_id = 'room-http-crash-matrix'
                  AND NEW.record_json LIKE '%alice-tablet%'
                BEGIN
                  SELECT RAISE(ROLLBACK, 'finitechat http test crash after account-room projection');
                END;
                "#
            }
            Self::RoomMembershipProjection => {
                r#"
                CREATE TRIGGER finitechat_http_test_crash_after_room_membership_projection
                AFTER UPDATE OF projection_json ON http_room_memberships
                WHEN NEW.room_id = 'room-http-crash-matrix'
                  AND NEW.projection_json LIKE '%alice-tablet%'
                BEGIN
                  SELECT RAISE(ROLLBACK, 'finitechat http test crash after room-membership projection');
                END;
                "#
            }
            Self::KeyPackageConsumedProjection => {
                r#"
                CREATE TRIGGER finitechat_http_test_crash_after_key_package_consumed
                AFTER UPDATE OF state_json ON http_key_package_inventory
                WHEN NEW.state_json = '"Consumed"'
                BEGIN
                  SELECT RAISE(ROLLBACK, 'finitechat http test crash after KeyPackage consumed projection');
                END;
                "#
            }
        }
    }
}

fn install_http_submit_commit_crash_trigger(
    db_path: &std::path::Path,
    point: HttpSubmitCommitCrashPoint,
) {
    clear_http_submit_commit_crash_triggers(db_path);
    let conn = Connection::open(db_path).expect("sqlite connection");
    conn.execute_batch(point.trigger_sql())
        .expect("install HTTP commit crash trigger");
}

fn clear_http_submit_commit_crash_triggers(db_path: &std::path::Path) {
    let conn = Connection::open(db_path).expect("sqlite connection");
    conn.execute_batch(
        r#"
        DROP TRIGGER IF EXISTS finitechat_http_test_crash_after_commit_delivery;
        DROP TRIGGER IF EXISTS finitechat_http_test_crash_after_commit_idempotency;
        DROP TRIGGER IF EXISTS finitechat_http_test_crash_after_welcome_delivery;
        DROP TRIGGER IF EXISTS finitechat_http_test_crash_after_welcome_idempotency;
        DROP TRIGGER IF EXISTS finitechat_http_test_crash_after_account_room_projection;
        DROP TRIGGER IF EXISTS finitechat_http_test_crash_after_room_membership_projection;
        DROP TRIGGER IF EXISTS finitechat_http_test_crash_after_key_package_consumed;
        "#,
    )
    .expect("clear HTTP commit crash triggers");
}

async fn assert_http_crash_commit_rolled_back(
    app: &Router,
    room_id: &str,
    tablet: &DeviceRef,
    first_seq: u64,
) {
    let response = post_json(
        app.clone(),
        "/sync/group",
        &GroupSyncRequest {
            group_id: group_id(room_id),
            after_seq: 0,
            limit: 10,
            requester: None,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let page: HttpSyncPage = read_json(response).await;
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].seq, first_seq);

    let response = post_json(
        app.clone(),
        "/sync/inbox",
        &InboxSyncRequest {
            recipient: member_for_device(tablet),
            after_seq: 0,
            limit: 10,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let inbox_page: HttpSyncPage = read_json(response).await;
    assert!(inbox_page.entries.is_empty());

    let page = account_room_page(app, "alice").await;
    assert_eq!(page.rooms.len(), 1);
    assert_eq!(page.rooms[0]["current_epoch"], 1);
    assert_eq!(page.rooms[0]["last_seq"], first_seq);
    assert!(
        !page.rooms[0]["devices"]
            .as_array()
            .expect("devices")
            .iter()
            .any(|device| device["device"]["device_id"] == "alice-tablet")
    );

    let inventory = key_package_inventory_for_device(app, tablet).await;
    assert_eq!(inventory.available, 0);
    assert_eq!(inventory.claimed, 1);
}

async fn assert_http_crash_commit_converged(
    app: &Router,
    room_id: &str,
    tablet: &DeviceRef,
    accepted_seq: u64,
) {
    let response = post_json(
        app.clone(),
        "/sync/group",
        &GroupSyncRequest {
            group_id: group_id(room_id),
            after_seq: 0,
            limit: 10,
            requester: None,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let page: HttpSyncPage = read_json(response).await;
    assert_eq!(page.entries.len(), 2);
    assert_eq!(page.entries[1].seq, accepted_seq);

    let response = post_json(
        app.clone(),
        "/sync/inbox",
        &InboxSyncRequest {
            recipient: member_for_device(tablet),
            after_seq: 0,
            limit: 10,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let inbox_page: HttpSyncPage = read_json(response).await;
    assert_eq!(inbox_page.entries.len(), 1);
    assert_eq!(
        inbox_page.entries[0].message.id,
        id("welcome-http-crash-tablet")
    );

    let page = account_room_page(app, "alice").await;
    assert_eq!(page.rooms.len(), 1);
    assert_eq!(page.rooms[0]["current_epoch"], 2);
    assert_eq!(page.rooms[0]["last_seq"], accepted_seq);
    assert!(
        page.rooms[0]["devices"]
            .as_array()
            .expect("devices")
            .iter()
            .any(|device| device["device"]["device_id"] == "alice-tablet")
    );

    let inventory = key_package_inventory_for_device(app, tablet).await;
    assert_eq!(inventory.available, 0);
    assert_eq!(inventory.claimed, 0);
}

async fn account_room_page(app: &Router, account_id: &str) -> ListAccountRoomDirectoryResponse {
    let response = post_json(
        app.clone(),
        "/account-rooms/list",
        &ListAccountRoomDirectoryRequest {
            account_id: account_id.to_owned(),
            after_room_id: None,
            limit: 10,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    read_json(response).await
}

fn commit_publish_request_for_test(
    request: &SubmitCommitRequest,
    message_id: &str,
) -> PublishMessageRequest {
    let transport_group_id = request.room_id.as_bytes().to_vec();
    let entry = finitechat_proto::RoomLogEntry {
        room_id: request.room_id.clone(),
        seq: 0,
        message_id: message_id.to_owned(),
        sender: request.sender.clone(),
        kind: LogEntryKind::Commit,
        epoch: request.expected_epoch,
        envelope: request.envelope.clone(),
        idempotency_key: request.idempotency_key.clone(),
    };
    let payload = serde_json::to_vec(&FiniteAccountRoomCommitProjection {
        entry,
        membership_delta: request.membership_delta.clone(),
    })
    .expect("commit projection payload");

    PublishMessageRequest {
        target: group_target(
            group_id(&request.room_id),
            transport_group_id.clone(),
            Some(HttpCommitAdmission {
                source_epoch: EpochId(request.expected_epoch),
            }),
        ),
        message: TransportMessage {
            id: id(message_id),
            payload,
            timestamp: Timestamp(0),
            causal_deps: Vec::new(),
            source: TransportSource(HTTP_SERVER_SOURCE.to_owned()),
            envelope: TransportEnvelope::GroupMessage { transport_group_id },
        },
        idempotency_key: Some(format!(
            "commit:{}:{}",
            request.room_id, request.idempotency_key
        )),
    }
}

fn event_publish_request_for_test(
    request: &AppendEventRequest,
    message_id: &str,
) -> PublishMessageRequest {
    let transport_group_id = request.room_id.as_bytes().to_vec();
    let entry = finitechat_proto::RoomLogEntry {
        room_id: request.room_id.clone(),
        seq: 0,
        message_id: message_id.to_owned(),
        sender: request.sender.clone(),
        kind: request.envelope.kind,
        epoch: request.envelope.epoch,
        envelope: request.envelope.clone(),
        idempotency_key: request.idempotency_key.clone(),
    };
    PublishMessageRequest {
        target: group_target(group_id(&request.room_id), transport_group_id.clone(), None),
        message: TransportMessage {
            id: id(message_id),
            payload: serde_json::to_vec(&entry).expect("event projection payload"),
            timestamp: Timestamp(0),
            causal_deps: Vec::new(),
            source: TransportSource(HTTP_SERVER_SOURCE.to_owned()),
            envelope: TransportEnvelope::GroupMessage { transport_group_id },
        },
        idempotency_key: Some(format!(
            "event:{}:{}",
            request.room_id, request.idempotency_key
        )),
    }
}

fn insert_durable_commit_publish_without_projection(
    db_path: &std::path::Path,
    request: &PublishMessageRequest,
    seq: u64,
) {
    let operation_json = serde_json::to_string(&serde_json::json!({
        "PublishMessage": {
            "target": &request.target,
            "message": &request.message,
            "idempotency_key": &request.idempotency_key,
        }
    }))
    .expect("persisted operation json");
    let fingerprint_json = serde_json::to_string(&serde_json::json!({
        "target": &request.target,
        "message": &request.message,
    }))
    .expect("publish fingerprint json");
    let receipt_json = serde_json::to_string(&HttpPublishReceipt {
        message_id: request.message.id.clone(),
        plane: HttpDeliveryPlane::Group,
        seq,
        duplicate: false,
    })
    .expect("publish receipt json");
    let idempotency_key = request
        .idempotency_key
        .as_deref()
        .expect("commit publish idempotency key");

    let conn = Connection::open(db_path).expect("sqlite connection");
    conn.execute(
        "INSERT INTO http_delivery_ops (kind, body_json) VALUES (?1, ?2)",
        params!["publish_message", operation_json],
    )
    .expect("insert durable publish operation");
    conn.execute(
        "INSERT INTO http_publish_idempotency (
            idempotency_key,
            fingerprint_json,
            receipt_json
        ) VALUES (?1, ?2, ?3)",
        params![idempotency_key, fingerprint_json, receipt_json],
    )
    .expect("insert durable publish idempotency");
}

fn insert_durable_publish_idempotency_only(
    db_path: &std::path::Path,
    request: &PublishMessageRequest,
    seq: u64,
) {
    let fingerprint_json = serde_json::to_string(&serde_json::json!({
        "target": &request.target,
        "message": &request.message,
    }))
    .expect("publish fingerprint json");
    let receipt_json = serde_json::to_string(&HttpPublishReceipt {
        message_id: request.message.id.clone(),
        plane: HttpDeliveryPlane::Group,
        seq,
        duplicate: false,
    })
    .expect("publish receipt json");
    let idempotency_key = request
        .idempotency_key
        .as_deref()
        .expect("publish idempotency key");

    let conn = Connection::open(db_path).expect("sqlite connection");
    conn.execute(
        "INSERT INTO http_publish_idempotency (
            idempotency_key,
            fingerprint_json,
            receipt_json
        ) VALUES (?1, ?2, ?3)",
        params![idempotency_key, fingerprint_json, receipt_json],
    )
    .expect("insert durable publish idempotency");
}

fn append_application_request(
    room_id: &str,
    mls_group_id: &str,
    sender: &DeviceRef,
    epoch: u64,
    payload: &[u8],
    idempotency_key: &str,
) -> AppendEventRequest {
    AppendEventRequest {
        room_id: room_id.to_owned(),
        sender: sender.clone(),
        envelope: FiniteEnvelope {
            room_id: room_id.to_owned(),
            mls_group_id: mls_group_id.to_owned(),
            epoch,
            sender: sender.clone(),
            kind: LogEntryKind::Application,
            payload: payload.to_vec(),
        },
        idempotency_key: idempotency_key.to_owned(),
    }
}

fn ephemeral_activity_request(
    room_id: &str,
    mls_group_id: &str,
    sender: &DeviceRef,
    epoch: u64,
    conversation_id: Option<&str>,
    received_at_ms: u64,
) -> AppendEphemeralActivityRequest {
    AppendEphemeralActivityRequest {
        room_id: room_id.to_owned(),
        mls_group_id: mls_group_id.to_owned(),
        epoch,
        sender: sender.clone(),
        conversation_id: conversation_id.map(str::to_owned),
        payload: format!("activity-{}-{received_at_ms}", sender.device_id).into_bytes(),
        received_at_ms,
        expires_at_ms: received_at_ms + 1_000,
    }
}

fn raw_commit_publish_request(
    room_id: &str,
    mls_group_id: &str,
    sender: &DeviceRef,
    epoch: u64,
    message_id: &str,
    idempotency_key: &str,
) -> PublishMessageRequest {
    let entry = finitechat_proto::RoomLogEntry {
        room_id: room_id.to_owned(),
        seq: 0,
        message_id: message_id.to_owned(),
        sender: sender.clone(),
        kind: LogEntryKind::Commit,
        epoch,
        envelope: FiniteEnvelope {
            room_id: room_id.to_owned(),
            mls_group_id: mls_group_id.to_owned(),
            epoch,
            sender: sender.clone(),
            kind: LogEntryKind::Commit,
            payload: b"raw-commit-without-membership-delta".to_vec(),
        },
        idempotency_key: idempotency_key.to_owned(),
    };
    let transport_group_id = room_id.as_bytes().to_vec();
    let payload = serde_json::to_vec(&entry).expect("room log entry json");
    PublishMessageRequest {
        target: group_target(
            group_id(room_id),
            transport_group_id.clone(),
            Some(HttpCommitAdmission {
                source_epoch: EpochId(epoch),
            }),
        ),
        message: group_message(message_id, transport_group_id, &payload),
        idempotency_key: Some(idempotency_key.to_owned()),
    }
}

fn key_package_publication(
    key_package_id: &str,
    owner: MemberId,
    bytes: &[u8],
) -> HttpKeyPackagePublication {
    HttpKeyPackagePublication {
        key_package_id: HttpKeyPackageId::new(key_package_id.as_bytes().to_vec()),
        owner,
        key_package: KeyPackage::new(bytes.to_vec()),
    }
}

fn fanout_room_plan(
    room_id: &str,
    key_package_id: &str,
    welcome_id: &str,
    commit_idempotency_key: &str,
) -> HttpFanoutRoomPlan {
    HttpFanoutRoomPlan {
        room_id: group_id(room_id),
        key_package_id: HttpKeyPackageId::new(key_package_id.as_bytes().to_vec()),
        welcome_id: id(welcome_id),
        commit_idempotency_key: commit_idempotency_key.to_owned(),
        claimed_key_package_id: Some(HttpKeyPackageId::new(key_package_id.as_bytes().to_vec())),
    }
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

fn welcome_message(message_id: &str, recipient: MemberId, payload: &[u8]) -> TransportMessage {
    TransportMessage {
        id: id(message_id),
        payload: payload.to_vec(),
        timestamp: Timestamp(43),
        causal_deps: Vec::new(),
        source: TransportSource(HTTP_SERVER_SOURCE.to_owned()),
        envelope: TransportEnvelope::Welcome { recipient },
    }
}
