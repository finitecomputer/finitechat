use std::collections::BTreeSet;
use std::path::Path;

use finitechat_engine::{
    AppendApplicationEventRequest, AppendEphemeralActivityRequest, AppendEventRequest,
    CommitAccepted, CreateDirectRoomRequest, CreateRoomRequest, DeliveryService, DeviceStatus,
    EngineError, KeyPackageRecord, LinkSessionRecord, LinkSessionState, ListAccountRoomsRequest,
    RoomRecord, SubmitCommitRequest, UploadKeyPackageRequest, WelcomeRecord, device, envelope,
    idempotency_scope_key,
};
use finitechat_proto::{
    ApplicationDeliveryPolicy, DeviceRef, DurableAppEventKind, KeyPackageState, LogEntryKind,
    MAX_ENVELOPE_PAYLOAD_BYTES, MAX_EPHEMERAL_ACTIVITY_EXPIRY_MILLIS,
    MAX_IDEMPOTENCY_RECORDS_PER_ROOM_DEVICE, MAX_KEY_PACKAGES_PER_DEVICE,
    MAX_LINK_SESSION_PAYLOAD_BYTES, MAX_SYNC_PAGE_ENTRIES, MembershipAddV1, MembershipDeltaV1,
    MembershipRemoveV1, ProtocolLimitError, RoomStatus, StagedWelcomeV1, WelcomeState,
};
use finitechat_store::{SqliteDeliveryStore, StoreError};
use rusqlite::{Connection, ErrorCode, params};
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
                key_package_payload: fake_key_package_payload(key_package_id),
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
            staged_welcomes: vec![staged_welcome(welcome_id)],
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
            staged_welcomes: vec![],
        }
    }

    fn charlie_replaces_bob_request(&self) -> SubmitCommitRequest {
        let sender = alice();
        let removed = bob();
        let added = charlie();
        let commit = envelope(
            self.room_id.clone(),
            self.group_id.clone(),
            sender.clone(),
            1,
            LogEntryKind::Commit,
            format!(
                "replace:{}:{}:{}:{}",
                removed.account_id, removed.device_id, added.account_id, added.device_id
            )
            .into_bytes(),
        );
        let commit_message_id = commit.message_id().unwrap();
        SubmitCommitRequest {
            room_id: self.room_id.clone(),
            sender,
            expected_epoch: 1,
            envelope: commit,
            membership_delta: MembershipDeltaV1 {
                base_epoch: 1,
                post_commit_epoch: 2,
                commit_message_id,
                adds: vec![MembershipAddV1 {
                    device: added,
                    key_package_id: "kp_charlie_1".to_string(),
                    key_package_ref: "ref_kp_charlie_1".to_string(),
                    key_package_hash: "hash_kp_charlie_1".to_string(),
                    welcome_id: "welcome_charlie_1".to_string(),
                }],
                removes: vec![MembershipRemoveV1 {
                    device: removed,
                    removed_leaf_index: 1,
                }],
            },
            idempotency_key: "crash_commit".to_string(),
            staged_welcomes: vec![staged_welcome("welcome_charlie_1")],
        }
    }

    fn app_message_request(
        &self,
        sender: DeviceRef,
        epoch: u64,
        body: &str,
        idempotency_key: &str,
    ) -> AppendEventRequest {
        AppendEventRequest {
            room_id: self.room_id.clone(),
            sender: sender.clone(),
            envelope: envelope(
                self.room_id.clone(),
                self.group_id.clone(),
                sender,
                epoch,
                LogEntryKind::Application,
                body.as_bytes().to_vec(),
            ),
            idempotency_key: idempotency_key.to_string(),
        }
    }

    fn application_event_request(
        &self,
        sender: DeviceRef,
        epoch: u64,
        payload: &[u8],
        idempotency_key: &str,
        delivery_policy: ApplicationDeliveryPolicy,
    ) -> AppendApplicationEventRequest {
        AppendApplicationEventRequest {
            event: AppendEventRequest {
                room_id: self.room_id.clone(),
                sender: sender.clone(),
                envelope: envelope(
                    self.room_id.clone(),
                    self.group_id.clone(),
                    sender,
                    epoch,
                    LogEntryKind::Application,
                    payload.to_vec(),
                ),
                idempotency_key: idempotency_key.to_string(),
            },
            delivery_policy,
        }
    }

    fn ephemeral_activity_request(
        &self,
        sender: DeviceRef,
        epoch: u64,
        conversation_id: Option<&str>,
        received_at_ms: u64,
    ) -> AppendEphemeralActivityRequest {
        AppendEphemeralActivityRequest {
            room_id: self.room_id.clone(),
            mls_group_id: self.group_id.clone(),
            epoch,
            sender,
            conversation_id: conversation_id.map(str::to_string),
            payload: b"opaque activity ciphertext".to_vec(),
            received_at_ms,
            expires_at_ms: received_at_ms + MAX_EPHEMERAL_ACTIVITY_EXPIRY_MILLIS,
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

struct DualDeliveryWorld {
    _dir: TempDir,
    memory: DeliveryService,
    sqlite: SqliteDeliveryStore,
    room_id: String,
    group_id: String,
    known_key_packages: BTreeSet<String>,
    known_welcomes: BTreeSet<String>,
    known_devices: Vec<DeviceRef>,
    last_commit_request: Option<SubmitCommitRequest>,
    last_event_request: Option<AppendEventRequest>,
}

impl DualDeliveryWorld {
    fn new(seed: u64) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("finitechat_fuzz.sqlite3");
        let mut memory = DeliveryService::new();
        let mut sqlite = SqliteDeliveryStore::open(&db_path).unwrap();
        let room_id = format!("room_fuzz_{seed}");
        let group_id = format!("mls_fuzz_{seed}");
        let create = CreateRoomRequest {
            room_id: room_id.clone(),
            mls_group_id: group_id.clone(),
            creator: alice(),
        };
        memory.create_room(create.clone()).unwrap();
        sqlite.create_room(create).unwrap();
        let world = Self {
            _dir: dir,
            memory,
            sqlite,
            room_id,
            group_id,
            known_key_packages: BTreeSet::new(),
            known_welcomes: BTreeSet::new(),
            known_devices: vec![alice(), bob(), charlie(), dana()],
            last_commit_request: None,
            last_event_request: None,
        };
        world.assert_equivalent();
        world
    }

    fn register_device(&mut self, device: DeviceRef) {
        let memory = self.memory.register_device(device.clone());
        let sqlite = sqlite_result(self.sqlite.register_device(device));
        assert_eq!(memory, sqlite);
        self.assert_equivalent();
    }

    fn revoke_device(&mut self, device: DeviceRef) {
        let memory = self.memory.revoke_device(device.clone());
        let sqlite = sqlite_result(self.sqlite.revoke_device(device));
        assert_eq!(memory, sqlite);
        self.assert_equivalent();
    }

    fn upload_key_package(&mut self, owner: DeviceRef, key_package_id: &str) {
        let request = upload_key_package_request(owner, key_package_id);
        self.known_key_packages
            .insert(request.key_package_id.clone());
        let memory = self.memory.upload_key_package(request.clone());
        let sqlite = sqlite_result(self.sqlite.upload_key_package(request));
        assert_eq!(memory, sqlite);
        self.assert_equivalent();
    }

    fn claim_key_package(&mut self, key_package_id: &str) {
        self.known_key_packages.insert(key_package_id.to_string());
        let memory = self.memory.claim_key_package(key_package_id);
        let sqlite = sqlite_result(self.sqlite.claim_key_package(key_package_id));
        assert_eq!(memory, sqlite);
        self.assert_equivalent();
    }

    fn claim_key_packages_for_account(&mut self, account_id: &str) {
        let memory = self.memory.claim_key_packages_for_account(account_id);
        let sqlite = sqlite_result(self.sqlite.claim_key_packages_for_account(account_id));
        assert_eq!(memory, sqlite);
        self.assert_equivalent();
    }

    fn expire_key_package_lease(&mut self, key_package_id: &str) {
        self.known_key_packages.insert(key_package_id.to_string());
        let memory = self.memory.expire_key_package_lease(key_package_id);
        let sqlite = sqlite_result(self.sqlite.expire_key_package_lease(key_package_id));
        assert_eq!(memory, sqlite);
        self.assert_equivalent();
    }

    fn claim_welcomes(&mut self, device: &DeviceRef) {
        let memory = self.memory.claim_welcomes(device);
        let sqlite = sqlite_result(self.sqlite.claim_welcomes(device));
        assert_eq!(memory, sqlite);
        self.assert_equivalent();
    }

    fn ack_welcome(&mut self, welcome_id: &str, activated: bool) {
        self.known_welcomes.insert(welcome_id.to_string());
        let memory = self.memory.ack_welcome(welcome_id, activated);
        let sqlite = sqlite_result(self.sqlite.ack_welcome(welcome_id, activated));
        assert_eq!(memory, sqlite);
        self.assert_equivalent();
    }

    fn append_event(&mut self, request: AppendEventRequest) {
        self.last_event_request = Some(request.clone());
        let memory = self.memory.append_event(request.clone());
        let sqlite = sqlite_result(self.sqlite.append_event(request));
        assert_eq!(memory, sqlite);
        self.assert_equivalent();
    }

    fn retry_last_event(&mut self) {
        if let Some(request) = self.last_event_request.clone() {
            let memory = self.memory.append_event(request.clone());
            let sqlite = sqlite_result(self.sqlite.append_event(request));
            assert_eq!(memory, sqlite);
            self.assert_equivalent();
        }
    }

    fn submit_commit(&mut self, request: SubmitCommitRequest) {
        self.remember_commit_artifacts(&request);
        self.last_commit_request = Some(request.clone());
        let memory = self.memory.submit_commit(request.clone());
        let sqlite = sqlite_result(self.sqlite.submit_commit(request));
        assert_eq!(memory, sqlite);
        self.assert_equivalent();
    }

    fn retry_last_commit(&mut self) {
        if let Some(request) = self.last_commit_request.clone() {
            let memory = self.memory.submit_commit(request.clone());
            let sqlite = sqlite_result(self.sqlite.submit_commit(request));
            assert_eq!(memory, sqlite);
            self.assert_equivalent();
        }
    }

    fn add_request(
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
            format!(
                "fuzz:add:{}:{}:{}",
                target.account_id, target.device_id, idempotency_key
            )
            .into_bytes(),
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
            staged_welcomes: vec![staged_welcome(welcome_id)],
        }
    }

    fn remove_request(
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
            format!(
                "fuzz:remove:{}:{}:{}",
                target.account_id, target.device_id, idempotency_key
            )
            .into_bytes(),
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
            staged_welcomes: vec![],
        }
    }

    fn event_request(
        &self,
        sender: DeviceRef,
        epoch: u64,
        idempotency_key: &str,
    ) -> AppendEventRequest {
        AppendEventRequest {
            room_id: self.room_id.clone(),
            sender: sender.clone(),
            envelope: envelope(
                self.room_id.clone(),
                self.group_id.clone(),
                sender,
                epoch,
                LogEntryKind::Application,
                format!("fuzz:event:{idempotency_key}").into_bytes(),
            ),
            idempotency_key: idempotency_key.to_string(),
        }
    }

    fn current_epoch(&self) -> u64 {
        self.memory.room(&self.room_id).unwrap().current_epoch
    }

    fn maybe_stale_epoch(&self, stale: bool) -> u64 {
        let current_epoch = self.current_epoch();
        if stale && current_epoch > 0 {
            current_epoch - 1
        } else {
            current_epoch
        }
    }

    fn remember_commit_artifacts(&mut self, request: &SubmitCommitRequest) {
        for add in &request.membership_delta.adds {
            self.known_key_packages.insert(add.key_package_id.clone());
            self.known_welcomes.insert(add.welcome_id.clone());
        }
    }

    fn assert_equivalent(&self) {
        assert_eq!(
            self.memory.room(&self.room_id),
            self.sqlite.room(&self.room_id).unwrap().as_ref()
        );
        for key_package_id in &self.known_key_packages {
            assert_eq!(
                self.memory.key_package(key_package_id),
                self.sqlite.key_package(key_package_id).unwrap().as_ref()
            );
        }
        for welcome_id in &self.known_welcomes {
            assert_eq!(
                self.memory.welcome(welcome_id),
                self.sqlite.welcome(welcome_id).unwrap().as_ref()
            );
        }
        for device in &self.known_devices {
            assert_eq!(
                self.memory.device(device),
                self.sqlite.device(device).unwrap().as_ref()
            );
            assert_eq!(
                self.memory.key_package_inventory(device),
                sqlite_result(self.sqlite.key_package_inventory(device))
            );
        }
    }
}

fn sqlite_result<T>(result: Result<T, StoreError>) -> Result<T, EngineError> {
    result.map_err(store_engine_error)
}

fn upload_key_package_request(owner: DeviceRef, key_package_id: &str) -> UploadKeyPackageRequest {
    UploadKeyPackageRequest {
        key_package_id: key_package_id.to_string(),
        owner,
        key_package_ref: format!("ref_{key_package_id}"),
        key_package_hash: format!("hash_{key_package_id}"),
        key_package_payload: fake_key_package_payload(key_package_id),
    }
}

struct Lcg {
    state: u64,
}

impl Lcg {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self
            .state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.state
    }

    fn next_usize(&mut self, modulo: usize) -> usize {
        debug_assert!(modulo > 0);
        (self.next_u64() % modulo as u64) as usize
    }

    fn next_bool(&mut self) -> bool {
        self.next_usize(2) == 0
    }
}

fn fuzz_device(index: usize) -> DeviceRef {
    match index % 4 {
        0 => alice(),
        1 => bob(),
        2 => charlie(),
        _ => dana(),
    }
}

fn fuzz_device_label(device: &DeviceRef) -> &'static str {
    match device.device_id.as_str() {
        "alice_browser" => "alice",
        "bob_runtime" => "bob",
        "charlie_phone" => "charlie",
        "dana_tablet" => "dana",
        _ => "device",
    }
}

fn fuzz_key_package_id(device: &DeviceRef, slot: usize) -> String {
    format!("kp_fuzz_{}_{}", fuzz_device_label(device), slot % 4)
}

fn fuzz_welcome_id(device: &DeviceRef, slot: usize) -> String {
    format!("welcome_fuzz_{}_{}", fuzz_device_label(device), slot % 4)
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

fn dana() -> DeviceRef {
    device("dana_npub", "dana_tablet")
}

fn staged_welcome(welcome_id: &str) -> StagedWelcomeV1 {
    StagedWelcomeV1 {
        welcome_id: welcome_id.to_string(),
        welcome_payload: format!("welcome:{welcome_id}").into_bytes(),
        ratchet_tree_payload: format!("tree:{welcome_id}").into_bytes(),
    }
}

fn fake_key_package_payload(key_package_id: &str) -> Vec<u8> {
    format!("key-package:{key_package_id}").into_bytes()
}

fn upload_available_key_package(
    store: &mut SqliteDeliveryStore,
    owner: DeviceRef,
    key_package_id: &str,
) {
    store
        .upload_key_package(UploadKeyPackageRequest {
            key_package_id: key_package_id.to_string(),
            owner,
            key_package_ref: format!("ref_{key_package_id}"),
            key_package_hash: format!("hash_{key_package_id}"),
            key_package_payload: fake_key_package_payload(key_package_id),
        })
        .unwrap();
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

fn device_record(
    store: &SqliteDeliveryStore,
    device: &DeviceRef,
) -> finitechat_engine::DeviceRecord {
    store.device(device).unwrap().unwrap()
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

#[derive(Copy, Clone, Debug)]
enum CommitCrashPoint {
    LogEntry,
    RoomHead,
    RemovedMembership,
    AddedMembership,
    ConsumedKeyPackage,
    ReleasedWelcome,
    IdempotencyRecord,
}

impl CommitCrashPoint {
    const ALL: [Self; 7] = [
        Self::LogEntry,
        Self::RoomHead,
        Self::RemovedMembership,
        Self::AddedMembership,
        Self::ConsumedKeyPackage,
        Self::ReleasedWelcome,
        Self::IdempotencyRecord,
    ];

    fn trigger_sql(self) -> &'static str {
        match self {
            Self::LogEntry => {
                r#"
                CREATE TRIGGER finitechat_test_crash_after_log_entry
                AFTER INSERT ON room_log_entries
                WHEN NEW.room_id = 'room_direct'
                  AND NEW.idempotency_key = 'crash_commit'
                BEGIN
                  SELECT RAISE(ROLLBACK, 'finitechat test crash after log entry');
                END;
                "#
            }
            Self::RoomHead => {
                r#"
                CREATE TRIGGER finitechat_test_crash_after_room_head
                AFTER UPDATE OF current_epoch, last_seq ON rooms
                WHEN NEW.room_id = 'room_direct'
                  AND NEW.current_epoch = 2
                  AND NEW.last_seq = 2
                BEGIN
                  SELECT RAISE(ROLLBACK, 'finitechat test crash after room head');
                END;
                "#
            }
            Self::RemovedMembership => {
                r#"
                CREATE TRIGGER finitechat_test_crash_after_removed_membership
                AFTER UPDATE OF end_seq ON room_membership_intervals
                WHEN NEW.room_id = 'room_direct'
                  AND NEW.account_id = 'bob_npub'
                  AND NEW.device_id = 'bob_runtime'
                  AND NEW.start_seq = 1
                  AND NEW.end_seq = 2
                BEGIN
                  SELECT RAISE(ROLLBACK, 'finitechat test crash after removed membership');
                END;
                "#
            }
            Self::AddedMembership => {
                r#"
                CREATE TRIGGER finitechat_test_crash_after_added_membership
                AFTER INSERT ON room_membership_intervals
                WHEN NEW.room_id = 'room_direct'
                  AND NEW.account_id = 'charlie_npub'
                  AND NEW.device_id = 'charlie_phone'
                  AND NEW.start_seq = 2
                BEGIN
                  SELECT RAISE(ROLLBACK, 'finitechat test crash after added membership');
                END;
                "#
            }
            Self::ConsumedKeyPackage => {
                r#"
                CREATE TRIGGER finitechat_test_crash_after_consumed_key_package
                AFTER UPDATE OF state ON key_packages
                WHEN NEW.key_package_id = 'kp_charlie_1'
                  AND NEW.state = 'consumed'
                BEGIN
                  SELECT RAISE(ROLLBACK, 'finitechat test crash after consumed key package');
                END;
                "#
            }
            Self::ReleasedWelcome => {
                r#"
                CREATE TRIGGER finitechat_test_crash_after_released_welcome
                AFTER INSERT ON welcomes
                WHEN NEW.welcome_id = 'welcome_charlie_1'
                  AND NEW.state = 'released'
                BEGIN
                  SELECT RAISE(ROLLBACK, 'finitechat test crash after released welcome');
                END;
                "#
            }
            Self::IdempotencyRecord => {
                r#"
                CREATE TRIGGER finitechat_test_crash_after_idempotency_record
                AFTER INSERT ON idempotency_records
                WHEN NEW.room_id = 'room_direct'
                  AND NEW.operation = 'submit_commit'
                  AND NEW.sender_account_id = 'alice_npub'
                  AND NEW.sender_device_id = 'alice_browser'
                BEGIN
                  SELECT RAISE(ROLLBACK, 'finitechat test crash after idempotency record');
                END;
                "#
            }
        }
    }
}

fn install_commit_crash_trigger(db_path: &Path, point: CommitCrashPoint) {
    clear_commit_crash_triggers(db_path);
    let conn = Connection::open(db_path).unwrap();
    conn.execute_batch(point.trigger_sql()).unwrap();
}

fn clear_commit_crash_triggers(db_path: &Path) {
    let conn = Connection::open(db_path).unwrap();
    conn.execute_batch(
        r#"
        DROP TRIGGER IF EXISTS finitechat_test_crash_after_log_entry;
        DROP TRIGGER IF EXISTS finitechat_test_crash_after_room_head;
        DROP TRIGGER IF EXISTS finitechat_test_crash_after_removed_membership;
        DROP TRIGGER IF EXISTS finitechat_test_crash_after_added_membership;
        DROP TRIGGER IF EXISTS finitechat_test_crash_after_consumed_key_package;
        DROP TRIGGER IF EXISTS finitechat_test_crash_after_released_welcome;
        DROP TRIGGER IF EXISTS finitechat_test_crash_after_idempotency_record;
        "#,
    )
    .unwrap();
}

fn count_rows(db_path: &Path, sql: &str) -> i64 {
    let conn = Connection::open(db_path).unwrap();
    conn.query_row(sql, [], |row| row.get(0)).unwrap()
}

fn crash_commit_idempotency_count(db_path: &Path) -> i64 {
    let scope_key = idempotency_scope_key("room_direct", &alice(), "submit_commit", "crash_commit");
    let conn = Connection::open(db_path).unwrap();
    conn.query_row(
        "SELECT COUNT(*) FROM idempotency_records WHERE scope_key = ?1",
        params![scope_key],
        |row| row.get(0),
    )
    .unwrap()
}

fn assert_sqlite_constraint(error: rusqlite::Error) {
    match error {
        rusqlite::Error::SqliteFailure(sqlite_error, _) => {
            assert_eq!(sqlite_error.code, ErrorCode::ConstraintViolation);
        }
        other => panic!("expected sqlite constraint error, got {other:?}"),
    }
}

fn assert_crash_commit_rolled_back(world: &SqliteWorld) {
    let reopened = world.reopen();
    let room = room(&reopened, &world.room_id);
    assert_eq!(room.current_epoch, 1);
    assert_eq!(room.last_seq, 1);
    assert_eq!(room.log.len(), 1);
    assert!(room.device_active_at_head(&bob()));
    assert!(!room.device_active_at_head(&charlie()));
    assert_eq!(
        key_package(&reopened, "kp_charlie_1").state,
        KeyPackageState::Leased
    );
    assert!(maybe_welcome(&reopened, "welcome_charlie_1").is_none());
    assert_eq!(
        count_rows(
            &world.db_path,
            "SELECT COUNT(*) FROM room_log_entries WHERE room_id = 'room_direct' AND idempotency_key = 'crash_commit'"
        ),
        0
    );
    assert_eq!(crash_commit_idempotency_count(&world.db_path), 0);
    assert_eq!(
        count_rows(
            &world.db_path,
            "SELECT COUNT(*) FROM room_membership_intervals WHERE room_id = 'room_direct' AND account_id = 'charlie_npub' AND device_id = 'charlie_phone'"
        ),
        0
    );
}

fn assert_crash_commit_converged(
    store: &SqliteDeliveryStore,
    world: &SqliteWorld,
    accepted: &CommitAccepted,
) {
    assert_eq!(accepted.seq, 2);
    assert_eq!(accepted.released_welcomes, vec!["welcome_charlie_1"]);

    let room = room(store, &world.room_id);
    assert_eq!(room.current_epoch, 2);
    assert_eq!(room.last_seq, 2);
    assert_eq!(room.log.len(), 2);
    assert!(!room.device_active_at_head(&bob()));
    assert!(!room.device_active_at_head(&charlie()));
    assert_eq!(
        key_package(store, "kp_charlie_1").state,
        KeyPackageState::Consumed
    );
    assert_eq!(
        welcome(store, "welcome_charlie_1").state,
        WelcomeState::Released
    );
    assert_eq!(
        count_rows(
            &world.db_path,
            "SELECT COUNT(*) FROM room_log_entries WHERE room_id = 'room_direct' AND idempotency_key = 'crash_commit'"
        ),
        1
    );
    assert_eq!(crash_commit_idempotency_count(&world.db_path), 1);
    assert_eq!(
        count_rows(
            &world.db_path,
            "SELECT COUNT(*) FROM room_membership_intervals WHERE room_id = 'room_direct' AND account_id = 'bob_npub' AND device_id = 'bob_runtime' AND start_seq = 1 AND end_seq = 2 AND active = 1"
        ),
        1
    );
    assert_eq!(
        count_rows(
            &world.db_path,
            "SELECT COUNT(*) FROM room_membership_intervals WHERE room_id = 'room_direct' AND account_id = 'charlie_npub' AND device_id = 'charlie_phone' AND start_seq = 2 AND end_seq IS NULL AND active = 0"
        ),
        1
    );
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
    let stored_welcome = welcome(&reopened, "welcome_bob_1");
    assert_eq!(stored_welcome.state, WelcomeState::Released);
    assert_eq!(stored_welcome.welcome_payload, b"welcome:welcome_bob_1");
    assert_eq!(stored_welcome.ratchet_tree_payload, b"tree:welcome_bob_1");
}

#[test]
fn sqlite_key_package_payload_survives_reopen_and_claim() {
    let mut world = SqliteWorld::direct_room();
    world
        .server
        .upload_key_package(UploadKeyPackageRequest {
            key_package_id: "kp_bob_1".to_string(),
            owner: bob(),
            key_package_ref: "ref_kp_bob_1".to_string(),
            key_package_hash: "hash_kp_bob_1".to_string(),
            key_package_payload: fake_key_package_payload("kp_bob_1"),
        })
        .unwrap();

    let reopened = world.reopen();
    let stored = key_package(&reopened, "kp_bob_1");
    assert_eq!(
        stored.key_package_payload,
        fake_key_package_payload("kp_bob_1")
    );

    let mut reopened = world.reopen();
    let claimed = reopened.claim_key_package("kp_bob_1").unwrap();
    assert_eq!(claimed.owner, bob());
    assert_eq!(claimed.key_package_ref, "ref_kp_bob_1");
    assert_eq!(claimed.key_package_hash, "hash_kp_bob_1");
    assert_eq!(
        claimed.key_package_payload,
        fake_key_package_payload("kp_bob_1")
    );
}

#[test]
fn sqlite_exact_key_package_upload_retry_is_idempotent_after_reopen() {
    let mut world = SqliteWorld::direct_room();
    upload_available_key_package(&mut world.server, bob(), "kp_bob_1");

    let mut reopened = world.reopen();
    reopened
        .upload_key_package(UploadKeyPackageRequest {
            key_package_id: "kp_bob_1".to_string(),
            owner: bob(),
            key_package_ref: "ref_kp_bob_1".to_string(),
            key_package_hash: "hash_kp_bob_1".to_string(),
            key_package_payload: fake_key_package_payload("kp_bob_1"),
        })
        .unwrap();
    let err = reopened
        .upload_key_package(UploadKeyPackageRequest {
            key_package_id: "kp_bob_1".to_string(),
            owner: bob(),
            key_package_ref: "ref_kp_bob_1_duplicate".to_string(),
            key_package_hash: "hash_kp_bob_1_duplicate".to_string(),
            key_package_payload: fake_key_package_payload("kp_bob_1_duplicate"),
        })
        .unwrap_err();

    assert_eq!(
        store_engine_error(err),
        EngineError::KeyPackageAlreadyExists("kp_bob_1".to_string())
    );
    assert_eq!(
        key_package(&reopened, "kp_bob_1").key_package_payload,
        fake_key_package_payload("kp_bob_1")
    );
}

#[test]
fn sqlite_account_key_package_claim_survives_reopen() {
    let mut world = SqliteWorld::direct_room();
    let bob_phone = device("bob_npub", "bob_phone");
    let bob_laptop = device("bob_npub", "bob_laptop");
    upload_available_key_package(&mut world.server, bob_phone.clone(), "kp_bob_phone_1");
    upload_available_key_package(&mut world.server, bob_phone.clone(), "kp_bob_phone_2");
    upload_available_key_package(&mut world.server, bob_laptop.clone(), "kp_bob_laptop_1");
    upload_available_key_package(&mut world.server, charlie(), "kp_charlie_1");

    let mut reopened = world.reopen();
    let claimed = reopened.claim_key_packages_for_account("bob_npub").unwrap();

    assert_eq!(claimed.len(), 2);
    assert_eq!(claimed[0].owner, bob_laptop);
    assert_eq!(claimed[0].key_package_id, "kp_bob_laptop_1");
    assert_eq!(claimed[1].owner, bob_phone);
    assert_eq!(claimed[1].key_package_id, "kp_bob_phone_1");
    assert_eq!(
        key_package(&reopened, "kp_bob_laptop_1").state,
        KeyPackageState::Leased
    );
    assert_eq!(
        key_package(&reopened, "kp_bob_phone_1").state,
        KeyPackageState::Leased
    );
    assert_eq!(
        key_package(&reopened, "kp_bob_phone_2").state,
        KeyPackageState::Available
    );
    assert_eq!(
        key_package(&reopened, "kp_charlie_1").state,
        KeyPackageState::Available
    );
}

#[test]
fn sqlite_device_key_package_claim_survives_reopen() {
    let mut world = SqliteWorld::direct_room();
    let bob_phone = device("bob_npub", "bob_phone");
    let bob_laptop = device("bob_npub", "bob_laptop");
    upload_available_key_package(&mut world.server, bob_phone.clone(), "kp_bob_phone_1");
    upload_available_key_package(&mut world.server, bob_phone.clone(), "kp_bob_phone_2");
    upload_available_key_package(&mut world.server, bob_laptop, "kp_bob_laptop_1");

    let mut reopened = world.reopen();
    let first = reopened
        .claim_key_package_for_device(&bob_phone)
        .unwrap()
        .unwrap();
    assert_eq!(first.key_package_id, "kp_bob_phone_1");
    let second = reopened
        .claim_key_package_for_device(&bob_phone)
        .unwrap()
        .unwrap();
    assert_eq!(second.key_package_id, "kp_bob_phone_2");
    assert!(
        reopened
            .claim_key_package_for_device(&bob_phone)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        key_package(&reopened, "kp_bob_laptop_1").state,
        KeyPackageState::Available
    );
}

#[test]
fn sqlite_key_package_inventory_cap_survives_reopen_and_consumed_frees_space() {
    let mut world = SqliteWorld::direct_room();
    for index in 0..MAX_KEY_PACKAGES_PER_DEVICE {
        upload_available_key_package(
            &mut world.server,
            bob(),
            &format!("kp_sqlite_inventory_{index}"),
        );
    }

    let mut reopened = world.reopen();
    let inventory = reopened.key_package_inventory(&bob()).unwrap();
    assert_eq!(inventory.available, MAX_KEY_PACKAGES_PER_DEVICE);
    assert_eq!(inventory.leased, 0);
    assert_eq!(
        store_engine_error(
            reopened
                .upload_key_package(UploadKeyPackageRequest {
                    key_package_id: "kp_sqlite_inventory_overflow".to_string(),
                    owner: bob(),
                    key_package_ref: "ref_kp_sqlite_inventory_overflow".to_string(),
                    key_package_hash: "hash_kp_sqlite_inventory_overflow".to_string(),
                    key_package_payload: fake_key_package_payload("kp_sqlite_inventory_overflow"),
                })
                .unwrap_err()
        ),
        EngineError::KeyPackageInventoryFull {
            owner: bob(),
            available: MAX_KEY_PACKAGES_PER_DEVICE,
            leased: 0,
            max: MAX_KEY_PACKAGES_PER_DEVICE,
        }
    );

    reopened.claim_key_package("kp_sqlite_inventory_0").unwrap();
    let inventory = reopened.key_package_inventory(&bob()).unwrap();
    assert_eq!(inventory.available, MAX_KEY_PACKAGES_PER_DEVICE - 1);
    assert_eq!(inventory.leased, 1);
    assert_eq!(
        store_engine_error(
            reopened
                .upload_key_package(UploadKeyPackageRequest {
                    key_package_id: "kp_sqlite_inventory_still_full".to_string(),
                    owner: bob(),
                    key_package_ref: "ref_kp_sqlite_inventory_still_full".to_string(),
                    key_package_hash: "hash_kp_sqlite_inventory_still_full".to_string(),
                    key_package_payload: fake_key_package_payload("kp_sqlite_inventory_still_full"),
                })
                .unwrap_err()
        ),
        EngineError::KeyPackageInventoryFull {
            owner: bob(),
            available: MAX_KEY_PACKAGES_PER_DEVICE - 1,
            leased: 1,
            max: MAX_KEY_PACKAGES_PER_DEVICE,
        }
    );

    let request = world.add_device_request(
        alice(),
        bob(),
        "kp_sqlite_inventory_0",
        "welcome_sqlite_inventory",
        0,
        "add_sqlite_inventory",
    );
    reopened.submit_commit(request).unwrap();
    let mut reopened = world.reopen();
    let inventory = reopened.key_package_inventory(&bob()).unwrap();
    assert_eq!(inventory.available, MAX_KEY_PACKAGES_PER_DEVICE - 1);
    assert_eq!(inventory.leased, 0);
    upload_available_key_package(&mut reopened, bob(), "kp_sqlite_inventory_replacement");
    assert_eq!(
        reopened.key_package_inventory(&bob()).unwrap().available,
        MAX_KEY_PACKAGES_PER_DEVICE
    );
}

#[test]
fn sqlite_revoked_device_status_survives_reopen_and_blocks_key_packages() {
    let mut world = SqliteWorld::direct_room();
    upload_available_key_package(&mut world.server, bob(), "kp_bob_revoked_1");
    world.server.revoke_device(bob()).unwrap();

    let mut reopened = world.reopen();

    assert_eq!(
        device_record(&reopened, &bob()).status,
        DeviceStatus::Revoked
    );
    assert_eq!(
        store_engine_error(
            reopened
                .upload_key_package(UploadKeyPackageRequest {
                    key_package_id: "kp_bob_revoked_2".to_string(),
                    owner: bob(),
                    key_package_ref: "ref_kp_bob_revoked_2".to_string(),
                    key_package_hash: "hash_kp_bob_revoked_2".to_string(),
                    key_package_payload: fake_key_package_payload("kp_bob_revoked_2"),
                })
                .unwrap_err()
        ),
        EngineError::DeviceRevoked(bob())
    );
    assert_eq!(
        store_engine_error(reopened.claim_key_package("kp_bob_revoked_1").unwrap_err()),
        EngineError::DeviceRevoked(bob())
    );
    assert!(
        reopened
            .claim_key_packages_for_account(&bob().account_id)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        key_package(&reopened, "kp_bob_revoked_1").state,
        KeyPackageState::Available
    );
    assert_eq!(
        store_engine_error(reopened.register_device(bob()).unwrap_err()),
        EngineError::DeviceRevoked(bob())
    );
}

#[test]
fn sqlite_claimed_welcome_payload_survives_reopen() {
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

    let mut reopened = world.reopen();
    let claimed = reopened.claim_welcomes(&bob()).unwrap();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].welcome_payload, b"welcome:welcome_bob_1");
    assert_eq!(claimed[0].ratchet_tree_payload, b"tree:welcome_bob_1");

    let second_reopen = world.reopen();
    let stored_welcome = welcome(&second_reopen, "welcome_bob_1");
    assert_eq!(stored_welcome.state, WelcomeState::Claimed);
    assert_eq!(stored_welcome.welcome_payload, b"welcome:welcome_bob_1");
    assert_eq!(stored_welcome.ratchet_tree_payload, b"tree:welcome_bob_1");
}

#[test]
fn sqlite_revoked_device_blocks_welcome_activation_and_sends_after_reopen() {
    let mut pending_world = SqliteWorld::direct_room();
    pending_world.upload_and_claim(bob(), "kp_bob_revoked_welcome");
    let add_pending = pending_world.add_device_request(
        alice(),
        bob(),
        "kp_bob_revoked_welcome",
        "welcome_bob_revoked",
        0,
        "add_bob_revoked_welcome",
    );
    pending_world.server.submit_commit(add_pending).unwrap();
    pending_world.server.revoke_device(bob()).unwrap();
    let mut reopened = pending_world.reopen();
    assert_eq!(
        store_engine_error(reopened.claim_welcomes(&bob()).unwrap_err()),
        EngineError::DeviceRevoked(bob())
    );

    let mut active_world = SqliteWorld::direct_room();
    active_world.upload_and_claim(bob(), "kp_bob_active_revoked");
    let add_active = active_world.add_device_request(
        alice(),
        bob(),
        "kp_bob_active_revoked",
        "welcome_bob_active_revoked",
        0,
        "add_bob_active_revoked",
    );
    active_world.server.submit_commit(add_active).unwrap();
    let claimed = active_world.server.claim_welcomes(&bob()).unwrap();
    assert_eq!(claimed.len(), 1);
    active_world
        .server
        .ack_welcome("welcome_bob_active_revoked", true)
        .unwrap();
    active_world.server.revoke_device(bob()).unwrap();

    let mut reopened = active_world.reopen();
    assert_eq!(
        store_engine_error(
            reopened
                .append_event(active_world.app_message_request(
                    bob(),
                    1,
                    "revoked send",
                    "revoked_send"
                ))
                .unwrap_err()
        ),
        EngineError::DeviceRevoked(bob())
    );
    let remove = active_world.remove_device_request(bob(), alice(), 1, "revoked_commit");
    assert_eq!(
        store_engine_error(reopened.submit_commit(remove).unwrap_err()),
        EngineError::DeviceRevoked(bob())
    );

    let mut claimed_world = SqliteWorld::direct_room();
    claimed_world.upload_and_claim(bob(), "kp_bob_claimed_then_revoked");
    let add_claimed = claimed_world.add_device_request(
        alice(),
        bob(),
        "kp_bob_claimed_then_revoked",
        "welcome_bob_claimed_then_revoked",
        0,
        "add_bob_claimed_then_revoked",
    );
    claimed_world.server.submit_commit(add_claimed).unwrap();
    let claimed = claimed_world.server.claim_welcomes(&bob()).unwrap();
    assert_eq!(claimed.len(), 1);
    claimed_world.server.revoke_device(bob()).unwrap();
    let mut reopened = claimed_world.reopen();
    assert_eq!(
        store_engine_error(
            reopened
                .ack_welcome("welcome_bob_claimed_then_revoked", true)
                .unwrap_err()
        ),
        EngineError::DeviceRevoked(bob())
    );
    assert!(!room(&reopened, &claimed_world.room_id).device_active_at_head(&bob()));
}

#[test]
fn sqlite_operation_fuzz_matches_in_memory_delivery_service() {
    for seed in 1..=32 {
        run_sqlite_operation_fuzz(seed);
    }
}

fn run_sqlite_operation_fuzz(seed: u64) {
    const STEPS: usize = 64;

    let mut world = DualDeliveryWorld::new(seed);
    let bootstrap_key_package = format!("kp_fuzz_bootstrap_bob_{seed}");
    let bootstrap_welcome = format!("welcome_fuzz_bootstrap_bob_{seed}");
    world.upload_key_package(bob(), &bootstrap_key_package);
    world.claim_key_package(&bootstrap_key_package);
    let bootstrap = world.add_request(
        alice(),
        bob(),
        &bootstrap_key_package,
        &bootstrap_welcome,
        0,
        &format!("fuzz_bootstrap_add_bob_{seed}"),
    );
    world.submit_commit(bootstrap);
    world.claim_welcomes(&bob());
    world.ack_welcome(&bootstrap_welcome, true);

    let mut rng = Lcg::new(seed);
    for step in 0..STEPS {
        let device = fuzz_device(rng.next_usize(4));
        let slot = rng.next_usize(4);
        let key_package_id = fuzz_key_package_id(&device, slot);
        let welcome_id = fuzz_welcome_id(&device, slot);
        let idempotency_key = format!("fuzz_{seed}_{step}");
        match rng.next_usize(13) {
            0 => world.register_device(device),
            1 => world.revoke_device(device),
            2 => world.upload_key_package(device, &key_package_id),
            3 => world.claim_key_package(&key_package_id),
            4 => world.claim_key_packages_for_account(&device.account_id),
            5 => world.expire_key_package_lease(&key_package_id),
            6 => {
                let sender = fuzz_device(rng.next_usize(4));
                let expected_epoch = world.maybe_stale_epoch(rng.next_usize(4) == 0);
                let request = world.add_request(
                    sender,
                    device,
                    &key_package_id,
                    &welcome_id,
                    expected_epoch,
                    &format!("{idempotency_key}_add"),
                );
                world.submit_commit(request);
            }
            7 => world.claim_welcomes(&device),
            8 => world.ack_welcome(&welcome_id, rng.next_bool()),
            9 => {
                let epoch = world.maybe_stale_epoch(rng.next_usize(4) == 0);
                let request =
                    world.event_request(device, epoch, &format!("{idempotency_key}_event"));
                world.append_event(request);
            }
            10 => {
                let sender = fuzz_device(rng.next_usize(4));
                let expected_epoch = world.maybe_stale_epoch(rng.next_usize(4) == 0);
                let request = world.remove_request(
                    sender,
                    device,
                    expected_epoch,
                    &format!("{idempotency_key}_remove"),
                );
                world.submit_commit(request);
            }
            11 => world.retry_last_commit(),
            _ => world.retry_last_event(),
        }
    }
}

#[test]
fn sqlite_add_commit_requires_staged_welcome_bytes_before_mutation() {
    let mut world = SqliteWorld::direct_room();
    world.upload_and_claim(bob(), "kp_bob_1");
    let mut request = world.add_device_request(
        alice(),
        bob(),
        "kp_bob_1",
        "welcome_bob_1",
        0,
        "missing_welcome",
    );
    request.staged_welcomes.clear();

    let err = store_engine_error(world.server.submit_commit(request).unwrap_err());

    assert_eq!(
        err,
        EngineError::MissingStagedWelcome("welcome_bob_1".to_string())
    );
    let reopened = world.reopen();
    let room = room(&reopened, &world.room_id);
    assert_eq!(room.current_epoch, 0);
    assert_eq!(room.last_seq, 0);
    assert!(maybe_welcome(&reopened, "welcome_bob_1").is_none());
    assert_eq!(
        key_package(&reopened, "kp_bob_1").state,
        KeyPackageState::Leased
    );
}

#[test]
fn sqlite_commit_crash_matrix_rolls_back_and_retry_converges() {
    for crash_point in CommitCrashPoint::ALL {
        let mut world = SqliteWorld::direct_room();
        world.provision_bob();
        world.upload_and_claim(charlie(), "kp_charlie_1");
        let request = world.charlie_replaces_bob_request();

        install_commit_crash_trigger(&world.db_path, crash_point);
        let crash = world.server.submit_commit(request.clone()).unwrap_err();
        assert!(
            matches!(crash, StoreError::Sqlite(_)),
            "expected sqlite crash at {crash_point:?}, got {crash:?}"
        );
        clear_commit_crash_triggers(&world.db_path);
        assert_crash_commit_rolled_back(&world);

        let mut reopened = world.reopen();
        let accepted = reopened.submit_commit(request.clone()).unwrap();
        let replayed = reopened.submit_commit(request).unwrap();

        assert_eq!(replayed, accepted);
        assert_crash_commit_converged(&reopened, &world, &accepted);
    }
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
    assert_eq!(room(&reopened, &world.room_id).log.len(), 1);
    assert_eq!(
        key_package(&reopened, "kp_charlie_1").state,
        KeyPackageState::Leased
    );
    assert!(maybe_welcome(&reopened, "welcome_charlie_1").is_none());
}

#[test]
fn sqlite_commit_epoch_unique_index_blocks_second_commit_row() {
    let mut world = SqliteWorld::direct_room();
    world.upload_and_claim(bob(), "kp_bob_1");
    let request =
        world.add_device_request(alice(), bob(), "kp_bob_1", "welcome_bob_1", 0, "winner");
    world.server.submit_commit(request).unwrap();

    let conn = Connection::open(&world.db_path).unwrap();
    let sender = alice();
    let err = conn
        .execute(
            r#"
            INSERT INTO room_log_entries (
              room_id, seq, message_id, sender_account_id, sender_device_id,
              kind, epoch, mls_group_id, payload, idempotency_key
            ) VALUES (?1, ?2, ?3, ?4, ?5, 'commit', ?6, ?7, ?8, ?9)
            "#,
            params![
                world.room_id.as_str(),
                2_i64,
                "manual_same_epoch_commit",
                sender.account_id,
                sender.device_id,
                0_i64,
                world.group_id.as_str(),
                b"manual".as_slice(),
                "manual_same_epoch",
            ],
        )
        .unwrap_err();

    assert_sqlite_constraint(err);
    assert_eq!(room(&world.reopen(), "room_direct").log.len(), 1);
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
    let second = world.add_device_request(
        alice(),
        charlie(),
        "kp_bob_1",
        "welcome_charlie_reuse",
        1,
        "second",
    );

    let err = store_engine_error(world.server.submit_commit(second).unwrap_err());

    assert_eq!(
        err,
        EngineError::KeyPackageUnavailable {
            key_package_id: "kp_bob_1".to_string(),
            state: KeyPackageState::Consumed
        }
    );
    assert!(maybe_welcome(&world.server, "welcome_charlie_reuse").is_none());
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

    let err = store_engine_error(
        world
            .server
            .append_event(finitechat_engine::AppendEventRequest {
                room_id: world.room_id.clone(),
                sender: bob(),
                envelope: envelope(
                    world.room_id.clone(),
                    world.group_id.clone(),
                    bob(),
                    2,
                    LogEntryKind::Application,
                    b"stale send".to_vec(),
                ),
                idempotency_key: "msg_stale_bob".to_string(),
            })
            .unwrap_err(),
    );
    assert_eq!(err, EngineError::SenderNotActive(bob()));
    let stale_commit = world.remove_device_request(bob(), alice(), 2, "commit_stale_bob");
    let err = store_engine_error(world.server.submit_commit(stale_commit).unwrap_err());
    assert_eq!(err, EngineError::SenderNotActive(bob()));
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
fn sqlite_account_room_discovery_pages_after_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("finitechat.sqlite3");
    let mut server = SqliteDeliveryStore::open(&path).unwrap();
    server
        .create_room(CreateRoomRequest {
            room_id: "room_account_a".to_string(),
            mls_group_id: "mls_account_a".to_string(),
            creator: alice(),
        })
        .unwrap();
    server
        .create_room(CreateRoomRequest {
            room_id: "room_account_b".to_string(),
            mls_group_id: "mls_account_b".to_string(),
            creator: alice(),
        })
        .unwrap();
    server
        .create_room(CreateRoomRequest {
            room_id: "room_other_account".to_string(),
            mls_group_id: "mls_other_account".to_string(),
            creator: bob(),
        })
        .unwrap();
    let server = SqliteDeliveryStore::open(&path).unwrap();

    let first_page = server
        .list_account_rooms(ListAccountRoomsRequest {
            account_id: alice().account_id,
            after_room_id: None,
            limit: 1,
        })
        .unwrap();
    assert_eq!(first_page.rooms.len(), 1);
    assert_eq!(first_page.rooms[0].room_id, "room_account_a");
    assert_eq!(first_page.rooms[0].devices.len(), 1);
    assert_eq!(first_page.rooms[0].devices[0].device, alice());
    assert!(first_page.rooms[0].devices[0].active);
    assert!(first_page.has_more);

    let second_page = server
        .list_account_rooms(ListAccountRoomsRequest {
            account_id: alice().account_id,
            after_room_id: first_page.next_after_room_id,
            limit: 8,
        })
        .unwrap();
    assert_eq!(second_page.rooms.len(), 1);
    assert_eq!(second_page.rooms[0].room_id, "room_account_b");
    assert!(!second_page.has_more);
    assert_eq!(
        second_page.next_after_room_id.as_deref(),
        Some("room_account_b")
    );
}

#[test]
fn sqlite_duplicate_pending_device_add_is_rejected_before_side_effects() {
    let mut world = SqliteWorld::direct_room();
    world.upload_and_claim(bob(), "kp_bob_1");
    let add_bob =
        world.add_device_request(alice(), bob(), "kp_bob_1", "welcome_bob_1", 0, "add_bob");
    world.server.submit_commit(add_bob).unwrap();
    world.upload_and_claim(bob(), "kp_bob_retry");

    let duplicate = world.add_device_request(
        alice(),
        bob(),
        "kp_bob_retry",
        "welcome_bob_retry",
        1,
        "add_bob_retry",
    );
    let err = store_engine_error(world.server.submit_commit(duplicate).unwrap_err());
    assert_eq!(err, EngineError::DeviceAlreadyInRoom(bob()));
    assert_eq!(
        key_package(&world.server, "kp_bob_retry").state,
        KeyPackageState::Leased
    );
    assert!(world.server.welcome("welcome_bob_retry").unwrap().is_none());
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
            key_package_payload: fake_key_package_payload("kp_charlie"),
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
        staged_welcomes: vec![staged_welcome("welcome_charlie")],
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

#[test]
fn sqlite_idempotency_capacity_rejects_new_mutations_but_allows_replay() {
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
            b"body_0".to_vec(),
        ),
        idempotency_key: "msg_0".to_string(),
    };
    let first_result = world.server.append_event(first.clone()).unwrap();

    for index in 1..MAX_IDEMPOTENCY_RECORDS_PER_ROOM_DEVICE {
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
                    format!("body_{index}").into_bytes(),
                ),
                idempotency_key: format!("msg_{index}"),
            })
            .unwrap();
    }

    let replayed = world.server.append_event(first).unwrap();
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
                    b"body_overflow".to_vec(),
                ),
                idempotency_key: format!("msg_{MAX_IDEMPOTENCY_RECORDS_PER_ROOM_DEVICE}"),
            })
            .unwrap_err(),
    );

    assert_eq!(replayed, first_result);
    assert!(matches!(
        err,
        EngineError::IdempotencyCapacityExceeded { room_id, sender, max_records }
            if room_id == world.room_id
                && sender == alice()
                && max_records == MAX_IDEMPOTENCY_RECORDS_PER_ROOM_DEVICE
    ));
    assert_eq!(
        room(&world.reopen(), &world.room_id).log.len(),
        MAX_IDEMPOTENCY_RECORDS_PER_ROOM_DEVICE as usize
    );
}

#[test]
fn sqlite_chat_receipt_is_durable_but_push_never_after_reopen() {
    let mut world = SqliteWorld::direct_room();
    let request = world.application_event_request(
        alice(),
        0,
        br#"{"type":"chat.receipt","message_id":"m1"}"#,
        "sqlite_receipt_1",
        DurableAppEventKind::ChatReceipt.delivery_policy(),
    );

    let accepted = world.server.append_application_event(request).unwrap();
    let reopened = world.reopen();

    assert_eq!(room(&reopened, &world.room_id).last_seq, 1);
    assert_eq!(reopened.push_outbox_len().unwrap(), 0);
    assert_eq!(reopened.unread_len().unwrap(), 0);
    assert_eq!(reopened.command_inbox_len().unwrap(), 0);
    assert!(
        !reopened
            .application_effect(&accepted.message_id)
            .unwrap()
            .unwrap()
            .creates_push()
    );
}

#[test]
fn sqlite_runtime_state_snapshot_does_not_create_unread_or_inbox_work() {
    let mut world = SqliteWorld::direct_room();
    let request = world.application_event_request(
        alice(),
        0,
        br#"{"type":"runtime.state.snapshot","state_key":"runtime.gateway"}"#,
        "sqlite_state_1",
        DurableAppEventKind::RuntimeStateSnapshot.delivery_policy(),
    );

    world.server.append_application_event(request).unwrap();
    let reopened = world.reopen();

    assert_eq!(room(&reopened, &world.room_id).last_seq, 1);
    assert_eq!(reopened.push_outbox_len().unwrap(), 0);
    assert_eq!(reopened.unread_len().unwrap(), 0);
    assert_eq!(reopened.command_inbox_len().unwrap(), 0);
}

#[test]
fn sqlite_runtime_command_request_creates_command_inbox_work_after_reopen() {
    let mut world = SqliteWorld::direct_room();
    let request = world.application_event_request(
        alice(),
        0,
        br#"{"type":"runtime.command.request","command":"finitecomputer.runtime.gateway.restart"}"#,
        "sqlite_command_1",
        DurableAppEventKind::RuntimeCommandRequest.delivery_policy(),
    );

    let accepted = world.server.append_application_event(request).unwrap();
    let replay = world
        .server
        .append_application_event(world.application_event_request(
        alice(),
        0,
        br#"{"type":"runtime.command.request","command":"finitecomputer.runtime.gateway.restart"}"#,
        "sqlite_command_1",
        DurableAppEventKind::RuntimeCommandRequest.delivery_policy(),
    ));
    assert_eq!(replay.unwrap(), accepted);

    let reopened = world.reopen();
    assert_eq!(reopened.push_outbox_len().unwrap(), 1);
    assert_eq!(reopened.unread_len().unwrap(), 0);
    assert_eq!(reopened.command_inbox_len().unwrap(), 1);
    assert!(
        reopened
            .application_effect(&accepted.message_id)
            .unwrap()
            .unwrap()
            .creates_command_inbox_work()
    );
}

#[test]
fn sqlite_ephemeral_activity_does_not_persist_or_advance_sequence() {
    let mut world = SqliteWorld::direct_room();
    let activity = world.ephemeral_activity_request(alice(), 0, Some("topic_1"), 1_000);

    world.server.append_ephemeral_activity(activity).unwrap();

    assert_eq!(
        world
            .server
            .ephemeral_activity_len_for_route(&world.room_id, Some("topic_1"), &alice()),
        1
    );
    assert_eq!(room(&world.server, &world.room_id).last_seq, 0);
    assert!(
        world
            .server
            .sync_events(&world.room_id, &alice(), 0)
            .unwrap()
            .entries
            .is_empty()
    );
    let reopened = world.reopen();
    assert_eq!(
        reopened.ephemeral_activity_len_for_route(&world.room_id, Some("topic_1"), &alice()),
        0
    );
    assert_eq!(room(&reopened, &world.room_id).last_seq, 0);
}

#[test]
fn sqlite_ephemeral_activity_rejects_pending_and_removed_devices() {
    let mut world = SqliteWorld::direct_room();
    world.upload_and_claim(bob(), "kp_sqlite_ephemeral_bob");
    let add = world.add_device_request(
        alice(),
        bob(),
        "kp_sqlite_ephemeral_bob",
        "welcome_sqlite_ephemeral_bob",
        0,
        "add_sqlite_ephemeral_bob",
    );
    world.server.submit_commit(add).unwrap();
    let pending = world.ephemeral_activity_request(bob(), 1, None, 1_000);
    assert_eq!(
        store_engine_error(world.server.append_ephemeral_activity(pending).unwrap_err()),
        EngineError::SenderNotActive(bob())
    );

    world.server.claim_welcomes(&bob()).unwrap();
    world
        .server
        .ack_welcome("welcome_sqlite_ephemeral_bob", true)
        .unwrap();
    let remove = world.remove_device_request(alice(), bob(), 1, "remove_sqlite_ephemeral_bob");
    world.server.submit_commit(remove).unwrap();
    let removed = world.ephemeral_activity_request(bob(), 2, None, 1_000);
    assert_eq!(
        store_engine_error(world.server.append_ephemeral_activity(removed).unwrap_err()),
        EngineError::SenderNotActive(bob())
    );
}
