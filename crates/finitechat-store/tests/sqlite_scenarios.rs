use finitechat_engine::{
    CreateDirectRoomRequest, CreateRoomRequest, EngineError, KeyPackageRecord, LinkSessionRecord,
    LinkSessionState, RoomRecord, SubmitCommitRequest, UploadKeyPackageRequest, WelcomeRecord,
    device, envelope,
};
use finitechat_proto::{
    DeviceRef, KeyPackageState, LogEntryKind, MAX_ENVELOPE_PAYLOAD_BYTES,
    MAX_LINK_SESSION_PAYLOAD_BYTES, MAX_SYNC_PAGE_ENTRIES, MembershipAddV1, MembershipDeltaV1,
    MembershipRemoveV1, ProtocolLimitError, RoomStatus, WelcomeState,
};
use finitechat_store::{SqliteDeliveryStore, StoreError};
use tempfile::TempDir;

struct SqliteWorld {
    _dir: TempDir,
    db_path: std::path::PathBuf,
    server: SqliteDeliveryStore,
    room_id: String,
    group_id: String,
}

impl SqliteWorld {
    fn direct_room() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("finitechat.sqlite3");
        let mut server = SqliteDeliveryStore::open(&db_path).unwrap();
        server
            .create_room(CreateRoomRequest {
                room_id: "room_direct".to_string(),
                mls_group_id: "mls_direct".to_string(),
                creator: alice(),
            })
            .unwrap();
        Self {
            _dir: dir,
            db_path,
            server,
            room_id: "room_direct".to_string(),
            group_id: "mls_direct".to_string(),
        }
    }

    fn reopen(&self) -> SqliteDeliveryStore {
        SqliteDeliveryStore::open(&self.db_path).unwrap()
    }

    fn upload_and_claim(&mut self, owner: DeviceRef, key_package_id: &str) {
        self.server
            .upload_key_package(UploadKeyPackageRequest {
                key_package_id: key_package_id.to_string(),
                owner,
                key_package_ref: format!("ref_{key_package_id}"),
                key_package_hash: format!("hash_{key_package_id}"),
            })
            .unwrap();
        self.server.claim_key_package(key_package_id).unwrap();
    }

    fn add_device_request(
        &self,
        sender: DeviceRef,
        target: DeviceRef,
        key_package_id: &str,
        welcome_id: &str,
        expected_epoch: u64,
        idempotency_key: &str,
    ) -> SubmitCommitRequest {
        let commit = envelope(
            self.room_id.clone(),
            self.group_id.clone(),
            sender.clone(),
            expected_epoch,
            LogEntryKind::Commit,
            format!("add:{}:{}", target.account_id, target.device_id).into_bytes(),
        );
        let commit_message_id = commit.message_id().unwrap();
        SubmitCommitRequest {
            room_id: self.room_id.clone(),
            sender,
            expected_epoch,
            envelope: commit,
            membership_delta: MembershipDeltaV1 {
                base_epoch: expected_epoch,
                post_commit_epoch: expected_epoch + 1,
                commit_message_id,
                adds: vec![MembershipAddV1 {
                    device: target,
                    key_package_id: key_package_id.to_string(),
                    key_package_ref: format!("ref_{key_package_id}"),
                    key_package_hash: format!("hash_{key_package_id}"),
                    welcome_id: welcome_id.to_string(),
                }],
                removes: vec![],
            },
            idempotency_key: idempotency_key.to_string(),
        }
    }

    fn remove_device_request(
        &self,
        sender: DeviceRef,
        target: DeviceRef,
        expected_epoch: u64,
        idempotency_key: &str,
    ) -> SubmitCommitRequest {
        let commit = envelope(
            self.room_id.clone(),
            self.group_id.clone(),
            sender.clone(),
            expected_epoch,
            LogEntryKind::Commit,
            format!("remove:{}:{}", target.account_id, target.device_id).into_bytes(),
        );
        let commit_message_id = commit.message_id().unwrap();
        SubmitCommitRequest {
            room_id: self.room_id.clone(),
            sender,
            expected_epoch,
            envelope: commit,
            membership_delta: MembershipDeltaV1 {
                base_epoch: expected_epoch,
                post_commit_epoch: expected_epoch + 1,
                commit_message_id,
                adds: vec![],
                removes: vec![MembershipRemoveV1 {
                    device: target,
                    removed_leaf_index: 1,
                }],
            },
            idempotency_key: idempotency_key.to_string(),
        }
    }

    fn provision_bob(&mut self) {
        self.upload_and_claim(bob(), "kp_bob_1");
        let request =
            self.add_device_request(alice(), bob(), "kp_bob_1", "welcome_bob_1", 0, "add_bob");
        self.server.submit_commit(request).unwrap();
        self.server.claim_welcomes(&bob()).unwrap();
        self.server.ack_welcome("welcome_bob_1", true).unwrap();
    }
}

fn alice() -> DeviceRef {
    device("alice_npub", "alice_browser")
}

fn bob() -> DeviceRef {
    device("bob_npub", "bob_runtime")
}

fn charlie() -> DeviceRef {
    device("charlie_npub", "charlie_phone")
}

fn store_engine_error(error: StoreError) -> EngineError {
    match error {
        StoreError::Engine(error) => error,
        other => panic!("expected engine error, got {other:?}"),
    }
}

fn room(store: &SqliteDeliveryStore, room_id: &str) -> RoomRecord {
    store.room(room_id).unwrap().unwrap()
}

fn key_package(store: &SqliteDeliveryStore, key_package_id: &str) -> KeyPackageRecord {
    store.key_package(key_package_id).unwrap().unwrap()
}

fn welcome(store: &SqliteDeliveryStore, welcome_id: &str) -> WelcomeRecord {
    store.welcome(welcome_id).unwrap().unwrap()
}

fn maybe_welcome(store: &SqliteDeliveryStore, welcome_id: &str) -> Option<WelcomeRecord> {
    store.welcome(welcome_id).unwrap()
}

fn link_session(store: &SqliteDeliveryStore, link_session_id: &str) -> LinkSessionRecord {
    store.link_session(link_session_id).unwrap().unwrap()
}

#[test]
fn sqlite_create_dm_room_and_release_welcome_after_commit() {
    let mut world = SqliteWorld::direct_room();
    world.upload_and_claim(bob(), "kp_bob_1");
    let request = world.add_device_request(
        alice(),
        bob(),
        "kp_bob_1",
        "welcome_bob_1",
        0,
        "idem_add_bob",
    );
    world.server.submit_commit(request).unwrap();

    let reopened = world.reopen();
    let room = room(&reopened, &world.room_id);
    assert_eq!(room.current_epoch, 1);
    assert_eq!(room.last_seq, 1);
    assert_eq!(room.log.len(), 1);
    assert_eq!(
        key_package(&reopened, "kp_bob_1").state,
        KeyPackageState::Consumed
    );
    assert_eq!(
        welcome(&reopened, "welcome_bob_1").state,
        WelcomeState::Released
    );
}

#[test]
fn sqlite_duplicate_commit_retry_after_reopen_returns_same_result() {
    let mut world = SqliteWorld::direct_room();
    world.upload_and_claim(bob(), "kp_bob_1");
    let request = world.add_device_request(
        alice(),
        bob(),
        "kp_bob_1",
        "welcome_bob_1",
        0,
        "idem_add_bob",
    );
    let first = world.server.submit_commit(request.clone()).unwrap();

    let mut reopened = world.reopen();
    let second = reopened.submit_commit(request).unwrap();

    assert_eq!(first, second);
    assert_eq!(room(&reopened, &world.room_id).log.len(), 1);
}

#[test]
fn sqlite_rejected_commit_is_replayable_after_reopen() {
    let mut world = SqliteWorld::direct_room();
    world.upload_and_claim(bob(), "kp_bob_1");
    world.upload_and_claim(charlie(), "kp_charlie_1");
    let winner = world.add_device_request(alice(), bob(), "kp_bob_1", "welcome_bob_1", 0, "winner");
    let loser = world.add_device_request(
        alice(),
        charlie(),
        "kp_charlie_1",
        "welcome_charlie_1",
        0,
        "loser",
    );

    world.server.submit_commit(winner).unwrap();
    assert_eq!(room(&world.server, &world.room_id).current_epoch, 1);
    let first_err = store_engine_error(world.server.submit_commit(loser.clone()).unwrap_err());
    let mut reopened = world.reopen();
    let replayed_err = store_engine_error(reopened.submit_commit(loser).unwrap_err());

    assert_eq!(first_err, replayed_err);
    assert!(matches!(
        replayed_err,
        EngineError::WrongEpoch {
            expected: 1,
            actual: 0
        }
    ));
    assert!(maybe_welcome(&reopened, "welcome_charlie_1").is_none());
}

#[test]
fn sqlite_conflicting_idempotency_key_has_no_side_effects() {
    let mut world = SqliteWorld::direct_room();
    world.upload_and_claim(bob(), "kp_bob_1");
    world.upload_and_claim(charlie(), "kp_charlie_1");
    let first =
        world.add_device_request(alice(), bob(), "kp_bob_1", "welcome_bob_1", 0, "same_key");
    let conflict = world.add_device_request(
        alice(),
        charlie(),
        "kp_charlie_1",
        "welcome_charlie_1",
        0,
        "same_key",
    );

    world.server.submit_commit(first).unwrap();
    let err = store_engine_error(world.server.submit_commit(conflict).unwrap_err());

    assert_eq!(err, EngineError::ConflictingIdempotencyKey);
    assert!(maybe_welcome(&world.server, "welcome_charlie_1").is_none());
    assert_eq!(
        key_package(&world.server, "kp_charlie_1").state,
        KeyPackageState::Leased
    );
}

#[test]
fn sqlite_welcome_not_released_before_accepted_commit() {
    let mut world = SqliteWorld::direct_room();
    world.upload_and_claim(bob(), "kp_bob_1");
    let mut bad =
        world.add_device_request(alice(), bob(), "kp_bob_1", "welcome_bob_1", 0, "bad_add");
    bad.membership_delta.adds[0].key_package_hash = "wrong".to_string();
    assert!(world.server.submit_commit(bad).is_err());
    assert!(maybe_welcome(&world.server, "welcome_bob_1").is_none());

    let good = world.add_device_request(alice(), bob(), "kp_bob_1", "welcome_bob_1", 0, "good_add");
    world.server.submit_commit(good).unwrap();
    assert_eq!(
        welcome(&world.server, "welcome_bob_1").state,
        WelcomeState::Released
    );
}

#[test]
fn sqlite_key_package_lease_expiry_and_reclaim_survives_reopen() {
    let mut world = SqliteWorld::direct_room();
    world.upload_and_claim(bob(), "kp_bob_1");
    world.server.expire_key_package_lease("kp_bob_1").unwrap();

    let mut reopened = world.reopen();
    assert_eq!(
        key_package(&reopened, "kp_bob_1").state,
        KeyPackageState::Available
    );
    reopened.claim_key_package("kp_bob_1").unwrap();
    assert_eq!(
        key_package(&reopened, "kp_bob_1").state,
        KeyPackageState::Leased
    );
}

#[test]
fn sqlite_consumed_key_package_cannot_be_reused() {
    let mut world = SqliteWorld::direct_room();
    world.upload_and_claim(bob(), "kp_bob_1");
    let first = world.add_device_request(alice(), bob(), "kp_bob_1", "welcome_bob_1", 0, "first");
    world.server.submit_commit(first).unwrap();
    let second = world.add_device_request(alice(), bob(), "kp_bob_1", "welcome_bob_2", 1, "second");

    let err = store_engine_error(world.server.submit_commit(second).unwrap_err());

    assert_eq!(
        err,
        EngineError::KeyPackageUnavailable {
            key_package_id: "kp_bob_1".to_string(),
            state: KeyPackageState::Consumed
        }
    );
    assert!(maybe_welcome(&world.server, "welcome_bob_2").is_none());
}

#[test]
fn sqlite_removed_device_can_sync_through_removal_after_reopen() {
    let mut world = SqliteWorld::direct_room();
    world.provision_bob();
    let remove = world.remove_device_request(alice(), bob(), 1, "remove_bob");
    let accepted = world.server.submit_commit(remove).unwrap();

    let reopened = world.reopen();
    let bob_page = reopened.sync_events(&world.room_id, &bob(), 1).unwrap();
    assert_eq!(bob_page.entries.len(), 1);
    assert_eq!(bob_page.entries[0].seq, accepted.seq);
    assert_eq!(bob_page.next_after_seq, accepted.seq);
    assert!(!bob_page.has_more);
}

#[test]
fn sqlite_invalid_commit_report_blocks_room_after_reopen() {
    let mut world = SqliteWorld::direct_room();
    world.upload_and_claim(bob(), "kp_bob_1");
    let request = world.add_device_request(alice(), bob(), "kp_bob_1", "welcome_bob_1", 0, "add");
    let accepted = world.server.submit_commit(request).unwrap();
    world
        .server
        .report_invalid_commit(&world.room_id, &alice(), accepted.seq)
        .unwrap();

    let reopened = world.reopen();
    assert_eq!(
        room(&reopened, &world.room_id).status,
        RoomStatus::NeedsRepair
    );
}

#[test]
fn sqlite_welcome_claim_crash_before_ack_resumes_after_reopen() {
    let mut world = SqliteWorld::direct_room();
    world.upload_and_claim(bob(), "kp_bob_1");
    let request =
        world.add_device_request(alice(), bob(), "kp_bob_1", "welcome_bob_1", 0, "add_bob");
    world.server.submit_commit(request).unwrap();
    world.server.claim_welcomes(&bob()).unwrap();

    let mut reopened = world.reopen();
    reopened.ack_welcome("welcome_bob_1", true).unwrap();
    assert!(
        reopened
            .room(&world.room_id)
            .unwrap()
            .unwrap()
            .device_active_at_head(&bob())
    );
    assert_eq!(
        welcome(&reopened, "welcome_bob_1").state,
        WelcomeState::Acked
    );
}

#[test]
fn sqlite_delayed_welcome_syncs_forward_from_commit_seq() {
    let mut world = SqliteWorld::direct_room();
    world.upload_and_claim(bob(), "kp_bob_1");
    let request =
        world.add_device_request(alice(), bob(), "kp_bob_1", "welcome_bob_1", 0, "add_bob");
    world.server.submit_commit(request).unwrap();
    world
        .server
        .append_event(finitechat_engine::AppendEventRequest {
            room_id: world.room_id.clone(),
            sender: alice(),
            envelope: envelope(
                world.room_id.clone(),
                world.group_id.clone(),
                alice(),
                1,
                LogEntryKind::Application,
                b"later".to_vec(),
            ),
            idempotency_key: "msg_later".to_string(),
        })
        .unwrap();
    world.server.claim_welcomes(&bob()).unwrap();
    world.server.ack_welcome("welcome_bob_1", true).unwrap();

    let reopened = world.reopen();
    let page = reopened.sync_events(&world.room_id, &bob(), 1).unwrap();
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].seq, 2);
    assert_eq!(page.next_after_seq, 2);
    assert!(!page.has_more);
}

#[test]
fn sqlite_terminal_welcome_failure_keeps_interval_inactive() {
    let mut world = SqliteWorld::direct_room();
    world.upload_and_claim(bob(), "kp_bob_1");
    let request =
        world.add_device_request(alice(), bob(), "kp_bob_1", "welcome_bob_1", 0, "add_bob");
    world.server.submit_commit(request).unwrap();
    world.server.claim_welcomes(&bob()).unwrap();
    world.server.ack_welcome("welcome_bob_1", false).unwrap();

    let reopened = world.reopen();
    assert_eq!(
        welcome(&reopened, "welcome_bob_1").state,
        WelcomeState::Failed
    );
    assert!(
        !reopened
            .room(&world.room_id)
            .unwrap()
            .unwrap()
            .device_active_at_head(&bob())
    );
}

#[test]
fn sqlite_link_session_state_machine_survives_reopen() {
    let mut world = SqliteWorld::direct_room();
    world
        .server
        .create_link_session("link_1", "pairing_key_1")
        .unwrap();
    world
        .server
        .upload_link_payload("link_1", b"ciphertext".to_vec())
        .unwrap();
    let (_, token) = world.server.claim_link_payload("link_1").unwrap();

    let mut reopened = world.reopen();
    assert_eq!(
        link_session(&reopened, "link_1").state,
        LinkSessionState::Claimed
    );
    reopened.ack_link_payload("link_1", &token).unwrap();
    assert_eq!(
        link_session(&reopened, "link_1").state,
        LinkSessionState::Delivered
    );
}

#[test]
fn sqlite_direct_room_create_or_get_and_third_account_rejection() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("finitechat.sqlite3");
    let mut server = SqliteDeliveryStore::open(&path).unwrap();
    let first = server
        .create_or_get_direct_room(CreateDirectRoomRequest {
            room_id: "direct_ab".to_string(),
            mls_group_id: "mls_ab".to_string(),
            creator: alice(),
            other_account_id: bob().account_id,
        })
        .unwrap();
    let second = server
        .create_or_get_direct_room(CreateDirectRoomRequest {
            room_id: "direct_ba_should_not_create".to_string(),
            mls_group_id: "mls_ba".to_string(),
            creator: bob(),
            other_account_id: alice().account_id,
        })
        .unwrap();
    assert_eq!(first, second);

    server
        .upload_key_package(UploadKeyPackageRequest {
            key_package_id: "kp_charlie".to_string(),
            owner: charlie(),
            key_package_ref: "ref_kp_charlie".to_string(),
            key_package_hash: "hash_kp_charlie".to_string(),
        })
        .unwrap();
    server.claim_key_package("kp_charlie").unwrap();
    let commit = envelope(
        first.clone(),
        "mls_ab",
        alice(),
        0,
        LogEntryKind::Commit,
        b"add-charlie".to_vec(),
    );
    let commit_id = commit.message_id().unwrap();
    let request = SubmitCommitRequest {
        room_id: first,
        sender: alice(),
        expected_epoch: 0,
        envelope: commit,
        membership_delta: MembershipDeltaV1 {
            base_epoch: 0,
            post_commit_epoch: 1,
            commit_message_id: commit_id,
            adds: vec![MembershipAddV1 {
                device: charlie(),
                key_package_id: "kp_charlie".to_string(),
                key_package_ref: "ref_kp_charlie".to_string(),
                key_package_hash: "hash_kp_charlie".to_string(),
                welcome_id: "welcome_charlie".to_string(),
            }],
            removes: vec![],
        },
        idempotency_key: "add_third".to_string(),
    };

    let err = store_engine_error(server.submit_commit(request).unwrap_err());
    assert_eq!(
        err,
        EngineError::DirectRoomThirdAccount(charlie().account_id)
    );
}

#[test]
fn sqlite_oversized_application_payload_is_rejected_without_persisting_log() {
    let mut world = SqliteWorld::direct_room();
    let err = store_engine_error(
        world
            .server
            .append_event(finitechat_engine::AppendEventRequest {
                room_id: world.room_id.clone(),
                sender: alice(),
                envelope: envelope(
                    world.room_id.clone(),
                    world.group_id.clone(),
                    alice(),
                    0,
                    LogEntryKind::Application,
                    vec![0; MAX_ENVELOPE_PAYLOAD_BYTES as usize + 1],
                ),
                idempotency_key: "oversized_payload".to_string(),
            })
            .unwrap_err(),
    );

    assert!(matches!(
        err,
        EngineError::ProtocolLimit(ProtocolLimitError::BytesTooLong { field, .. })
            if field == "envelope.payload"
    ));
    assert_eq!(room(&world.server, &world.room_id).log.len(), 0);
    assert_eq!(room(&world.reopen(), &world.room_id).log.len(), 0);
}

#[test]
fn sqlite_sync_events_returns_bounded_page_after_reopen() {
    let mut world = SqliteWorld::direct_room();
    for index in 0..=MAX_SYNC_PAGE_ENTRIES {
        world
            .server
            .append_event(finitechat_engine::AppendEventRequest {
                room_id: world.room_id.clone(),
                sender: alice(),
                envelope: envelope(
                    world.room_id.clone(),
                    world.group_id.clone(),
                    alice(),
                    0,
                    LogEntryKind::Application,
                    format!("small_{index}").into_bytes(),
                ),
                idempotency_key: format!("msg_{index}"),
            })
            .unwrap();
    }

    let reopened = world.reopen();
    let page = reopened.sync_events(&world.room_id, &alice(), 0).unwrap();

    assert_eq!(page.entries.len(), MAX_SYNC_PAGE_ENTRIES as usize);
    assert_eq!(page.entries.first().unwrap().seq, 1);
    assert_eq!(
        page.entries.last().unwrap().seq,
        u64::from(MAX_SYNC_PAGE_ENTRIES)
    );
    assert_eq!(page.next_after_seq, u64::from(MAX_SYNC_PAGE_ENTRIES));
    assert!(page.has_more);
}

#[test]
fn sqlite_duplicate_message_id_is_typed_engine_error() {
    let mut world = SqliteWorld::direct_room();
    let first = finitechat_engine::AppendEventRequest {
        room_id: world.room_id.clone(),
        sender: alice(),
        envelope: envelope(
            world.room_id.clone(),
            world.group_id.clone(),
            alice(),
            0,
            LogEntryKind::Application,
            b"same ciphertext".to_vec(),
        ),
        idempotency_key: "msg_first".to_string(),
    };
    let duplicate = finitechat_engine::AppendEventRequest {
        idempotency_key: "msg_second".to_string(),
        ..first.clone()
    };
    let message_id = first.envelope.message_id().unwrap();

    world.server.append_event(first).unwrap();
    let err = store_engine_error(world.server.append_event(duplicate).unwrap_err());

    assert_eq!(err, EngineError::DuplicateMessageId(message_id));
    assert_eq!(room(&world.reopen(), &world.room_id).log.len(), 1);
}

#[test]
fn sqlite_link_payload_limit_is_rejected() {
    let mut world = SqliteWorld::direct_room();
    world
        .server
        .create_link_session("link_big", "pairing_key_1")
        .unwrap();
    let err = store_engine_error(
        world
            .server
            .upload_link_payload(
                "link_big",
                vec![0; MAX_LINK_SESSION_PAYLOAD_BYTES as usize + 1],
            )
            .unwrap_err(),
    );

    assert!(matches!(
        err,
        EngineError::ProtocolLimit(ProtocolLimitError::BytesTooLong { field, .. })
            if field == "link_session.encrypted_payload"
    ));
    assert!(
        link_session(&world.server, "link_big")
            .encrypted_payload
            .is_none()
    );
}
