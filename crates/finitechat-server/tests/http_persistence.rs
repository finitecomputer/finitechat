use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Method, Request, Response, StatusCode};
use cgka_traits::engine::KeyPackage;
use cgka_traits::transport::{Timestamp, TransportEnvelope, TransportMessage, TransportSource};
use cgka_traits::{EpochId, GroupId, MemberId, MessageId};
use finitechat_engine::{
    AccountRoomDevice, AccountRoomRecord, AppendEventRequest, CommitAccepted, EventAccepted,
    SubmitCommitRequest, WelcomeRecord,
};
use finitechat_http::{
    AckWelcomeRequest, AckWelcomeResponse, BootstrapAccountRoomRequest,
    BootstrapAccountRoomResponse, ClaimKeyPackageRequest, ClaimKeyPackagesRequest,
    ClaimWelcomesRequest, ErrorResponse, FiniteAccountRoomCommitProjection, GetFanoutRequest,
    GroupSyncRequest, HttpClaimedWelcome, HttpFanoutPlan, HttpFanoutRoomPlan, HttpFanoutRoomStatus,
    HttpKeyPackageClaim, HttpKeyPackageInventory, InboxSyncRequest, KeyPackageInventoryRequest,
    ListAccountRoomDirectoryRequest, ListAccountRoomDirectoryResponse, MarkFanoutDoneRequest,
    MarkFanoutPreparedRequest, PublishMessageRequest, SaveAccountRoomRequest,
    SaveAccountRoomResponse, SaveFanoutRoomRequest,
};
use finitechat_proto::{
    DeviceRef, FiniteEnvelope, LogEntryKind, MembershipAddV1, MembershipDeltaV1, RoomStatus,
    StagedWelcomeV1, WelcomeState,
};
use finitechat_server::{HttpServerState, http_router};
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
        app,
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
        app,
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
                key_package_id: "key-package-add-device".to_owned(),
                key_package_ref: "key-package-ref-add-device".to_owned(),
                key_package_hash: "key-package-hash-add-device".to_owned(),
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
