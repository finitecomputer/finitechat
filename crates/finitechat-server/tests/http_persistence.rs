use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, Response, StatusCode};
use cgka_traits::engine::KeyPackage;
use cgka_traits::transport::{Timestamp, TransportEnvelope, TransportMessage, TransportSource};
use cgka_traits::{EpochId, GroupId, MemberId, MessageId};
use finitechat_engine::{AccountRoomDevice, AccountRoomRecord};
use finitechat_proto::{DeviceRef, RoomStatus};
use finitechat_server::{
    AckWelcomeRequest, AckWelcomeResponse, BootstrapAccountRoomRequest,
    BootstrapAccountRoomResponse, ClaimKeyPackageRequest, ClaimKeyPackagesRequest,
    ClaimWelcomesRequest, ErrorResponse, GroupSyncRequest, HttpClaimedWelcome, HttpFanoutPlan,
    HttpFanoutRoomStatus, HttpKeyPackageClaim, HttpKeyPackageInventory, HttpServerState,
    KeyPackageInventoryRequest, ListAccountRoomDirectoryRequest, ListAccountRoomDirectoryResponse,
    MarkFanoutDoneRequest, MarkFanoutPreparedRequest, PublishMessageRequest,
    SaveAccountRoomRequest, SaveAccountRoomResponse, SaveFanoutRoomRequest, http_router,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tempfile::TempDir;
use tower::ServiceExt;
use transport_http_server::{
    HTTP_SERVER_SOURCE, HttpClaimedKeyPackage, HttpCommitAdmission, HttpKeyPackageId,
    HttpKeyPackagePublication, HttpPublishReceipt, HttpPublishTarget, HttpSyncPage,
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
        app,
        "/sync/group",
        &GroupSyncRequest {
            group_id,
            after_seq: 0,
            limit: 10,
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
        app,
        "/sync/group",
        &GroupSyncRequest {
            group_id,
            after_seq: 0,
            limit: 10,
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
        app,
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
        app,
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
    let response = post_json(
        app,
        "/fanouts/get",
        &finitechat_server::GetFanoutRequest { fanout_id },
    )
    .await;
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
    let response = post_json(
        app,
        "/fanouts/get",
        &finitechat_server::GetFanoutRequest { fanout_id },
    )
    .await;
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

fn id(label: &str) -> MessageId {
    MessageId::new(label.as_bytes().to_vec())
}

fn group_id(label: &str) -> GroupId {
    GroupId::new(label.as_bytes().to_vec())
}

fn member(label: &str) -> MemberId {
    MemberId::new(label.as_bytes().to_vec())
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
) -> finitechat_server::HttpFanoutRoomPlan {
    finitechat_server::HttpFanoutRoomPlan {
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
