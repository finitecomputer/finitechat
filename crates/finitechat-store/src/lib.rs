use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use finitechat_engine::{
    AccountRoomDevice, AccountRoomRecord, AppendEventRequest, ClaimKeyPackageResult,
    CommitAccepted, CreateDirectRoomRequest, CreateRoomRequest, DeviceMembership, DeviceRecord,
    DeviceStatus, EngineError, EventAccepted, KeyPackageRecord, LinkSessionId, LinkSessionRecord,
    LinkSessionState, ListAccountRoomsPage, ListAccountRoomsRequest, MembershipInterval,
    RoomRecord, SubmitCommitRequest, SyncEventsPage, UploadKeyPackageRequest, WelcomeRecord,
    direct_room_key, direct_room_key_string, idempotency_scope_key, lease_token_for, request_hash,
    staged_welcomes_by_id, sync_events_page_for_room,
};
use finitechat_proto::{
    AccountId, DeviceRef, Epoch, FiniteEnvelope, KeyPackageState, LeaseToken, LogEntryKind,
    MAX_ACCOUNT_DEVICES_PER_ROOM, MAX_DIRECT_ROOM_DEVICES_PER_ACCOUNT,
    MAX_IDEMPOTENCY_RECORDS_PER_ROOM_DEVICE, MAX_KEY_PACKAGE_PAYLOAD_BYTES,
    MAX_LINK_SESSION_PAYLOAD_BYTES, MAX_OBJECT_ID_BYTES, MAX_WELCOME_CLAIMS_PER_REQUEST, MessageId,
    MlsGroupId, RoomId, RoomLogEntry, RoomStatus, Seq, StagedWelcomeV1, WelcomeId, WelcomeState,
    validate_bytes_len, validate_bytes_non_empty, validate_room_id, validate_string_bytes,
};
use rusqlite::{Connection, OptionalExtension, Row, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct SqliteDeliveryStore {
    db_path: PathBuf,
}

#[derive(Debug, Error)]
pub enum StoreError {
    #[error(transparent)]
    Engine(#[from] EngineError),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error("corrupt finitechat state: {0}")]
    CorruptState(String),
    #[error("{field} value {value} exceeds sqlite INTEGER range")]
    NumberOutOfRange { field: &'static str, value: u64 },
    #[error("failed to create sqlite store directory {path}: {source}")]
    CreateDir {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl SqliteDeliveryStore {
    pub fn open(db_path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let db_path = db_path.as_ref().to_path_buf();
        if let Some(parent) = db_path.parent() {
            fs::create_dir_all(parent).map_err(|source| StoreError::CreateDir {
                path: parent.to_path_buf(),
                source,
            })?;
        }

        let store = Self { db_path };
        let conn = store.connect()?;
        migrate(&conn)?;
        Ok(store)
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub fn room(&self, room_id: &str) -> Result<Option<RoomRecord>, StoreError> {
        let conn = self.connect()?;
        load_room(&conn, room_id)
    }

    pub fn key_package(
        &self,
        key_package_id: &str,
    ) -> Result<Option<KeyPackageRecord>, StoreError> {
        let conn = self.connect()?;
        load_key_package(&conn, key_package_id)
    }

    pub fn device(&self, device: &DeviceRef) -> Result<Option<DeviceRecord>, StoreError> {
        device.validate_limits().map_err(EngineError::from)?;
        let conn = self.connect()?;
        load_device_record(&conn, device)
    }

    pub fn welcome(&self, welcome_id: &str) -> Result<Option<WelcomeRecord>, StoreError> {
        let conn = self.connect()?;
        load_welcome(&conn, welcome_id)
    }

    pub fn link_session(
        &self,
        link_session_id: &str,
    ) -> Result<Option<LinkSessionRecord>, StoreError> {
        let conn = self.connect()?;
        load_link_session(&conn, link_session_id)
    }

    pub fn create_room(&mut self, request: CreateRoomRequest) -> Result<(), StoreError> {
        request.validate_limits().map_err(EngineError::from)?;
        self.with_transaction(|tx| {
            observe_active_device(tx, &request.creator)?;
            if room_exists(tx, &request.room_id)? {
                return Err(EngineError::RoomAlreadyExists(request.room_id).into());
            }
            insert_room(
                tx,
                &request.room_id,
                &request.mls_group_id,
                RoomStatus::Open,
                &request.creator,
                None,
            )?;
            insert_membership_interval(tx, &request.room_id, &request.creator, 0, None, true)?;
            Ok(())
        })
    }

    pub fn create_or_get_direct_room(
        &mut self,
        request: CreateDirectRoomRequest,
    ) -> Result<RoomId, StoreError> {
        request.validate_limits().map_err(EngineError::from)?;
        self.with_transaction(|tx| {
            observe_active_device(tx, &request.creator)?;
            let account_pair =
                direct_room_key(&request.creator.account_id, &request.other_account_id);
            let direct_key = direct_room_key_string(&account_pair);
            if let Some(room_id) = direct_room_id(tx, &direct_key)? {
                return Ok(room_id);
            }
            if room_exists(tx, &request.room_id)? {
                return Err(EngineError::RoomAlreadyExists(request.room_id).into());
            }

            insert_room(
                tx,
                &request.room_id,
                &request.mls_group_id,
                RoomStatus::Open,
                &request.creator,
                Some(&account_pair),
            )?;
            insert_membership_interval(tx, &request.room_id, &request.creator, 0, None, true)?;
            tx.execute(
                "INSERT INTO direct_rooms (direct_key, room_id) VALUES (?1, ?2)",
                params![direct_key, request.room_id],
            )?;
            Ok(request.room_id)
        })
    }

    pub fn register_device(&mut self, device: DeviceRef) -> Result<(), StoreError> {
        device.validate_limits().map_err(EngineError::from)?;
        self.with_transaction(|tx| observe_active_device(tx, &device))
    }

    pub fn revoke_device(&mut self, device: DeviceRef) -> Result<(), StoreError> {
        device.validate_limits().map_err(EngineError::from)?;
        self.with_transaction(|tx| {
            tx.execute(
                r#"
                INSERT INTO devices (account_id, device_id, status)
                VALUES (?1, ?2, ?3)
                ON CONFLICT(account_id, device_id)
                DO UPDATE SET status = excluded.status
                "#,
                params![
                    device.account_id,
                    device.device_id,
                    encode_device_status(DeviceStatus::Revoked),
                ],
            )?;
            Ok(())
        })
    }

    pub fn upload_key_package(
        &mut self,
        request: UploadKeyPackageRequest,
    ) -> Result<(), StoreError> {
        request.validate_limits().map_err(EngineError::from)?;
        self.with_transaction(|tx| {
            observe_active_device(tx, &request.owner)?;
            if load_key_package(tx, &request.key_package_id)?.is_some() {
                return Err(EngineError::KeyPackageAlreadyExists(request.key_package_id).into());
            }
            tx.execute(
                r#"
                INSERT INTO key_packages (
                  key_package_id, owner_account_id, owner_device_id,
                  key_package_ref, key_package_hash, key_package_payload, state, lease_token
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL)
                "#,
                params![
                    request.key_package_id,
                    request.owner.account_id,
                    request.owner.device_id,
                    request.key_package_ref,
                    request.key_package_hash,
                    request.key_package_payload,
                    encode_key_package_state(KeyPackageState::Available),
                ],
            )?;
            Ok(())
        })
    }

    pub fn claim_key_package(
        &mut self,
        key_package_id: &str,
    ) -> Result<ClaimKeyPackageResult, StoreError> {
        validate_string_bytes("key_package_id", key_package_id, MAX_OBJECT_ID_BYTES)
            .map_err(EngineError::from)?;
        let key_package_id = key_package_id.to_string();
        self.with_transaction(|tx| {
            let package = load_key_package_required(tx, &key_package_id)?;
            claim_available_key_package(tx, package)
        })
    }

    pub fn claim_key_packages_for_account(
        &mut self,
        account_id: &str,
    ) -> Result<Vec<ClaimKeyPackageResult>, StoreError> {
        validate_string_bytes(
            "account_id",
            account_id,
            finitechat_proto::MAX_ACCOUNT_ID_BYTES,
        )
        .map_err(EngineError::from)?;
        let account_id = account_id.to_string();
        self.with_transaction(|tx| {
            let packages = load_available_key_packages_for_account(tx, &account_id)?;
            let mut claimed = Vec::with_capacity(packages.len());
            for package in packages {
                claimed.push(claim_available_key_package(tx, package)?);
            }
            debug_assert!(claimed.len() <= MAX_DIRECT_ROOM_DEVICES_PER_ACCOUNT as usize);
            Ok(claimed)
        })
    }

    pub fn release_key_package_lease(&mut self, key_package_id: &str) -> Result<(), StoreError> {
        release_key_package_lease(self, key_package_id)
    }

    pub fn expire_key_package_lease(&mut self, key_package_id: &str) -> Result<(), StoreError> {
        release_key_package_lease(self, key_package_id)
    }

    pub fn append_event(
        &mut self,
        request: AppendEventRequest,
    ) -> Result<EventAccepted, StoreError> {
        request.validate_limits().map_err(EngineError::from)?;
        if request.envelope.kind == LogEntryKind::Commit {
            return Err(EngineError::WrongEnvelopeKind {
                expected: LogEntryKind::Application,
                actual: request.envelope.kind,
            }
            .into());
        }

        self.with_replayable_engine_result(|tx| {
            let request_hash = request_hash(&request)?;
            let scope_key = idempotency_scope_key(
                &request.room_id,
                &request.sender,
                "append_event",
                &request.idempotency_key,
            );
            if let Some(response) = load_idempotency(tx, &scope_key)? {
                if response.request_hash != request_hash {
                    return Ok(Err(EngineError::ConflictingIdempotencyKey));
                }
                return response.response.into_event_result();
            }

            ensure_idempotency_capacity(tx, &request.room_id, &request.sender)?;
            let result = append_event_inner(tx, &request)?;
            insert_idempotency(
                tx,
                &scope_key,
                &request.room_id,
                &request.sender,
                "append_event",
                &request_hash,
                &PersistedIdempotencyResponse::Event(result.clone()),
            )?;
            Ok(result)
        })
    }

    pub fn submit_commit(
        &mut self,
        request: SubmitCommitRequest,
    ) -> Result<CommitAccepted, StoreError> {
        request.validate_limits().map_err(EngineError::from)?;
        self.with_replayable_engine_result(|tx| {
            let request_hash = request_hash(&request)?;
            let scope_key = idempotency_scope_key(
                &request.room_id,
                &request.sender,
                "submit_commit",
                &request.idempotency_key,
            );
            if let Some(response) = load_idempotency(tx, &scope_key)? {
                if response.request_hash != request_hash {
                    return Ok(Err(EngineError::ConflictingIdempotencyKey));
                }
                return response.response.into_commit_result();
            }

            ensure_idempotency_capacity(tx, &request.room_id, &request.sender)?;
            let result = submit_commit_inner(tx, &request)?;
            insert_idempotency(
                tx,
                &scope_key,
                &request.room_id,
                &request.sender,
                "submit_commit",
                &request_hash,
                &PersistedIdempotencyResponse::Commit(result.clone()),
            )?;
            Ok(result)
        })
    }

    pub fn claim_welcomes(&mut self, device: &DeviceRef) -> Result<Vec<WelcomeRecord>, StoreError> {
        device.validate_limits().map_err(EngineError::from)?;
        let device = device.clone();
        self.with_transaction(|tx| {
            ensure_device_not_revoked(tx, &device)?;
            let mut claimed = Vec::new();
            let mut welcomes = load_released_welcomes_for_device(tx, &device)?;
            for welcome in &mut welcomes {
                tx.execute(
                    "UPDATE welcomes SET state = ?1 WHERE welcome_id = ?2",
                    params![
                        encode_welcome_state(WelcomeState::Claimed),
                        welcome.welcome_id,
                    ],
                )?;
                welcome.state = WelcomeState::Claimed;
                claimed.push(welcome.clone());
            }
            Ok(claimed)
        })
    }

    pub fn ack_welcome(&mut self, welcome_id: &str, activated: bool) -> Result<(), StoreError> {
        let welcome_id = welcome_id.to_string();
        self.with_transaction(|tx| {
            let welcome = load_welcome_required(tx, &welcome_id)?;
            if welcome.state != WelcomeState::Claimed {
                return Err(EngineError::WelcomeNotClaimed(welcome_id).into());
            }

            let next_state = if activated {
                ensure_device_not_revoked(tx, &welcome.recipient)?;
                WelcomeState::Acked
            } else {
                WelcomeState::Failed
            };
            tx.execute(
                "UPDATE welcomes SET state = ?1 WHERE welcome_id = ?2",
                params![encode_welcome_state(next_state), welcome.welcome_id],
            )?;

            if activated {
                let updated = tx.execute(
                    r#"
                    UPDATE room_membership_intervals
                    SET active = 1
                    WHERE room_id = ?1
                      AND account_id = ?2
                      AND device_id = ?3
                      AND start_seq = ?4
                      AND active = 0
                    "#,
                    params![
                        welcome.room_id,
                        welcome.recipient.account_id,
                        welcome.recipient.device_id,
                        to_i64("welcome.commit_seq", welcome.commit_seq)?,
                    ],
                )?;
                if updated != 1 {
                    return Err(StoreError::CorruptState(format!(
                        "welcome {} has no inactive membership interval",
                        welcome.welcome_id
                    )));
                }
            }
            Ok(())
        })
    }

    pub fn release_welcome_claim(&mut self, welcome_id: &str) -> Result<(), StoreError> {
        let welcome_id = welcome_id.to_string();
        self.with_transaction(|tx| {
            let welcome = load_welcome_required(tx, &welcome_id)?;
            if welcome.state != WelcomeState::Claimed {
                return Err(EngineError::WelcomeNotClaimed(welcome_id).into());
            }
            tx.execute(
                "UPDATE welcomes SET state = ?1 WHERE welcome_id = ?2",
                params![
                    encode_welcome_state(WelcomeState::Released),
                    welcome.welcome_id,
                ],
            )?;
            Ok(())
        })
    }

    pub fn report_invalid_commit(
        &mut self,
        room_id: &str,
        reporter: &DeviceRef,
        offending_seq: Seq,
    ) -> Result<(), StoreError> {
        let room_id = room_id.to_string();
        let reporter = reporter.clone();
        self.with_transaction(|tx| {
            let room = load_room(tx, &room_id)?
                .ok_or_else(|| EngineError::RoomNotFound(room_id.clone()))?;
            if !room.device_was_member_for_seq(&reporter, offending_seq) {
                return Err(EngineError::ReporterNotInInterval(reporter).into());
            }
            tx.execute(
                "UPDATE rooms SET status = ?1 WHERE room_id = ?2",
                params![encode_room_status(RoomStatus::NeedsRepair), room.room_id],
            )?;
            Ok(())
        })
    }

    pub fn sync_events(
        &self,
        room_id: &str,
        requester: &DeviceRef,
        after_seq: Seq,
    ) -> Result<SyncEventsPage, StoreError> {
        validate_room_id(room_id).map_err(EngineError::from)?;
        requester.validate_limits().map_err(EngineError::from)?;
        let conn = self.connect()?;
        let room = load_room(&conn, room_id)?
            .ok_or_else(|| EngineError::RoomNotFound(room_id.to_string()))?;
        Ok(sync_events_page_for_room(&room, requester, after_seq))
    }

    pub fn list_account_rooms(
        &self,
        request: ListAccountRoomsRequest,
    ) -> Result<ListAccountRoomsPage, StoreError> {
        request.validate_limits().map_err(EngineError::from)?;
        let conn = self.connect()?;
        let page = load_account_rooms_page(&conn, &request)?;
        page.validate_limits().map_err(EngineError::from)?;
        Ok(page)
    }

    pub fn create_link_session(
        &mut self,
        link_session_id: impl Into<LinkSessionId>,
        pairing_public_key: impl Into<String>,
    ) -> Result<(), StoreError> {
        let link_session_id = link_session_id.into();
        let pairing_public_key = pairing_public_key.into();
        validate_string_bytes("link_session_id", &link_session_id, MAX_OBJECT_ID_BYTES)
            .map_err(EngineError::from)?;
        validate_string_bytes(
            "pairing_public_key",
            &pairing_public_key,
            MAX_OBJECT_ID_BYTES,
        )
        .map_err(EngineError::from)?;
        self.with_transaction(|tx| {
            if load_link_session(tx, &link_session_id)?.is_some() {
                return Err(EngineError::LinkSessionAlreadyExists(link_session_id).into());
            }
            tx.execute(
                r#"
                INSERT INTO link_sessions (
                  link_session_id, pairing_public_key, encrypted_payload, state, claim_token
                ) VALUES (?1, ?2, NULL, ?3, NULL)
                "#,
                params![
                    link_session_id,
                    pairing_public_key,
                    encode_link_session_state(LinkSessionState::Created),
                ],
            )?;
            Ok(())
        })
    }

    pub fn upload_link_payload(
        &mut self,
        link_session_id: &str,
        encrypted_payload: Vec<u8>,
    ) -> Result<(), StoreError> {
        validate_string_bytes("link_session_id", link_session_id, MAX_OBJECT_ID_BYTES)
            .map_err(EngineError::from)?;
        validate_bytes_len(
            "link_session.encrypted_payload",
            encrypted_payload.len(),
            MAX_LINK_SESSION_PAYLOAD_BYTES,
        )
        .map_err(EngineError::from)?;
        let link_session_id = link_session_id.to_string();
        self.with_transaction(|tx| {
            let session = load_link_session_required(tx, &link_session_id)?;
            match session.state {
                LinkSessionState::Created => {
                    update_link_payload(
                        tx,
                        &session.link_session_id,
                        &encrypted_payload,
                        LinkSessionState::PayloadUploaded,
                        None,
                    )?;
                    Ok(())
                }
                LinkSessionState::PayloadUploaded
                    if session.encrypted_payload.as_deref()
                        == Some(encrypted_payload.as_slice()) =>
                {
                    Ok(())
                }
                LinkSessionState::PayloadUploaded => Err(EngineError::LinkSessionConflict.into()),
                LinkSessionState::Claimed
                | LinkSessionState::Delivered
                | LinkSessionState::Expired => Err(EngineError::LinkSessionClosed.into()),
            }
        })
    }

    pub fn claim_link_payload(
        &mut self,
        link_session_id: &str,
    ) -> Result<(Vec<u8>, LeaseToken), StoreError> {
        let link_session_id = link_session_id.to_string();
        self.with_transaction(|tx| {
            let session = load_link_session_required(tx, &link_session_id)?;
            if session.state != LinkSessionState::PayloadUploaded {
                return Err(EngineError::LinkSessionNotReady.into());
            }
            let payload = session
                .encrypted_payload
                .clone()
                .ok_or(EngineError::LinkSessionNotReady)?;
            let token = lease_token_for(
                &session.link_session_id,
                &DeviceRef {
                    account_id: "link".to_string(),
                    device_id: session.pairing_public_key,
                },
            );
            tx.execute(
                "UPDATE link_sessions SET state = ?1, claim_token = ?2 WHERE link_session_id = ?3",
                params![
                    encode_link_session_state(LinkSessionState::Claimed),
                    token,
                    session.link_session_id,
                ],
            )?;
            Ok((payload, token))
        })
    }

    pub fn ack_link_payload(
        &mut self,
        link_session_id: &str,
        claim_token: &str,
    ) -> Result<(), StoreError> {
        let link_session_id = link_session_id.to_string();
        let claim_token = claim_token.to_string();
        self.with_transaction(|tx| {
            let session = load_link_session_required(tx, &link_session_id)?;
            if session.state != LinkSessionState::Claimed {
                return Err(EngineError::LinkSessionNotReady.into());
            }
            if session.claim_token.as_deref() != Some(&claim_token) {
                return Err(EngineError::BadLinkSessionClaimToken.into());
            }
            tx.execute(
                "UPDATE link_sessions SET state = ?1 WHERE link_session_id = ?2",
                params![
                    encode_link_session_state(LinkSessionState::Delivered),
                    session.link_session_id,
                ],
            )?;
            Ok(())
        })
    }

    pub fn release_link_claim(&mut self, link_session_id: &str) -> Result<(), StoreError> {
        let link_session_id = link_session_id.to_string();
        self.with_transaction(|tx| {
            let session = load_link_session_required(tx, &link_session_id)?;
            if session.state != LinkSessionState::Claimed {
                return Err(EngineError::LinkSessionNotReady.into());
            }
            tx.execute(
                "UPDATE link_sessions SET state = ?1, claim_token = NULL WHERE link_session_id = ?2",
                params![
                    encode_link_session_state(LinkSessionState::PayloadUploaded),
                    session.link_session_id,
                ],
            )?;
            Ok(())
        })
    }

    pub fn expire_link_session(&mut self, link_session_id: &str) -> Result<(), StoreError> {
        let link_session_id = link_session_id.to_string();
        self.with_transaction(|tx| {
            let session = load_link_session_required(tx, &link_session_id)?;
            if session.state == LinkSessionState::Delivered {
                return Err(EngineError::LinkSessionClosed.into());
            }
            tx.execute(
                "UPDATE link_sessions SET state = ?1 WHERE link_session_id = ?2",
                params![
                    encode_link_session_state(LinkSessionState::Expired),
                    session.link_session_id,
                ],
            )?;
            Ok(())
        })
    }

    fn connect(&self) -> Result<Connection, StoreError> {
        let conn = Connection::open(&self.db_path)?;
        conn.execute_batch(
            r#"
            PRAGMA journal_mode = WAL;
            PRAGMA synchronous = FULL;
            PRAGMA foreign_keys = ON;
            PRAGMA busy_timeout = 5000;
            "#,
        )?;
        Ok(conn)
    }

    fn with_transaction<T>(
        &mut self,
        f: impl FnOnce(&Transaction<'_>) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let mut conn = self.connect()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let value = f(&tx)?;
        tx.commit()?;
        Ok(value)
    }

    fn with_replayable_engine_result<T>(
        &mut self,
        f: impl FnOnce(&Transaction<'_>) -> Result<Result<T, EngineError>, StoreError>,
    ) -> Result<T, StoreError> {
        let mut conn = self.connect()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let result = f(&tx)?;
        tx.commit()?;
        Ok(result?)
    }
}

#[derive(Debug, Clone)]
struct RoomHeader {
    room_id: RoomId,
    mls_group_id: MlsGroupId,
    current_epoch: Epoch,
    last_seq: Seq,
    status: RoomStatus,
    created_by: DeviceRef,
    direct_accounts: Option<(AccountId, AccountId)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
enum PersistedIdempotencyResponse {
    Event(Result<EventAccepted, EngineError>),
    Commit(Result<CommitAccepted, EngineError>),
}

impl PersistedIdempotencyResponse {
    fn response_kind(&self) -> &'static str {
        match self {
            Self::Event(_) => "event",
            Self::Commit(_) => "commit",
        }
    }

    fn into_event_result(self) -> Result<Result<EventAccepted, EngineError>, StoreError> {
        match self {
            Self::Event(result) => Ok(result),
            Self::Commit(_) => Err(StoreError::CorruptState(
                "commit idempotency response used for event operation".to_string(),
            )),
        }
    }

    fn into_commit_result(self) -> Result<Result<CommitAccepted, EngineError>, StoreError> {
        match self {
            Self::Commit(result) => Ok(result),
            Self::Event(_) => Err(StoreError::CorruptState(
                "event idempotency response used for commit operation".to_string(),
            )),
        }
    }
}

#[derive(Debug, Clone)]
struct IdempotencyRecord {
    request_hash: String,
    response: PersistedIdempotencyResponse,
}

fn migrate(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        r#"
        PRAGMA journal_mode = WAL;
        PRAGMA synchronous = FULL;
        PRAGMA foreign_keys = ON;

        CREATE TABLE IF NOT EXISTS rooms (
          room_id TEXT PRIMARY KEY,
          mls_group_id TEXT NOT NULL,
          current_epoch INTEGER NOT NULL CHECK (current_epoch >= 0),
          last_seq INTEGER NOT NULL CHECK (last_seq >= 0),
          status TEXT NOT NULL CHECK (status IN ('open', 'needs_repair', 'closed')),
          created_account_id TEXT NOT NULL,
          created_device_id TEXT NOT NULL,
          direct_account_left TEXT,
          direct_account_right TEXT,
          CHECK ((direct_account_left IS NULL) = (direct_account_right IS NULL))
        );

        CREATE TABLE IF NOT EXISTS direct_rooms (
          direct_key TEXT PRIMARY KEY,
          room_id TEXT NOT NULL UNIQUE REFERENCES rooms(room_id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS devices (
          account_id TEXT NOT NULL,
          device_id TEXT NOT NULL,
          status TEXT NOT NULL CHECK (status IN ('active', 'revoked')),
          PRIMARY KEY (account_id, device_id)
        );

        CREATE TABLE IF NOT EXISTS room_log_entries (
          room_id TEXT NOT NULL REFERENCES rooms(room_id) ON DELETE CASCADE,
          seq INTEGER NOT NULL CHECK (seq > 0),
          message_id TEXT NOT NULL,
          sender_account_id TEXT NOT NULL,
          sender_device_id TEXT NOT NULL,
          kind TEXT NOT NULL CHECK (kind IN ('application', 'proposal', 'commit')),
          epoch INTEGER NOT NULL CHECK (epoch >= 0),
          mls_group_id TEXT NOT NULL,
          payload BLOB NOT NULL,
          idempotency_key TEXT NOT NULL,
          PRIMARY KEY (room_id, seq),
          UNIQUE (room_id, message_id)
        );

        CREATE UNIQUE INDEX IF NOT EXISTS idx_room_commit_epoch_unique
        ON room_log_entries (room_id, epoch)
        WHERE kind = 'commit';

        CREATE TABLE IF NOT EXISTS room_membership_intervals (
          room_id TEXT NOT NULL REFERENCES rooms(room_id) ON DELETE CASCADE,
          account_id TEXT NOT NULL,
          device_id TEXT NOT NULL,
          start_seq INTEGER NOT NULL CHECK (start_seq >= 0),
          end_seq INTEGER CHECK (end_seq IS NULL OR end_seq >= start_seq),
          active INTEGER NOT NULL CHECK (active IN (0, 1)),
          PRIMARY KEY (room_id, account_id, device_id, start_seq)
        );

        CREATE INDEX IF NOT EXISTS idx_room_membership_account_current_room
        ON room_membership_intervals (account_id, end_seq, room_id, device_id);

        CREATE TABLE IF NOT EXISTS key_packages (
          key_package_id TEXT PRIMARY KEY,
          owner_account_id TEXT NOT NULL,
          owner_device_id TEXT NOT NULL,
          key_package_ref TEXT NOT NULL,
          key_package_hash TEXT NOT NULL,
          key_package_payload BLOB NOT NULL DEFAULT X'',
          state TEXT NOT NULL CHECK (state IN ('available', 'leased', 'consumed', 'released', 'expired')),
          lease_token TEXT
        );

        CREATE INDEX IF NOT EXISTS idx_key_packages_owner_state_device
        ON key_packages (owner_account_id, state, owner_device_id, key_package_id);

        CREATE TABLE IF NOT EXISTS welcomes (
          welcome_id TEXT PRIMARY KEY,
          room_id TEXT NOT NULL REFERENCES rooms(room_id) ON DELETE CASCADE,
          commit_seq INTEGER NOT NULL CHECK (commit_seq > 0),
          recipient_account_id TEXT NOT NULL,
          recipient_device_id TEXT NOT NULL,
          sender_account_id TEXT NOT NULL,
          sender_device_id TEXT NOT NULL,
          key_package_id TEXT NOT NULL,
          join_epoch INTEGER NOT NULL CHECK (join_epoch >= 0),
          state TEXT NOT NULL CHECK (state IN ('staged', 'released', 'claimed', 'acked', 'failed', 'expired', 'cancelled')),
          lease_token TEXT,
          welcome_payload BLOB NOT NULL DEFAULT X'',
          ratchet_tree_payload BLOB NOT NULL DEFAULT X''
        );

        CREATE TABLE IF NOT EXISTS link_sessions (
          link_session_id TEXT PRIMARY KEY,
          pairing_public_key TEXT NOT NULL,
          encrypted_payload BLOB,
          state TEXT NOT NULL CHECK (state IN ('created', 'payload_uploaded', 'claimed', 'delivered', 'expired')),
          claim_token TEXT
        );

        CREATE TABLE IF NOT EXISTS idempotency_records (
          scope_key TEXT PRIMARY KEY,
          room_id TEXT NOT NULL,
          sender_account_id TEXT NOT NULL,
          sender_device_id TEXT NOT NULL,
          operation TEXT NOT NULL CHECK (operation IN ('append_event', 'submit_commit')),
          request_hash TEXT NOT NULL,
          response_kind TEXT NOT NULL CHECK (response_kind IN ('event', 'commit')),
          response_json TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_idempotency_room_sender
        ON idempotency_records (room_id, sender_account_id, sender_device_id);
        "#,
    )?;
    ensure_column(
        conn,
        "key_packages",
        "key_package_payload",
        "BLOB NOT NULL DEFAULT X''",
    )?;
    ensure_column(
        conn,
        "welcomes",
        "welcome_payload",
        "BLOB NOT NULL DEFAULT X''",
    )?;
    ensure_column(
        conn,
        "welcomes",
        "ratchet_tree_payload",
        "BLOB NOT NULL DEFAULT X''",
    )?;
    Ok(())
}

fn ensure_column(
    conn: &Connection,
    table: &'static str,
    column: &'static str,
    definition: &'static str,
) -> Result<(), StoreError> {
    let pragma = format!("PRAGMA table_info({table})");
    let mut statement = conn.prepare(&pragma)?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let existing_column: String = row.get(1)?;
        if existing_column == column {
            return Ok(());
        }
    }

    let alter = format!("ALTER TABLE {table} ADD COLUMN {column} {definition}");
    conn.execute(&alter, [])?;
    Ok(())
}

fn observe_active_device(conn: &Connection, device: &DeviceRef) -> Result<(), StoreError> {
    if let Some(record) = load_device_record(conn, device)? {
        debug_assert_eq!(record.device, *device);
        if record.status == DeviceStatus::Revoked {
            return Err(EngineError::DeviceRevoked(device.clone()).into());
        }
        return Ok(());
    }
    conn.execute(
        "INSERT INTO devices (account_id, device_id, status) VALUES (?1, ?2, ?3)",
        params![
            device.account_id,
            device.device_id,
            encode_device_status(DeviceStatus::Active),
        ],
    )?;
    Ok(())
}

fn ensure_device_not_revoked(conn: &Connection, device: &DeviceRef) -> Result<(), StoreError> {
    if device_is_revoked(conn, device)? {
        Err(EngineError::DeviceRevoked(device.clone()).into())
    } else {
        Ok(())
    }
}

fn device_revocation_error(
    conn: &Connection,
    device: &DeviceRef,
) -> Result<Option<EngineError>, StoreError> {
    if device_is_revoked(conn, device)? {
        Ok(Some(EngineError::DeviceRevoked(device.clone())))
    } else {
        Ok(None)
    }
}

fn device_is_revoked(conn: &Connection, device: &DeviceRef) -> Result<bool, StoreError> {
    Ok(matches!(
        load_device_status(conn, device)?,
        Some(DeviceStatus::Revoked)
    ))
}

fn release_key_package_lease(
    store: &mut SqliteDeliveryStore,
    key_package_id: &str,
) -> Result<(), StoreError> {
    validate_string_bytes("key_package_id", key_package_id, MAX_OBJECT_ID_BYTES)
        .map_err(EngineError::from)?;
    let key_package_id = key_package_id.to_string();
    store.with_transaction(|tx| {
        let package = load_key_package_required(tx, &key_package_id)?;
        if package.state != KeyPackageState::Leased {
            return Err(EngineError::KeyPackageUnavailable {
                key_package_id,
                state: package.state,
            }
            .into());
        }
        tx.execute(
            "UPDATE key_packages SET state = ?1, lease_token = NULL WHERE key_package_id = ?2",
            params![
                encode_key_package_state(KeyPackageState::Available),
                package.key_package_id,
            ],
        )?;
        Ok(())
    })
}

fn claim_available_key_package(
    tx: &Transaction<'_>,
    package: KeyPackageRecord,
) -> Result<ClaimKeyPackageResult, StoreError> {
    if package.state != KeyPackageState::Available {
        return Err(EngineError::KeyPackageUnavailable {
            key_package_id: package.key_package_id,
            state: package.state,
        }
        .into());
    }
    ensure_device_not_revoked(tx, &package.owner)?;
    validate_key_package_payload(&package.key_package_payload)?;

    let lease_token = lease_token_for(&package.key_package_id, &package.owner);
    let updated = tx.execute(
        "UPDATE key_packages SET state = ?1, lease_token = ?2 WHERE key_package_id = ?3",
        params![
            encode_key_package_state(KeyPackageState::Leased),
            lease_token.as_str(),
            package.key_package_id.as_str(),
        ],
    )?;
    if updated != 1 {
        return Err(StoreError::CorruptState(format!(
            "KeyPackage {} vanished during claim",
            package.key_package_id
        )));
    }
    Ok(claimed_key_package_result(package, lease_token))
}

fn validate_key_package_payload(payload: &[u8]) -> Result<(), StoreError> {
    validate_bytes_non_empty("key_package_payload", payload.len()).map_err(EngineError::from)?;
    validate_bytes_len(
        "key_package_payload",
        payload.len(),
        MAX_KEY_PACKAGE_PAYLOAD_BYTES,
    )
    .map_err(EngineError::from)?;
    Ok(())
}

fn claimed_key_package_result(
    package: KeyPackageRecord,
    lease_token: LeaseToken,
) -> ClaimKeyPackageResult {
    ClaimKeyPackageResult {
        key_package_id: package.key_package_id,
        owner: package.owner,
        key_package_ref: package.key_package_ref,
        key_package_hash: package.key_package_hash,
        key_package_payload: package.key_package_payload,
        lease_token,
    }
}

fn append_event_inner(
    tx: &Transaction<'_>,
    request: &AppendEventRequest,
) -> Result<Result<EventAccepted, EngineError>, StoreError> {
    if let Some(error) = device_revocation_error(tx, &request.sender)? {
        return Ok(Err(error));
    }
    let room = match load_room_state(tx, &request.room_id)? {
        Some(room) => room,
        None => return Ok(Err(EngineError::RoomNotFound(request.room_id.clone()))),
    };
    if let Some(error) = validate_room_for_event(&room, request) {
        return Ok(Err(error));
    }

    let message_id = request.envelope.message_id()?;
    if message_id_exists(tx, &request.room_id, &message_id)? {
        return Ok(Err(EngineError::DuplicateMessageId(message_id)));
    }
    let seq = room.last_seq + 1;
    insert_log_entry(
        tx,
        seq,
        &message_id,
        &request.envelope,
        &request.idempotency_key,
    )?;
    update_room_head(
        tx,
        &request.room_id,
        room.current_epoch,
        room.last_seq,
        room.current_epoch,
        seq,
    )?;
    Ok(Ok(EventAccepted { seq, message_id }))
}

fn submit_commit_inner(
    tx: &Transaction<'_>,
    request: &SubmitCommitRequest,
) -> Result<Result<CommitAccepted, EngineError>, StoreError> {
    if let Some(error) = device_revocation_error(tx, &request.sender)? {
        return Ok(Err(error));
    }
    let actual_commit_message_id = request.envelope.message_id()?;
    if let Err(error) = request
        .membership_delta
        .validate_structure(request.expected_epoch, &actual_commit_message_id)
    {
        return Ok(Err(error.into()));
    }
    let staged_welcomes =
        match staged_welcomes_by_id(&request.membership_delta, &request.staged_welcomes) {
            Ok(staged_welcomes) => staged_welcomes,
            Err(error) => return Ok(Err(error)),
        };

    let room = match load_room_state(tx, &request.room_id)? {
        Some(room) => room,
        None => return Ok(Err(EngineError::RoomNotFound(request.room_id.clone()))),
    };
    if let Some(error) = validate_room_for_commit(&room, request) {
        return Ok(Err(error));
    }
    if let Err(error) = validate_commit_key_packages(tx, &room, request) {
        return match error {
            StoreError::Engine(engine) => Ok(Err(engine)),
            other => Err(other),
        };
    }
    if let Err(error) = validate_commit_welcomes(tx, &request.membership_delta)? {
        return Ok(Err(error));
    }
    if message_id_exists(tx, &request.room_id, &actual_commit_message_id)? {
        return Ok(Err(EngineError::DuplicateMessageId(
            actual_commit_message_id,
        )));
    }

    let seq = room.last_seq + 1;
    let next_epoch = room.current_epoch + 1;
    insert_log_entry(
        tx,
        seq,
        &actual_commit_message_id,
        &request.envelope,
        &request.idempotency_key,
    )?;
    update_room_head(
        tx,
        &request.room_id,
        room.current_epoch,
        room.last_seq,
        next_epoch,
        seq,
    )?;
    apply_membership_delta(
        tx,
        &request.room_id,
        seq,
        next_epoch,
        request,
        &staged_welcomes,
    )?;

    let released_welcomes = request
        .membership_delta
        .adds
        .iter()
        .map(|add| add.welcome_id.clone())
        .collect();
    Ok(Ok(CommitAccepted {
        seq,
        message_id: actual_commit_message_id,
        released_welcomes,
    }))
}

fn validate_room_for_event(room: &RoomRecord, request: &AppendEventRequest) -> Option<EngineError> {
    if let Some(error) = validate_room_open(room) {
        return Some(error);
    }
    if let Some(error) = validate_envelope(room, &request.envelope, request.envelope.kind) {
        return Some(error);
    }
    if request.envelope.epoch != room.current_epoch {
        return Some(EngineError::WrongEpoch {
            expected: room.current_epoch,
            actual: request.envelope.epoch,
        });
    }
    if request.envelope.sender != request.sender {
        return Some(EngineError::EnvelopeSenderMismatch);
    }
    if !room.device_active_at_head(&request.sender) {
        return Some(EngineError::SenderNotActive(request.sender.clone()));
    }
    None
}

fn validate_room_for_commit(
    room: &RoomRecord,
    request: &SubmitCommitRequest,
) -> Option<EngineError> {
    if let Some(error) = validate_room_open(room) {
        return Some(error);
    }
    if let Some(error) = validate_envelope(room, &request.envelope, LogEntryKind::Commit) {
        return Some(error);
    }
    if request.expected_epoch != room.current_epoch {
        return Some(EngineError::WrongEpoch {
            expected: room.current_epoch,
            actual: request.expected_epoch,
        });
    }
    if request.envelope.epoch != request.expected_epoch {
        return Some(EngineError::WrongEpoch {
            expected: request.expected_epoch,
            actual: request.envelope.epoch,
        });
    }
    if request.envelope.sender != request.sender {
        return Some(EngineError::EnvelopeSenderMismatch);
    }
    if !room.device_active_at_head(&request.sender) {
        return Some(EngineError::SenderNotActive(request.sender.clone()));
    }
    None
}

fn validate_room_open(room: &RoomRecord) -> Option<EngineError> {
    if room.status == RoomStatus::Open {
        None
    } else {
        Some(EngineError::RoomNotOpen)
    }
}

fn validate_envelope(
    room: &RoomRecord,
    envelope: &FiniteEnvelope,
    expected_kind: LogEntryKind,
) -> Option<EngineError> {
    if envelope.kind != expected_kind {
        return Some(EngineError::WrongEnvelopeKind {
            expected: expected_kind,
            actual: envelope.kind,
        });
    }
    if envelope.room_id != room.room_id {
        return Some(EngineError::EnvelopeRoomMismatch);
    }
    if envelope.mls_group_id != room.mls_group_id {
        return Some(EngineError::EnvelopeGroupMismatch);
    }
    None
}

fn validate_commit_key_packages(
    tx: &Transaction<'_>,
    room: &RoomRecord,
    request: &SubmitCommitRequest,
) -> Result<(), StoreError> {
    let mut seen_packages = std::collections::BTreeSet::new();
    let mut added_devices_by_account = std::collections::BTreeMap::<AccountId, usize>::new();
    for add in &request.membership_delta.adds {
        ensure_device_not_revoked(tx, &add.device)?;
        if let Some((left, right)) = &room.direct_accounts
            && add.device.account_id != *left
            && add.device.account_id != *right
        {
            return Err(EngineError::DirectRoomThirdAccount(add.device.account_id.clone()).into());
        }
        let current_devices =
            room.current_or_pending_device_count_for_account(&add.device.account_id);
        let added_devices = added_devices_by_account
            .entry(add.device.account_id.clone())
            .or_insert(0);
        *added_devices += 1;
        finitechat_proto::validate_item_count(
            "room.devices_per_account",
            current_devices + *added_devices,
            MAX_ACCOUNT_DEVICES_PER_ROOM,
        )
        .map_err(EngineError::from)?;
        if room.direct_accounts.is_some() {
            finitechat_proto::validate_item_count(
                "direct_room.devices_per_account",
                current_devices + *added_devices,
                MAX_DIRECT_ROOM_DEVICES_PER_ACCOUNT,
            )
            .map_err(EngineError::from)?;
        }
        if room.device_current_or_pending_at_head(&add.device) {
            return Err(EngineError::DeviceAlreadyInRoom(add.device.clone()).into());
        }
        if !seen_packages.insert(add.key_package_id.clone()) {
            return Err(EngineError::DuplicateKeyPackage(add.key_package_id.clone()).into());
        }
        let package = load_key_package(tx, &add.key_package_id)?
            .ok_or_else(|| EngineError::KeyPackageNotFound(add.key_package_id.clone()))?;
        if package.state != KeyPackageState::Leased {
            return Err(EngineError::KeyPackageUnavailable {
                key_package_id: add.key_package_id.clone(),
                state: package.state,
            }
            .into());
        }
        if package.owner != add.device {
            return Err(EngineError::KeyPackageOwnerMismatch(add.key_package_id.clone()).into());
        }
        if package.key_package_ref != add.key_package_ref
            || package.key_package_hash != add.key_package_hash
        {
            return Err(EngineError::KeyPackageRefMismatch(add.key_package_id.clone()).into());
        }
    }
    Ok(())
}

fn validate_commit_welcomes(
    tx: &Transaction<'_>,
    delta: &finitechat_proto::MembershipDeltaV1,
) -> Result<Result<(), EngineError>, StoreError> {
    for add in &delta.adds {
        if load_welcome(tx, &add.welcome_id)?.is_some() {
            return Ok(Err(EngineError::WelcomeAlreadyExists(
                add.welcome_id.clone(),
            )));
        }
    }
    Ok(Ok(()))
}

fn apply_membership_delta(
    tx: &Transaction<'_>,
    room_id: &str,
    seq: Seq,
    next_epoch: Epoch,
    request: &SubmitCommitRequest,
    staged_welcomes: &BTreeMap<WelcomeId, &StagedWelcomeV1>,
) -> Result<(), StoreError> {
    for remove in &request.membership_delta.removes {
        tx.execute(
            r#"
            UPDATE room_membership_intervals
            SET end_seq = ?1
            WHERE room_id = ?2
              AND account_id = ?3
              AND device_id = ?4
              AND active = 1
              AND end_seq IS NULL
            "#,
            params![
                to_i64("seq", seq)?,
                room_id,
                remove.device.account_id,
                remove.device.device_id,
            ],
        )?;
    }

    for add in &request.membership_delta.adds {
        insert_membership_interval(tx, room_id, &add.device, seq, None, false)?;
        tx.execute(
            "UPDATE key_packages SET state = ?1, lease_token = NULL WHERE key_package_id = ?2",
            params![
                encode_key_package_state(KeyPackageState::Consumed),
                add.key_package_id,
            ],
        )?;
        let lease_token = lease_token_for(&add.welcome_id, &add.device);
        let staged_welcome = staged_welcomes
            .get(&add.welcome_id)
            .expect("staged welcome was validated");
        tx.execute(
            r#"
            INSERT INTO welcomes (
              welcome_id, room_id, commit_seq,
              recipient_account_id, recipient_device_id,
              sender_account_id, sender_device_id,
              key_package_id, join_epoch, state, lease_token,
              welcome_payload, ratchet_tree_payload
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
            "#,
            params![
                add.welcome_id,
                room_id,
                to_i64("seq", seq)?,
                add.device.account_id,
                add.device.device_id,
                request.sender.account_id,
                request.sender.device_id,
                add.key_package_id,
                to_i64("next_epoch", next_epoch)?,
                encode_welcome_state(WelcomeState::Released),
                lease_token,
                &staged_welcome.welcome_payload,
                &staged_welcome.ratchet_tree_payload,
            ],
        )?;
    }
    Ok(())
}

fn insert_room(
    tx: &Transaction<'_>,
    room_id: &str,
    mls_group_id: &str,
    status: RoomStatus,
    created_by: &DeviceRef,
    direct_accounts: Option<&(AccountId, AccountId)>,
) -> Result<(), StoreError> {
    let (direct_account_left, direct_account_right) = match direct_accounts {
        Some((left, right)) => (Some(left.as_str()), Some(right.as_str())),
        None => (None, None),
    };
    tx.execute(
        r#"
        INSERT INTO rooms (
          room_id, mls_group_id, current_epoch, last_seq, status,
          created_account_id, created_device_id, direct_account_left, direct_account_right
        ) VALUES (?1, ?2, 0, 0, ?3, ?4, ?5, ?6, ?7)
        "#,
        params![
            room_id,
            mls_group_id,
            encode_room_status(status),
            created_by.account_id,
            created_by.device_id,
            direct_account_left,
            direct_account_right,
        ],
    )?;
    Ok(())
}

fn insert_membership_interval(
    tx: &Transaction<'_>,
    room_id: &str,
    device: &DeviceRef,
    start_seq: Seq,
    end_seq: Option<Seq>,
    active: bool,
) -> Result<(), StoreError> {
    tx.execute(
        r#"
        INSERT INTO room_membership_intervals (
          room_id, account_id, device_id, start_seq, end_seq, active
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
        "#,
        params![
            room_id,
            device.account_id,
            device.device_id,
            to_i64("start_seq", start_seq)?,
            optional_i64("end_seq", end_seq)?,
            bool_to_i64(active),
        ],
    )?;
    Ok(())
}

fn insert_log_entry(
    tx: &Transaction<'_>,
    seq: Seq,
    message_id: &MessageId,
    envelope: &FiniteEnvelope,
    idempotency_key: &str,
) -> Result<(), StoreError> {
    tx.execute(
        r#"
        INSERT INTO room_log_entries (
          room_id, seq, message_id, sender_account_id, sender_device_id,
          kind, epoch, mls_group_id, payload, idempotency_key
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
        "#,
        params![
            envelope.room_id,
            to_i64("seq", seq)?,
            message_id,
            envelope.sender.account_id,
            envelope.sender.device_id,
            encode_log_entry_kind(envelope.kind),
            to_i64("epoch", envelope.epoch)?,
            envelope.mls_group_id,
            envelope.payload,
            idempotency_key,
        ],
    )?;
    Ok(())
}

fn update_room_head(
    tx: &Transaction<'_>,
    room_id: &str,
    expected_epoch: Epoch,
    expected_seq: Seq,
    next_epoch: Epoch,
    next_seq: Seq,
) -> Result<(), StoreError> {
    let updated = tx.execute(
        r#"
        UPDATE rooms
        SET current_epoch = ?1, last_seq = ?2
        WHERE room_id = ?3
          AND current_epoch = ?4
          AND last_seq = ?5
        "#,
        params![
            to_i64("current_epoch", next_epoch)?,
            to_i64("last_seq", next_seq)?,
            room_id,
            to_i64("expected_epoch", expected_epoch)?,
            to_i64("expected_seq", expected_seq)?,
        ],
    )?;
    if updated == 1 {
        Ok(())
    } else {
        Err(StoreError::CorruptState(format!(
            "room {room_id} head changed during transaction"
        )))
    }
}

fn update_link_payload(
    tx: &Transaction<'_>,
    link_session_id: &str,
    encrypted_payload: &[u8],
    state: LinkSessionState,
    claim_token: Option<&str>,
) -> Result<(), StoreError> {
    tx.execute(
        r#"
        UPDATE link_sessions
        SET encrypted_payload = ?1, state = ?2, claim_token = ?3
        WHERE link_session_id = ?4
        "#,
        params![
            encrypted_payload,
            encode_link_session_state(state),
            claim_token,
            link_session_id,
        ],
    )?;
    Ok(())
}

fn insert_idempotency(
    tx: &Transaction<'_>,
    scope_key: &str,
    room_id: &str,
    sender: &DeviceRef,
    operation: &str,
    request_hash: &str,
    response: &PersistedIdempotencyResponse,
) -> Result<(), StoreError> {
    let response_json = serde_json::to_string(response)?;
    tx.execute(
        r#"
        INSERT INTO idempotency_records (
          scope_key, room_id, sender_account_id, sender_device_id, operation,
          request_hash, response_kind, response_json
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
        "#,
        params![
            scope_key,
            room_id,
            sender.account_id,
            sender.device_id,
            operation,
            request_hash,
            response.response_kind(),
            response_json,
        ],
    )?;
    Ok(())
}

fn ensure_idempotency_capacity(
    conn: &Connection,
    room_id: &str,
    sender: &DeviceRef,
) -> Result<(), StoreError> {
    let count = conn.query_row(
        r#"
            SELECT COUNT(*)
            FROM idempotency_records
            WHERE room_id = ?1
              AND sender_account_id = ?2
              AND sender_device_id = ?3
            "#,
        params![room_id, sender.account_id, sender.device_id],
        |row| row.get::<_, i64>(0),
    )?;
    if count < i64::from(MAX_IDEMPOTENCY_RECORDS_PER_ROOM_DEVICE) {
        Ok(())
    } else {
        Err(EngineError::IdempotencyCapacityExceeded {
            room_id: room_id.to_string(),
            sender: sender.clone(),
            max_records: MAX_IDEMPOTENCY_RECORDS_PER_ROOM_DEVICE,
        }
        .into())
    }
}

fn room_exists(conn: &Connection, room_id: &str) -> Result<bool, StoreError> {
    let exists = conn
        .query_row(
            "SELECT 1 FROM rooms WHERE room_id = ?1",
            params![room_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .is_some();
    Ok(exists)
}

fn direct_room_id(conn: &Connection, direct_key: &str) -> Result<Option<RoomId>, StoreError> {
    let room_id = conn
        .query_row(
            "SELECT room_id FROM direct_rooms WHERE direct_key = ?1",
            params![direct_key],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    Ok(room_id)
}

fn message_id_exists(
    conn: &Connection,
    room_id: &str,
    message_id: &str,
) -> Result<bool, StoreError> {
    let exists = conn
        .query_row(
            "SELECT 1 FROM room_log_entries WHERE room_id = ?1 AND message_id = ?2",
            params![room_id, message_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()?
        .is_some();
    Ok(exists)
}

fn load_room(conn: &Connection, room_id: &str) -> Result<Option<RoomRecord>, StoreError> {
    let Some(header) = load_room_header(conn, room_id)? else {
        return Ok(None);
    };
    let log = load_room_log(conn, room_id)?;
    let membership = load_room_membership(conn, room_id)?;
    let room = RoomRecord {
        room_id: header.room_id,
        mls_group_id: header.mls_group_id,
        current_epoch: header.current_epoch,
        last_seq: header.last_seq,
        status: header.status,
        created_by: header.created_by,
        log,
        membership,
        direct_accounts: header.direct_accounts,
    };
    validate_room_shape(&room)?;
    Ok(Some(room))
}

fn load_room_state(conn: &Connection, room_id: &str) -> Result<Option<RoomRecord>, StoreError> {
    let Some(header) = load_room_header(conn, room_id)? else {
        return Ok(None);
    };
    let membership = load_room_membership(conn, room_id)?;
    let room = RoomRecord {
        room_id: header.room_id,
        mls_group_id: header.mls_group_id,
        current_epoch: header.current_epoch,
        last_seq: header.last_seq,
        status: header.status,
        created_by: header.created_by,
        log: Vec::new(),
        membership,
        direct_accounts: header.direct_accounts,
    };
    validate_membership_shape(&room)?;
    Ok(Some(room))
}

fn load_room_header(conn: &Connection, room_id: &str) -> Result<Option<RoomHeader>, StoreError> {
    let mut statement = conn.prepare(
        r#"
        SELECT room_id, mls_group_id, current_epoch, last_seq, status,
               created_account_id, created_device_id, direct_account_left, direct_account_right
        FROM rooms
        WHERE room_id = ?1
        "#,
    )?;
    let mut rows = statement.query(params![room_id])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };

    let current_epoch = from_i64("current_epoch", row.get(2)?)?;
    let last_seq = from_i64("last_seq", row.get(3)?)?;
    let status = decode_room_status(row.get::<_, String>(4)?.as_str())?;
    let direct_account_left: Option<String> = row.get(7)?;
    let direct_account_right: Option<String> = row.get(8)?;
    let direct_accounts = match (direct_account_left, direct_account_right) {
        (Some(left), Some(right)) => Some((left, right)),
        (None, None) => None,
        _ => {
            return Err(StoreError::CorruptState(format!(
                "room {room_id} has partial direct account pair"
            )));
        }
    };
    Ok(Some(RoomHeader {
        room_id: row.get(0)?,
        mls_group_id: row.get(1)?,
        current_epoch,
        last_seq,
        status,
        created_by: DeviceRef {
            account_id: row.get(5)?,
            device_id: row.get(6)?,
        },
        direct_accounts,
    }))
}

fn load_room_log(conn: &Connection, room_id: &str) -> Result<Vec<RoomLogEntry>, StoreError> {
    let mut statement = conn.prepare(
        r#"
        SELECT seq, message_id, sender_account_id, sender_device_id, kind,
               epoch, mls_group_id, payload, idempotency_key
        FROM room_log_entries
        WHERE room_id = ?1
        ORDER BY seq
        "#,
    )?;
    let mut rows = statement.query(params![room_id])?;
    let mut entries = Vec::new();
    while let Some(row) = rows.next()? {
        let seq = from_i64("seq", row.get(0)?)?;
        let sender = DeviceRef {
            account_id: row.get(2)?,
            device_id: row.get(3)?,
        };
        let kind = decode_log_entry_kind(row.get::<_, String>(4)?.as_str())?;
        let epoch = from_i64("epoch", row.get(5)?)?;
        let mls_group_id = row.get::<_, String>(6)?;
        let payload = row.get::<_, Vec<u8>>(7)?;
        entries.push(RoomLogEntry {
            room_id: room_id.to_string(),
            seq,
            message_id: row.get(1)?,
            sender: sender.clone(),
            kind,
            epoch,
            envelope: FiniteEnvelope {
                room_id: room_id.to_string(),
                mls_group_id,
                epoch,
                sender,
                kind,
                payload,
            },
            idempotency_key: row.get(8)?,
        });
    }
    Ok(entries)
}

fn load_room_membership(
    conn: &Connection,
    room_id: &str,
) -> Result<std::collections::BTreeMap<String, DeviceMembership>, StoreError> {
    let mut statement = conn.prepare(
        r#"
        SELECT account_id, device_id, start_seq, end_seq, active
        FROM room_membership_intervals
        WHERE room_id = ?1
        ORDER BY account_id, device_id, start_seq
        "#,
    )?;
    let mut rows = statement.query(params![room_id])?;
    let mut membership = std::collections::BTreeMap::new();
    while let Some(row) = rows.next()? {
        let device = DeviceRef {
            account_id: row.get(0)?,
            device_id: row.get(1)?,
        };
        let key = DeviceMembership::key(&device);
        let start_seq = from_i64("start_seq", row.get(2)?)?;
        let end_seq = optional_u64("end_seq", row.get(3)?)?;
        let active = bool_from_i64("active", row.get(4)?)?;
        membership
            .entry(key)
            .or_insert_with(|| DeviceMembership {
                device: device.clone(),
                intervals: Vec::new(),
            })
            .intervals
            .push(MembershipInterval {
                start_seq,
                end_seq,
                active,
            });
    }
    Ok(membership)
}

fn load_account_rooms_page(
    conn: &Connection,
    request: &ListAccountRoomsRequest,
) -> Result<ListAccountRoomsPage, StoreError> {
    let sql_limit = i64::from(request.limit) + 1;
    let mut statement = conn.prepare(
        r#"
        SELECT DISTINCT r.room_id, r.mls_group_id, r.current_epoch, r.last_seq, r.status
        FROM rooms r
        INNER JOIN room_membership_intervals m ON m.room_id = r.room_id
        WHERE m.account_id = ?1
          AND m.end_seq IS NULL
          AND (?2 IS NULL OR r.room_id > ?2)
        ORDER BY r.room_id
        LIMIT ?3
        "#,
    )?;
    let mut rows = statement.query(params![
        request.account_id,
        request.after_room_id,
        sql_limit,
    ])?;
    let mut rooms = Vec::with_capacity(request.limit as usize);
    while let Some(row) = rows.next()? {
        if rooms.len() == request.limit as usize {
            let next_after_room_id = rooms
                .last()
                .map(|room: &AccountRoomRecord| room.room_id.clone());
            let page = ListAccountRoomsPage {
                rooms,
                next_after_room_id,
                has_more: true,
            };
            page.validate_limits().map_err(EngineError::from)?;
            return Ok(page);
        }

        let room_id = row.get::<_, String>(0)?;
        let devices = load_current_account_room_devices(conn, &room_id, &request.account_id)?;
        if devices.is_empty() {
            return Err(StoreError::CorruptState(format!(
                "room {room_id} discovered without current account devices"
            )));
        }
        rooms.push(AccountRoomRecord {
            room_id,
            mls_group_id: row.get(1)?,
            current_epoch: from_i64("current_epoch", row.get(2)?)?,
            last_seq: from_i64("last_seq", row.get(3)?)?,
            status: decode_room_status(row.get::<_, String>(4)?.as_str())?,
            devices,
        });
    }
    let next_after_room_id = rooms.last().map(|room| room.room_id.clone());
    let page = ListAccountRoomsPage {
        rooms,
        next_after_room_id,
        has_more: false,
    };
    page.validate_limits().map_err(EngineError::from)?;
    Ok(page)
}

fn load_current_account_room_devices(
    conn: &Connection,
    room_id: &str,
    account_id: &str,
) -> Result<Vec<AccountRoomDevice>, StoreError> {
    let mut statement = conn.prepare(
        r#"
        SELECT device_id, MAX(active)
        FROM room_membership_intervals
        WHERE room_id = ?1
          AND account_id = ?2
          AND end_seq IS NULL
        GROUP BY device_id
        ORDER BY device_id
        LIMIT ?3
        "#,
    )?;
    let mut rows = statement.query(params![
        room_id,
        account_id,
        i64::from(MAX_ACCOUNT_DEVICES_PER_ROOM) + 1,
    ])?;
    let mut devices = Vec::new();
    while let Some(row) = rows.next()? {
        if devices.len() >= MAX_ACCOUNT_DEVICES_PER_ROOM as usize {
            return Err(StoreError::CorruptState(format!(
                "room {room_id} has too many current devices for account {account_id}"
            )));
        }
        devices.push(AccountRoomDevice {
            device: DeviceRef {
                account_id: account_id.to_string(),
                device_id: row.get(0)?,
            },
            active: bool_from_i64("active", row.get(1)?)?,
        });
    }
    debug_assert!(devices.len() <= MAX_ACCOUNT_DEVICES_PER_ROOM as usize);
    Ok(devices)
}

fn validate_room_shape(room: &RoomRecord) -> Result<(), StoreError> {
    let mut next_seq = 1;
    for entry in &room.log {
        if entry.room_id != room.room_id {
            return Err(StoreError::CorruptState(format!(
                "log entry {} belongs to wrong room",
                entry.seq
            )));
        }
        if entry.seq != next_seq {
            return Err(StoreError::CorruptState(format!(
                "room {} log has seq gap at {}",
                room.room_id, next_seq
            )));
        }
        next_seq += 1;
    }
    if room.last_seq + 1 != next_seq {
        return Err(StoreError::CorruptState(format!(
            "room {} last_seq does not match log",
            room.room_id
        )));
    }
    validate_membership_shape(room)
}

fn validate_membership_shape(room: &RoomRecord) -> Result<(), StoreError> {
    for (key, membership) in &room.membership {
        if *key != DeviceMembership::key(&membership.device) {
            return Err(StoreError::CorruptState(format!(
                "room {} has inconsistent membership key",
                room.room_id
            )));
        }
    }
    Ok(())
}

fn load_device_record(
    conn: &Connection,
    device: &DeviceRef,
) -> Result<Option<DeviceRecord>, StoreError> {
    let status = load_device_status(conn, device)?;
    Ok(status.map(|status| DeviceRecord {
        device: device.clone(),
        status,
    }))
}

fn load_device_status(
    conn: &Connection,
    device: &DeviceRef,
) -> Result<Option<DeviceStatus>, StoreError> {
    let status = conn
        .query_row(
            r#"
            SELECT status
            FROM devices
            WHERE account_id = ?1
              AND device_id = ?2
            "#,
            params![device.account_id, device.device_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    status
        .map(|status| decode_device_status(&status))
        .transpose()
}

fn load_key_package(
    conn: &Connection,
    key_package_id: &str,
) -> Result<Option<KeyPackageRecord>, StoreError> {
    let mut statement = conn.prepare(
        r#"
        SELECT key_package_id, owner_account_id, owner_device_id,
               key_package_ref, key_package_hash, key_package_payload, state, lease_token
        FROM key_packages
        WHERE key_package_id = ?1
        "#,
    )?;
    let mut rows = statement.query(params![key_package_id])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    Ok(Some(row_to_key_package(row)?))
}

fn load_available_key_packages_for_account(
    conn: &Connection,
    account_id: &str,
) -> Result<Vec<KeyPackageRecord>, StoreError> {
    let mut statement = conn.prepare(
        r#"
        SELECT key_package_id, owner_account_id, owner_device_id,
               key_package_ref, key_package_hash, key_package_payload, state, lease_token
        FROM key_packages
        WHERE owner_account_id = ?1
          AND state = ?2
          AND NOT EXISTS (
            SELECT 1
            FROM devices
            WHERE devices.account_id = key_packages.owner_account_id
              AND devices.device_id = key_packages.owner_device_id
              AND devices.status = 'revoked'
          )
        ORDER BY owner_device_id, key_package_id
        "#,
    )?;
    let mut rows = statement.query(params![
        account_id,
        encode_key_package_state(KeyPackageState::Available),
    ])?;
    let mut packages = Vec::new();
    let mut seen_devices = BTreeSet::<String>::new();
    while let Some(row) = rows.next()? {
        if packages.len() >= MAX_DIRECT_ROOM_DEVICES_PER_ACCOUNT as usize {
            break;
        }
        let package = row_to_key_package(row)?;
        if !seen_devices.insert(package.owner.device_id.clone()) {
            continue;
        }
        validate_key_package_payload(&package.key_package_payload)?;
        packages.push(package);
    }
    debug_assert_eq!(packages.len(), seen_devices.len());
    Ok(packages)
}

fn load_key_package_required(
    conn: &Connection,
    key_package_id: &str,
) -> Result<KeyPackageRecord, StoreError> {
    load_key_package(conn, key_package_id)?
        .ok_or_else(|| EngineError::KeyPackageNotFound(key_package_id.to_string()).into())
}

fn row_to_key_package(row: &Row<'_>) -> Result<KeyPackageRecord, StoreError> {
    Ok(KeyPackageRecord {
        key_package_id: row.get(0)?,
        owner: DeviceRef {
            account_id: row.get(1)?,
            device_id: row.get(2)?,
        },
        key_package_ref: row.get(3)?,
        key_package_hash: row.get(4)?,
        key_package_payload: row.get(5)?,
        state: decode_key_package_state(row.get::<_, String>(6)?.as_str())?,
        lease_token: row.get(7)?,
    })
}

fn load_welcome(conn: &Connection, welcome_id: &str) -> Result<Option<WelcomeRecord>, StoreError> {
    let mut statement = conn.prepare(
        r#"
        SELECT welcome_id, room_id, commit_seq,
               recipient_account_id, recipient_device_id,
               sender_account_id, sender_device_id,
               key_package_id, join_epoch, state, lease_token,
               welcome_payload, ratchet_tree_payload
        FROM welcomes
        WHERE welcome_id = ?1
        "#,
    )?;
    let mut rows = statement.query(params![welcome_id])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    Ok(Some(row_to_welcome(row)?))
}

fn load_welcome_required(conn: &Connection, welcome_id: &str) -> Result<WelcomeRecord, StoreError> {
    load_welcome(conn, welcome_id)?
        .ok_or_else(|| EngineError::WelcomeNotFound(welcome_id.to_string()).into())
}

fn load_released_welcomes_for_device(
    conn: &Connection,
    device: &DeviceRef,
) -> Result<Vec<WelcomeRecord>, StoreError> {
    let mut statement = conn.prepare(
        r#"
        SELECT welcome_id, room_id, commit_seq,
               recipient_account_id, recipient_device_id,
               sender_account_id, sender_device_id,
               key_package_id, join_epoch, state, lease_token,
               welcome_payload, ratchet_tree_payload
        FROM welcomes
        WHERE recipient_account_id = ?1
          AND recipient_device_id = ?2
          AND state = ?3
        ORDER BY room_id, commit_seq, welcome_id
        LIMIT ?4
        "#,
    )?;
    let mut rows = statement.query(params![
        device.account_id,
        device.device_id,
        encode_welcome_state(WelcomeState::Released),
        i64::from(MAX_WELCOME_CLAIMS_PER_REQUEST),
    ])?;
    let mut welcomes = Vec::new();
    while let Some(row) = rows.next()? {
        welcomes.push(row_to_welcome(row)?);
    }
    Ok(welcomes)
}

fn row_to_welcome(row: &rusqlite::Row<'_>) -> Result<WelcomeRecord, StoreError> {
    Ok(WelcomeRecord {
        welcome_id: row.get(0)?,
        room_id: row.get(1)?,
        commit_seq: from_i64("commit_seq", row.get(2)?)?,
        recipient: DeviceRef {
            account_id: row.get(3)?,
            device_id: row.get(4)?,
        },
        sender: DeviceRef {
            account_id: row.get(5)?,
            device_id: row.get(6)?,
        },
        key_package_id: row.get(7)?,
        join_epoch: from_i64("join_epoch", row.get(8)?)?,
        state: decode_welcome_state(row.get::<_, String>(9)?.as_str())?,
        lease_token: row.get(10)?,
        welcome_payload: row.get(11)?,
        ratchet_tree_payload: row.get(12)?,
    })
}

fn load_link_session(
    conn: &Connection,
    link_session_id: &str,
) -> Result<Option<LinkSessionRecord>, StoreError> {
    let mut statement = conn.prepare(
        r#"
        SELECT link_session_id, pairing_public_key, encrypted_payload, state, claim_token
        FROM link_sessions
        WHERE link_session_id = ?1
        "#,
    )?;
    let mut rows = statement.query(params![link_session_id])?;
    let Some(row) = rows.next()? else {
        return Ok(None);
    };
    Ok(Some(LinkSessionRecord {
        link_session_id: row.get(0)?,
        pairing_public_key: row.get(1)?,
        encrypted_payload: row.get(2)?,
        state: decode_link_session_state(row.get::<_, String>(3)?.as_str())?,
        claim_token: row.get(4)?,
    }))
}

fn load_link_session_required(
    conn: &Connection,
    link_session_id: &str,
) -> Result<LinkSessionRecord, StoreError> {
    load_link_session(conn, link_session_id)?
        .ok_or_else(|| EngineError::LinkSessionNotFound(link_session_id.to_string()).into())
}

fn load_idempotency(
    conn: &Connection,
    scope_key: &str,
) -> Result<Option<IdempotencyRecord>, StoreError> {
    let row = conn
        .query_row(
            r#"
            SELECT request_hash, response_kind, response_json
            FROM idempotency_records
            WHERE scope_key = ?1
            "#,
            params![scope_key],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()?;
    let Some((request_hash, response_kind, response_json)) = row else {
        return Ok(None);
    };
    let response: PersistedIdempotencyResponse = serde_json::from_str(&response_json)?;
    if response.response_kind() != response_kind {
        return Err(StoreError::CorruptState(format!(
            "idempotency response kind mismatch for {scope_key}"
        )));
    }
    Ok(Some(IdempotencyRecord {
        request_hash,
        response,
    }))
}

fn encode_device_status(status: DeviceStatus) -> &'static str {
    match status {
        DeviceStatus::Active => "active",
        DeviceStatus::Revoked => "revoked",
    }
}

fn decode_device_status(value: &str) -> Result<DeviceStatus, StoreError> {
    match value {
        "active" => Ok(DeviceStatus::Active),
        "revoked" => Ok(DeviceStatus::Revoked),
        other => Err(StoreError::CorruptState(format!(
            "unknown device status {other}"
        ))),
    }
}

fn encode_room_status(status: RoomStatus) -> &'static str {
    match status {
        RoomStatus::Open => "open",
        RoomStatus::NeedsRepair => "needs_repair",
        RoomStatus::Closed => "closed",
    }
}

fn decode_room_status(value: &str) -> Result<RoomStatus, StoreError> {
    match value {
        "open" => Ok(RoomStatus::Open),
        "needs_repair" => Ok(RoomStatus::NeedsRepair),
        "closed" => Ok(RoomStatus::Closed),
        other => Err(StoreError::CorruptState(format!(
            "unknown room status {other}"
        ))),
    }
}

fn encode_log_entry_kind(kind: LogEntryKind) -> &'static str {
    match kind {
        LogEntryKind::Application => "application",
        LogEntryKind::Proposal => "proposal",
        LogEntryKind::Commit => "commit",
    }
}

fn decode_log_entry_kind(value: &str) -> Result<LogEntryKind, StoreError> {
    match value {
        "application" => Ok(LogEntryKind::Application),
        "proposal" => Ok(LogEntryKind::Proposal),
        "commit" => Ok(LogEntryKind::Commit),
        other => Err(StoreError::CorruptState(format!(
            "unknown log entry kind {other}"
        ))),
    }
}

fn encode_key_package_state(state: KeyPackageState) -> &'static str {
    match state {
        KeyPackageState::Available => "available",
        KeyPackageState::Leased => "leased",
        KeyPackageState::Consumed => "consumed",
        KeyPackageState::Released => "released",
        KeyPackageState::Expired => "expired",
    }
}

fn decode_key_package_state(value: &str) -> Result<KeyPackageState, StoreError> {
    match value {
        "available" => Ok(KeyPackageState::Available),
        "leased" => Ok(KeyPackageState::Leased),
        "consumed" => Ok(KeyPackageState::Consumed),
        "released" => Ok(KeyPackageState::Released),
        "expired" => Ok(KeyPackageState::Expired),
        other => Err(StoreError::CorruptState(format!(
            "unknown key package state {other}"
        ))),
    }
}

fn encode_welcome_state(state: WelcomeState) -> &'static str {
    match state {
        WelcomeState::Staged => "staged",
        WelcomeState::Released => "released",
        WelcomeState::Claimed => "claimed",
        WelcomeState::Acked => "acked",
        WelcomeState::Failed => "failed",
        WelcomeState::Expired => "expired",
        WelcomeState::Cancelled => "cancelled",
    }
}

fn decode_welcome_state(value: &str) -> Result<WelcomeState, StoreError> {
    match value {
        "staged" => Ok(WelcomeState::Staged),
        "released" => Ok(WelcomeState::Released),
        "claimed" => Ok(WelcomeState::Claimed),
        "acked" => Ok(WelcomeState::Acked),
        "failed" => Ok(WelcomeState::Failed),
        "expired" => Ok(WelcomeState::Expired),
        "cancelled" => Ok(WelcomeState::Cancelled),
        other => Err(StoreError::CorruptState(format!(
            "unknown welcome state {other}"
        ))),
    }
}

fn encode_link_session_state(state: LinkSessionState) -> &'static str {
    match state {
        LinkSessionState::Created => "created",
        LinkSessionState::PayloadUploaded => "payload_uploaded",
        LinkSessionState::Claimed => "claimed",
        LinkSessionState::Delivered => "delivered",
        LinkSessionState::Expired => "expired",
    }
}

fn decode_link_session_state(value: &str) -> Result<LinkSessionState, StoreError> {
    match value {
        "created" => Ok(LinkSessionState::Created),
        "payload_uploaded" => Ok(LinkSessionState::PayloadUploaded),
        "claimed" => Ok(LinkSessionState::Claimed),
        "delivered" => Ok(LinkSessionState::Delivered),
        "expired" => Ok(LinkSessionState::Expired),
        other => Err(StoreError::CorruptState(format!(
            "unknown link session state {other}"
        ))),
    }
}

fn to_i64(field: &'static str, value: u64) -> Result<i64, StoreError> {
    i64::try_from(value).map_err(|_| StoreError::NumberOutOfRange { field, value })
}

fn optional_i64(field: &'static str, value: Option<u64>) -> Result<Option<i64>, StoreError> {
    value.map(|value| to_i64(field, value)).transpose()
}

fn from_i64(field: &'static str, value: i64) -> Result<u64, StoreError> {
    if value >= 0 {
        Ok(value as u64)
    } else {
        Err(StoreError::CorruptState(format!(
            "{field} has negative value {value}"
        )))
    }
}

fn optional_u64(field: &'static str, value: Option<i64>) -> Result<Option<u64>, StoreError> {
    value.map(|value| from_i64(field, value)).transpose()
}

fn bool_to_i64(value: bool) -> i64 {
    i64::from(value)
}

fn bool_from_i64(field: &'static str, value: i64) -> Result<bool, StoreError> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        other => Err(StoreError::CorruptState(format!(
            "{field} has non-boolean value {other}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use finitechat_engine::{CreateRoomRequest, device};

    #[test]
    fn sqlite_store_persists_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("finitechat.sqlite3");
        let mut store = SqliteDeliveryStore::open(&path).unwrap();
        store
            .create_room(CreateRoomRequest {
                room_id: "room_1".to_string(),
                mls_group_id: "group_1".to_string(),
                creator: device("alice", "browser"),
            })
            .unwrap();

        let reopened = SqliteDeliveryStore::open(&path).unwrap();
        assert!(reopened.room("room_1").unwrap().is_some());
    }
}
