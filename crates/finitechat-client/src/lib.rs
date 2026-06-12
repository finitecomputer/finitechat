use cgka_traits::engine::KeyPackage as HttpKeyPackage;
use cgka_traits::{
    GroupId as HttpGroupId, MemberId as HttpMemberId,
    MessageId as HttpMessageId,
};
use finitechat_proto::{
    AccountRoomRecord, AppendApplicationEventRequest, AppendEventRequest,
    ApplicationDeliveryPolicy, ClaimKeyPackageResult, CommitAccepted,
    CreateRoomRequest, EngineError, EventAccepted,
    KeyPackageInventory, ListAccountRoomsPage, ListAccountRoomsRequest, SubmitCommitRequest,
    SyncEventsPage, UploadKeyPackageRequest, WelcomeRecord, envelope, lease_token_for,
};
use finitechat_http::{
    AckWelcomeRequest, AckWelcomeResponse, BootstrapAccountRoomRequest,
    BootstrapAccountRoomResponse, ClaimKeyPackageRequest, ClaimWelcomesRequest,
    CreateInviteSessionRequest, FiniteAccountRoomCommitProjection, GroupSyncRequest,
    HttpClaimedWelcome, HttpInviteJoinRequestRecord, HttpInviteJoinState,
    HttpInviteSessionRecord, HttpKeyPackageInventory, InviteJoinStatusRequest,
    InviteJoinStatusResponse, KeyPackageInventoryRequest, ListAccountRoomDirectoryRequest,
    ListAccountRoomDirectoryResponse, ListInviteJoinRequestsRequest,
    ListInviteJoinRequestsResponse, PublishKeyPackageResponse, RespondInviteJoinRequest,
    SaveAccountRoomRequest, SaveAccountRoomResponse, SubmitInviteJoinRequest,
};
use finitechat_mls::{
    ExpectedDeviceCredential, FiniteDeviceCredentialV1, MlsCredentialError, NOSTR_PUBLIC_KEY_BYTES,
    NOSTR_SECRET_KEY_BYTES, NostrPublicKey, NostrSecretKey,
};
use finitechat_proto::message_id_for_bytes;
use finitechat_proto::{
    AppendEphemeralActivityRequest, EphemeralActivityAccepted, InviteCodeV1,
    MAX_INVITE_TTL_MILLIS, invite_join_proof, verify_invite_join_proof,
};
use finitechat_proto::{
    DeviceRef, KeyPackageId, LogEntryKind, MAX_ACCOUNT_ID_BYTES,
    MAX_ACCOUNT_ROOM_DISCOVERY_RESULTS, MAX_DEVICE_ID_BYTES, MAX_ENVELOPE_PAYLOAD_BYTES,
    MAX_IDEMPOTENCY_KEY_BYTES, MAX_KEY_PACKAGE_PAYLOAD_BYTES, MAX_KEY_PACKAGES_PER_DEVICE,
    MAX_MLS_GROUP_ID_BYTES, MAX_OBJECT_ID_BYTES, MAX_RATCHET_TREE_PAYLOAD_BYTES, MAX_ROOM_ID_BYTES,
    MAX_STAGED_WELCOMES_PER_COMMIT, MAX_WELCOME_CLAIMS_PER_REQUEST, MAX_WELCOME_PAYLOAD_BYTES,
    MembershipAddV1, MembershipDeltaV1, MembershipRemoveV1, MlsGroupId, ProtocolLimitError, RoomId,
    RoomLogEntry, StagedWelcomeV1, WelcomeId, WelcomeState, validate_bytes_len,
    validate_bytes_non_empty, validate_idempotency_key, validate_item_count, validate_mls_group_id,
    validate_room_id, validate_string_bytes,
};
use openmls::prelude::tls_codec::{Deserialize as _, Serialize as _};
use openmls::prelude::{
    AeadType, Ciphersuite, CredentialWithKey, GroupId, KeyPackage, KeyPackageIn, LeafNodeIndex,
    LeafNodeParameters, MlsGroup, MlsGroupCreateConfig, MlsMessageBodyIn, MlsMessageIn,
    MlsMessageOut, OpenMlsCrypto, OpenMlsProvider, OpenMlsRand, ProcessedMessageContent,
    ProtocolMessage, ProtocolVersion, RatchetTreeIn, StagedCommit, StagedWelcome, Welcome,
};
use openmls_basic_credential::SignatureKeyPair;
use openmls_rust_crypto::OpenMlsRustCrypto;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;
use transport_http_server::{
    HttpKeyPackageId, HttpKeyPackagePublication, HttpSyncPage, MAX_HTTP_SYNC_PAGE_ENTRIES,
};

pub const FINITECHAT_CIPHERSUITE: Ciphersuite =
    Ciphersuite::MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519;

const CLIENT_STORE_KEY_DERIVATION_DOMAIN: &[u8] = b"finitechat.client-store-key.v1";
const CLIENT_STATE_SNAPSHOT_MAGIC: &[u8] = b"finitechat.client-state-snapshot.v1";
const CLIENT_STATE_SNAPSHOT_VERSION: u16 = 8;
const CLIENT_STORE_KEY_BYTES: usize = 32;
const CLIENT_STORE_NONCE_BYTES: usize = 12;
const CLIENT_STORE_AEAD_TAG_BYTES: u32 = 16;
const MAX_PERSISTED_ROOMS: u32 = 1024;
const MAX_ROOM_SERVER_URL_BYTES: u32 = 2048;
const ACTIVITY_EXPORTER_LABEL: &str = "finite-activity-v1";
const ACTIVITY_NONCE_BYTES: usize = 12;
const MAX_PENDING_CLIENT_WELCOMES: u32 = MAX_WELCOME_CLAIMS_PER_REQUEST;
const MAX_PENDING_KEY_PACKAGE_UPLOADS: u32 = MAX_KEY_PACKAGES_PER_DEVICE;
const MAX_LINK_FANOUTS: u32 = 16;
const MAX_LINK_FANOUT_ROOMS: u32 = MAX_ACCOUNT_ROOM_DISCOVERY_RESULTS;
const MAX_RUNTIME_SYNC_PAGES_PER_ROOM: u32 = 64;
const MAX_RUNTIME_LINK_FANOUT_DISCOVERY_PAGES_PER_TICK: u32 = MAX_LINK_FANOUT_ROOMS;
const MAX_RUNTIME_LINK_FANOUT_COMMITS_PER_TICK: u32 = MAX_LINK_FANOUT_ROOMS;
const MAX_OPENMLS_STORAGE_RECORDS: u32 = 8192;
const MAX_CLIENT_SIGNER_PUBLIC_KEY_BYTES: u32 = MAX_OBJECT_ID_BYTES;
const MAX_CLIENT_CREDENTIAL_IDENTITY_BYTES: u32 = 1024;
const MAX_OPENMLS_STORAGE_KEY_BYTES: u32 = 4 * 1024;
const MAX_OPENMLS_STORAGE_VALUE_BYTES: u32 = 8 * 1024 * 1024;
const MAX_CLIENT_STATE_PLAINTEXT_BYTES: u32 = 32 * 1024 * 1024;
const MAX_CLIENT_STATE_CIPHERTEXT_BYTES: u32 =
    MAX_CLIENT_STATE_PLAINTEXT_BYTES + CLIENT_STORE_AEAD_TAG_BYTES;
const U16_BYTES: usize = 2;
const U32_BYTES: usize = 4;
const U64_BYTES: usize = 8;
const LINK_FANOUT_STATUS_PENDING: u16 = 0;
const LINK_FANOUT_STATUS_PREPARED: u16 = 1;
const LINK_FANOUT_STATUS_DONE: u16 = 2;
const LOG_ENTRY_KIND_APPLICATION: u16 = 0;
const LOG_ENTRY_KIND_PROPOSAL: u16 = 1;
const LOG_ENTRY_KIND_COMMIT: u16 = 2;

const _: () = {
    assert!(NOSTR_PUBLIC_KEY_BYTES == 32);
    assert!(CLIENT_STORE_KEY_BYTES == 32);
    assert!(CLIENT_STORE_NONCE_BYTES == 12);
    assert!(CLIENT_STORE_AEAD_TAG_BYTES == 16);
    assert!(MAX_PERSISTED_ROOMS > 0);
    assert!(MAX_PENDING_CLIENT_WELCOMES > 0);
    assert!(MAX_PENDING_KEY_PACKAGE_UPLOADS > 0);
    assert!(MAX_LINK_FANOUTS > 0);
    assert!(MAX_LINK_FANOUT_ROOMS > 0);
    assert!(MAX_RUNTIME_SYNC_PAGES_PER_ROOM > 0);
    assert!(MAX_RUNTIME_LINK_FANOUT_DISCOVERY_PAGES_PER_TICK > 0);
    assert!(MAX_RUNTIME_LINK_FANOUT_COMMITS_PER_TICK > 0);
    assert!(MAX_OPENMLS_STORAGE_RECORDS > 0);
    assert!(MAX_OPENMLS_STORAGE_KEY_BYTES > 0);
    assert!(MAX_OPENMLS_STORAGE_VALUE_BYTES > MAX_OPENMLS_STORAGE_KEY_BYTES);
    assert!(MAX_CLIENT_STATE_CIPHERTEXT_BYTES > MAX_CLIENT_STATE_PLAINTEXT_BYTES);
};

#[derive(Debug, Clone)]
pub struct FiniteChatDeviceConfig {
    pub account_secret_key: NostrSecretKey,
    pub device_id: String,
    pub now_unix_seconds: u64,
    pub credential_not_before_unix_seconds: u64,
    pub credential_not_after_unix_seconds: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecryptedApplicationEntry {
    pub plaintext: Vec<u8>,
    /// MLS-authenticated sender derived from the verified message credential.
    pub sender: DeviceRef,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedCommit {
    pub request: SubmitCommitRequest,
    pub message_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FiniteChatDeviceState {
    pub device_ref: DeviceRef,
    pub signer_public_key: Vec<u8>,
    pub credential_identity: Vec<u8>,
    pub rooms: Vec<PersistedRoomState>,
    pub pending_welcomes: Vec<PendingWelcomeState>,
    pub pending_welcome_acks: Vec<PendingWelcomeAckState>,
    pub pending_key_package_uploads: Vec<UploadKeyPackageRequest>,
    pub link_fanouts: Vec<LinkFanoutState>,
    pub openmls_storage_records: Vec<OpenMlsStorageRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedRoomState {
    pub room_id: RoomId,
    pub mls_group_id: MlsGroupId,
    pub last_applied_seq: u64,
    /// Which server hosts this room's ordered log. `None` means the
    /// device's home server (ADR 0005); invited rooms record the address
    /// from their invite code so the sync ticks can group rooms by server.
    pub server_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingWelcomeState {
    pub welcome_id: WelcomeId,
    pub room_id: RoomId,
    pub commit_seq: u64,
    pub welcome_payload: Vec<u8>,
    pub ratchet_tree_payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingWelcomeAckState {
    pub welcome_id: WelcomeId,
    pub room_id: RoomId,
    pub commit_seq: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkFanoutState {
    pub fanout_id: String,
    pub target_device: DeviceRef,
    pub after_room_id: Option<RoomId>,
    pub discovery_complete: bool,
    pub rooms: Vec<LinkFanoutRoomState>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkFanoutRoomState {
    pub plan: LinkFanoutRoomPlan,
    pub claimed_key_package: Option<ClaimKeyPackageResult>,
    pub status: LinkFanoutRoomStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkFanoutRoomPlan {
    pub room_id: RoomId,
    pub key_package_id: KeyPackageId,
    pub welcome_id: WelcomeId,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkFanoutRoomStatus {
    Pending,
    Prepared { prepared: Box<PreparedCommit> },
    Done { accepted_seq: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenMlsStorageRecord {
    pub key: Vec<u8>,
    pub value: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppliedLogEntry {
    Application {
        plaintext: Vec<u8>,
        /// The MLS-authenticated sender (from the decrypted message's
        /// verified credential), not the server-claimed envelope sender.
        sender: DeviceRef,
    },
    Commit { sender: DeviceRef, epoch: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyPackageReplenishmentPlan {
    pub inventory: KeyPackageInventory,
    pub target_available: u32,
    pub upload_requests: Vec<UploadKeyPackageRequest>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeSyncOptions {
    pub key_package_target_available: u32,
    pub max_sync_pages_per_room: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeLinkFanoutOptions {
    pub max_discovery_pages_per_tick: u32,
    pub max_commit_rooms_per_tick: u32,
    pub max_completion_sync_pages_per_room: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeSyncReport {
    pub uploaded_key_packages: u32,
    pub claimed_welcomes: u32,
    pub activated_welcome_acks_sent: u32,
    pub sync_pages: u32,
    pub applied_entries: Vec<RuntimeAppliedEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeAppliedEntry {
    pub room_id: RoomId,
    pub seq: u64,
    pub message_id: String,
    pub entry: AppliedLogEntry,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeLinkFanoutReport {
    pub discovery_pages: u32,
    pub queued_rooms: u32,
    pub claimed_key_packages: u32,
    pub prepared_commits: u32,
    pub submitted_commits: u32,
    pub completion_sync_pages: u32,
    pub completed_rooms: u32,
    pub applied_entries: Vec<RuntimeAppliedEntry>,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoomSyncCursor {
    pub room_id: RoomId,
    pub after_seq: u64,
    /// `None` = the device's home server (ADR 0005).
    pub server_url: Option<String>,
}

impl RuntimeSyncOptions {
    fn validate_limits(&self) -> Result<(), ClientError> {
        validate_item_count(
            "runtime.key_package_target_available",
            self.key_package_target_available as usize,
            MAX_KEY_PACKAGES_PER_DEVICE,
        )?;
        validate_bytes_non_empty(
            "runtime.max_sync_pages_per_room",
            self.max_sync_pages_per_room as usize,
        )?;
        validate_item_count(
            "runtime.max_sync_pages_per_room",
            self.max_sync_pages_per_room as usize,
            MAX_RUNTIME_SYNC_PAGES_PER_ROOM,
        )?;
        Ok(())
    }
}

impl RuntimeLinkFanoutOptions {
    fn validate_limits(&self) -> Result<(), ClientError> {
        validate_bytes_non_empty(
            "runtime_link_fanout.max_discovery_pages_per_tick",
            self.max_discovery_pages_per_tick as usize,
        )?;
        validate_item_count(
            "runtime_link_fanout.max_discovery_pages_per_tick",
            self.max_discovery_pages_per_tick as usize,
            MAX_RUNTIME_LINK_FANOUT_DISCOVERY_PAGES_PER_TICK,
        )?;
        validate_bytes_non_empty(
            "runtime_link_fanout.max_commit_rooms_per_tick",
            self.max_commit_rooms_per_tick as usize,
        )?;
        validate_item_count(
            "runtime_link_fanout.max_commit_rooms_per_tick",
            self.max_commit_rooms_per_tick as usize,
            MAX_RUNTIME_LINK_FANOUT_COMMITS_PER_TICK,
        )?;
        validate_bytes_non_empty(
            "runtime_link_fanout.max_completion_sync_pages_per_room",
            self.max_completion_sync_pages_per_room as usize,
        )?;
        validate_item_count(
            "runtime_link_fanout.max_completion_sync_pages_per_room",
            self.max_completion_sync_pages_per_room as usize,
            MAX_RUNTIME_SYNC_PAGES_PER_ROOM,
        )?;
        Ok(())
    }
}

impl RuntimeSyncReport {
    fn record_uploaded_key_package(&mut self) -> Result<(), ClientError> {
        self.uploaded_key_packages = self
            .uploaded_key_packages
            .checked_add(1)
            .ok_or(ClientError::RuntimeCounterOverflow)?;
        Ok(())
    }

    fn record_claimed_welcomes(&mut self, count: usize) -> Result<(), ClientError> {
        let count = u32::try_from(count).map_err(|_| ClientError::RuntimeCounterOverflow)?;
        self.claimed_welcomes = self
            .claimed_welcomes
            .checked_add(count)
            .ok_or(ClientError::RuntimeCounterOverflow)?;
        Ok(())
    }

    fn record_welcome_ack(&mut self) -> Result<(), ClientError> {
        self.activated_welcome_acks_sent = self
            .activated_welcome_acks_sent
            .checked_add(1)
            .ok_or(ClientError::RuntimeCounterOverflow)?;
        Ok(())
    }

    fn record_sync_page(&mut self) -> Result<(), ClientError> {
        self.sync_pages = self
            .sync_pages
            .checked_add(1)
            .ok_or(ClientError::RuntimeCounterOverflow)?;
        Ok(())
    }
}

impl RuntimeLinkFanoutReport {
    fn record_discovery_page(&mut self) -> Result<(), ClientError> {
        self.discovery_pages = self
            .discovery_pages
            .checked_add(1)
            .ok_or(ClientError::RuntimeCounterOverflow)?;
        Ok(())
    }

    fn record_queued_room(&mut self) -> Result<(), ClientError> {
        self.queued_rooms = self
            .queued_rooms
            .checked_add(1)
            .ok_or(ClientError::RuntimeCounterOverflow)?;
        Ok(())
    }

    fn record_claimed_key_package(&mut self) -> Result<(), ClientError> {
        self.claimed_key_packages = self
            .claimed_key_packages
            .checked_add(1)
            .ok_or(ClientError::RuntimeCounterOverflow)?;
        Ok(())
    }

    fn record_prepared_commit(&mut self) -> Result<(), ClientError> {
        self.prepared_commits = self
            .prepared_commits
            .checked_add(1)
            .ok_or(ClientError::RuntimeCounterOverflow)?;
        Ok(())
    }

    fn record_submitted_commit(&mut self) -> Result<(), ClientError> {
        self.submitted_commits = self
            .submitted_commits
            .checked_add(1)
            .ok_or(ClientError::RuntimeCounterOverflow)?;
        Ok(())
    }

    fn record_completion_sync_page(&mut self) -> Result<(), ClientError> {
        self.completion_sync_pages = self
            .completion_sync_pages
            .checked_add(1)
            .ok_or(ClientError::RuntimeCounterOverflow)?;
        Ok(())
    }

    fn record_completed_room(&mut self) -> Result<(), ClientError> {
        self.completed_rooms = self
            .completed_rooms
            .checked_add(1)
            .ok_or(ClientError::RuntimeCounterOverflow)?;
        Ok(())
    }
}

pub struct FiniteChatDevice {
    provider: OpenMlsRustCrypto,
    device_ref: DeviceRef,
    now_unix_seconds: u64,
    credential: FiniteDeviceCredentialV1,
    credential_with_key: CredentialWithKey,
    signer: SignatureKeyPair,
    groups: BTreeMap<RoomId, MlsGroup>,
    room_cursors: BTreeMap<RoomId, u64>,
    room_server_urls: BTreeMap<RoomId, String>,
    pending_welcomes: BTreeMap<WelcomeId, PendingWelcomeState>,
    pending_welcome_acks: BTreeMap<WelcomeId, PendingWelcomeAckState>,
    pending_key_package_uploads: BTreeMap<KeyPackageId, UploadKeyPackageRequest>,
    link_fanouts: BTreeMap<String, LinkFanoutState>,
}

impl FiniteChatDevice {
    pub fn new(config: FiniteChatDeviceConfig) -> Result<Self, ClientError> {
        let provider = OpenMlsRustCrypto::default();
        let signer = SignatureKeyPair::new(FINITECHAT_CIPHERSUITE.signature_algorithm())
            .map_err(|_| ClientError::CreateSigner)?;
        signer
            .store(provider.storage())
            .map_err(|_| ClientError::StoreSigner)?;

        let account_public_key = config.account_secret_key.public_key();
        let credential = FiniteDeviceCredentialV1::sign(
            &config.account_secret_key,
            config.device_id.clone(),
            signer.to_public_vec(),
            config.credential_not_before_unix_seconds,
            config.credential_not_after_unix_seconds,
        )?;
        credential.verify_expected(ExpectedDeviceCredential {
            account_public_key,
            device_id: &config.device_id,
            mls_leaf_signing_public_key: signer.public(),
            now_unix_seconds: config.now_unix_seconds,
        })?;

        let credential_with_key = credential.to_openmls_credential_with_key();
        let device_ref = DeviceRef {
            account_id: hex_lower(account_public_key.as_bytes()),
            device_id: config.device_id,
        };
        device_ref.validate_limits()?;

        let device = Self {
            provider,
            device_ref,
            now_unix_seconds: config.now_unix_seconds,
            credential,
            credential_with_key,
            signer,
            groups: BTreeMap::new(),
            room_cursors: BTreeMap::new(),
            room_server_urls: BTreeMap::new(),
            pending_welcomes: BTreeMap::new(),
            pending_welcome_acks: BTreeMap::new(),
            pending_key_package_uploads: BTreeMap::new(),
            link_fanouts: BTreeMap::new(),
        };
        debug_assert_eq!(
            device.credential_with_key.signature_key.as_slice(),
            device.signer.public()
        );
        Ok(device)
    }

    pub fn from_state(
        config: FiniteChatDeviceConfig,
        state: FiniteChatDeviceState,
    ) -> Result<Self, ClientError> {
        state.validate_limits()?;

        let provider = OpenMlsRustCrypto::default();
        {
            let mut values = provider
                .storage()
                .values
                .write()
                .map_err(|_| ClientError::OpenMlsStorageLock)?;
            values.clear();
            for record in &state.openmls_storage_records {
                values.insert(record.key.clone(), record.value.clone());
            }
        }

        let credential = FiniteDeviceCredentialV1::from_identity_bytes(&state.credential_identity)?;
        let account_public_key = config.account_secret_key.public_key();
        if credential.account_public_key() != account_public_key {
            return Err(ClientError::PersistedAccountMismatch);
        }
        if credential.device_id() != config.device_id {
            return Err(ClientError::PersistedDeviceMismatch);
        }
        credential.verify_expected(ExpectedDeviceCredential {
            account_public_key,
            device_id: &config.device_id,
            mls_leaf_signing_public_key: &state.signer_public_key,
            now_unix_seconds: config.now_unix_seconds,
        })?;

        let signer = SignatureKeyPair::read(
            provider.storage(),
            &state.signer_public_key,
            FINITECHAT_CIPHERSUITE.signature_algorithm(),
        )
        .ok_or(ClientError::MissingStoredSigner)?;
        if signer.public() != state.signer_public_key {
            return Err(ClientError::StoredSignerMismatch);
        }

        let credential_with_key = credential.to_openmls_credential_with_key();
        let device_ref = DeviceRef {
            account_id: hex_lower(account_public_key.as_bytes()),
            device_id: config.device_id,
        };
        if device_ref != state.device_ref {
            return Err(ClientError::PersistedDeviceMismatch);
        }

        let mut groups = BTreeMap::new();
        for room in &state.rooms {
            let group_id = GroupId::from_slice(room.mls_group_id.as_bytes());
            let group = MlsGroup::load(provider.storage(), &group_id)
                .map_err(|_| ClientError::LoadGroupState(room.room_id.clone()))?
                .ok_or_else(|| ClientError::MissingGroupState(room.room_id.clone()))?;
            if mls_group_id_string(group.group_id())? != room.mls_group_id {
                return Err(ClientError::PersistedGroupIdMismatch(room.room_id.clone()));
            }
            if groups.insert(room.room_id.clone(), group).is_some() {
                return Err(ClientError::DuplicatePersistedRoom(room.room_id.clone()));
            }
        }
        let room_server_urls = state
            .rooms
            .iter()
            .filter_map(|room| {
                room.server_url
                    .clone()
                    .map(|server_url| (room.room_id.clone(), server_url))
            })
            .collect::<BTreeMap<_, _>>();
        let room_cursors = state
            .rooms
            .iter()
            .map(|room| (room.room_id.clone(), room.last_applied_seq))
            .collect::<BTreeMap<_, _>>();
        let pending_welcomes = state
            .pending_welcomes
            .iter()
            .map(|welcome| (welcome.welcome_id.clone(), welcome.clone()))
            .collect::<BTreeMap<_, _>>();
        let pending_welcome_acks = state
            .pending_welcome_acks
            .iter()
            .map(|ack| (ack.welcome_id.clone(), ack.clone()))
            .collect::<BTreeMap<_, _>>();
        let pending_key_package_uploads = state
            .pending_key_package_uploads
            .iter()
            .map(|request| (request.key_package_id.clone(), request.clone()))
            .collect::<BTreeMap<_, _>>();
        let link_fanouts = state
            .link_fanouts
            .iter()
            .map(|fanout| (fanout.fanout_id.clone(), fanout.clone()))
            .collect::<BTreeMap<_, _>>();

        let device = Self {
            provider,
            device_ref,
            now_unix_seconds: config.now_unix_seconds,
            credential,
            credential_with_key,
            signer,
            groups,
            room_cursors,
            room_server_urls,
            pending_welcomes,
            pending_welcome_acks,
            pending_key_package_uploads,
            link_fanouts,
        };
        debug_assert_eq!(
            device.credential_with_key.signature_key.as_slice(),
            device.signer.public()
        );
        Ok(device)
    }

    pub fn export_state(&self) -> Result<FiniteChatDeviceState, ClientError> {
        let values = self
            .provider
            .storage()
            .values
            .read()
            .map_err(|_| ClientError::OpenMlsStorageLock)?;
        let mut openmls_storage_records = values
            .iter()
            .map(|(key, value)| OpenMlsStorageRecord {
                key: key.clone(),
                value: value.clone(),
            })
            .collect::<Vec<_>>();
        openmls_storage_records.sort_by(|left, right| left.key.cmp(&right.key));

        let rooms = self
            .groups
            .iter()
            .map(|(room_id, group)| {
                Ok(PersistedRoomState {
                    room_id: room_id.clone(),
                    mls_group_id: mls_group_id_string(group.group_id())?,
                    last_applied_seq: *self.room_cursors.get(room_id).unwrap_or(&0),
                    server_url: self.room_server_urls.get(room_id).cloned(),
                })
            })
            .collect::<Result<Vec<_>, ClientError>>()?;
        // self.groups is a BTreeMap, so rooms are already sorted by room_id.
        let pending_welcomes = self.pending_welcomes.values().cloned().collect::<Vec<_>>();
        let pending_welcome_acks = self
            .pending_welcome_acks
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let pending_key_package_uploads = self
            .pending_key_package_uploads
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let link_fanouts = self.link_fanouts.values().cloned().collect::<Vec<_>>();

        let state = FiniteChatDeviceState {
            device_ref: self.device_ref.clone(),
            signer_public_key: self.signer.public().to_vec(),
            credential_identity: self.credential.identity_bytes(),
            rooms,
            pending_welcomes,
            pending_welcome_acks,
            pending_key_package_uploads,
            link_fanouts,
            openmls_storage_records,
        };
        state.validate_limits()?;
        Ok(state)
    }

    pub fn device_ref(&self) -> &DeviceRef {
        &self.device_ref
    }


    pub fn create_group_state(
        &mut self,
        room_id: impl Into<RoomId>,
        mls_group_id: impl AsRef<str>,
    ) -> Result<(), ClientError> {
        let room_id = room_id.into();
        if self.groups.contains_key(&room_id) {
            return Err(ClientError::GroupAlreadyExists(room_id));
        }

        let group = MlsGroup::new_with_group_id(
            &self.provider,
            &self.signer,
            &openmls_group_config(),
            GroupId::from_slice(mls_group_id.as_ref().as_bytes()),
            self.credential_with_key.clone(),
        )
        .map_err(|_| ClientError::CreateGroup)?;
        self.groups.insert(room_id.clone(), group);
        self.room_cursors.insert(room_id, 0);
        Ok(())
    }

    pub fn upload_key_package_request(
        &self,
        key_package_id: impl Into<String>,
    ) -> Result<UploadKeyPackageRequest, ClientError> {
        self.build_upload_key_package_request(Some(key_package_id.into()))
    }

    pub fn upload_key_package_auto_id_request(
        &self,
    ) -> Result<UploadKeyPackageRequest, ClientError> {
        self.build_upload_key_package_request(None)
    }

    fn build_upload_key_package_request(
        &self,
        key_package_id: Option<String>,
    ) -> Result<UploadKeyPackageRequest, ClientError> {
        let key_package = KeyPackage::builder()
            .build(
                FINITECHAT_CIPHERSUITE,
                &self.provider,
                &self.signer,
                self.credential_with_key.clone(),
            )
            .map_err(|_| ClientError::BuildKeyPackage)?;
        let payload = key_package
            .key_package()
            .tls_serialize_detached()
            .map_err(|_| ClientError::SerializeKeyPackage)?;
        let key_package_ref = key_package
            .key_package()
            .hash_ref(self.provider.crypto())
            .map_err(|_| ClientError::HashKeyPackageRef)?;
        let key_package_hash = message_id_for_bytes(&payload);
        let key_package_id = key_package_id.unwrap_or_else(|| format!("kp_{key_package_hash}"));

        let request = UploadKeyPackageRequest {
            key_package_id,
            owner: self.device_ref.clone(),
            key_package_ref: hex_lower(key_package_ref.as_slice()),
            key_package_hash,
            key_package_payload: payload,
        };
        request.validate_limits()?;
        Ok(request)
    }

    pub fn key_package_replenishment_plan(
        &mut self,
        inventory: KeyPackageInventory,
        target_available: u32,
    ) -> Result<KeyPackageReplenishmentPlan, ClientError> {
        inventory.owner.validate_limits()?;
        if inventory.owner != self.device_ref {
            return Err(ClientError::KeyPackageInventoryOwnerMismatch {
                expected: self.device_ref.clone(),
                actual: inventory.owner,
            });
        }
        validate_item_count(
            "key_package_replenishment.target_available",
            target_available as usize,
            MAX_KEY_PACKAGES_PER_DEVICE,
        )?;
        validate_item_count(
            "key_package_replenishment.available",
            inventory.available as usize,
            MAX_KEY_PACKAGES_PER_DEVICE,
        )?;
        validate_item_count(
            "key_package_replenishment.leased",
            inventory.leased as usize,
            MAX_KEY_PACKAGES_PER_DEVICE,
        )?;
        if inventory.unconsumed() > u64::from(MAX_KEY_PACKAGES_PER_DEVICE) {
            return Err(ClientError::KeyPackageInventoryOverCap {
                available: inventory.available,
                leased: inventory.leased,
                max: MAX_KEY_PACKAGES_PER_DEVICE,
            });
        }

        validate_item_count(
            "key_package_replenishment.pending_uploads",
            self.pending_key_package_uploads.len(),
            MAX_PENDING_KEY_PACKAGE_UPLOADS,
        )?;
        let pending_upload_count = self.pending_key_package_uploads.len() as u32;
        let unconsumed_with_pending = inventory
            .unconsumed()
            .checked_add(u64::from(pending_upload_count))
            .ok_or(ClientError::KeyPackageInventoryOverCap {
                available: inventory.available,
                leased: inventory.leased,
                max: MAX_KEY_PACKAGES_PER_DEVICE,
            })?;
        if unconsumed_with_pending > u64::from(MAX_KEY_PACKAGES_PER_DEVICE) {
            return Err(ClientError::KeyPackagePendingUploadOverCap {
                available: inventory.available,
                leased: inventory.leased,
                pending: pending_upload_count,
                max: MAX_KEY_PACKAGES_PER_DEVICE,
            });
        }

        let missing_available = target_available
            .saturating_sub(inventory.available)
            .saturating_sub(pending_upload_count);
        let remaining_cap = u32::try_from(
            u64::from(MAX_KEY_PACKAGES_PER_DEVICE)
                .checked_sub(unconsumed_with_pending)
                .ok_or(ClientError::KeyPackageInventoryOverCap {
                    available: inventory.available,
                    leased: inventory.leased,
                    max: MAX_KEY_PACKAGES_PER_DEVICE,
                })?,
        )
        .map_err(|_| ClientError::KeyPackageInventoryOverCap {
            available: inventory.available,
            leased: inventory.leased,
            max: MAX_KEY_PACKAGES_PER_DEVICE,
        })?;
        let upload_count = missing_available.min(remaining_cap);
        let mut upload_requests =
            Vec::with_capacity(self.pending_key_package_uploads.len() + upload_count as usize);
        upload_requests.extend(self.pending_key_package_uploads());
        for _ in 0..upload_count {
            let request = self.upload_key_package_auto_id_request()?;
            self.store_pending_key_package_upload(request.clone())?;
            upload_requests.push(request);
        }

        debug_assert!(upload_requests.len() <= MAX_KEY_PACKAGES_PER_DEVICE as usize);
        debug_assert!(
            upload_requests
                .iter()
                .all(|request| request.owner == self.device_ref)
        );
        debug_assert_eq!(
            upload_requests.len(),
            self.pending_key_package_uploads.len()
        );
        Ok(KeyPackageReplenishmentPlan {
            inventory,
            target_available,
            upload_requests,
        })
    }

    pub fn prepare_add_member_commit(
        &mut self,
        room_id: &str,
        claimed_key_package: &ClaimKeyPackageResult,
        welcome_id: impl Into<WelcomeId>,
        idempotency_key: impl Into<String>,
    ) -> Result<PreparedCommit, ClientError> {
        let welcome_ids = [welcome_id.into()];
        self.prepare_add_members_commit(
            room_id,
            std::slice::from_ref(claimed_key_package),
            &welcome_ids,
            idempotency_key,
        )
    }

    pub fn prepare_add_members_commit(
        &mut self,
        room_id: &str,
        claimed_key_packages: &[ClaimKeyPackageResult],
        welcome_ids: &[WelcomeId],
        idempotency_key: impl Into<String>,
    ) -> Result<PreparedCommit, ClientError> {
        validate_room_id(room_id)?;
        let idempotency_key = idempotency_key.into();
        validate_idempotency_key(&idempotency_key)?;
        if claimed_key_packages.is_empty() {
            return Err(ClientError::EmptyInviteBatch);
        }
        if claimed_key_packages.len() != welcome_ids.len() {
            return Err(ClientError::InviteWelcomeCountMismatch {
                key_packages: claimed_key_packages.len(),
                welcome_ids: welcome_ids.len(),
            });
        }
        finitechat_proto::validate_item_count(
            "claimed_key_packages",
            claimed_key_packages.len(),
            MAX_STAGED_WELCOMES_PER_COMMIT,
        )?;

        let mut seen_devices = BTreeSet::<DeviceRef>::new();
        let mut seen_key_packages = BTreeSet::<KeyPackageId>::new();
        let mut seen_welcomes = BTreeSet::<WelcomeId>::new();
        for claimed_key_package in claimed_key_packages {
            claimed_key_package
                .owner
                .validate_limits()
                .map_err(ClientError::from)?;
            validate_string_bytes(
                "key_package_id",
                &claimed_key_package.key_package_id,
                MAX_OBJECT_ID_BYTES,
            )?;
            validate_string_bytes(
                "key_package_ref",
                &claimed_key_package.key_package_ref,
                MAX_OBJECT_ID_BYTES,
            )?;
            validate_string_bytes(
                "key_package_hash",
                &claimed_key_package.key_package_hash,
                MAX_OBJECT_ID_BYTES,
            )?;
            if !seen_devices.insert(claimed_key_package.owner.clone()) {
                return Err(ClientError::DuplicateInviteDevice(
                    claimed_key_package.owner.clone(),
                ));
            }
            if !seen_key_packages.insert(claimed_key_package.key_package_id.clone()) {
                return Err(ClientError::DuplicateInviteKeyPackage(
                    claimed_key_package.key_package_id.clone(),
                ));
            }
        }
        for welcome_id in welcome_ids {
            validate_string_bytes("welcome_id", welcome_id, MAX_OBJECT_ID_BYTES)?;
            if !seen_welcomes.insert(welcome_id.clone()) {
                return Err(ClientError::DuplicateInviteWelcome(welcome_id.clone()));
            }
        }

        let mut key_packages = Vec::with_capacity(claimed_key_packages.len());
        for claimed_key_package in claimed_key_packages {
            key_packages.push(verified_key_package_from_claim(
                &self.provider,
                claimed_key_package,
                self.now_unix_seconds,
            )?);
        }
        let provider = &self.provider;
        let signer = &self.signer;
        let sender = self.device_ref.clone();
        let group = self
            .groups
            .get_mut(room_id)
            .ok_or_else(|| ClientError::GroupNotFound(room_id.to_string()))?;
        if group.pending_commit().is_some() {
            return Err(ClientError::PendingCommitExists(room_id.to_string()));
        }

        let (commit_message, welcome_message, _group_info) = group
            .add_members(provider, signer, &key_packages)
            .map_err(|_| ClientError::AddMember)?;
        let commit_payload = mls_message_out_bytes(commit_message)?;
        let welcome_payload = mls_message_out_bytes(welcome_message)?;
        let ratchet_tree = group
            .pending_commit()
            .ok_or_else(|| ClientError::MissingPendingCommit(room_id.to_string()))?
            .export_ratchet_tree(provider.crypto(), group.export_ratchet_tree())
            .map_err(|_| ClientError::ExportPendingRatchetTree)?
            .ok_or(ClientError::ExportPendingRatchetTree)?;
        let ratchet_tree_payload = ratchet_tree
            .tls_serialize_detached()
            .map_err(|_| ClientError::SerializeRatchetTree)?;
        let expected_epoch = group.epoch().as_u64();
        let mls_group_id = mls_group_id_string(group.group_id())?;
        let commit_envelope = envelope(
            room_id.to_string(),
            mls_group_id,
            sender.clone(),
            expected_epoch,
            LogEntryKind::Commit,
            commit_payload,
        );
        let commit_message_id = commit_envelope
            .message_id()
            .map_err(ClientError::EnvelopeMessageId)?;
        let mut adds = Vec::with_capacity(claimed_key_packages.len());
        let mut staged_welcomes = Vec::with_capacity(claimed_key_packages.len());
        for (claimed_key_package, welcome_id) in claimed_key_packages.iter().zip(welcome_ids) {
            adds.push(MembershipAddV1 {
                device: claimed_key_package.owner.clone(),
                key_package_id: claimed_key_package.key_package_id.clone(),
                key_package_ref: claimed_key_package.key_package_ref.clone(),
                key_package_hash: claimed_key_package.key_package_hash.clone(),
                welcome_id: welcome_id.clone(),
            });
            staged_welcomes.push(StagedWelcomeV1 {
                welcome_id: welcome_id.clone(),
                welcome_payload: welcome_payload.clone(),
                ratchet_tree_payload: ratchet_tree_payload.clone(),
            });
        }
        debug_assert_eq!(adds.len(), staged_welcomes.len());
        debug_assert!(!adds.is_empty());
        let request = SubmitCommitRequest {
            room_id: room_id.to_string(),
            sender,
            expected_epoch,
            envelope: commit_envelope,
            membership_delta: MembershipDeltaV1 {
                base_epoch: expected_epoch,
                post_commit_epoch: expected_epoch + 1,
                commit_message_id: commit_message_id.clone(),
                adds,
                removes: vec![],
            },
            staged_welcomes,
            idempotency_key,
        };
        request.validate_limits()?;
        Ok(PreparedCommit {
            request,
            message_id: commit_message_id,
        })
    }

    pub fn prepare_remove_member_commit(
        &mut self,
        room_id: &str,
        removed_device: &DeviceRef,
        idempotency_key: impl Into<String>,
    ) -> Result<PreparedCommit, ClientError> {
        validate_room_id(room_id)?;
        removed_device.validate_limits()?;
        if removed_device == &self.device_ref {
            return Err(ClientError::CannotRemoveSelf);
        }
        let idempotency_key = idempotency_key.into();
        validate_idempotency_key(&idempotency_key)?;

        let provider = &self.provider;
        let signer = &self.signer;
        let sender = self.device_ref.clone();
        let now_unix_seconds = self.now_unix_seconds;
        let group = self
            .groups
            .get_mut(room_id)
            .ok_or_else(|| ClientError::GroupNotFound(room_id.to_string()))?;
        if group.pending_commit().is_some() {
            return Err(ClientError::PendingCommitExists(room_id.to_string()));
        }
        let removed_leaf_index =
            verified_member_leaf_index(group, removed_device, now_unix_seconds)?;

        let (commit_message, welcome_message, _group_info) = group
            .remove_members(provider, signer, &[removed_leaf_index])
            .map_err(|_| ClientError::RemoveMember)?;
        if welcome_message.is_some() {
            return Err(ClientError::UnexpectedWelcomeForNonAddCommit);
        }
        let commit_payload = mls_message_out_bytes(commit_message)?;
        let expected_epoch = group.epoch().as_u64();
        let mls_group_id = mls_group_id_string(group.group_id())?;
        let commit_envelope = envelope(
            room_id.to_string(),
            mls_group_id,
            sender.clone(),
            expected_epoch,
            LogEntryKind::Commit,
            commit_payload,
        );
        let commit_message_id = commit_envelope
            .message_id()
            .map_err(ClientError::EnvelopeMessageId)?;
        let request = SubmitCommitRequest {
            room_id: room_id.to_string(),
            sender,
            expected_epoch,
            envelope: commit_envelope,
            membership_delta: MembershipDeltaV1 {
                base_epoch: expected_epoch,
                post_commit_epoch: post_commit_epoch(expected_epoch)?,
                commit_message_id: commit_message_id.clone(),
                adds: vec![],
                removes: vec![MembershipRemoveV1 {
                    device: removed_device.clone(),
                    removed_leaf_index: removed_leaf_index.u32(),
                }],
            },
            staged_welcomes: vec![],
            idempotency_key,
        };
        request.validate_limits()?;
        Ok(PreparedCommit {
            request,
            message_id: commit_message_id,
        })
    }

    pub fn prepare_self_update_commit(
        &mut self,
        room_id: &str,
        idempotency_key: impl Into<String>,
    ) -> Result<PreparedCommit, ClientError> {
        validate_room_id(room_id)?;
        let idempotency_key = idempotency_key.into();
        validate_idempotency_key(&idempotency_key)?;

        let provider = &self.provider;
        let signer = &self.signer;
        let sender = self.device_ref.clone();
        let group = self
            .groups
            .get_mut(room_id)
            .ok_or_else(|| ClientError::GroupNotFound(room_id.to_string()))?;
        if group.pending_commit().is_some() {
            return Err(ClientError::PendingCommitExists(room_id.to_string()));
        }

        let (commit_message, welcome_message, _group_info) = group
            .self_update(provider, signer, LeafNodeParameters::default())
            .map_err(|_| ClientError::SelfUpdate)?
            .into_messages();
        if welcome_message.is_some() {
            return Err(ClientError::UnexpectedWelcomeForNonAddCommit);
        }
        let commit_payload = mls_message_out_bytes(commit_message)?;
        let expected_epoch = group.epoch().as_u64();
        let mls_group_id = mls_group_id_string(group.group_id())?;
        let commit_envelope = envelope(
            room_id.to_string(),
            mls_group_id,
            sender.clone(),
            expected_epoch,
            LogEntryKind::Commit,
            commit_payload,
        );
        let commit_message_id = commit_envelope
            .message_id()
            .map_err(ClientError::EnvelopeMessageId)?;
        let request = SubmitCommitRequest {
            room_id: room_id.to_string(),
            sender,
            expected_epoch,
            envelope: commit_envelope,
            membership_delta: MembershipDeltaV1 {
                base_epoch: expected_epoch,
                post_commit_epoch: post_commit_epoch(expected_epoch)?,
                commit_message_id: commit_message_id.clone(),
                adds: vec![],
                removes: vec![],
            },
            staged_welcomes: vec![],
            idempotency_key,
        };
        request.validate_limits()?;
        Ok(PreparedCommit {
            request,
            message_id: commit_message_id,
        })
    }

    pub fn merge_pending_commit_from_log(
        &mut self,
        room_id: &str,
        entries: &[RoomLogEntry],
        message_id: &str,
    ) -> Result<(), ClientError> {
        let sender = self.device_ref.clone();
        let observed = entries.iter().any(|entry| {
            entry.message_id == message_id
                && entry.kind == LogEntryKind::Commit
                && entry.sender == sender
        });
        if !observed {
            return Err(ClientError::PendingCommitNotObserved(
                message_id.to_string(),
            ));
        }

        let provider = &self.provider;
        let group = self
            .groups
            .get_mut(room_id)
            .ok_or_else(|| ClientError::GroupNotFound(room_id.to_string()))?;
        if group.pending_commit().is_none() {
            return Err(ClientError::MissingPendingCommit(room_id.to_string()));
        }
        group
            .merge_pending_commit(provider)
            .map_err(|_| ClientError::MergePendingCommit)?;
        debug_assert!(group.pending_commit().is_none());
        Ok(())
    }

    pub fn apply_log_entry(
        &mut self,
        room_id: &str,
        entry: &RoomLogEntry,
    ) -> Result<AppliedLogEntry, ClientError> {
        match entry.kind {
            LogEntryKind::Application => {
                let decrypted = self.decrypt_application_entry(room_id, entry)?;
                Ok(AppliedLogEntry::Application {
                    plaintext: decrypted.plaintext,
                    sender: decrypted.sender,
                })
            }
            LogEntryKind::Commit => {
                self.apply_commit_entry(room_id, entry)?;
                Ok(AppliedLogEntry::Commit {
                    sender: entry.sender.clone(),
                    epoch: post_commit_epoch(entry.epoch)?,
                })
            }
            LogEntryKind::Proposal => Err(ClientError::UnsupportedLogEntryKind(entry.kind)),
        }
    }

    pub fn apply_commit_entry(
        &mut self,
        room_id: &str,
        entry: &RoomLogEntry,
    ) -> Result<(), ClientError> {
        validate_log_entry_shape(room_id, entry, LogEntryKind::Commit)?;
        let post_commit_epoch = post_commit_epoch(entry.epoch)?;
        let own_device_ref = self.device_ref.clone();
        let now_unix_seconds = self.now_unix_seconds;
        let provider = &self.provider;
        let group = self
            .groups
            .get_mut(room_id)
            .ok_or_else(|| ClientError::GroupNotFound(room_id.to_string()))?;
        let current_epoch = group.epoch().as_u64();
        if current_epoch != entry.epoch {
            return Err(ClientError::UnexpectedCommitEpoch {
                room_id: room_id.to_string(),
                current_epoch,
                entry_epoch: entry.epoch,
            });
        }

        if entry.sender == own_device_ref {
            if group.pending_commit().is_none() {
                return Err(ClientError::OwnCommitWithoutPendingState(
                    entry.message_id.clone(),
                ));
            }
            group
                .merge_pending_commit(provider)
                .map_err(|_| ClientError::MergePendingCommit)?;
            if group.epoch().as_u64() != post_commit_epoch {
                return Err(ClientError::UnexpectedPostCommitEpoch {
                    room_id: room_id.to_string(),
                    expected_epoch: post_commit_epoch,
                    actual_epoch: group.epoch().as_u64(),
                });
            }
            debug_assert!(group.pending_commit().is_none());
            return Ok(());
        }

        if group.pending_commit().is_some() {
            group
                .clear_pending_commit(provider.storage())
                .map_err(|_| ClientError::ClearPendingCommit)?;
        }

        let processed = group
            .process_message(
                provider,
                protocol_message_from_bytes(&entry.envelope.payload)?,
            )
            .map_err(|_| ClientError::ProcessMessage)?;
        let ProcessedMessageContent::StagedCommitMessage(staged_commit) = processed.into_content()
        else {
            return Err(ClientError::UnexpectedMessage);
        };
        verify_staged_commit_credentials(now_unix_seconds, &staged_commit)?;
        group
            .merge_staged_commit(provider, *staged_commit)
            .map_err(|_| ClientError::MergeStagedCommit)?;
        if group.epoch().as_u64() != post_commit_epoch {
            return Err(ClientError::UnexpectedPostCommitEpoch {
                room_id: room_id.to_string(),
                expected_epoch: post_commit_epoch,
                actual_epoch: group.epoch().as_u64(),
            });
        }
        debug_assert!(group.pending_commit().is_none());
        Ok(())
    }

    pub fn activate_welcome(
        &mut self,
        room_id: impl Into<RoomId>,
        welcome_payload: &[u8],
        ratchet_tree_payload: &[u8],
    ) -> Result<(), ClientError> {
        let room_id = room_id.into();
        if self.groups.contains_key(&room_id) {
            return Err(ClientError::GroupAlreadyExists(room_id));
        }

        let group_config = openmls_group_config();
        let group = StagedWelcome::new_from_welcome(
            &self.provider,
            group_config.join_config(),
            welcome_from_bytes(welcome_payload)?,
            Some(ratchet_tree_from_bytes(ratchet_tree_payload)?),
        )
        .map_err(|_| ClientError::StageWelcome)?
        .into_group(&self.provider)
        .map_err(|_| ClientError::ActivateWelcome)?;
        self.verify_member_in_group(&group, &self.device_ref)?;
        self.groups.insert(room_id.clone(), group);
        self.room_cursors.insert(room_id, 0);
        Ok(())
    }

    pub fn last_applied_seq(&self, room_id: &str) -> Result<u64, ClientError> {
        validate_room_id(room_id)?;
        self.group(room_id)?;
        Ok(*self.room_cursors.get(room_id).unwrap_or(&0))
    }

    pub fn room_sync_cursors(&self) -> Vec<RoomSyncCursor> {
        self.groups
            .keys()
            .map(|room_id| RoomSyncCursor {
                room_id: room_id.clone(),
                after_seq: *self.room_cursors.get(room_id).unwrap_or(&0),
                server_url: self.room_server_urls.get(room_id).cloned(),
            })
            .collect()
    }

    /// Record which server hosts a room's ordered log. `None` means the
    /// home server. Joined-via-invite rooms record the address from their
    /// invite code.
    pub fn set_room_server_url(
        &mut self,
        room_id: &str,
        server_url: Option<String>,
    ) -> Result<(), ClientError> {
        validate_room_id(room_id)?;
        self.group(room_id)?;
        if let Some(server_url) = &server_url {
            validate_string_bytes("room.server_url", server_url, MAX_ROOM_SERVER_URL_BYTES)?;
        }
        match server_url {
            Some(server_url) => {
                self.room_server_urls.insert(room_id.to_owned(), server_url);
            }
            None => {
                self.room_server_urls.remove(room_id);
            }
        }
        Ok(())
    }

    pub fn room_server_url(&self, room_id: &str) -> Option<&str> {
        self.room_server_urls.get(room_id).map(String::as_str)
    }

    pub fn pending_welcome_count(&self) -> usize {
        self.pending_welcomes.len()
    }

    pub fn pending_welcome_ack_count(&self) -> usize {
        self.pending_welcome_acks.len()
    }

    pub fn pending_key_package_upload_count(&self) -> usize {
        self.pending_key_package_uploads.len()
    }

    fn store_pending_welcome(&mut self, welcome: &WelcomeRecord) -> Result<(), ClientError> {
        if welcome.recipient != self.device_ref {
            return Err(ClientError::PendingWelcomeRecipientMismatch);
        }
        let pending = PendingWelcomeState::from_welcome_record(welcome)?;
        if self.pending_welcomes.contains_key(&pending.welcome_id) {
            return Err(ClientError::DuplicatePendingWelcome(pending.welcome_id));
        }
        self.pending_welcomes
            .insert(pending.welcome_id.clone(), pending);
        debug_assert!(!self.pending_welcomes.is_empty());
        Ok(())
    }

    fn pending_welcome_ids(&self) -> Vec<WelcomeId> {
        self.pending_welcomes.keys().cloned().collect()
    }

    fn pending_welcome(&self, welcome_id: &str) -> Result<PendingWelcomeState, ClientError> {
        validate_string_bytes("welcome_id", welcome_id, MAX_OBJECT_ID_BYTES)?;
        self.pending_welcomes
            .get(welcome_id)
            .cloned()
            .ok_or_else(|| ClientError::PendingWelcomeNotFound(welcome_id.to_string()))
    }

    fn clear_pending_welcome(&mut self, welcome_id: &str) -> Result<(), ClientError> {
        validate_string_bytes("welcome_id", welcome_id, MAX_OBJECT_ID_BYTES)?;
        if self.pending_welcomes.remove(welcome_id).is_none() {
            return Err(ClientError::PendingWelcomeNotFound(welcome_id.to_string()));
        }
        debug_assert!(!self.pending_welcomes.contains_key(welcome_id));
        Ok(())
    }

    fn activate_pending_welcome(
        &mut self,
        welcome_id: &str,
    ) -> Result<PendingWelcomeAckState, ClientError> {
        let pending = self.pending_welcome(welcome_id)?;
        self.activate_welcome(
            pending.room_id.clone(),
            &pending.welcome_payload,
            &pending.ratchet_tree_payload,
        )?;
        self.set_last_applied_seq(&pending.room_id, pending.commit_seq)?;
        self.clear_pending_welcome(welcome_id)?;
        let ack = PendingWelcomeAckState::from_pending_welcome(&pending)?;
        self.store_pending_welcome_ack(ack.clone())?;
        debug_assert!(!self.pending_welcomes.contains_key(welcome_id));
        debug_assert!(self.pending_welcome_acks.contains_key(welcome_id));
        Ok(ack)
    }

    fn store_pending_welcome_ack(
        &mut self,
        ack: PendingWelcomeAckState,
    ) -> Result<(), ClientError> {
        ack.validate_limits()?;
        if self.pending_welcomes.contains_key(&ack.welcome_id) {
            return Err(ClientError::PendingWelcomeAlsoNeedsAck(ack.welcome_id));
        }
        if self
            .pending_welcome_acks
            .insert(ack.welcome_id.clone(), ack.clone())
            .is_some()
        {
            return Err(ClientError::DuplicatePendingWelcomeAck(ack.welcome_id));
        }
        debug_assert!(self.pending_welcome_acks.contains_key(&ack.welcome_id));
        Ok(())
    }

    fn pending_welcome_acks(&self) -> Vec<PendingWelcomeAckState> {
        self.pending_welcome_acks.values().cloned().collect()
    }

    fn clear_pending_welcome_ack(&mut self, welcome_id: &str) -> Result<(), ClientError> {
        validate_string_bytes("welcome_ack.welcome_id", welcome_id, MAX_OBJECT_ID_BYTES)?;
        if self.pending_welcome_acks.remove(welcome_id).is_none() {
            return Err(ClientError::PendingWelcomeAckNotFound(
                welcome_id.to_string(),
            ));
        }
        debug_assert!(!self.pending_welcome_acks.contains_key(welcome_id));
        Ok(())
    }

    fn store_pending_key_package_upload(
        &mut self,
        request: UploadKeyPackageRequest,
    ) -> Result<(), ClientError> {
        request.validate_limits()?;
        if request.owner != self.device_ref {
            return Err(ClientError::PendingKeyPackageUploadOwnerMismatch {
                expected: self.device_ref.clone(),
                actual: request.owner,
            });
        }
        if self
            .pending_key_package_uploads
            .insert(request.key_package_id.clone(), request.clone())
            .is_some()
        {
            return Err(ClientError::DuplicatePendingKeyPackageUpload(
                request.key_package_id,
            ));
        }
        debug_assert!(
            self.pending_key_package_uploads
                .contains_key(&request.key_package_id)
        );
        Ok(())
    }

    fn pending_key_package_uploads(&self) -> Vec<UploadKeyPackageRequest> {
        self.pending_key_package_uploads.values().cloned().collect()
    }

    fn clear_pending_key_package_upload(
        &mut self,
        key_package_id: &str,
    ) -> Result<(), ClientError> {
        validate_string_bytes(
            "pending_key_package_upload.key_package_id",
            key_package_id,
            MAX_OBJECT_ID_BYTES,
        )?;
        if self
            .pending_key_package_uploads
            .remove(key_package_id)
            .is_none()
        {
            return Err(ClientError::PendingKeyPackageUploadNotFound(
                key_package_id.to_string(),
            ));
        }
        debug_assert!(
            !self
                .pending_key_package_uploads
                .contains_key(key_package_id)
        );
        Ok(())
    }

    pub fn start_link_fanout(
        &mut self,
        fanout_id: impl Into<String>,
        target_device: DeviceRef,
    ) -> Result<(), ClientError> {
        let fanout_id = fanout_id.into();
        validate_string_bytes("link_fanout.fanout_id", &fanout_id, MAX_OBJECT_ID_BYTES)?;
        target_device.validate_limits()?;
        if target_device.account_id != self.device_ref.account_id {
            return Err(ClientError::LinkFanoutAccountMismatch {
                expected: self.device_ref.account_id.clone(),
                actual: target_device.account_id,
            });
        }
        if target_device == self.device_ref {
            return Err(ClientError::LinkFanoutCannotTargetSelf);
        }
        if self.link_fanouts.contains_key(&fanout_id) {
            return Err(ClientError::DuplicateLinkFanout(fanout_id));
        }
        let fanout = LinkFanoutState {
            fanout_id: fanout_id.clone(),
            target_device,
            after_room_id: None,
            discovery_complete: false,
            rooms: Vec::new(),
        };
        fanout.validate_limits()?;
        self.link_fanouts.insert(fanout_id, fanout);
        debug_assert!(!self.link_fanouts.is_empty());
        Ok(())
    }

    pub fn queue_link_fanout_page(
        &mut self,
        fanout_id: &str,
        page: &ListAccountRoomsPage,
        plans: &[LinkFanoutRoomPlan],
    ) -> Result<(), ClientError> {
        page.validate_limits().map_err(ClientError::from)?;
        validate_item_count("link_fanout.page_plans", plans.len(), MAX_LINK_FANOUT_ROOMS)?;
        let target_device = self.link_fanout(fanout_id)?.target_device.clone();
        let mut plans_by_room = BTreeMap::<RoomId, LinkFanoutRoomPlan>::new();
        for plan in plans {
            plan.validate_limits()?;
            if plans_by_room
                .insert(plan.room_id.clone(), plan.clone())
                .is_some()
            {
                return Err(ClientError::DuplicateLinkFanoutRoom(plan.room_id.clone()));
            }
        }

        let mut queued = Vec::new();
        for room in &page.rooms {
            let target_already_current = room
                .devices
                .iter()
                .any(|device| device.device == target_device);
            if target_already_current {
                continue;
            }
            self.group(&room.room_id)?;
            let plan = plans_by_room
                .remove(&room.room_id)
                .ok_or_else(|| ClientError::MissingLinkFanoutRoomPlan(room.room_id.clone()))?;
            if plan.key_package_id.is_empty()
                || plan.welcome_id.is_empty()
                || plan.idempotency_key.is_empty()
            {
                return Err(ClientError::MissingLinkFanoutRoomPlan(plan.room_id));
            }
            queued.push(LinkFanoutRoomState {
                plan,
                claimed_key_package: None,
                status: LinkFanoutRoomStatus::Pending,
            });
        }
        if let Some((extra_room_id, _)) = plans_by_room.into_iter().next() {
            return Err(ClientError::UnexpectedLinkFanoutRoomPlan(extra_room_id));
        }

        let fanout = self.link_fanout_mut(fanout_id)?;
        let mut seen_rooms = fanout
            .rooms
            .iter()
            .map(|room| room.plan.room_id.clone())
            .collect::<BTreeSet<_>>();
        for room in queued {
            if !seen_rooms.insert(room.plan.room_id.clone()) {
                return Err(ClientError::DuplicateLinkFanoutRoom(
                    room.plan.room_id.clone(),
                ));
            }
            fanout.rooms.push(room);
        }
        fanout.after_room_id = page.next_after_room_id.clone();
        fanout.discovery_complete = !page.has_more;
        fanout.validate_limits()?;
        Ok(())
    }

    pub fn queue_claimed_link_fanout_page(
        &mut self,
        fanout_id: &str,
        page: &ListAccountRoomsPage,
        claimed_key_packages: &[ClaimKeyPackageResult],
    ) -> Result<(), ClientError> {
        page.validate_limits().map_err(ClientError::from)?;
        validate_item_count(
            "link_fanout.claimed_key_packages",
            claimed_key_packages.len(),
            MAX_LINK_FANOUT_ROOMS,
        )?;
        let fanout = self.link_fanout(fanout_id)?;
        let target_device = fanout.target_device.clone();
        let mut claims = claimed_key_packages.iter();
        let mut queued = Vec::new();
        for room in &page.rooms {
            let target_already_current = room
                .devices
                .iter()
                .any(|device| device.device == target_device);
            if target_already_current {
                continue;
            }
            self.group(&room.room_id)?;
            let Some(claimed_key_package) = claims.next() else {
                return Err(ClientError::MissingLinkFanoutClaim(room.room_id.clone()));
            };
            validate_claimed_key_package(claimed_key_package)?;
            if claimed_key_package.owner != target_device {
                return Err(ClientError::LinkFanoutClaimTargetMismatch {
                    expected: target_device.clone(),
                    actual: claimed_key_package.owner.clone(),
                });
            }
            let plan = LinkFanoutRoomPlan {
                room_id: room.room_id.clone(),
                key_package_id: claimed_key_package.key_package_id.clone(),
                welcome_id: link_fanout_derived_id(
                    "welcome",
                    fanout_id,
                    &room.room_id,
                    &claimed_key_package.key_package_id,
                )?,
                idempotency_key: link_fanout_derived_id(
                    "link",
                    fanout_id,
                    &room.room_id,
                    &claimed_key_package.key_package_id,
                )?,
            };
            plan.validate_limits()?;
            queued.push(LinkFanoutRoomState {
                plan,
                claimed_key_package: Some(claimed_key_package.clone()),
                status: LinkFanoutRoomStatus::Pending,
            });
        }
        if let Some(extra) = claims.next() {
            return Err(ClientError::UnexpectedLinkFanoutClaim(
                extra.key_package_id.clone(),
            ));
        }

        let fanout = self.link_fanout_mut(fanout_id)?;
        let mut seen_rooms = fanout
            .rooms
            .iter()
            .map(|room| room.plan.room_id.clone())
            .collect::<BTreeSet<_>>();
        for room in queued {
            if !seen_rooms.insert(room.plan.room_id.clone()) {
                return Err(ClientError::DuplicateLinkFanoutRoom(
                    room.plan.room_id.clone(),
                ));
            }
            fanout.rooms.push(room);
        }
        fanout.after_room_id = page.next_after_room_id.clone();
        fanout.discovery_complete = !page.has_more;
        fanout.validate_limits()?;
        Ok(())
    }

    pub fn prepare_link_fanout_room_commit(
        &mut self,
        fanout_id: &str,
        room_id: &str,
        claimed_key_package: &ClaimKeyPackageResult,
    ) -> Result<PreparedCommit, ClientError> {
        validate_room_id(room_id)?;
        let (target_device, plan) = {
            let fanout = self.link_fanout(fanout_id)?;
            let room = fanout
                .rooms
                .iter()
                .find(|room| room.plan.room_id == room_id)
                .ok_or_else(|| ClientError::LinkFanoutRoomNotFound(room_id.to_string()))?;
            match &room.status {
                LinkFanoutRoomStatus::Pending => {}
                LinkFanoutRoomStatus::Prepared { .. } if !self.has_pending_commit(room_id)? => {}
                _ => return Err(ClientError::LinkFanoutRoomNotPending(room_id.to_string())),
            }
            (fanout.target_device.clone(), room.plan.clone())
        };
        if claimed_key_package.owner != target_device {
            return Err(ClientError::LinkFanoutClaimTargetMismatch {
                expected: target_device,
                actual: claimed_key_package.owner.clone(),
            });
        }
        if claimed_key_package.key_package_id != plan.key_package_id {
            return Err(ClientError::LinkFanoutClaimPackageMismatch {
                expected: plan.key_package_id,
                actual: claimed_key_package.key_package_id.clone(),
            });
        }
        let prepared =
            self.prepare_link_fanout_room_commit_inner(fanout_id, room_id, claimed_key_package)?;
        Ok(prepared)
    }

    pub fn prepare_claimed_link_fanout_room_commit(
        &mut self,
        fanout_id: &str,
        room_id: &str,
    ) -> Result<PreparedCommit, ClientError> {
        let claimed_key_package = self
            .link_fanout_room(fanout_id, room_id)?
            .claimed_key_package
            .clone()
            .ok_or_else(|| ClientError::MissingLinkFanoutClaim(room_id.to_string()))?;
        self.prepare_link_fanout_room_commit_inner(fanout_id, room_id, &claimed_key_package)
    }

    fn prepare_link_fanout_room_commit_inner(
        &mut self,
        fanout_id: &str,
        room_id: &str,
        claimed_key_package: &ClaimKeyPackageResult,
    ) -> Result<PreparedCommit, ClientError> {
        validate_room_id(room_id)?;
        let (target_device, plan) = {
            let fanout = self.link_fanout(fanout_id)?;
            let room = fanout
                .rooms
                .iter()
                .find(|room| room.plan.room_id == room_id)
                .ok_or_else(|| ClientError::LinkFanoutRoomNotFound(room_id.to_string()))?;
            match &room.status {
                LinkFanoutRoomStatus::Pending => {}
                LinkFanoutRoomStatus::Prepared { .. } if !self.has_pending_commit(room_id)? => {}
                _ => return Err(ClientError::LinkFanoutRoomNotPending(room_id.to_string())),
            }
            (fanout.target_device.clone(), room.plan.clone())
        };
        if claimed_key_package.owner != target_device {
            return Err(ClientError::LinkFanoutClaimTargetMismatch {
                expected: target_device,
                actual: claimed_key_package.owner.clone(),
            });
        }
        if claimed_key_package.key_package_id != plan.key_package_id {
            return Err(ClientError::LinkFanoutClaimPackageMismatch {
                expected: plan.key_package_id,
                actual: claimed_key_package.key_package_id.clone(),
            });
        }
        let prepared = self.prepare_add_member_commit(
            &plan.room_id,
            claimed_key_package,
            plan.welcome_id.clone(),
            plan.idempotency_key.clone(),
        )?;
        prepared.validate_limits()?;
        let fanout = self.link_fanout_mut(fanout_id)?;
        let room = fanout
            .rooms
            .iter_mut()
            .find(|room| room.plan.room_id == room_id)
            .ok_or_else(|| ClientError::LinkFanoutRoomNotFound(room_id.to_string()))?;
        room.status = LinkFanoutRoomStatus::Prepared {
            prepared: Box::new(prepared.clone()),
        };
        fanout.validate_limits()?;
        Ok(prepared)
    }

    pub fn prepared_link_fanout_commit(
        &self,
        fanout_id: &str,
        room_id: &str,
    ) -> Result<PreparedCommit, ClientError> {
        let room = self.link_fanout_room(fanout_id, room_id)?;
        match &room.status {
            LinkFanoutRoomStatus::Prepared { prepared } => Ok((**prepared).clone()),
            _ => Err(ClientError::LinkFanoutRoomNotPrepared(room_id.to_string())),
        }
    }

    pub fn complete_link_fanout_room_from_log(
        &mut self,
        fanout_id: &str,
        room_id: &str,
        entry: &RoomLogEntry,
    ) -> Result<AppliedLogEntry, ClientError> {
        let prepared = self.prepared_link_fanout_commit(fanout_id, room_id)?;
        if entry.message_id != prepared.message_id {
            return Err(ClientError::LinkFanoutPreparedCommitMismatch {
                expected: prepared.message_id,
                actual: entry.message_id.clone(),
            });
        }
        let applied = self.apply_log_entry(room_id, entry)?;
        let fanout = self.link_fanout_mut(fanout_id)?;
        let room = fanout
            .rooms
            .iter_mut()
            .find(|room| room.plan.room_id == room_id)
            .ok_or_else(|| ClientError::LinkFanoutRoomNotFound(room_id.to_string()))?;
        room.claimed_key_package = None;
        room.status = LinkFanoutRoomStatus::Done {
            accepted_seq: entry.seq,
        };
        fanout.validate_limits()?;
        Ok(applied)
    }

    pub fn link_fanout_room_status(
        &self,
        fanout_id: &str,
        room_id: &str,
    ) -> Result<LinkFanoutRoomStatus, ClientError> {
        Ok(self.link_fanout_room(fanout_id, room_id)?.status.clone())
    }

    pub fn link_fanout_room_count(&self, fanout_id: &str) -> Result<usize, ClientError> {
        Ok(self.link_fanout(fanout_id)?.rooms.len())
    }

    fn link_fanout_target_device(&self, fanout_id: &str) -> Result<DeviceRef, ClientError> {
        Ok(self.link_fanout(fanout_id)?.target_device.clone())
    }

    fn link_fanout_after_room_id(&self, fanout_id: &str) -> Result<Option<RoomId>, ClientError> {
        Ok(self.link_fanout(fanout_id)?.after_room_id.clone())
    }

    fn link_fanout_discovery_complete(&self, fanout_id: &str) -> Result<bool, ClientError> {
        Ok(self.link_fanout(fanout_id)?.discovery_complete)
    }

    fn pending_link_fanout_room_ids(&self, fanout_id: &str) -> Result<Vec<RoomId>, ClientError> {
        Ok(self
            .link_fanout(fanout_id)?
            .rooms
            .iter()
            .filter(|room| matches!(room.status, LinkFanoutRoomStatus::Pending))
            .map(|room| room.plan.room_id.clone())
            .collect())
    }

    fn prepared_link_fanout_room_ids(&self, fanout_id: &str) -> Result<Vec<RoomId>, ClientError> {
        Ok(self
            .link_fanout(fanout_id)?
            .rooms
            .iter()
            .filter(|room| matches!(room.status, LinkFanoutRoomStatus::Prepared { .. }))
            .map(|room| room.plan.room_id.clone())
            .collect())
    }

    pub fn link_fanout_is_complete(&self, fanout_id: &str) -> Result<bool, ClientError> {
        let fanout = self.link_fanout(fanout_id)?;
        Ok(fanout.discovery_complete
            && fanout
                .rooms
                .iter()
                .all(|room| matches!(room.status, LinkFanoutRoomStatus::Done { .. })))
    }

    fn link_fanout(&self, fanout_id: &str) -> Result<&LinkFanoutState, ClientError> {
        validate_string_bytes("link_fanout.fanout_id", fanout_id, MAX_OBJECT_ID_BYTES)?;
        self.link_fanouts
            .get(fanout_id)
            .ok_or_else(|| ClientError::LinkFanoutNotFound(fanout_id.to_string()))
    }

    fn link_fanout_mut(&mut self, fanout_id: &str) -> Result<&mut LinkFanoutState, ClientError> {
        validate_string_bytes("link_fanout.fanout_id", fanout_id, MAX_OBJECT_ID_BYTES)?;
        self.link_fanouts
            .get_mut(fanout_id)
            .ok_or_else(|| ClientError::LinkFanoutNotFound(fanout_id.to_string()))
    }

    fn link_fanout_room(
        &self,
        fanout_id: &str,
        room_id: &str,
    ) -> Result<&LinkFanoutRoomState, ClientError> {
        validate_room_id(room_id)?;
        self.link_fanout(fanout_id)?
            .rooms
            .iter()
            .find(|room| room.plan.room_id == room_id)
            .ok_or_else(|| ClientError::LinkFanoutRoomNotFound(room_id.to_string()))
    }

    pub fn create_application_request(
        &mut self,
        room_id: &str,
        plaintext: &[u8],
        idempotency_key: impl Into<String>,
    ) -> Result<AppendEventRequest, ClientError> {
        let own_device_ref = self.device_ref.clone();
        let provider = &self.provider;
        let signer = &self.signer;
        let group = self
            .groups
            .get_mut(room_id)
            .ok_or_else(|| ClientError::GroupNotFound(room_id.to_string()))?;
        if group.pending_commit().is_some() {
            return Err(ClientError::PendingCommitMustBeMerged(room_id.to_string()));
        }
        let app_message = group
            .create_message(provider, signer, plaintext)
            .map_err(|_| ClientError::CreateApplicationMessage)?;
        let payload = mls_message_out_bytes(app_message)?;
        let request = AppendEventRequest {
            room_id: room_id.to_string(),
            sender: own_device_ref.clone(),
            envelope: envelope(
                room_id.to_string(),
                mls_group_id_string(group.group_id())?,
                own_device_ref,
                group.epoch().as_u64(),
                LogEntryKind::Application,
                payload,
            ),
            idempotency_key: idempotency_key.into(),
        };
        request.validate_limits()?;
        Ok(request)
    }

    pub fn decrypt_application_entry(
        &mut self,
        room_id: &str,
        entry: &RoomLogEntry,
    ) -> Result<DecryptedApplicationEntry, ClientError> {
        validate_log_entry_shape(room_id, entry, LogEntryKind::Application)?;
        let provider = &self.provider;
        let group = self
            .groups
            .get_mut(room_id)
            .ok_or_else(|| ClientError::GroupNotFound(room_id.to_string()))?;
        let processed = group
            .process_message(
                provider,
                protocol_message_from_bytes(&entry.envelope.payload)?,
            )
            .map_err(|_| ClientError::ProcessMessage)?;
        let credential = FiniteDeviceCredentialV1::from_credential(processed.credential().clone())?;
        let sender = DeviceRef {
            account_id: hex_lower(credential.account_public_key().as_bytes()),
            device_id: credential.device_id().to_owned(),
        };
        let ProcessedMessageContent::ApplicationMessage(message) = processed.into_content() else {
            return Err(ClientError::UnexpectedMessage);
        };
        Ok(DecryptedApplicationEntry {
            plaintext: message.into_bytes(),
            sender,
        })
    }

    pub fn verified_member_count(
        &self,
        room_id: &str,
        device: &DeviceRef,
    ) -> Result<u32, ClientError> {
        let group = self.group(room_id)?;
        let expected_account_public_key = account_public_key_from_device_ref(device)?;
        let mut count = 0u32;
        for member in group.members() {
            let credential = FiniteDeviceCredentialV1::from_credential(member.credential)?;
            // Device ids are only unique per account (DeviceRef is the
            // tuple); two accounts may share a device-id string.
            if credential.account_public_key() == expected_account_public_key
                && credential.device_id() == device.device_id
            {
                credential.verify_expected(ExpectedDeviceCredential {
                    account_public_key: expected_account_public_key,
                    device_id: &device.device_id,
                    mls_leaf_signing_public_key: &member.signature_key,
                    now_unix_seconds: self.now_unix_seconds,
                })?;
                count += 1;
            }
        }
        Ok(count)
    }

    fn random_bytes<const N: usize>(&self) -> Result<[u8; N], ClientError> {
        self.provider
            .rand()
            .random_array()
            .map_err(|_| ClientError::Randomness)
    }

    /// Fresh high-entropy invite token (ADR 0006): lives only in the
    /// invite URL/QR, never alone on the server.
    pub fn generate_invite_token(&self) -> Result<Vec<u8>, ClientError> {
        Ok(self.random_bytes::<16>()?.to_vec())
    }

    pub fn generate_object_id(&self, prefix: &str) -> Result<String, ClientError> {
        let bytes = self.random_bytes::<8>()?;
        Ok(format!("{prefix}-{}", hex_lower(&bytes)))
    }

    /// Verify that some member of the room carries a valid credential for
    /// the given account. Joiners run this against the inviter account from
    /// the invite code after activating the Welcome: a hostile rendezvous
    /// server that admits the device to a different group fails here.
    pub fn verify_room_member_account(
        &self,
        room_id: &str,
        account_id: &str,
    ) -> Result<(), ClientError> {
        let group = self.group(room_id)?;
        let expected = NostrPublicKey::from_bytes(decode_lower_hex_32(account_id)?)
            .map_err(ClientError::from)?;
        for member in group.members() {
            let Ok(credential) = FiniteDeviceCredentialV1::from_credential(member.credential)
            else {
                continue;
            };
            if credential.account_public_key() != expected {
                continue;
            }
            credential.verify_expected(ExpectedDeviceCredential {
                account_public_key: expected,
                device_id: credential.device_id(),
                mls_leaf_signing_public_key: &member.signature_key,
                now_unix_seconds: self.now_unix_seconds,
            })?;
            return Ok(());
        }
        Err(ClientError::AccountNotInRoom {
            room_id: room_id.to_owned(),
            account_id: account_id.to_owned(),
        })
    }

    /// Encrypt an ephemeral activity payload (typing/working indicators)
    /// under a key derived from the room's MLS exporter secret at the
    /// current epoch. Activities are disposable: a payload from another
    /// epoch simply fails to decrypt and is skipped (forward secrecy means
    /// old epoch secrets are gone by design).
    pub fn encrypt_activity_payload(
        &self,
        room_id: &str,
        plaintext: &[u8],
    ) -> Result<Vec<u8>, ClientError> {
        let group = self.group(room_id)?;
        let epoch = group.epoch().as_u64();
        let key = group
            .export_secret(self.provider.crypto(), ACTIVITY_EXPORTER_LABEL, &[], 32)
            .map_err(|_| ClientError::ActivityCiphertext)?;
        let nonce: [u8; ACTIVITY_NONCE_BYTES] = self.random_bytes()?;
        let ciphertext = self
            .provider
            .crypto()
            .aead_encrypt(
                AeadType::Aes256Gcm,
                &key,
                plaintext,
                &nonce,
                room_id.as_bytes(),
            )
            .map_err(|_| ClientError::ActivityCiphertext)?;
        let mut out = Vec::with_capacity(8 + ACTIVITY_NONCE_BYTES + ciphertext.len());
        out.extend_from_slice(&epoch.to_be_bytes());
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&ciphertext);
        Ok(out)
    }

    pub fn decrypt_activity_payload(
        &self,
        room_id: &str,
        payload: &[u8],
    ) -> Result<Vec<u8>, ClientError> {
        let group = self.group(room_id)?;
        if payload.len() < 8 + ACTIVITY_NONCE_BYTES {
            return Err(ClientError::ActivityCiphertext);
        }
        let epoch = u64::from_be_bytes(payload[..8].try_into().expect("8 bytes"));
        if epoch != group.epoch().as_u64() {
            return Err(ClientError::ActivityEpochMismatch {
                payload_epoch: epoch,
                group_epoch: group.epoch().as_u64(),
            });
        }
        let key = group
            .export_secret(self.provider.crypto(), ACTIVITY_EXPORTER_LABEL, &[], 32)
            .map_err(|_| ClientError::ActivityCiphertext)?;
        let nonce = &payload[8..8 + ACTIVITY_NONCE_BYTES];
        self.provider
            .crypto()
            .aead_decrypt(
                AeadType::Aes256Gcm,
                &key,
                &payload[8 + ACTIVITY_NONCE_BYTES..],
                nonce,
                room_id.as_bytes(),
            )
            .map_err(|_| ClientError::ActivityCiphertext)
    }

    pub fn room_mls_group_id(&self, room_id: &str) -> Result<String, ClientError> {
        mls_group_id_string(self.group(room_id)?.group_id())
    }

    pub fn group_epoch(&self, room_id: &str) -> Result<u64, ClientError> {
        Ok(self.group(room_id)?.epoch().as_u64())
    }

    pub fn has_pending_commit(&self, room_id: &str) -> Result<bool, ClientError> {
        Ok(self.group(room_id)?.pending_commit().is_some())
    }

    fn verify_member_in_group(
        &self,
        group: &MlsGroup,
        device: &DeviceRef,
    ) -> Result<(), ClientError> {
        let expected_account_public_key = account_public_key_from_device_ref(device)?;
        let mut count = 0u32;
        for member in group.members() {
            let credential = FiniteDeviceCredentialV1::from_credential(member.credential)?;
            // Device ids are only unique per account (DeviceRef is the
            // tuple); two accounts may share a device-id string.
            if credential.account_public_key() == expected_account_public_key
                && credential.device_id() == device.device_id
            {
                credential.verify_expected(ExpectedDeviceCredential {
                    account_public_key: expected_account_public_key,
                    device_id: &device.device_id,
                    mls_leaf_signing_public_key: &member.signature_key,
                    now_unix_seconds: self.now_unix_seconds,
                })?;
                count += 1;
            }
        }
        if count == 1 {
            Ok(())
        } else {
            Err(ClientError::MemberCredentialMissing(device.clone()))
        }
    }

    fn group(&self, room_id: &str) -> Result<&MlsGroup, ClientError> {
        self.groups
            .get(room_id)
            .ok_or_else(|| ClientError::GroupNotFound(room_id.to_string()))
    }

    fn set_last_applied_seq(&mut self, room_id: &str, seq: u64) -> Result<(), ClientError> {
        validate_room_id(room_id)?;
        self.group(room_id)?;
        let current_seq = self.room_cursors.get(room_id).copied().unwrap_or(0);
        if seq < current_seq {
            return Err(ClientError::AppliedSeqRegression {
                room_id: room_id.to_string(),
                current_seq,
                attempted_seq: seq,
            });
        }
        self.room_cursors.insert(room_id.to_string(), seq);
        debug_assert!(self.room_cursors.contains_key(room_id));
        Ok(())
    }
}

impl FiniteChatDeviceState {
    fn validate_limits(&self) -> Result<(), ClientError> {
        self.device_ref.validate_limits()?;
        validate_bytes_non_empty("signer_public_key", self.signer_public_key.len())?;
        validate_bytes_len(
            "signer_public_key",
            self.signer_public_key.len(),
            MAX_CLIENT_SIGNER_PUBLIC_KEY_BYTES,
        )?;
        validate_bytes_non_empty("credential_identity", self.credential_identity.len())?;
        validate_bytes_len(
            "credential_identity",
            self.credential_identity.len(),
            MAX_CLIENT_CREDENTIAL_IDENTITY_BYTES,
        )?;
        validate_item_count("client_state.rooms", self.rooms.len(), MAX_PERSISTED_ROOMS)?;
        validate_item_count(
            "client_state.pending_welcomes",
            self.pending_welcomes.len(),
            MAX_PENDING_CLIENT_WELCOMES,
        )?;
        validate_item_count(
            "client_state.pending_welcome_acks",
            self.pending_welcome_acks.len(),
            MAX_PENDING_CLIENT_WELCOMES,
        )?;
        validate_item_count(
            "client_state.pending_key_package_uploads",
            self.pending_key_package_uploads.len(),
            MAX_PENDING_KEY_PACKAGE_UPLOADS,
        )?;
        validate_item_count(
            "client_state.link_fanouts",
            self.link_fanouts.len(),
            MAX_LINK_FANOUTS,
        )?;
        validate_item_count(
            "client_state.openmls_storage_records",
            self.openmls_storage_records.len(),
            MAX_OPENMLS_STORAGE_RECORDS,
        )?;
        for room in &self.rooms {
            room.validate_limits()?;
        }
        if self.openmls_storage_records.is_empty() {
            return Err(ClientError::MissingOpenMlsStorage);
        }
        let mut seen_storage_keys = BTreeSet::<Vec<u8>>::new();
        let mut seen_rooms = BTreeSet::<RoomId>::new();
        let mut seen_pending_welcomes = BTreeSet::<WelcomeId>::new();
        let mut seen_pending_welcome_acks = BTreeSet::<WelcomeId>::new();
        let mut seen_pending_key_package_uploads = BTreeSet::<KeyPackageId>::new();
        let mut seen_link_fanouts = BTreeSet::<String>::new();
        for room in &self.rooms {
            if !seen_rooms.insert(room.room_id.clone()) {
                return Err(ClientError::DuplicatePersistedRoom(room.room_id.clone()));
            }
        }
        for welcome in &self.pending_welcomes {
            welcome.validate_limits()?;
            if !seen_pending_welcomes.insert(welcome.welcome_id.clone()) {
                return Err(ClientError::DuplicatePendingWelcome(
                    welcome.welcome_id.clone(),
                ));
            }
        }
        for ack in &self.pending_welcome_acks {
            ack.validate_limits()?;
            if !seen_pending_welcome_acks.insert(ack.welcome_id.clone()) {
                return Err(ClientError::DuplicatePendingWelcomeAck(
                    ack.welcome_id.clone(),
                ));
            }
            if seen_pending_welcomes.contains(&ack.welcome_id) {
                return Err(ClientError::PendingWelcomeAlsoNeedsAck(
                    ack.welcome_id.clone(),
                ));
            }
            if !seen_rooms.contains(&ack.room_id) {
                return Err(ClientError::PendingWelcomeAckRoomMissing(
                    ack.room_id.clone(),
                ));
            }
        }
        for request in &self.pending_key_package_uploads {
            request.validate_limits()?;
            if request.owner != self.device_ref {
                return Err(ClientError::PendingKeyPackageUploadOwnerMismatch {
                    expected: self.device_ref.clone(),
                    actual: request.owner.clone(),
                });
            }
            if !seen_pending_key_package_uploads.insert(request.key_package_id.clone()) {
                return Err(ClientError::DuplicatePendingKeyPackageUpload(
                    request.key_package_id.clone(),
                ));
            }
        }
        for fanout in &self.link_fanouts {
            fanout.validate_limits()?;
            if fanout.target_device.account_id != self.device_ref.account_id {
                return Err(ClientError::LinkFanoutAccountMismatch {
                    expected: self.device_ref.account_id.clone(),
                    actual: fanout.target_device.account_id.clone(),
                });
            }
            if fanout.target_device == self.device_ref {
                return Err(ClientError::LinkFanoutCannotTargetSelf);
            }
            if !seen_link_fanouts.insert(fanout.fanout_id.clone()) {
                return Err(ClientError::DuplicateLinkFanout(fanout.fanout_id.clone()));
            }
        }
        for record in &self.openmls_storage_records {
            record.validate_limits()?;
            if !seen_storage_keys.insert(record.key.clone()) {
                return Err(ClientError::DuplicateOpenMlsStorageKey);
            }
        }
        debug_assert!(!self.signer_public_key.is_empty());
        debug_assert!(!self.credential_identity.is_empty());
        Ok(())
    }
}

impl PersistedRoomState {
    fn validate_limits(&self) -> Result<(), ClientError> {
        validate_room_id(&self.room_id)?;
        validate_mls_group_id(&self.mls_group_id)?;
        if let Some(server_url) = &self.server_url {
            validate_string_bytes("room.server_url", server_url, MAX_ROOM_SERVER_URL_BYTES)?;
        }
        Ok(())
    }
}

impl PendingWelcomeState {
    fn from_welcome_record(welcome: &WelcomeRecord) -> Result<Self, ClientError> {
        let pending = Self {
            welcome_id: welcome.welcome_id.clone(),
            room_id: welcome.room_id.clone(),
            commit_seq: welcome.commit_seq,
            welcome_payload: welcome.welcome_payload.clone(),
            ratchet_tree_payload: welcome.ratchet_tree_payload.clone(),
        };
        pending.validate_limits()?;
        Ok(pending)
    }

    fn validate_limits(&self) -> Result<(), ClientError> {
        validate_string_bytes("welcome_id", &self.welcome_id, MAX_OBJECT_ID_BYTES)?;
        validate_room_id(&self.room_id)?;
        validate_bytes_non_empty(
            "pending_welcome.welcome_payload",
            self.welcome_payload.len(),
        )?;
        validate_bytes_len(
            "pending_welcome.welcome_payload",
            self.welcome_payload.len(),
            MAX_WELCOME_PAYLOAD_BYTES,
        )?;
        validate_bytes_non_empty(
            "pending_welcome.ratchet_tree_payload",
            self.ratchet_tree_payload.len(),
        )?;
        validate_bytes_len(
            "pending_welcome.ratchet_tree_payload",
            self.ratchet_tree_payload.len(),
            MAX_RATCHET_TREE_PAYLOAD_BYTES,
        )?;
        Ok(())
    }
}

impl PendingWelcomeAckState {
    fn from_pending_welcome(pending: &PendingWelcomeState) -> Result<Self, ClientError> {
        let ack = Self {
            welcome_id: pending.welcome_id.clone(),
            room_id: pending.room_id.clone(),
            commit_seq: pending.commit_seq,
        };
        ack.validate_limits()?;
        Ok(ack)
    }

    fn validate_limits(&self) -> Result<(), ClientError> {
        validate_string_bytes(
            "welcome_ack.welcome_id",
            &self.welcome_id,
            MAX_OBJECT_ID_BYTES,
        )?;
        validate_room_id(&self.room_id)?;
        Ok(())
    }
}

impl PreparedCommit {
    fn validate_limits(&self) -> Result<(), ClientError> {
        self.request.validate_limits().map_err(ClientError::from)?;
        validate_string_bytes(
            "prepared_commit.message_id",
            &self.message_id,
            MAX_OBJECT_ID_BYTES,
        )?;
        validate_bytes_non_empty("prepared_commit.message_id", self.message_id.len())?;
        if self.request.membership_delta.commit_message_id != self.message_id {
            return Err(ClientError::PreparedCommitMessageIdMismatch);
        }
        if self
            .request
            .envelope
            .message_id()
            .map_err(ClientError::EnvelopeMessageId)?
            != self.message_id
        {
            return Err(ClientError::PreparedCommitMessageIdMismatch);
        }
        Ok(())
    }
}

impl LinkFanoutState {
    fn validate_limits(&self) -> Result<(), ClientError> {
        validate_string_bytes(
            "link_fanout.fanout_id",
            &self.fanout_id,
            MAX_OBJECT_ID_BYTES,
        )?;
        validate_bytes_non_empty("link_fanout.fanout_id", self.fanout_id.len())?;
        self.target_device.validate_limits()?;
        if let Some(after_room_id) = &self.after_room_id {
            validate_room_id(after_room_id)?;
        }
        validate_item_count("link_fanout.rooms", self.rooms.len(), MAX_LINK_FANOUT_ROOMS)?;
        let mut seen_rooms = BTreeSet::<RoomId>::new();
        for room in &self.rooms {
            room.validate_limits()?;
            if let Some(claimed_key_package) = &room.claimed_key_package {
                if matches!(room.status, LinkFanoutRoomStatus::Done { .. }) {
                    return Err(ClientError::UnexpectedLinkFanoutClaim(
                        claimed_key_package.key_package_id.clone(),
                    ));
                }
                if claimed_key_package.owner != self.target_device {
                    return Err(ClientError::LinkFanoutClaimTargetMismatch {
                        expected: self.target_device.clone(),
                        actual: claimed_key_package.owner.clone(),
                    });
                }
                if claimed_key_package.key_package_id != room.plan.key_package_id {
                    return Err(ClientError::LinkFanoutClaimPackageMismatch {
                        expected: room.plan.key_package_id.clone(),
                        actual: claimed_key_package.key_package_id.clone(),
                    });
                }
            }
            if !seen_rooms.insert(room.plan.room_id.clone()) {
                return Err(ClientError::DuplicateLinkFanoutRoom(
                    room.plan.room_id.clone(),
                ));
            }
        }
        Ok(())
    }
}

impl LinkFanoutRoomState {
    fn validate_limits(&self) -> Result<(), ClientError> {
        self.plan.validate_limits()?;
        if let Some(claimed_key_package) = &self.claimed_key_package {
            validate_claimed_key_package(claimed_key_package)?;
        }
        self.status.validate_limits()?;
        Ok(())
    }
}

impl LinkFanoutRoomPlan {
    fn validate_limits(&self) -> Result<(), ClientError> {
        validate_room_id(&self.room_id)?;
        validate_bytes_non_empty("link_fanout.key_package_id", self.key_package_id.len())?;
        validate_string_bytes(
            "link_fanout.key_package_id",
            &self.key_package_id,
            MAX_OBJECT_ID_BYTES,
        )?;
        validate_bytes_non_empty("link_fanout.welcome_id", self.welcome_id.len())?;
        validate_string_bytes(
            "link_fanout.welcome_id",
            &self.welcome_id,
            MAX_OBJECT_ID_BYTES,
        )?;
        validate_bytes_non_empty("link_fanout.idempotency_key", self.idempotency_key.len())?;
        validate_idempotency_key(&self.idempotency_key)?;
        Ok(())
    }
}

impl LinkFanoutRoomStatus {
    fn validate_limits(&self) -> Result<(), ClientError> {
        match self {
            Self::Pending => Ok(()),
            Self::Prepared { prepared } => prepared.validate_limits(),
            Self::Done { .. } => Ok(()),
        }
    }
}

impl OpenMlsStorageRecord {
    fn validate_limits(&self) -> Result<(), ClientError> {
        validate_bytes_non_empty("openmls_storage.key", self.key.len())?;
        validate_bytes_len(
            "openmls_storage.key",
            self.key.len(),
            MAX_OPENMLS_STORAGE_KEY_BYTES,
        )?;
        validate_bytes_non_empty("openmls_storage.value", self.value.len())?;
        validate_bytes_len(
            "openmls_storage.value",
            self.value.len(),
            MAX_OPENMLS_STORAGE_VALUE_BYTES,
        )?;
        Ok(())
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct ClientStoreEncryptionKey {
    bytes: [u8; CLIENT_STORE_KEY_BYTES],
}

impl ClientStoreEncryptionKey {
    pub fn from_nostr_secret(
        account_secret_key: &NostrSecretKey,
        device_id: &str,
    ) -> Result<Self, ClientStoreError> {
        validate_bytes_non_empty("client_store.device_id", device_id.len())
            .map_err(ClientError::from)?;
        validate_string_bytes("client_store.device_id", device_id, MAX_DEVICE_ID_BYTES)
            .map_err(ClientError::from)?;
        let bytes = account_secret_key
            .derive_secret_32(CLIENT_STORE_KEY_DERIVATION_DOMAIN, device_id.as_bytes())
            .map_err(ClientError::from)?;
        debug_assert_eq!(bytes.len(), CLIENT_STORE_KEY_BYTES);
        Ok(Self { bytes })
    }

    fn as_bytes(&self) -> &[u8; CLIENT_STORE_KEY_BYTES] {
        &self.bytes
    }
}

impl std::fmt::Debug for ClientStoreEncryptionKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ClientStoreEncryptionKey(REDACTED)")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqliteClientStoreOptions {
    pub encryption_key: ClientStoreEncryptionKey,
}

impl SqliteClientStoreOptions {
    pub fn from_nostr_secret(
        account_secret_key: &NostrSecretKey,
        device_id: &str,
    ) -> Result<Self, ClientStoreError> {
        Ok(Self {
            encryption_key: ClientStoreEncryptionKey::from_nostr_secret(
                account_secret_key,
                device_id,
            )?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyClientStoreTable {
    Profiles,
    Rooms,
    OpenMlsStorage,
}

impl LegacyClientStoreTable {
    fn name(self) -> &'static str {
        match self {
            Self::Profiles => "client_profiles",
            Self::Rooms => "client_rooms",
            Self::OpenMlsStorage => "client_openmls_storage",
        }
    }
}

#[derive(Debug)]
pub struct SqliteClientStore {
    db_path: PathBuf,
    conn: Connection,
    options: SqliteClientStoreOptions,
}

impl SqliteClientStore {
    pub fn open(
        db_path: impl AsRef<Path>,
        options: SqliteClientStoreOptions,
    ) -> Result<Self, ClientStoreError> {
        let db_path = db_path.as_ref().to_path_buf();
        if let Some(parent) = db_path.parent() {
            fs::create_dir_all(parent).map_err(|source| ClientStoreError::CreateDir {
                path: parent.to_path_buf(),
                source,
            })?;
        }

        let conn = open_client_store_connection(&db_path)?;
        migrate_client_store(&conn)?;
        Ok(Self {
            db_path,
            conn,
            options,
        })
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub fn save_device_state(&mut self, device: &FiniteChatDevice) -> Result<(), ClientStoreError> {
        let state = device.export_state()?;
        let encryption_key = self.options.encryption_key.clone();
        self.with_transaction(|tx| save_device_state_tx(tx, &state, &encryption_key))
    }

    pub fn load_device(
        &self,
        config: FiniteChatDeviceConfig,
    ) -> Result<FiniteChatDevice, ClientStoreError> {
        let account_id = hex_lower(config.account_secret_key.public_key().as_bytes());
        let device_id = config.device_id.clone();
        let state =
            load_device_state(&self.conn, &self.options.encryption_key, &account_id, &device_id)?
                .ok_or(ClientStoreError::DeviceStateNotFound {
                    account_id,
                    device_id,
                })?;
        Ok(FiniteChatDevice::from_state(config, state)?)
    }

    pub fn activate_welcome_and_save(
        &mut self,
        device: &mut FiniteChatDevice,
        welcome_id: impl Into<WelcomeId>,
        room_id: impl Into<RoomId>,
        welcome_payload: &[u8],
        ratchet_tree_payload: &[u8],
        commit_seq: u64,
    ) -> Result<(), ClientStoreError> {
        let welcome_id = welcome_id.into();
        let room_id = room_id.into();
        device.activate_welcome(room_id.clone(), welcome_payload, ratchet_tree_payload)?;
        device.set_last_applied_seq(&room_id, commit_seq)?;
        device.store_pending_welcome_ack(PendingWelcomeAckState {
            welcome_id,
            room_id,
            commit_seq,
        })?;
        self.save_device_state(device)
    }

    pub fn store_pending_welcome_and_save(
        &mut self,
        device: &mut FiniteChatDevice,
        welcome: &WelcomeRecord,
    ) -> Result<(), ClientStoreError> {
        device.store_pending_welcome(welcome)?;
        self.save_device_state(device)
    }

    pub fn activate_pending_welcome_and_save(
        &mut self,
        device: &mut FiniteChatDevice,
        welcome_id: &str,
    ) -> Result<u64, ClientStoreError> {
        let ack = device.activate_pending_welcome(welcome_id)?;
        self.save_device_state(device)?;
        Ok(ack.commit_seq)
    }

    pub fn clear_pending_welcome_ack_and_save(
        &mut self,
        device: &mut FiniteChatDevice,
        welcome_id: &str,
    ) -> Result<(), ClientStoreError> {
        device.clear_pending_welcome_ack(welcome_id)?;
        self.save_device_state(device)
    }

    pub fn clear_pending_key_package_upload_and_save(
        &mut self,
        device: &mut FiniteChatDevice,
        key_package_id: &str,
    ) -> Result<(), ClientStoreError> {
        device.clear_pending_key_package_upload(key_package_id)?;
        self.save_device_state(device)
    }

    pub fn start_link_fanout_and_save(
        &mut self,
        device: &mut FiniteChatDevice,
        fanout_id: impl Into<String>,
        target_device: DeviceRef,
    ) -> Result<(), ClientStoreError> {
        device.start_link_fanout(fanout_id, target_device)?;
        self.save_device_state(device)
    }

    pub fn queue_link_fanout_page_and_save(
        &mut self,
        device: &mut FiniteChatDevice,
        fanout_id: &str,
        page: &ListAccountRoomsPage,
        plans: &[LinkFanoutRoomPlan],
    ) -> Result<(), ClientStoreError> {
        device.queue_link_fanout_page(fanout_id, page, plans)?;
        self.save_device_state(device)
    }

    pub fn queue_claimed_link_fanout_page_and_save(
        &mut self,
        device: &mut FiniteChatDevice,
        fanout_id: &str,
        page: &ListAccountRoomsPage,
        claimed_key_packages: &[ClaimKeyPackageResult],
    ) -> Result<(), ClientStoreError> {
        device.queue_claimed_link_fanout_page(fanout_id, page, claimed_key_packages)?;
        self.save_device_state(device)
    }

    pub fn prepare_link_fanout_room_commit_and_save(
        &mut self,
        device: &mut FiniteChatDevice,
        fanout_id: &str,
        room_id: &str,
        claimed_key_package: &ClaimKeyPackageResult,
    ) -> Result<PreparedCommit, ClientStoreError> {
        let prepared =
            device.prepare_link_fanout_room_commit(fanout_id, room_id, claimed_key_package)?;
        self.save_device_state(device)?;
        Ok(prepared)
    }

    pub fn prepare_claimed_link_fanout_room_commit_and_save(
        &mut self,
        device: &mut FiniteChatDevice,
        fanout_id: &str,
        room_id: &str,
    ) -> Result<PreparedCommit, ClientStoreError> {
        let prepared = device.prepare_claimed_link_fanout_room_commit(fanout_id, room_id)?;
        self.save_device_state(device)?;
        Ok(prepared)
    }

    pub fn complete_link_fanout_room_from_log_and_save(
        &mut self,
        device: &mut FiniteChatDevice,
        fanout_id: &str,
        room_id: &str,
        entry: &RoomLogEntry,
    ) -> Result<Option<AppliedLogEntry>, ClientStoreError> {
        if entry.seq <= device.last_applied_seq(room_id)? {
            return Ok(None);
        }
        let applied = device.complete_link_fanout_room_from_log(fanout_id, room_id, entry)?;
        device.set_last_applied_seq(room_id, entry.seq)?;
        self.save_device_state(device)?;
        Ok(Some(applied))
    }

    pub fn apply_log_entry_and_save(
        &mut self,
        device: &mut FiniteChatDevice,
        room_id: &str,
        entry: &RoomLogEntry,
    ) -> Result<Option<AppliedLogEntry>, ClientStoreError> {
        let applied = apply_log_entry_in_memory(device, room_id, entry)?;
        if applied.is_some() {
            self.save_device_state(device)?;
        }
        Ok(applied)
    }

    pub fn advance_room_cursor_and_save(
        &mut self,
        device: &mut FiniteChatDevice,
        room_id: &str,
        seq: u64,
    ) -> Result<(), ClientStoreError> {
        device.set_last_applied_seq(room_id, seq)?;
        self.save_device_state(device)
    }

    fn with_transaction<T>(
        &mut self,
        f: impl FnOnce(&Transaction<'_>) -> Result<T, ClientStoreError>,
    ) -> Result<T, ClientStoreError> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let value = f(&tx)?;
        tx.commit()?;
        Ok(value)
    }
}

/// Apply one ordered log entry to the in-memory device without persisting.
///
/// The sync workers apply a whole page in memory and save once per page;
/// crash recovery replays at most one page, and the `seq <=
/// last_applied_seq` guard makes that replay idempotent against whatever
/// state was last saved.
fn apply_log_entry_in_memory(
    device: &mut FiniteChatDevice,
    room_id: &str,
    entry: &RoomLogEntry,
) -> Result<Option<AppliedLogEntry>, ClientStoreError> {
    validate_room_id(room_id).map_err(ClientError::from)?;
    if entry.room_id != room_id {
        return Err(ClientError::LogEntryRoomMismatch {
            expected: room_id.to_string(),
            actual: entry.room_id.clone(),
        }
        .into());
    }
    if entry.seq <= device.last_applied_seq(room_id)? {
        return Ok(None);
    }
    // Own application messages cannot be decrypted by their sender (MLS);
    // they advance the cursor without producing an applied entry. Commits
    // are never skipped: own commits go through the pending-merge rule.
    if entry.kind == LogEntryKind::Application && entry.envelope.sender == *device.device_ref() {
        device.set_last_applied_seq(room_id, entry.seq)?;
        return Ok(None);
    }
    let applied = device.apply_log_entry(room_id, entry)?;
    device.set_last_applied_seq(room_id, entry.seq)?;
    Ok(Some(applied))
}

fn open_client_store_connection(db_path: &Path) -> Result<Connection, ClientStoreError> {
    let conn = Connection::open(db_path)?;
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

pub trait RuntimeDelivery {
    type Error;

    fn key_package_inventory(
        &mut self,
        owner: &DeviceRef,
    ) -> Result<KeyPackageInventory, Self::Error>;

    fn upload_key_package(&mut self, request: UploadKeyPackageRequest) -> Result<(), Self::Error>;

    fn claim_key_package_for_device(
        &mut self,
        owner: &DeviceRef,
    ) -> Result<Option<ClaimKeyPackageResult>, Self::Error>;

    fn submit_commit(
        &mut self,
        request: SubmitCommitRequest,
    ) -> Result<CommitAccepted, Self::Error>;

    fn list_account_rooms(
        &mut self,
        request: ListAccountRoomsRequest,
    ) -> Result<ListAccountRoomsPage, Self::Error>;

    fn claim_welcomes(&mut self, device: &DeviceRef) -> Result<Vec<WelcomeRecord>, Self::Error>;

    fn ack_welcome(&mut self, welcome_id: &str) -> Result<(), Self::Error>;

    fn sync_events(
        &mut self,
        room_id: &str,
        requester: &DeviceRef,
        after_seq: u64,
    ) -> Result<SyncEventsPage, Self::Error>;

    fn create_invite_session(
        &mut self,
        request: CreateInviteSessionRequest,
    ) -> Result<HttpInviteSessionRecord, Self::Error>;

    fn submit_invite_join(
        &mut self,
        request: SubmitInviteJoinRequest,
    ) -> Result<HttpInviteJoinRequestRecord, Self::Error>;

    fn list_invite_join_requests(
        &mut self,
        invite_id: &str,
    ) -> Result<ListInviteJoinRequestsResponse, Self::Error>;

    fn respond_invite_join(
        &mut self,
        request: RespondInviteJoinRequest,
    ) -> Result<HttpInviteJoinRequestRecord, Self::Error>;

    fn invite_join_status(
        &mut self,
        invite_id: &str,
        request_id: &str,
    ) -> Result<InviteJoinStatusResponse, Self::Error>;
}

pub trait HttpRuntimeTransport {
    type Error;

    fn post_json<T, R>(&mut self, uri: &str, body: &T) -> Result<R, Self::Error>
    where
        T: Serialize,
        R: DeserializeOwned;
}

#[derive(Debug, Clone)]
pub struct ReqwestHttpRuntimeTransport {
    base_url: String,
    client: reqwest::blocking::Client,
}

impl ReqwestHttpRuntimeTransport {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self::with_client(base_url, reqwest::blocking::Client::new())
    }

    pub fn with_client(base_url: impl Into<String>, client: reqwest::blocking::Client) -> Self {
        Self {
            base_url: base_url.into(),
            client,
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    fn route_url(&self, uri: &str) -> String {
        format!(
            "{}/{}",
            self.base_url.trim_end_matches('/'),
            uri.trim_start_matches('/')
        )
    }
}

impl HttpRuntimeTransport for ReqwestHttpRuntimeTransport {
    type Error = ReqwestHttpRuntimeTransportError;

    fn post_json<T, R>(&mut self, uri: &str, body: &T) -> Result<R, Self::Error>
    where
        T: Serialize,
        R: DeserializeOwned,
    {
        let response = self
            .client
            .post(self.route_url(uri))
            .json(body)
            .send()
            .map_err(ReqwestHttpRuntimeTransportError::Request)?;
        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .map_err(ReqwestHttpRuntimeTransportError::Request)?;
            return Err(ReqwestHttpRuntimeTransportError::Server { status, body });
        }
        response
            .json()
            .map_err(ReqwestHttpRuntimeTransportError::Request)
    }
}

#[derive(Debug, Error)]
pub enum ReqwestHttpRuntimeTransportError {
    #[error("HTTP runtime request failed: {0}")]
    Request(reqwest::Error),
    #[error("server returned {status}: {body}")]
    Server {
        status: reqwest::StatusCode,
        body: String,
    },
}

#[derive(Debug, Clone)]
pub struct HttpRuntimeDelivery<T> {
    transport: T,
}

impl<T> HttpRuntimeDelivery<T> {
    pub fn new(transport: T) -> Self {
        Self { transport }
    }

    pub fn transport(&self) -> &T {
        &self.transport
    }

    pub fn transport_mut(&mut self) -> &mut T {
        &mut self.transport
    }

    pub fn into_transport(self) -> T {
        self.transport
    }
}

impl<T: HttpRuntimeTransport> HttpRuntimeDelivery<T> {



    pub fn publish_account_room_record(
        &mut self,
        account_id: &str,
        record: &AccountRoomRecord,
    ) -> Result<(), HttpRuntimeDeliveryError<T::Error>> {
        let request = SaveAccountRoomRequest {
            account_id: account_id.to_owned(),
            room_id: record.room_id.clone(),
            record: serde_json::to_value(record)
                .map_err(|error| HttpRuntimeDeliveryError::Json(error.to_string()))?,
        };
        let _: SaveAccountRoomResponse = self.post_json("/account-rooms", &request)?;
        Ok(())
    }

    pub fn bootstrap_account_room(
        &mut self,
        request: &CreateRoomRequest,
    ) -> Result<(), HttpRuntimeDeliveryError<T::Error>> {
        let request = BootstrapAccountRoomRequest {
            room_id: request.room_id.clone(),
            mls_group_id: request.mls_group_id.clone(),
            creator: request.creator.clone(),
            protocol: request.protocol.clone(),
        };
        let _: BootstrapAccountRoomResponse =
            self.post_json("/account-rooms/bootstrap", &request)?;
        Ok(())
    }

    pub fn append_activity(
        &mut self,
        request: &AppendEphemeralActivityRequest,
    ) -> Result<EphemeralActivityAccepted, HttpRuntimeDeliveryError<T::Error>> {
        self.post_json("/activities", request)
    }

    pub fn append_event(
        &mut self,
        request: &AppendEventRequest,
        delivery_policy: ApplicationDeliveryPolicy,
    ) -> Result<EventAccepted, HttpRuntimeDeliveryError<T::Error>> {
        self.post_json(
            "/events",
            &AppendApplicationEventRequest {
                event: request.clone(),
                delivery_policy,
            },
        )
    }

    fn post_json<B, R>(
        &mut self,
        uri: &str,
        body: &B,
    ) -> Result<R, HttpRuntimeDeliveryError<T::Error>>
    where
        B: Serialize,
        R: DeserializeOwned,
    {
        self.transport
            .post_json(uri, body)
            .map_err(HttpRuntimeDeliveryError::Transport)
    }
}

impl<T: HttpRuntimeTransport> RuntimeDelivery for HttpRuntimeDelivery<T> {
    type Error = HttpRuntimeDeliveryError<T::Error>;

    fn key_package_inventory(
        &mut self,
        owner: &DeviceRef,
    ) -> Result<KeyPackageInventory, Self::Error> {
        let owner_id = http_member_id_for_device(owner)?;
        let inventory: HttpKeyPackageInventory = self.post_json(
            "/key-packages/inventory",
            &KeyPackageInventoryRequest { owner: owner_id },
        )?;
        Ok(KeyPackageInventory {
            owner: owner.clone(),
            available: inventory.available,
            leased: inventory.claimed,
        })
    }

    fn upload_key_package(&mut self, request: UploadKeyPackageRequest) -> Result<(), Self::Error> {
        let publication = HttpKeyPackagePublication {
            key_package_id: HttpKeyPackageId::new(request.key_package_id.as_bytes().to_vec()),
            owner: http_member_id_for_device(&request.owner)?,
            key_package: HttpKeyPackage::new(
                serde_json::to_vec(&request)
                    .map_err(|error| HttpRuntimeDeliveryError::Json(error.to_string()))?,
            ),
        };
        let _: PublishKeyPackageResponse = self.post_json("/key-packages", &publication)?;
        Ok(())
    }

    fn claim_key_package_for_device(
        &mut self,
        owner: &DeviceRef,
    ) -> Result<Option<ClaimKeyPackageResult>, Self::Error> {
        let claimed: Option<transport_http_server::HttpClaimedKeyPackage> = self.post_json(
            "/key-packages/claim",
            &ClaimKeyPackageRequest {
                owner: http_member_id_for_device(owner)?,
            },
        )?;
        claimed
            .map(|claimed| {
                let request: UploadKeyPackageRequest =
                    serde_json::from_slice(claimed.key_package.bytes())
                        .map_err(|error| HttpRuntimeDeliveryError::Json(error.to_string()))?;
                if claimed.key_package_id.as_slice() != request.key_package_id.as_bytes() {
                    return Err(HttpRuntimeDeliveryError::KeyPackageIdMismatch {
                        envelope_id: claimed.key_package_id.as_slice().to_vec(),
                        body_id: request.key_package_id,
                    });
                }
                if request.owner != *owner {
                    return Err(HttpRuntimeDeliveryError::KeyPackageOwnerMismatch {
                        expected: owner.clone(),
                        actual: request.owner,
                    });
                }
                Ok(ClaimKeyPackageResult {
                    lease_token: lease_token_for(&request.key_package_id, &request.owner),
                    key_package_id: request.key_package_id,
                    owner: request.owner,
                    key_package_ref: request.key_package_ref,
                    key_package_hash: request.key_package_hash,
                    key_package_payload: request.key_package_payload,
                })
            })
            .transpose()
    }

    fn submit_commit(
        &mut self,
        request: SubmitCommitRequest,
    ) -> Result<CommitAccepted, Self::Error> {
        request
            .validate_limits()
            .map_err(|error| HttpRuntimeDeliveryError::CommitValidation(error.to_string()))?;
        let message_id = request
            .envelope
            .message_id()
            .map_err(|error| HttpRuntimeDeliveryError::Json(error.to_string()))?;
        if request.envelope.kind != LogEntryKind::Commit {
            return Err(HttpRuntimeDeliveryError::CommitValidation(
                "commit request envelope must be a commit".to_owned(),
            ));
        }
        if request.envelope.epoch != request.expected_epoch {
            return Err(HttpRuntimeDeliveryError::CommitValidation(format!(
                "commit envelope epoch {} does not match expected epoch {}",
                request.envelope.epoch, request.expected_epoch
            )));
        }
        if request.envelope.sender != request.sender {
            return Err(HttpRuntimeDeliveryError::CommitValidation(
                "commit envelope sender does not match request sender".to_owned(),
            ));
        }
        request
            .membership_delta
            .validate_structure(request.expected_epoch, &message_id)
            .map_err(|error| HttpRuntimeDeliveryError::CommitValidation(error.to_string()))?;
        self.post_json("/commits", &request)
    }

    fn list_account_rooms(
        &mut self,
        request: ListAccountRoomsRequest,
    ) -> Result<ListAccountRoomsPage, Self::Error> {
        let response: ListAccountRoomDirectoryResponse = self.post_json(
            "/account-rooms/list",
            &ListAccountRoomDirectoryRequest {
                account_id: request.account_id.clone(),
                after_room_id: request.after_room_id.clone(),
                limit: request.limit as usize,
            },
        )?;
        let rooms = response
            .rooms
            .into_iter()
            .map(|record| {
                serde_json::from_value(record)
                    .map_err(|error| HttpRuntimeDeliveryError::Json(error.to_string()))
            })
            .collect::<Result<Vec<AccountRoomRecord>, _>>()?;
        let page = ListAccountRoomsPage {
            rooms,
            next_after_room_id: response.next_after_room_id,
            has_more: response.has_more,
        };
        page.validate_limits()
            .map_err(|error| HttpRuntimeDeliveryError::Json(error.to_string()))?;
        Ok(page)
    }

    fn claim_welcomes(&mut self, device: &DeviceRef) -> Result<Vec<WelcomeRecord>, Self::Error> {
        let claimed: Vec<HttpClaimedWelcome> = self.post_json(
            "/welcomes/claim",
            &ClaimWelcomesRequest {
                recipient: http_member_id_for_device(device)?,
                limit: MAX_WELCOME_CLAIMS_PER_REQUEST as usize,
            },
        )?;
        claimed
            .into_iter()
            .map(|claim| {
                let mut welcome: WelcomeRecord = serde_json::from_slice(&claim.message.payload)
                    .map_err(|error| HttpRuntimeDeliveryError::Json(error.to_string()))?;
                if claim.message.id.as_slice() != welcome.welcome_id.as_bytes() {
                    return Err(HttpRuntimeDeliveryError::WelcomeIdMismatch {
                        message_id: claim.message.id.as_slice().to_vec(),
                        welcome_id: welcome.welcome_id,
                    });
                }
                if welcome.recipient != *device {
                    return Err(HttpRuntimeDeliveryError::WelcomeRecipientMismatch {
                        expected: device.clone(),
                        actual: welcome.recipient,
                    });
                }
                welcome.state = WelcomeState::Claimed;
                Ok(welcome)
            })
            .collect()
    }

    fn ack_welcome(&mut self, welcome_id: &str) -> Result<(), Self::Error> {
        let _: AckWelcomeResponse = self.post_json(
            "/welcomes/ack",
            &AckWelcomeRequest {
                message_id: HttpMessageId::new(welcome_id.as_bytes().to_vec()),
            },
        )?;
        Ok(())
    }

    fn create_invite_session(
        &mut self,
        request: CreateInviteSessionRequest,
    ) -> Result<HttpInviteSessionRecord, Self::Error> {
        self.post_json("/invites", &request)
    }

    fn submit_invite_join(
        &mut self,
        request: SubmitInviteJoinRequest,
    ) -> Result<HttpInviteJoinRequestRecord, Self::Error> {
        self.post_json("/invites/join", &request)
    }

    fn list_invite_join_requests(
        &mut self,
        invite_id: &str,
    ) -> Result<ListInviteJoinRequestsResponse, Self::Error> {
        self.post_json(
            "/invites/requests",
            &ListInviteJoinRequestsRequest {
                invite_id: invite_id.to_owned(),
            },
        )
    }

    fn respond_invite_join(
        &mut self,
        request: RespondInviteJoinRequest,
    ) -> Result<HttpInviteJoinRequestRecord, Self::Error> {
        self.post_json("/invites/respond", &request)
    }

    fn invite_join_status(
        &mut self,
        invite_id: &str,
        request_id: &str,
    ) -> Result<InviteJoinStatusResponse, Self::Error> {
        self.post_json(
            "/invites/status",
            &InviteJoinStatusRequest {
                invite_id: invite_id.to_owned(),
                request_id: request_id.to_owned(),
            },
        )
    }

    fn sync_events(
        &mut self,
        room_id: &str,
        requester: &DeviceRef,
        after_seq: u64,
    ) -> Result<SyncEventsPage, Self::Error> {
        let page: HttpSyncPage = self.post_json(
            "/sync/group",
            &GroupSyncRequest {
                group_id: http_group_id_for_room(room_id),
                after_seq,
                limit: MAX_HTTP_SYNC_PAGE_ENTRIES,
                requester: Some(http_member_id_for_device(requester)?),
            },
        )?;
        let entries = page
            .entries
            .into_iter()
            .map(|queued| {
                let mut entry = decode_http_room_log_entry(&queued.message.payload)?;
                if entry.room_id != room_id {
                    return Err(HttpRuntimeDeliveryError::RoomEntryMismatch {
                        expected: room_id.to_owned(),
                        actual: entry.room_id,
                    });
                }
                entry.seq = queued.seq;
                Ok(entry)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(SyncEventsPage {
            entries,
            next_after_seq: page.next_after_seq,
            has_more: page.has_more,
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum HttpRuntimeDeliveryError<E> {
    Transport(E),
    Json(String),
    WelcomeIdMismatch {
        message_id: Vec<u8>,
        welcome_id: String,
    },
    WelcomeRecipientMismatch {
        expected: DeviceRef,
        actual: DeviceRef,
    },
    KeyPackageIdMismatch {
        envelope_id: Vec<u8>,
        body_id: String,
    },
    KeyPackageOwnerMismatch {
        expected: DeviceRef,
        actual: DeviceRef,
    },
    RoomEntryMismatch {
        expected: String,
        actual: String,
    },
    CommitValidation(String),
}

impl<E: fmt::Display> fmt::Display for HttpRuntimeDeliveryError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(error) => write!(formatter, "HTTP runtime transport failed: {error}"),
            Self::Json(error) => write!(formatter, "HTTP runtime JSON error: {error}"),
            Self::WelcomeIdMismatch {
                message_id,
                welcome_id,
            } => write!(
                formatter,
                "Welcome id mismatch: envelope {message_id:?}, payload {welcome_id}"
            ),
            Self::WelcomeRecipientMismatch { expected, actual } => write!(
                formatter,
                "Welcome recipient mismatch: expected {expected:?}, actual {actual:?}"
            ),
            Self::KeyPackageIdMismatch {
                envelope_id,
                body_id,
            } => write!(
                formatter,
                "KeyPackage id mismatch: envelope {envelope_id:?}, payload {body_id}"
            ),
            Self::KeyPackageOwnerMismatch { expected, actual } => write!(
                formatter,
                "KeyPackage owner mismatch: expected {expected:?}, actual {actual:?}"
            ),
            Self::RoomEntryMismatch { expected, actual } => write!(
                formatter,
                "room log entry mismatch: expected {expected}, actual {actual}"
            ),
            Self::CommitValidation(error) => {
                write!(formatter, "HTTP runtime commit validation failed: {error}")
            }
        }
    }
}

impl<E> std::error::Error for HttpRuntimeDeliveryError<E>
where
    E: std::error::Error + 'static,
{
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Transport(error) => Some(error),
            Self::Json(_)
            | Self::WelcomeIdMismatch { .. }
            | Self::WelcomeRecipientMismatch { .. }
            | Self::KeyPackageIdMismatch { .. }
            | Self::KeyPackageOwnerMismatch { .. }
            | Self::RoomEntryMismatch { .. }
            | Self::CommitValidation(_) => None,
        }
    }
}

fn decode_http_room_log_entry<E>(
    payload: &[u8],
) -> Result<RoomLogEntry, HttpRuntimeDeliveryError<E>> {
    if let Ok(projection) = serde_json::from_slice::<FiniteAccountRoomCommitProjection>(payload) {
        return Ok(projection.entry);
    }
    serde_json::from_slice(payload)
        .map_err(|error| HttpRuntimeDeliveryError::Json(error.to_string()))
}

fn http_member_id_for_device<E>(
    device: &DeviceRef,
) -> Result<HttpMemberId, HttpRuntimeDeliveryError<E>> {
    serde_json::to_vec(device)
        .map(HttpMemberId::new)
        .map_err(|error| HttpRuntimeDeliveryError::Json(error.to_string()))
}

fn http_group_id_for_room(room_id: &str) -> HttpGroupId {
    HttpGroupId::new(room_id.as_bytes().to_vec())
}



pub fn run_runtime_sync_tick<D: RuntimeDelivery>(
    store: &mut SqliteClientStore,
    device: &mut FiniteChatDevice,
    delivery: &mut D,
    options: &RuntimeSyncOptions,
) -> Result<RuntimeSyncReport, RuntimeWorkerError<D::Error>> {
    options.validate_limits()?;
    let mut report = RuntimeSyncReport::default();

    let inventory = delivery
        .key_package_inventory(device.device_ref())
        .map_err(RuntimeWorkerError::Delivery)?;
    let replenishment =
        device.key_package_replenishment_plan(inventory, options.key_package_target_available)?;
    let upload_requests = replenishment.upload_requests;
    if !upload_requests.is_empty() {
        store.save_device_state(device)?;
    }
    for request in upload_requests {
        delivery
            .upload_key_package(request.clone())
            .map_err(RuntimeWorkerError::Delivery)?;
        store.clear_pending_key_package_upload_and_save(device, &request.key_package_id)?;
        report.record_uploaded_key_package()?;
    }

    let claimed_welcomes = delivery
        .claim_welcomes(device.device_ref())
        .map_err(RuntimeWorkerError::Delivery)?;
    report.record_claimed_welcomes(claimed_welcomes.len())?;
    for welcome in claimed_welcomes {
        store.store_pending_welcome_and_save(device, &welcome)?;
    }

    for welcome_id in device.pending_welcome_ids() {
        store.activate_pending_welcome_and_save(device, &welcome_id)?;
    }

    for ack in device.pending_welcome_acks() {
        delivery
            .ack_welcome(&ack.welcome_id)
            .map_err(RuntimeWorkerError::Delivery)?;
        store.clear_pending_welcome_ack_and_save(device, &ack.welcome_id)?;
        report.record_welcome_ack()?;
    }

    // Group rooms by server (ADR 0005): this tick talks to the home
    // server, so rooms pinned to another room server are synced by
    // `run_room_server_sync_tick` with that server's transport.
    for cursor in device.room_sync_cursors() {
        if cursor.server_url.is_some() {
            continue;
        }
        sync_room_pages(
            store,
            device,
            delivery,
            options,
            cursor.room_id,
            cursor.after_seq,
            &mut report,
        )?;
    }

    Ok(report)
}

/// Sync the rooms hosted on one specific room server (ADR 0005). The
/// caller provides a delivery bound to that server's address; welcomes are
/// claimed there too, because an invited room's Welcome lives on the room's
/// server (ADR 0006).
pub fn run_room_server_sync_tick<D: RuntimeDelivery>(
    store: &mut SqliteClientStore,
    device: &mut FiniteChatDevice,
    delivery: &mut D,
    options: &RuntimeSyncOptions,
    server_url: &str,
) -> Result<RuntimeSyncReport, RuntimeWorkerError<D::Error>> {
    options.validate_limits()?;
    let mut report = RuntimeSyncReport::default();

    let claimed_welcomes = delivery
        .claim_welcomes(device.device_ref())
        .map_err(RuntimeWorkerError::Delivery)?;
    report.record_claimed_welcomes(claimed_welcomes.len())?;
    for welcome in claimed_welcomes {
        store.store_pending_welcome_and_save(device, &welcome)?;
    }

    for welcome_id in device.pending_welcome_ids() {
        store.activate_pending_welcome_and_save(device, &welcome_id)?;
    }

    for ack in device.pending_welcome_acks() {
        delivery
            .ack_welcome(&ack.welcome_id)
            .map_err(RuntimeWorkerError::Delivery)?;
        store.clear_pending_welcome_ack_and_save(device, &ack.welcome_id)?;
        report.record_welcome_ack()?;
    }

    for cursor in device.room_sync_cursors() {
        if cursor.server_url.as_deref() != Some(server_url) {
            continue;
        }
        sync_room_pages(
            store,
            device,
            delivery,
            options,
            cursor.room_id,
            cursor.after_seq,
            &mut report,
        )?;
    }

    Ok(report)
}

/// Parameters for creating a room invite (ADR 0006). `server_url` is the
/// public address of the server hosting the room — it goes into the invite
/// code verbatim, because joining a room is discovering where it lives.
#[derive(Debug, Clone)]
pub struct CreateRoomInviteParams<'a> {
    pub room_id: &'a str,
    pub server_url: &'a str,
    pub display_name: Option<String>,
    pub max_joins: u32,
    pub ttl_ms: u64,
    pub now_ms: u64,
}

/// Create an invite session for a room this device administers and return
/// the invite code. The code carries the only copy of the invite token;
/// print it as a URL/QR and show `invite_current_pin` beside it.
pub fn create_room_invite<D: RuntimeDelivery>(
    device: &FiniteChatDevice,
    delivery: &mut D,
    params: CreateRoomInviteParams<'_>,
) -> Result<InviteCodeV1, RuntimeWorkerError<D::Error>> {
    let code = InviteCodeV1 {
        server_url: params.server_url.to_owned(),
        room_id: params.room_id.to_owned(),
        invite_id: device.generate_object_id("invite")?,
        invite_token: device.generate_invite_token()?,
        inviter_account_id: device.device_ref().account_id.clone(),
        display_name: params.display_name,
    };
    code.encode()
        .map_err(|error| ClientError::InviteCode(error.to_string()))?;
    delivery
        .create_invite_session(CreateInviteSessionRequest {
            invite_id: code.invite_id.clone(),
            room_id: code.room_id.clone(),
            inviter: device.device_ref().clone(),
            max_joins: params.max_joins,
            expires_at_ms: params
                .now_ms
                .saturating_add(params.ttl_ms.min(MAX_INVITE_TTL_MILLIS)),
        })
        .map_err(RuntimeWorkerError::Delivery)?;
    Ok(code)
}

#[derive(Debug, Default)]
pub struct InviteAcceptReport {
    pub accepted: Vec<DeviceRef>,
    pub rejected: Vec<DeviceRef>,
    /// True when pending joins exist but the room already has an unmerged
    /// pending commit; run a sync tick (which merges own commits from the
    /// log) and call again.
    pub deferred_pending_commit: bool,
}

/// Inviter-side processing of pending join requests (ADR 0006): verify each
/// proof for the current PIN window ±1, reject failures, and admit all
/// verified joiners in a single Add commit with their Welcomes. Nothing
/// enters the MLS group before its proof verifies.
pub fn accept_pending_invite_joins<D: RuntimeDelivery>(
    store: &mut SqliteClientStore,
    device: &mut FiniteChatDevice,
    delivery: &mut D,
    code: &InviteCodeV1,
    now_ms: u64,
) -> Result<InviteAcceptReport, RuntimeWorkerError<D::Error>> {
    let mut report = InviteAcceptReport::default();
    let listing = delivery
        .list_invite_join_requests(&code.invite_id)
        .map_err(RuntimeWorkerError::Delivery)?;
    if listing.session.room_id != code.room_id {
        return Err(ClientError::InviteRoomMismatch {
            expected: code.room_id.clone(),
            actual: listing.session.room_id,
        }
        .into());
    }
    let pending = listing
        .requests
        .into_iter()
        .filter(|request| request.state == HttpInviteJoinState::Pending)
        .collect::<Vec<_>>();
    if pending.is_empty() {
        return Ok(report);
    }
    if device.has_pending_commit(&code.room_id)? {
        report.deferred_pending_commit = true;
        return Ok(report);
    }

    let mut verified = Vec::new();
    for join in pending {
        let proof_ok = verify_invite_join_proof(
            &code.invite_token,
            now_ms / 1000,
            &join.joiner.account_id,
            &join.joiner.device_id,
            &join.key_package,
            &join.pin_proof,
        );
        let upload = proof_ok
            .then(|| serde_json::from_slice::<UploadKeyPackageRequest>(&join.key_package).ok())
            .flatten()
            .filter(|upload| upload.owner == join.joiner);
        match upload {
            Some(upload) => verified.push((join, upload)),
            None => {
                delivery
                    .respond_invite_join(RespondInviteJoinRequest {
                        invite_id: code.invite_id.clone(),
                        request_id: join.request_id.clone(),
                        accept: false,
                    })
                    .map_err(RuntimeWorkerError::Delivery)?;
                report.rejected.push(join.joiner);
            }
        }
    }
    if verified.is_empty() {
        return Ok(report);
    }

    // The joiner's KeyPackage rides inline in the join request; publish and
    // claim it on this server so the existing commit validation (claimed
    // KeyPackages only) applies unchanged.
    let mut claimed_key_packages = Vec::with_capacity(verified.len());
    let mut welcome_ids = Vec::with_capacity(verified.len());
    for (join, upload) in &verified {
        delivery
            .upload_key_package(upload.clone())
            .map_err(RuntimeWorkerError::Delivery)?;
        let claimed = delivery
            .claim_key_package_for_device(&join.joiner)
            .map_err(RuntimeWorkerError::Delivery)?
            .ok_or_else(|| {
                ClientError::InviteKeyPackageUnavailable(join.joiner.device_id.clone())
            })?;
        claimed_key_packages.push(claimed);
        welcome_ids.push(format!(
            "invite-welcome-{}-{}",
            code.invite_id, join.request_id
        ));
    }
    let mut request_ids = verified
        .iter()
        .map(|(join, _)| join.request_id.as_str())
        .collect::<Vec<_>>();
    request_ids.sort_unstable();
    let idempotency_key = format!("invite-add-{}-{}", code.invite_id, request_ids.join("+"));
    let prepared = device.prepare_add_members_commit(
        &code.room_id,
        &claimed_key_packages,
        &welcome_ids,
        idempotency_key,
    )?;
    store.save_device_state(device)?;
    delivery
        .submit_commit(prepared.request)
        .map_err(RuntimeWorkerError::Delivery)?;
    for (join, _) in &verified {
        delivery
            .respond_invite_join(RespondInviteJoinRequest {
                invite_id: code.invite_id.clone(),
                request_id: join.request_id.clone(),
                accept: true,
            })
            .map_err(RuntimeWorkerError::Delivery)?;
        report.accepted.push(join.joiner.clone());
    }
    Ok(report)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InviteJoinHandle {
    pub request_id: String,
    pub key_package_id: String,
}

/// Joiner-side join request (ADR 0006): build a fresh single-use KeyPackage,
/// persist its private material, and submit it with a proof binding the
/// typed PIN and the invite token to this exact identity and key material.
/// The raw PIN never leaves the device.
pub fn submit_invite_join_request<D: RuntimeDelivery>(
    store: &mut SqliteClientStore,
    device: &mut FiniteChatDevice,
    delivery: &mut D,
    code: &InviteCodeV1,
    pin: &str,
    display_name: Option<String>,
    now_ms: u64,
) -> Result<InviteJoinHandle, RuntimeWorkerError<D::Error>> {
    let upload = device.upload_key_package_auto_id_request()?;
    let key_package_bytes =
        serde_json::to_vec(&upload).map_err(|_| ClientError::SerializeKeyPackage)?;
    // Persist the KeyPackage private material before the bytes leave the
    // device: the agent may commit the Add immediately.
    store.save_device_state(device)?;
    let request_id = device.generate_object_id("join")?;
    let pin_proof = invite_join_proof(
        &code.invite_token,
        pin,
        &device.device_ref().account_id,
        &device.device_ref().device_id,
        &key_package_bytes,
    );
    delivery
        .submit_invite_join(SubmitInviteJoinRequest {
            invite_id: code.invite_id.clone(),
            request_id: request_id.clone(),
            joiner: device.device_ref().clone(),
            key_package: key_package_bytes,
            pin_proof,
            display_name,
            submitted_at_ms: now_ms,
        })
        .map_err(RuntimeWorkerError::Delivery)?;
    Ok(InviteJoinHandle {
        request_id,
        key_package_id: upload.key_package_id,
    })
}

/// After the invited room's Welcome has been activated (by the room-server
/// sync tick), verify the inviter is really in the room and pin the room to
/// the server the invite named. Mandatory (ADR 0006): this is the joiner's
/// defense against a hostile rendezvous server admitting it to a different
/// group.
pub fn finalize_invited_room(
    store: &mut SqliteClientStore,
    device: &mut FiniteChatDevice,
    code: &InviteCodeV1,
) -> Result<(), ClientStoreError> {
    device.verify_room_member_account(&code.room_id, &code.inviter_account_id)?;
    device.set_room_server_url(&code.room_id, Some(code.server_url.clone()))?;
    store.save_device_state(device)
}

pub fn run_link_fanout_tick<D: RuntimeDelivery>(
    store: &mut SqliteClientStore,
    device: &mut FiniteChatDevice,
    delivery: &mut D,
    fanout_id: &str,
    options: &RuntimeLinkFanoutOptions,
) -> Result<RuntimeLinkFanoutReport, RuntimeWorkerError<D::Error>> {
    validate_string_bytes("link_fanout.fanout_id", fanout_id, MAX_OBJECT_ID_BYTES)
        .map_err(ClientError::from)?;
    options.validate_limits()?;
    let mut report = RuntimeLinkFanoutReport::default();

    run_link_fanout_discovery(store, device, delivery, fanout_id, options, &mut report)?;
    let room_ids =
        link_fanout_rooms_to_advance(device, fanout_id, options.max_commit_rooms_per_tick)?;
    for room_id in &room_ids.pending {
        store.prepare_claimed_link_fanout_room_commit_and_save(device, fanout_id, room_id)?;
        report.record_prepared_commit()?;
    }
    for room_id in &room_ids.prepared {
        if !device.has_pending_commit(room_id)? {
            store.prepare_claimed_link_fanout_room_commit_and_save(device, fanout_id, room_id)?;
            report.record_prepared_commit()?;
        }
    }
    for room_id in room_ids.all_prepared() {
        let prepared = device.prepared_link_fanout_commit(fanout_id, &room_id)?;
        let accepted = delivery
            .submit_commit(prepared.request.clone())
            .map_err(RuntimeWorkerError::Delivery)?;
        if accepted.message_id != prepared.message_id {
            return Err(ClientError::LinkFanoutPreparedCommitMismatch {
                expected: prepared.message_id,
                actual: accepted.message_id,
            }
            .into());
        }
        report.record_submitted_commit()?;
        complete_link_fanout_room_from_sync(
            store,
            device,
            delivery,
            fanout_id,
            &room_id,
            options,
            &mut report,
        )?;
    }

    report.complete = device.link_fanout_is_complete(fanout_id)?;
    Ok(report)
}

fn run_link_fanout_discovery<D: RuntimeDelivery>(
    store: &mut SqliteClientStore,
    device: &mut FiniteChatDevice,
    delivery: &mut D,
    fanout_id: &str,
    options: &RuntimeLinkFanoutOptions,
    report: &mut RuntimeLinkFanoutReport,
) -> Result<(), RuntimeWorkerError<D::Error>> {
    let target_device = device.link_fanout_target_device(fanout_id)?;
    let mut pages = 0u32;
    while pages < options.max_discovery_pages_per_tick
        && !device.link_fanout_discovery_complete(fanout_id)?
    {
        let after_room_id = device.link_fanout_after_room_id(fanout_id)?;
        let page = delivery
            .list_account_rooms(ListAccountRoomsRequest {
                account_id: device.device_ref().account_id.clone(),
                after_room_id: after_room_id.clone(),
                limit: 1,
            })
            .map_err(RuntimeWorkerError::Delivery)?;
        if page.has_more && page.next_after_room_id == after_room_id {
            return Err(ClientError::RuntimeLinkFanoutDiscoveryStalled {
                fanout_id: fanout_id.to_string(),
                after_room_id,
            }
            .into());
        }

        let needs_target = page.rooms.iter().any(|room| {
            !room
                .devices
                .iter()
                .any(|device| device.device == target_device)
        });
        let mut claimed_key_packages = Vec::new();
        if needs_target {
            let Some(claim) = delivery
                .claim_key_package_for_device(&target_device)
                .map_err(RuntimeWorkerError::Delivery)?
            else {
                break;
            };
            claimed_key_packages.push(claim);
        }

        store.queue_claimed_link_fanout_page_and_save(
            device,
            fanout_id,
            &page,
            &claimed_key_packages,
        )?;
        report.record_discovery_page()?;
        if !claimed_key_packages.is_empty() {
            report.record_claimed_key_package()?;
            report.record_queued_room()?;
        }
        pages = pages
            .checked_add(1)
            .ok_or(ClientError::RuntimeCounterOverflow)?;
    }
    debug_assert!(pages <= options.max_discovery_pages_per_tick);
    Ok(())
}

struct LinkFanoutRoomsToAdvance {
    prepared: Vec<RoomId>,
    pending: Vec<RoomId>,
}

impl LinkFanoutRoomsToAdvance {
    fn all_prepared(self) -> Vec<RoomId> {
        let mut room_ids = Vec::with_capacity(self.prepared.len() + self.pending.len());
        room_ids.extend(self.prepared);
        room_ids.extend(self.pending);
        room_ids
    }
}

fn link_fanout_rooms_to_advance(
    device: &FiniteChatDevice,
    fanout_id: &str,
    max_rooms: u32,
) -> Result<LinkFanoutRoomsToAdvance, ClientError> {
    let mut prepared = device.prepared_link_fanout_room_ids(fanout_id)?;
    prepared.truncate(max_rooms as usize);
    let remaining = max_rooms as usize - prepared.len();
    let mut pending = device.pending_link_fanout_room_ids(fanout_id)?;
    pending.truncate(remaining);
    debug_assert!(prepared.len() + pending.len() <= max_rooms as usize);
    Ok(LinkFanoutRoomsToAdvance { prepared, pending })
}

fn complete_link_fanout_room_from_sync<D: RuntimeDelivery>(
    store: &mut SqliteClientStore,
    device: &mut FiniteChatDevice,
    delivery: &mut D,
    fanout_id: &str,
    room_id: &str,
    options: &RuntimeLinkFanoutOptions,
    report: &mut RuntimeLinkFanoutReport,
) -> Result<(), RuntimeWorkerError<D::Error>> {
    let prepared = device.prepared_link_fanout_commit(fanout_id, room_id)?;
    let mut after_seq = device.last_applied_seq(room_id)?;
    let mut pages = 0u32;
    while pages < options.max_completion_sync_pages_per_room {
        let page = delivery
            .sync_events(room_id, device.device_ref(), after_seq)
            .map_err(RuntimeWorkerError::Delivery)?;
        report.record_completion_sync_page()?;
        pages = pages
            .checked_add(1)
            .ok_or(ClientError::RuntimeCounterOverflow)?;
        if page.next_after_seq < after_seq {
            return Err(ClientError::RuntimeSyncCursorRegression {
                room_id: room_id.to_string(),
                current_seq: after_seq,
                next_after_seq: page.next_after_seq,
            }
            .into());
        }

        let mut dirty = false;
        for entry in page.entries {
            let seq = entry.seq;
            if entry.message_id == prepared.message_id {
                // The completion path persists the whole device state,
                // including any entries applied in memory above.
                let message_id = entry.message_id.clone();
                if let Some(applied) = store.complete_link_fanout_room_from_log_and_save(
                    device, fanout_id, room_id, &entry,
                )? {
                    report.applied_entries.push(RuntimeAppliedEntry {
                        room_id: room_id.to_string(),
                        seq,
                        message_id,
                        entry: applied,
                    });
                    report.record_completed_room()?;
                }
                return Ok(());
            }
            if let Some(applied) = apply_log_entry_in_memory(device, room_id, &entry)? {
                dirty = true;
                report.applied_entries.push(RuntimeAppliedEntry {
                    room_id: room_id.to_string(),
                    seq,
                    message_id: entry.message_id.clone(),
                    entry: applied,
                });
            }
        }

        if page.next_after_seq > device.last_applied_seq(room_id)? {
            device
                .set_last_applied_seq(room_id, page.next_after_seq)
                .map_err(ClientStoreError::from)?;
            dirty = true;
        }
        if dirty {
            store.save_device_state(device)?;
        }
        if !page.has_more {
            break;
        }
        if page.next_after_seq == after_seq {
            return Err(ClientError::RuntimeSyncStalled {
                room_id: room_id.to_string(),
                after_seq,
            }
            .into());
        }
        after_seq = page.next_after_seq;
    }
    debug_assert!(pages <= options.max_completion_sync_pages_per_room);
    Ok(())
}

fn sync_room_pages<D: RuntimeDelivery>(
    store: &mut SqliteClientStore,
    device: &mut FiniteChatDevice,
    delivery: &mut D,
    options: &RuntimeSyncOptions,
    room_id: RoomId,
    mut after_seq: u64,
    report: &mut RuntimeSyncReport,
) -> Result<(), RuntimeWorkerError<D::Error>> {
    let mut pages = 0u32;
    while pages < options.max_sync_pages_per_room {
        let page = delivery
            .sync_events(&room_id, device.device_ref(), after_seq)
            .map_err(RuntimeWorkerError::Delivery)?;
        report.record_sync_page()?;
        pages = pages
            .checked_add(1)
            .ok_or(ClientError::RuntimeCounterOverflow)?;
        if page.next_after_seq < after_seq {
            return Err(ClientError::RuntimeSyncCursorRegression {
                room_id,
                current_seq: after_seq,
                next_after_seq: page.next_after_seq,
            }
            .into());
        }

        let mut dirty = false;
        for entry in page.entries {
            let seq = entry.seq;
            let message_id = entry.message_id.clone();
            if let Some(applied) = apply_log_entry_in_memory(device, &room_id, &entry)? {
                dirty = true;
                report.applied_entries.push(RuntimeAppliedEntry {
                    room_id: room_id.clone(),
                    seq,
                    message_id,
                    entry: applied,
                });
            }
        }

        if page.next_after_seq > device.last_applied_seq(&room_id)? {
            device
                .set_last_applied_seq(&room_id, page.next_after_seq)
                .map_err(ClientStoreError::from)?;
            dirty = true;
        }
        if dirty {
            store.save_device_state(device)?;
        }

        if !page.has_more {
            break;
        }
        if page.next_after_seq == after_seq {
            return Err(ClientError::RuntimeSyncStalled { room_id, after_seq }.into());
        }
        after_seq = page.next_after_seq;
    }

    debug_assert!(pages <= options.max_sync_pages_per_room);
    Ok(())
}

#[derive(Debug)]
pub enum RuntimeWorkerError<E> {
    Delivery(E),
    Client(ClientError),
    ClientStore(ClientStoreError),
}

impl<E> From<ClientError> for RuntimeWorkerError<E> {
    fn from(error: ClientError) -> Self {
        Self::Client(error)
    }
}

impl<E> From<ClientStoreError> for RuntimeWorkerError<E> {
    fn from(error: ClientStoreError) -> Self {
        Self::ClientStore(error)
    }
}

impl<E: std::fmt::Display> std::fmt::Display for RuntimeWorkerError<E> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Delivery(error) => write!(formatter, "delivery operation failed: {error}"),
            Self::Client(error) => write!(formatter, "{error}"),
            Self::ClientStore(error) => write!(formatter, "{error}"),
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for RuntimeWorkerError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Delivery(error) => Some(error),
            Self::Client(error) => Some(error),
            Self::ClientStore(error) => Some(error),
        }
    }
}

#[derive(Debug, Error)]
pub enum ClientStoreError {
    #[error(transparent)]
    Client(#[from] ClientError),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error("failed to create sqlite client store directory {path}: {source}")]
    CreateDir {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("client state not found for {account_id}/{device_id}")]
    DeviceStateNotFound {
        account_id: String,
        device_id: String,
    },
    #[error(
        "legacy unencrypted client store table {table:?} contains state; explicit migration is required"
    )]
    LegacyUnencryptedStatePresent { table: LegacyClientStoreTable },
    #[error("failed to generate encrypted client store nonce")]
    Randomness,
    #[error("failed to encrypt client state")]
    EncryptState,
    #[error("failed to decrypt client state")]
    DecryptState,
    #[error("encrypted client state nonce has {actual_bytes} bytes")]
    InvalidNonceLength { actual_bytes: usize },
    #[error("encrypted client state snapshot has malformed magic")]
    StateSnapshotMagic,
    #[error("encrypted client state snapshot version {0} is not supported")]
    StateSnapshotVersion(u16),
    #[error("encrypted client state snapshot is truncated")]
    StateSnapshotTruncated,
    #[error("encrypted client state snapshot has trailing bytes")]
    StateSnapshotTrailingBytes,
    #[error("encrypted client state snapshot has invalid UTF-8")]
    StateSnapshotUtf8,
    #[error("encrypted client state snapshot enum {field} has unknown value {value}")]
    StateSnapshotEnum { field: &'static str, value: u64 },
    #[error("encrypted client state snapshot length overflow")]
    StateSnapshotLengthOverflow,
    #[error("encrypted client state snapshot identity does not match lookup")]
    StateSnapshotIdentityMismatch,
}

#[derive(Debug, Error)]
pub enum ClientError {
    #[error(transparent)]
    MlsCredential(#[from] MlsCredentialError),
    #[error(transparent)]
    ProtocolLimit(#[from] ProtocolLimitError),
    #[error("engine rejected request: {0}")]
    Engine(#[from] EngineError),
    #[error("failed to derive envelope message id")]
    EnvelopeMessageId(#[source] serde_json::Error),
    #[error("failed to create OpenMLS signer")]
    CreateSigner,
    #[error("failed to store OpenMLS signer")]
    StoreSigner,
    #[error("failed to create OpenMLS group")]
    CreateGroup,
    #[error("failed to build OpenMLS KeyPackage")]
    BuildKeyPackage,
    #[error("failed to draw randomness from the crypto provider")]
    Randomness,
    #[error("failed to seal or open an ephemeral activity payload")]
    ActivityCiphertext,
    #[error("activity payload epoch {payload_epoch} does not match group epoch {group_epoch}")]
    ActivityEpochMismatch { payload_epoch: u64, group_epoch: u64 },
    #[error("invite code is invalid: {0}")]
    InviteCode(String),
    #[error("invite session room {actual} does not match invite code room {expected}")]
    InviteRoomMismatch { expected: String, actual: String },
    #[error("no KeyPackage available for invited device {0}")]
    InviteKeyPackageUnavailable(String),
    #[error("room {room_id} has no verified member for account {account_id}")]
    AccountNotInRoom { room_id: String, account_id: String },
    #[error("failed to serialize OpenMLS KeyPackage")]
    SerializeKeyPackage,
    #[error("failed to parse OpenMLS KeyPackage")]
    ParseKeyPackage,
    #[error("claimed KeyPackage ref does not match payload")]
    KeyPackageRefMismatch,
    #[error("claimed KeyPackage hash does not match payload")]
    KeyPackageHashMismatch,
    #[error("KeyPackage inventory owner mismatch: expected {expected:?}, actual {actual:?}")]
    KeyPackageInventoryOwnerMismatch {
        expected: DeviceRef,
        actual: DeviceRef,
    },
    #[error(
        "KeyPackage inventory exceeds cap: {available} available and {leased} leased, max {max}"
    )]
    KeyPackageInventoryOverCap {
        available: u32,
        leased: u32,
        max: u32,
    },
    #[error(
        "KeyPackage replenishment exceeds cap: {available} available, {leased} leased, {pending} pending uploads, max {max}"
    )]
    KeyPackagePendingUploadOverCap {
        available: u32,
        leased: u32,
        pending: u32,
        max: u32,
    },
    #[error("failed to hash OpenMLS KeyPackage ref")]
    HashKeyPackageRef,
    #[error("failed to add OpenMLS member")]
    AddMember,
    #[error("failed to remove OpenMLS member")]
    RemoveMember,
    #[error("failed to create OpenMLS self-update commit")]
    SelfUpdate,
    #[error("remove-member commit cannot remove the sender")]
    CannotRemoveSelf,
    #[error("non-add commit unexpectedly produced a Welcome")]
    UnexpectedWelcomeForNonAddCommit,
    #[error("failed to serialize OpenMLS message")]
    SerializeMessage,
    #[error("failed to export pending ratchet tree")]
    ExportPendingRatchetTree,
    #[error("failed to serialize ratchet tree")]
    SerializeRatchetTree,
    #[error("failed to parse ratchet tree")]
    ParseRatchetTree,
    #[error("failed to parse Welcome")]
    ParseWelcome,
    #[error("failed to stage Welcome")]
    StageWelcome,
    #[error("failed to activate Welcome")]
    ActivateWelcome,
    #[error("failed to merge pending commit")]
    MergePendingCommit,
    #[error("failed to clear losing pending commit")]
    ClearPendingCommit,
    #[error("failed to merge staged remote commit")]
    MergeStagedCommit,
    #[error("failed to create application message")]
    CreateApplicationMessage,
    #[error("failed to parse protocol message")]
    ParseProtocolMessage,
    #[error("failed to process MLS message")]
    ProcessMessage,
    #[error("unexpected MLS message content")]
    UnexpectedMessage,
    #[error("group already exists: {0}")]
    GroupAlreadyExists(RoomId),
    #[error("group not found: {0}")]
    GroupNotFound(RoomId),
    #[error("pending commit already exists for group: {0}")]
    PendingCommitExists(RoomId),
    #[error("pending commit must be merged before sending application data: {0}")]
    PendingCommitMustBeMerged(RoomId),
    #[error("pending commit is missing for group: {0}")]
    MissingPendingCommit(RoomId),
    #[error("pending commit was not observed in the ordered server log: {0}")]
    PendingCommitNotObserved(String),
    #[error("invite batch must contain at least one KeyPackage")]
    EmptyInviteBatch,
    #[error("invite batch has {key_packages} KeyPackages but {welcome_ids} Welcome ids")]
    InviteWelcomeCountMismatch {
        key_packages: usize,
        welcome_ids: usize,
    },
    #[error("invite batch contains duplicate device: {0:?}")]
    DuplicateInviteDevice(DeviceRef),
    #[error("invite batch contains duplicate KeyPackage: {0}")]
    DuplicateInviteKeyPackage(KeyPackageId),
    #[error("invite batch contains duplicate Welcome id: {0}")]
    DuplicateInviteWelcome(WelcomeId),
    #[error("pending Welcome already exists: {0}")]
    DuplicatePendingWelcome(WelcomeId),
    #[error("pending Welcome ack already exists: {0}")]
    DuplicatePendingWelcomeAck(WelcomeId),
    #[error("pending Welcome cannot also need ack: {0}")]
    PendingWelcomeAlsoNeedsAck(WelcomeId),
    #[error("pending Welcome ack room is missing: {0}")]
    PendingWelcomeAckRoomMissing(RoomId),
    #[error("pending Welcome not found: {0}")]
    PendingWelcomeNotFound(WelcomeId),
    #[error("pending Welcome ack not found: {0}")]
    PendingWelcomeAckNotFound(WelcomeId),
    #[error("pending Welcome recipient does not match this device")]
    PendingWelcomeRecipientMismatch,
    #[error("pending KeyPackage upload already exists: {0}")]
    DuplicatePendingKeyPackageUpload(KeyPackageId),
    #[error("pending KeyPackage upload not found: {0}")]
    PendingKeyPackageUploadNotFound(KeyPackageId),
    #[error("pending KeyPackage upload owner mismatch: expected {expected:?}, actual {actual:?}")]
    PendingKeyPackageUploadOwnerMismatch {
        expected: DeviceRef,
        actual: DeviceRef,
    },
    #[error("prepared commit message id does not match request")]
    PreparedCommitMessageIdMismatch,
    #[error("link fanout already exists: {0}")]
    DuplicateLinkFanout(String),
    #[error("link fanout not found: {0}")]
    LinkFanoutNotFound(String),
    #[error("link fanout room already exists: {0}")]
    DuplicateLinkFanoutRoom(RoomId),
    #[error("link fanout room not found: {0}")]
    LinkFanoutRoomNotFound(RoomId),
    #[error("missing link fanout room plan: {0}")]
    MissingLinkFanoutRoomPlan(RoomId),
    #[error("unexpected link fanout room plan: {0}")]
    UnexpectedLinkFanoutRoomPlan(RoomId),
    #[error("missing link fanout claimed KeyPackage for room: {0}")]
    MissingLinkFanoutClaim(RoomId),
    #[error("unexpected link fanout claimed KeyPackage: {0}")]
    UnexpectedLinkFanoutClaim(KeyPackageId),
    #[error("link fanout target account mismatch: expected {expected}, actual {actual}")]
    LinkFanoutAccountMismatch { expected: String, actual: String },
    #[error("link fanout cannot target the current device")]
    LinkFanoutCannotTargetSelf,
    #[error("link fanout room is not pending: {0}")]
    LinkFanoutRoomNotPending(RoomId),
    #[error("link fanout room is not prepared: {0}")]
    LinkFanoutRoomNotPrepared(RoomId),
    #[error(
        "link fanout claimed KeyPackage owner mismatch: expected {expected:?}, actual {actual:?}"
    )]
    LinkFanoutClaimTargetMismatch {
        expected: DeviceRef,
        actual: DeviceRef,
    },
    #[error("link fanout claimed KeyPackage id mismatch: expected {expected}, actual {actual}")]
    LinkFanoutClaimPackageMismatch {
        expected: KeyPackageId,
        actual: KeyPackageId,
    },
    #[error("link fanout prepared Commit mismatch: expected {expected}, actual {actual}")]
    LinkFanoutPreparedCommitMismatch { expected: String, actual: String },
    #[error("member credential missing or duplicated: {0:?}")]
    MemberCredentialMissing(DeviceRef),
    #[error("persisted client state account does not match config")]
    PersistedAccountMismatch,
    #[error("persisted client state device does not match config")]
    PersistedDeviceMismatch,
    #[error("persisted room has duplicate room id: {0}")]
    DuplicatePersistedRoom(RoomId),
    #[error("persisted room {0} has mismatched MLS group id")]
    PersistedGroupIdMismatch(RoomId),
    #[error("persisted OpenMLS storage is empty")]
    MissingOpenMlsStorage,
    #[error("persisted OpenMLS storage has duplicate key")]
    DuplicateOpenMlsStorageKey,
    #[error("failed to lock OpenMLS storage")]
    OpenMlsStorageLock,
    #[error("persisted signer is missing")]
    MissingStoredSigner,
    #[error("persisted signer does not match credential leaf key")]
    StoredSignerMismatch,
    #[error("failed to load persisted MLS group state: {0}")]
    LoadGroupState(RoomId),
    #[error("persisted MLS group state is missing: {0}")]
    MissingGroupState(RoomId),
    #[error("runtime worker counter overflow")]
    RuntimeCounterOverflow,
    #[error("runtime sync cursor regressed for {room_id}: {current_seq} -> {next_after_seq}")]
    RuntimeSyncCursorRegression {
        room_id: RoomId,
        current_seq: u64,
        next_after_seq: u64,
    },
    #[error("runtime sync stalled for {room_id} after seq {after_seq}")]
    RuntimeSyncStalled { room_id: RoomId, after_seq: u64 },
    #[error("runtime link fanout discovery stalled for {fanout_id} after room {after_room_id:?}")]
    RuntimeLinkFanoutDiscoveryStalled {
        fanout_id: String,
        after_room_id: Option<RoomId>,
    },
    #[error("log entry room mismatch: expected {expected}, actual {actual}")]
    LogEntryRoomMismatch { expected: RoomId, actual: RoomId },
    #[error("log entry envelope room mismatch: entry {entry_room}, envelope {envelope_room}")]
    LogEntryEnvelopeRoomMismatch {
        entry_room: RoomId,
        envelope_room: RoomId,
    },
    #[error("log entry kind mismatch: expected {expected:?}, actual {actual:?}")]
    LogEntryKindMismatch {
        expected: LogEntryKind,
        actual: LogEntryKind,
    },
    #[error("log entry envelope kind mismatch: entry {entry_kind:?}, envelope {envelope_kind:?}")]
    LogEntryEnvelopeKindMismatch {
        entry_kind: LogEntryKind,
        envelope_kind: LogEntryKind,
    },
    #[error(
        "log entry message id does not match envelope: entry {entry_message_id}, envelope {envelope_message_id}"
    )]
    LogEntryMessageIdMismatch {
        entry_message_id: String,
        envelope_message_id: String,
    },
    #[error("log entry sender does not match envelope")]
    LogEntrySenderMismatch,
    #[error("log entry epoch {entry_epoch} does not match envelope epoch {envelope_epoch}")]
    LogEntryEpochMismatch {
        entry_epoch: u64,
        envelope_epoch: u64,
    },
    #[error("unsupported log entry kind: {0:?}")]
    UnsupportedLogEntryKind(LogEntryKind),
    #[error("commit epoch mismatch for {room_id}: local {current_epoch}, entry {entry_epoch}")]
    UnexpectedCommitEpoch {
        room_id: RoomId,
        current_epoch: u64,
        entry_epoch: u64,
    },
    #[error("post-commit epoch overflow")]
    EpochOverflow,
    #[error(
        "post-commit epoch mismatch for {room_id}: expected {expected_epoch}, actual {actual_epoch}"
    )]
    UnexpectedPostCommitEpoch {
        room_id: RoomId,
        expected_epoch: u64,
        actual_epoch: u64,
    },
    #[error("own commit has no pending local state: {0}")]
    OwnCommitWithoutPendingState(String),
    #[error("account id is not a 32-byte lowercase hex Nostr public key: {0}")]
    MalformedAccountId(String),
    #[error("MLS group id is not valid UTF-8")]
    MlsGroupIdNotUtf8,
    #[error(
        "applied seq regression for {room_id}: current {current_seq}, attempted {attempted_seq}"
    )]
    AppliedSeqRegression {
        room_id: RoomId,
        current_seq: u64,
        attempted_seq: u64,
    },
}

fn verified_key_package_from_claim(
    provider: &OpenMlsRustCrypto,
    claimed_key_package: &ClaimKeyPackageResult,
    now_unix_seconds: u64,
) -> Result<KeyPackage, ClientError> {
    validate_claimed_key_package(claimed_key_package)?;
    let key_package_in =
        KeyPackageIn::tls_deserialize_exact(claimed_key_package.key_package_payload.as_slice())
            .map_err(|_| ClientError::ParseKeyPackage)?;
    let key_package = key_package_in
        .validate(provider.crypto(), ProtocolVersion::Mls10)
        .map_err(|_| ClientError::ParseKeyPackage)?;
    let key_package_ref = key_package
        .hash_ref(provider.crypto())
        .map_err(|_| ClientError::HashKeyPackageRef)?;
    if hex_lower(key_package_ref.as_slice()) != claimed_key_package.key_package_ref {
        return Err(ClientError::KeyPackageRefMismatch);
    }
    if message_id_for_bytes(&claimed_key_package.key_package_payload)
        != claimed_key_package.key_package_hash
    {
        return Err(ClientError::KeyPackageHashMismatch);
    }

    let credential =
        FiniteDeviceCredentialV1::from_credential(key_package.leaf_node().credential().clone())?;
    credential.verify_expected(ExpectedDeviceCredential {
        account_public_key: account_public_key_from_device_ref(&claimed_key_package.owner)?,
        device_id: &claimed_key_package.owner.device_id,
        mls_leaf_signing_public_key: key_package.leaf_node().signature_key().as_slice(),
        now_unix_seconds,
    })?;
    Ok(key_package)
}

fn validate_claimed_key_package(
    claimed_key_package: &ClaimKeyPackageResult,
) -> Result<(), ClientError> {
    claimed_key_package
        .owner
        .validate_limits()
        .map_err(ClientError::from)?;
    validate_string_bytes(
        "claimed_key_package.key_package_id",
        &claimed_key_package.key_package_id,
        MAX_OBJECT_ID_BYTES,
    )?;
    validate_string_bytes(
        "claimed_key_package.key_package_ref",
        &claimed_key_package.key_package_ref,
        MAX_OBJECT_ID_BYTES,
    )?;
    validate_string_bytes(
        "claimed_key_package.key_package_hash",
        &claimed_key_package.key_package_hash,
        MAX_OBJECT_ID_BYTES,
    )?;
    validate_bytes_non_empty(
        "claimed_key_package.key_package_payload",
        claimed_key_package.key_package_payload.len(),
    )?;
    validate_bytes_len(
        "claimed_key_package.key_package_payload",
        claimed_key_package.key_package_payload.len(),
        MAX_KEY_PACKAGE_PAYLOAD_BYTES,
    )?;
    validate_string_bytes(
        "claimed_key_package.lease_token",
        &claimed_key_package.lease_token,
        MAX_OBJECT_ID_BYTES,
    )?;
    Ok(())
}

fn link_fanout_derived_id(
    prefix: &str,
    fanout_id: &str,
    room_id: &str,
    key_package_id: &str,
) -> Result<String, ClientError> {
    validate_string_bytes("link_fanout.id_prefix", prefix, MAX_OBJECT_ID_BYTES)?;
    validate_string_bytes("link_fanout.fanout_id", fanout_id, MAX_OBJECT_ID_BYTES)?;
    validate_room_id(room_id)?;
    validate_string_bytes(
        "link_fanout.key_package_id",
        key_package_id,
        MAX_OBJECT_ID_BYTES,
    )?;
    let mut seed = Vec::with_capacity(
        prefix.len() + fanout_id.len() + room_id.len() + key_package_id.len() + 4,
    );
    seed.extend_from_slice(prefix.as_bytes());
    seed.push(0);
    seed.extend_from_slice(fanout_id.as_bytes());
    seed.push(0);
    seed.extend_from_slice(room_id.as_bytes());
    seed.push(0);
    seed.extend_from_slice(key_package_id.as_bytes());
    let value = format!("{prefix}_{}", message_id_for_bytes(&seed));
    validate_string_bytes("link_fanout.derived_id", &value, MAX_OBJECT_ID_BYTES)?;
    debug_assert!(value.starts_with(prefix));
    Ok(value)
}

fn verify_staged_commit_credentials(
    now_unix_seconds: u64,
    staged_commit: &StagedCommit,
) -> Result<(), ClientError> {
    for credential in staged_commit.credentials_to_verify() {
        let credential = FiniteDeviceCredentialV1::from_credential(credential.clone())?;
        credential.verify_expected(ExpectedDeviceCredential {
            account_public_key: credential.account_public_key(),
            device_id: credential.device_id(),
            mls_leaf_signing_public_key: credential.mls_leaf_signing_public_key(),
            now_unix_seconds,
        })?;
    }
    Ok(())
}

fn verified_member_leaf_index(
    group: &MlsGroup,
    device: &DeviceRef,
    now_unix_seconds: u64,
) -> Result<LeafNodeIndex, ClientError> {
    let expected_account_public_key = account_public_key_from_device_ref(device)?;
    let mut matched_index = None;
    for member in group.members() {
        let credential = FiniteDeviceCredentialV1::from_credential(member.credential)?;
        if credential.account_public_key() != expected_account_public_key
            || credential.device_id() != device.device_id
        {
            continue;
        }
        credential.verify_expected(ExpectedDeviceCredential {
            account_public_key: expected_account_public_key,
            device_id: &device.device_id,
            mls_leaf_signing_public_key: &member.signature_key,
            now_unix_seconds,
        })?;
        if matched_index.replace(member.index).is_some() {
            return Err(ClientError::MemberCredentialMissing(device.clone()));
        }
    }
    matched_index.ok_or_else(|| ClientError::MemberCredentialMissing(device.clone()))
}

fn openmls_group_config() -> MlsGroupCreateConfig {
    MlsGroupCreateConfig::builder()
        .ciphersuite(FINITECHAT_CIPHERSUITE)
        .use_ratchet_tree_extension(false)
        .build()
}

fn welcome_from_bytes(bytes: &[u8]) -> Result<Welcome, ClientError> {
    let message = mls_message_in_from_bytes(bytes)?;
    let MlsMessageBodyIn::Welcome(welcome) = message.extract() else {
        return Err(ClientError::ParseWelcome);
    };
    Ok(welcome)
}

fn ratchet_tree_from_bytes(bytes: &[u8]) -> Result<RatchetTreeIn, ClientError> {
    if bytes.is_empty() {
        return Err(ClientError::ParseRatchetTree);
    }
    RatchetTreeIn::tls_deserialize_exact(bytes).map_err(|_| ClientError::ParseRatchetTree)
}

fn protocol_message_from_bytes(bytes: &[u8]) -> Result<ProtocolMessage, ClientError> {
    mls_message_in_from_bytes(bytes)?
        .try_into_protocol_message()
        .map_err(|_| ClientError::ParseProtocolMessage)
}

fn validate_log_entry_shape(
    room_id: &str,
    entry: &RoomLogEntry,
    expected_kind: LogEntryKind,
) -> Result<(), ClientError> {
    validate_room_id(room_id)?;
    if entry.room_id != room_id {
        return Err(ClientError::LogEntryRoomMismatch {
            expected: room_id.to_string(),
            actual: entry.room_id.clone(),
        });
    }
    if entry.envelope.room_id != entry.room_id {
        return Err(ClientError::LogEntryEnvelopeRoomMismatch {
            entry_room: entry.room_id.clone(),
            envelope_room: entry.envelope.room_id.clone(),
        });
    }
    if entry.kind != expected_kind {
        return Err(ClientError::LogEntryKindMismatch {
            expected: expected_kind,
            actual: entry.kind,
        });
    }
    if entry.envelope.kind != entry.kind {
        return Err(ClientError::LogEntryEnvelopeKindMismatch {
            entry_kind: entry.kind,
            envelope_kind: entry.envelope.kind,
        });
    }
    let envelope_message_id = entry
        .envelope
        .message_id()
        .map_err(ClientError::EnvelopeMessageId)?;
    if entry.message_id != envelope_message_id {
        return Err(ClientError::LogEntryMessageIdMismatch {
            entry_message_id: entry.message_id.clone(),
            envelope_message_id,
        });
    }
    if entry.sender != entry.envelope.sender {
        return Err(ClientError::LogEntrySenderMismatch);
    }
    if entry.epoch != entry.envelope.epoch {
        return Err(ClientError::LogEntryEpochMismatch {
            entry_epoch: entry.epoch,
            envelope_epoch: entry.envelope.epoch,
        });
    }
    debug_assert_eq!(entry.room_id, room_id);
    debug_assert_eq!(entry.kind, expected_kind);
    Ok(())
}

fn post_commit_epoch(epoch: u64) -> Result<u64, ClientError> {
    epoch.checked_add(1).ok_or(ClientError::EpochOverflow)
}

fn mls_message_out_bytes(message: MlsMessageOut) -> Result<Vec<u8>, ClientError> {
    let bytes = message
        .to_bytes()
        .map_err(|_| ClientError::SerializeMessage)?;
    debug_assert!(!bytes.is_empty());
    Ok(bytes)
}

fn mls_message_in_from_bytes(mut bytes: &[u8]) -> Result<MlsMessageIn, ClientError> {
    if bytes.is_empty() {
        return Err(ClientError::ParseProtocolMessage);
    }
    MlsMessageIn::tls_deserialize(&mut bytes).map_err(|_| ClientError::ParseProtocolMessage)
}

/// Generate a fresh account secret key from the crypto provider's RNG.
/// Used by agent onboarding (`hermes init`): each agent gets its own Nostr
/// identity that never needs a public relay (ADR 0006 §5).
pub fn generate_account_secret() -> Result<NostrSecretKey, ClientError> {
    let provider = OpenMlsRustCrypto::default();
    for _ in 0..8 {
        let bytes: [u8; NOSTR_SECRET_KEY_BYTES] = provider
            .rand()
            .random_array()
            .map_err(|_| ClientError::Randomness)?;
        if let Ok(secret) = NostrSecretKey::from_bytes(bytes) {
            return Ok(secret);
        }
    }
    Err(ClientError::Randomness)
}

fn account_public_key_from_device_ref(device: &DeviceRef) -> Result<NostrPublicKey, ClientError> {
    let bytes = decode_lower_hex_32(&device.account_id)?;
    NostrPublicKey::from_bytes(bytes).map_err(ClientError::from)
}

fn decode_lower_hex_32(value: &str) -> Result<[u8; NOSTR_PUBLIC_KEY_BYTES], ClientError> {
    if value.len() != NOSTR_PUBLIC_KEY_BYTES * 2 {
        return Err(ClientError::MalformedAccountId(value.to_string()));
    }
    let mut bytes = [0u8; NOSTR_PUBLIC_KEY_BYTES];
    for (index, chunk) in value.as_bytes().chunks_exact(2).enumerate() {
        let high = decode_lower_hex_nibble(chunk[0])
            .ok_or_else(|| ClientError::MalformedAccountId(value.to_string()))?;
        let low = decode_lower_hex_nibble(chunk[1])
            .ok_or_else(|| ClientError::MalformedAccountId(value.to_string()))?;
        bytes[index] = (high << 4) | low;
    }
    Ok(bytes)
}

fn decode_lower_hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

fn mls_group_id_string(group_id: &GroupId) -> Result<String, ClientError> {
    String::from_utf8(group_id.as_slice().to_vec()).map_err(|_| ClientError::MlsGroupIdNotUtf8)
}

fn hex_lower(bytes: &[u8]) -> String {
    const TABLE: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(TABLE[(byte >> 4) as usize] as char);
        out.push(TABLE[(byte & 0x0f) as usize] as char);
    }
    out
}

fn migrate_client_store(conn: &Connection) -> Result<(), ClientStoreError> {
    reject_or_remove_legacy_client_store_tables(conn)?;
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS client_device_states (
          account_id TEXT NOT NULL,
          device_id TEXT NOT NULL,
          nonce BLOB NOT NULL,
          ciphertext BLOB NOT NULL,
          PRIMARY KEY (account_id, device_id)
        );
        "#,
    )?;
    Ok(())
}

fn reject_or_remove_legacy_client_store_tables(conn: &Connection) -> Result<(), ClientStoreError> {
    let tables = [
        LegacyClientStoreTable::OpenMlsStorage,
        LegacyClientStoreTable::Rooms,
        LegacyClientStoreTable::Profiles,
    ];
    for table in tables {
        if !legacy_table_exists(conn, table)? {
            continue;
        }
        if legacy_table_row_count(conn, table)? > 0 {
            return Err(ClientStoreError::LegacyUnencryptedStatePresent { table });
        }
    }
    conn.execute_batch(
        r#"
        DROP TABLE IF EXISTS client_openmls_storage;
        DROP TABLE IF EXISTS client_rooms;
        DROP TABLE IF EXISTS client_profiles;
        "#,
    )?;
    Ok(())
}

fn legacy_table_exists(
    conn: &Connection,
    table: LegacyClientStoreTable,
) -> Result<bool, ClientStoreError> {
    let exists = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        params![table.name()],
        |row| row.get::<_, bool>(0),
    )?;
    Ok(exists)
}

fn legacy_table_row_count(
    conn: &Connection,
    table: LegacyClientStoreTable,
) -> Result<u64, ClientStoreError> {
    let count = match table {
        LegacyClientStoreTable::Profiles => {
            conn.query_row("SELECT COUNT(*) FROM client_profiles", [], |row| {
                row.get::<_, u64>(0)
            })?
        }
        LegacyClientStoreTable::Rooms => {
            conn.query_row("SELECT COUNT(*) FROM client_rooms", [], |row| {
                row.get::<_, u64>(0)
            })?
        }
        LegacyClientStoreTable::OpenMlsStorage => {
            conn.query_row("SELECT COUNT(*) FROM client_openmls_storage", [], |row| {
                row.get::<_, u64>(0)
            })?
        }
    };
    Ok(count)
}

fn save_device_state_tx(
    tx: &Transaction<'_>,
    state: &FiniteChatDeviceState,
    encryption_key: &ClientStoreEncryptionKey,
) -> Result<(), ClientStoreError> {
    state.validate_limits()?;
    let sealed = encrypt_device_state(encryption_key, state)?;
    tx.execute(
        r#"
        INSERT INTO client_device_states (
          account_id, device_id, nonce, ciphertext
        ) VALUES (?1, ?2, ?3, ?4)
        ON CONFLICT(account_id, device_id) DO UPDATE SET
          nonce = excluded.nonce,
          ciphertext = excluded.ciphertext
        "#,
        params![
            state.device_ref.account_id,
            state.device_ref.device_id,
            sealed.nonce,
            sealed.ciphertext,
        ],
    )?;
    Ok(())
}

fn load_device_state(
    conn: &Connection,
    encryption_key: &ClientStoreEncryptionKey,
    account_id: &str,
    device_id: &str,
) -> Result<Option<FiniteChatDeviceState>, ClientStoreError> {
    validate_string_bytes("account_id", account_id, MAX_ACCOUNT_ID_BYTES)
        .map_err(ClientError::from)?;
    validate_string_bytes("device_id", device_id, MAX_DEVICE_ID_BYTES)
        .map_err(ClientError::from)?;
    let sealed = conn
        .query_row(
            r#"
            SELECT nonce, ciphertext
            FROM client_device_states
            WHERE account_id = ?1 AND device_id = ?2
            "#,
            params![account_id, device_id],
            |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?)),
        )
        .optional()?;
    let Some((nonce, ciphertext)) = sealed else {
        return Ok(None);
    };

    let state = decrypt_device_state(encryption_key, account_id, device_id, &nonce, &ciphertext)?;
    if state.device_ref.account_id != account_id || state.device_ref.device_id != device_id {
        return Err(ClientStoreError::StateSnapshotIdentityMismatch);
    }
    state.validate_limits()?;
    Ok(Some(state))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SealedClientState {
    nonce: Vec<u8>,
    ciphertext: Vec<u8>,
}

fn encrypt_device_state(
    encryption_key: &ClientStoreEncryptionKey,
    state: &FiniteChatDeviceState,
) -> Result<SealedClientState, ClientStoreError> {
    state.validate_limits()?;
    let plaintext = encode_device_state(state)?;
    let aad = client_store_aad(&state.device_ref.account_id, &state.device_ref.device_id)?;
    let provider = OpenMlsRustCrypto::default();
    let nonce: [u8; CLIENT_STORE_NONCE_BYTES] = provider
        .rand()
        .random_array()
        .map_err(|_| ClientStoreError::Randomness)?;
    let ciphertext = provider
        .crypto()
        .aead_encrypt(
            AeadType::Aes256Gcm,
            encryption_key.as_bytes(),
            &plaintext,
            &nonce,
            &aad,
        )
        .map_err(|_| ClientStoreError::EncryptState)?;
    validate_bytes_len(
        "client_state.ciphertext",
        ciphertext.len(),
        MAX_CLIENT_STATE_CIPHERTEXT_BYTES,
    )
    .map_err(ClientError::from)?;
    debug_assert_eq!(nonce.len(), CLIENT_STORE_NONCE_BYTES);
    debug_assert!(!ciphertext.is_empty());
    Ok(SealedClientState {
        nonce: nonce.to_vec(),
        ciphertext,
    })
}

fn decrypt_device_state(
    encryption_key: &ClientStoreEncryptionKey,
    account_id: &str,
    device_id: &str,
    nonce: &[u8],
    ciphertext: &[u8],
) -> Result<FiniteChatDeviceState, ClientStoreError> {
    if nonce.len() != CLIENT_STORE_NONCE_BYTES {
        return Err(ClientStoreError::InvalidNonceLength {
            actual_bytes: nonce.len(),
        });
    }
    validate_bytes_non_empty("client_state.ciphertext", ciphertext.len())
        .map_err(ClientError::from)?;
    validate_bytes_len(
        "client_state.ciphertext",
        ciphertext.len(),
        MAX_CLIENT_STATE_CIPHERTEXT_BYTES,
    )
    .map_err(ClientError::from)?;
    let aad = client_store_aad(account_id, device_id)?;
    let provider = OpenMlsRustCrypto::default();
    let plaintext = provider
        .crypto()
        .aead_decrypt(
            AeadType::Aes256Gcm,
            encryption_key.as_bytes(),
            ciphertext,
            nonce,
            &aad,
        )
        .map_err(|_| ClientStoreError::DecryptState)?;
    decode_device_state(&plaintext)
}

fn client_store_aad(account_id: &str, device_id: &str) -> Result<Vec<u8>, ClientStoreError> {
    validate_string_bytes("account_id", account_id, MAX_ACCOUNT_ID_BYTES)
        .map_err(ClientError::from)?;
    validate_string_bytes("device_id", device_id, MAX_DEVICE_ID_BYTES)
        .map_err(ClientError::from)?;
    let mut aad = Vec::with_capacity(
        CLIENT_STATE_SNAPSHOT_MAGIC.len()
            + U16_BYTES
            + U32_BYTES
            + account_id.len()
            + U32_BYTES
            + device_id.len(),
    );
    aad.extend_from_slice(CLIENT_STATE_SNAPSHOT_MAGIC);
    aad.extend_from_slice(&CLIENT_STATE_SNAPSHOT_VERSION.to_be_bytes());
    append_raw_len_prefixed(&mut aad, account_id.as_bytes())?;
    append_raw_len_prefixed(&mut aad, device_id.as_bytes())?;
    debug_assert!(aad.len() >= CLIENT_STATE_SNAPSHOT_MAGIC.len() + U16_BYTES);
    Ok(aad)
}

fn encode_device_state(state: &FiniteChatDeviceState) -> Result<Vec<u8>, ClientStoreError> {
    state.validate_limits()?;
    let mut out = Vec::with_capacity(encoded_device_state_len(state)?);
    out.extend_from_slice(CLIENT_STATE_SNAPSHOT_MAGIC);
    out.extend_from_slice(&CLIENT_STATE_SNAPSHOT_VERSION.to_be_bytes());
    append_string_field(
        &mut out,
        "account_id",
        &state.device_ref.account_id,
        MAX_ACCOUNT_ID_BYTES,
    )?;
    append_string_field(
        &mut out,
        "device_id",
        &state.device_ref.device_id,
        MAX_DEVICE_ID_BYTES,
    )?;
    append_bytes_field(
        &mut out,
        "signer_public_key",
        &state.signer_public_key,
        MAX_CLIENT_SIGNER_PUBLIC_KEY_BYTES,
    )?;
    append_bytes_field(
        &mut out,
        "credential_identity",
        &state.credential_identity,
        MAX_CLIENT_CREDENTIAL_IDENTITY_BYTES,
    )?;
    append_count(
        &mut out,
        "client_state.rooms",
        state.rooms.len(),
        MAX_PERSISTED_ROOMS,
    )?;
    for room in &state.rooms {
        room.validate_limits()?;
        append_string_field(
            &mut out,
            "room_id",
            &room.room_id,
            finitechat_proto::MAX_ROOM_ID_BYTES,
        )?;
        append_string_field(
            &mut out,
            "mls_group_id",
            &room.mls_group_id,
            finitechat_proto::MAX_MLS_GROUP_ID_BYTES,
        )?;
        out.extend_from_slice(&room.last_applied_seq.to_be_bytes());
        append_raw_len_prefixed(&mut out, room.server_url.as_deref().unwrap_or("").as_bytes())?;
    }
    append_count(
        &mut out,
        "client_state.pending_welcomes",
        state.pending_welcomes.len(),
        MAX_PENDING_CLIENT_WELCOMES,
    )?;
    for welcome in &state.pending_welcomes {
        welcome.validate_limits()?;
        append_string_field(
            &mut out,
            "welcome_id",
            &welcome.welcome_id,
            MAX_OBJECT_ID_BYTES,
        )?;
        append_string_field(
            &mut out,
            "pending_welcome.room_id",
            &welcome.room_id,
            finitechat_proto::MAX_ROOM_ID_BYTES,
        )?;
        out.extend_from_slice(&welcome.commit_seq.to_be_bytes());
        append_bytes_field(
            &mut out,
            "pending_welcome.welcome_payload",
            &welcome.welcome_payload,
            MAX_WELCOME_PAYLOAD_BYTES,
        )?;
        append_bytes_field(
            &mut out,
            "pending_welcome.ratchet_tree_payload",
            &welcome.ratchet_tree_payload,
            MAX_RATCHET_TREE_PAYLOAD_BYTES,
        )?;
    }
    append_count(
        &mut out,
        "client_state.pending_welcome_acks",
        state.pending_welcome_acks.len(),
        MAX_PENDING_CLIENT_WELCOMES,
    )?;
    for ack in &state.pending_welcome_acks {
        ack.validate_limits()?;
        append_string_field(
            &mut out,
            "welcome_ack.welcome_id",
            &ack.welcome_id,
            MAX_OBJECT_ID_BYTES,
        )?;
        append_string_field(
            &mut out,
            "welcome_ack.room_id",
            &ack.room_id,
            finitechat_proto::MAX_ROOM_ID_BYTES,
        )?;
        out.extend_from_slice(&ack.commit_seq.to_be_bytes());
    }
    append_count(
        &mut out,
        "client_state.pending_key_package_uploads",
        state.pending_key_package_uploads.len(),
        MAX_PENDING_KEY_PACKAGE_UPLOADS,
    )?;
    for request in &state.pending_key_package_uploads {
        request.validate_limits().map_err(ClientError::from)?;
        append_upload_key_package_request(&mut out, request)?;
    }
    append_count(
        &mut out,
        "client_state.link_fanouts",
        state.link_fanouts.len(),
        MAX_LINK_FANOUTS,
    )?;
    for fanout in &state.link_fanouts {
        append_link_fanout_state(&mut out, fanout)?;
    }
    append_count(
        &mut out,
        "client_state.openmls_storage_records",
        state.openmls_storage_records.len(),
        MAX_OPENMLS_STORAGE_RECORDS,
    )?;
    for record in &state.openmls_storage_records {
        record.validate_limits()?;
        append_bytes_field(
            &mut out,
            "openmls_storage.key",
            &record.key,
            MAX_OPENMLS_STORAGE_KEY_BYTES,
        )?;
        append_bytes_field(
            &mut out,
            "openmls_storage.value",
            &record.value,
            MAX_OPENMLS_STORAGE_VALUE_BYTES,
        )?;
    }
    validate_bytes_len(
        "client_state.plaintext",
        out.len(),
        MAX_CLIENT_STATE_PLAINTEXT_BYTES,
    )
    .map_err(ClientError::from)?;
    debug_assert!(!out.is_empty());
    Ok(out)
}

fn decode_device_state(bytes: &[u8]) -> Result<FiniteChatDeviceState, ClientStoreError> {
    validate_bytes_non_empty("client_state.plaintext", bytes.len()).map_err(ClientError::from)?;
    validate_bytes_len(
        "client_state.plaintext",
        bytes.len(),
        MAX_CLIENT_STATE_PLAINTEXT_BYTES,
    )
    .map_err(ClientError::from)?;
    let mut cursor = ClientStateCursor::new(bytes);
    cursor.take_magic()?;
    let version = cursor.take_u16()?;
    if version != CLIENT_STATE_SNAPSHOT_VERSION {
        return Err(ClientStoreError::StateSnapshotVersion(version));
    }
    let account_id = cursor.take_string("account_id", MAX_ACCOUNT_ID_BYTES)?;
    let device_id = cursor.take_string("device_id", MAX_DEVICE_ID_BYTES)?;
    let signer_public_key =
        cursor.take_vec("signer_public_key", MAX_CLIENT_SIGNER_PUBLIC_KEY_BYTES)?;
    let credential_identity =
        cursor.take_vec("credential_identity", MAX_CLIENT_CREDENTIAL_IDENTITY_BYTES)?;

    let room_count = cursor.take_count("client_state.rooms", MAX_PERSISTED_ROOMS)?;
    let mut rooms = Vec::with_capacity(room_count);
    for _ in 0..room_count {
        let room_id = cursor.take_string("room_id", finitechat_proto::MAX_ROOM_ID_BYTES)?;
        let mls_group_id =
            cursor.take_string("mls_group_id", finitechat_proto::MAX_MLS_GROUP_ID_BYTES)?;
        let last_applied_seq = cursor.take_u64()?;
        let server_url_bytes = cursor.take_vec("room.server_url", MAX_ROOM_SERVER_URL_BYTES)?;
        let server_url = if server_url_bytes.is_empty() {
            None
        } else {
            Some(
                String::from_utf8(server_url_bytes)
                    .map_err(|_| ClientStoreError::StateSnapshotUtf8)?,
            )
        };
        rooms.push(PersistedRoomState {
            room_id,
            mls_group_id,
            last_applied_seq,
            server_url,
        });
    }

    let pending_welcome_count =
        cursor.take_count("client_state.pending_welcomes", MAX_PENDING_CLIENT_WELCOMES)?;
    let mut pending_welcomes = Vec::with_capacity(pending_welcome_count);
    for _ in 0..pending_welcome_count {
        pending_welcomes.push(PendingWelcomeState {
            welcome_id: cursor.take_string("welcome_id", MAX_OBJECT_ID_BYTES)?,
            room_id: cursor.take_string(
                "pending_welcome.room_id",
                finitechat_proto::MAX_ROOM_ID_BYTES,
            )?,
            commit_seq: cursor.take_u64()?,
            welcome_payload: cursor
                .take_vec("pending_welcome.welcome_payload", MAX_WELCOME_PAYLOAD_BYTES)?,
            ratchet_tree_payload: cursor.take_vec(
                "pending_welcome.ratchet_tree_payload",
                MAX_RATCHET_TREE_PAYLOAD_BYTES,
            )?,
        });
    }

    let pending_welcome_ack_count = cursor.take_count(
        "client_state.pending_welcome_acks",
        MAX_PENDING_CLIENT_WELCOMES,
    )?;
    let mut pending_welcome_acks = Vec::with_capacity(pending_welcome_ack_count);
    for _ in 0..pending_welcome_ack_count {
        pending_welcome_acks.push(PendingWelcomeAckState {
            welcome_id: cursor.take_string("welcome_ack.welcome_id", MAX_OBJECT_ID_BYTES)?,
            room_id: cursor
                .take_string("welcome_ack.room_id", finitechat_proto::MAX_ROOM_ID_BYTES)?,
            commit_seq: cursor.take_u64()?,
        });
    }

    let pending_key_package_upload_count = cursor.take_count(
        "client_state.pending_key_package_uploads",
        MAX_PENDING_KEY_PACKAGE_UPLOADS,
    )?;
    let mut pending_key_package_uploads = Vec::with_capacity(pending_key_package_upload_count);
    for _ in 0..pending_key_package_upload_count {
        pending_key_package_uploads.push(cursor.take_upload_key_package_request()?);
    }

    let link_fanout_count = cursor.take_count("client_state.link_fanouts", MAX_LINK_FANOUTS)?;
    let mut link_fanouts = Vec::with_capacity(link_fanout_count);
    for _ in 0..link_fanout_count {
        link_fanouts.push(cursor.take_link_fanout_state()?);
    }

    let storage_count = cursor.take_count(
        "client_state.openmls_storage_records",
        MAX_OPENMLS_STORAGE_RECORDS,
    )?;
    let mut openmls_storage_records = Vec::with_capacity(storage_count);
    for _ in 0..storage_count {
        openmls_storage_records.push(OpenMlsStorageRecord {
            key: cursor.take_vec("openmls_storage.key", MAX_OPENMLS_STORAGE_KEY_BYTES)?,
            value: cursor.take_vec("openmls_storage.value", MAX_OPENMLS_STORAGE_VALUE_BYTES)?,
        });
    }
    cursor.finish()?;

    let state = FiniteChatDeviceState {
        device_ref: DeviceRef {
            account_id,
            device_id,
        },
        signer_public_key,
        credential_identity,
        rooms,
        pending_welcomes,
        pending_welcome_acks,
        pending_key_package_uploads,
        link_fanouts,
        openmls_storage_records,
    };
    state.validate_limits()?;
    Ok(state)
}

fn encoded_device_state_len(state: &FiniteChatDeviceState) -> Result<usize, ClientStoreError> {
    let mut len = CLIENT_STATE_SNAPSHOT_MAGIC.len() + U16_BYTES;
    len = checked_len_add(len, U32_BYTES + state.device_ref.account_id.len())?;
    len = checked_len_add(len, U32_BYTES + state.device_ref.device_id.len())?;
    len = checked_len_add(len, U32_BYTES + state.signer_public_key.len())?;
    len = checked_len_add(len, U32_BYTES + state.credential_identity.len())?;
    len = checked_len_add(len, U32_BYTES)?;
    for room in &state.rooms {
        len = checked_len_add(len, U32_BYTES + room.room_id.len())?;
        len = checked_len_add(len, U32_BYTES + room.mls_group_id.len())?;
        len = checked_len_add(len, U64_BYTES)?;
        len = checked_len_add(
            len,
            U32_BYTES + room.server_url.as_deref().unwrap_or("").len(),
        )?;
    }
    len = checked_len_add(len, U32_BYTES)?;
    for welcome in &state.pending_welcomes {
        len = checked_len_add(len, U32_BYTES + welcome.welcome_id.len())?;
        len = checked_len_add(len, U32_BYTES + welcome.room_id.len())?;
        len = checked_len_add(len, U64_BYTES)?;
        len = checked_len_add(len, U32_BYTES + welcome.welcome_payload.len())?;
        len = checked_len_add(len, U32_BYTES + welcome.ratchet_tree_payload.len())?;
    }
    len = checked_len_add(len, U32_BYTES)?;
    for ack in &state.pending_welcome_acks {
        len = checked_len_add(len, U32_BYTES + ack.welcome_id.len())?;
        len = checked_len_add(len, U32_BYTES + ack.room_id.len())?;
        len = checked_len_add(len, U64_BYTES)?;
    }
    len = checked_len_add(len, U32_BYTES)?;
    for request in &state.pending_key_package_uploads {
        len = checked_len_add(len, encoded_upload_key_package_request_len(request)?)?;
    }
    len = checked_len_add(len, U32_BYTES)?;
    for fanout in &state.link_fanouts {
        len = checked_len_add(len, encoded_link_fanout_state_len(fanout)?)?;
    }
    len = checked_len_add(len, U32_BYTES)?;
    for record in &state.openmls_storage_records {
        len = checked_len_add(len, U32_BYTES + record.key.len())?;
        len = checked_len_add(len, U32_BYTES + record.value.len())?;
    }
    validate_bytes_len(
        "client_state.plaintext",
        len,
        MAX_CLIENT_STATE_PLAINTEXT_BYTES,
    )
    .map_err(ClientError::from)?;
    Ok(len)
}

fn append_upload_key_package_request(
    out: &mut Vec<u8>,
    request: &UploadKeyPackageRequest,
) -> Result<(), ClientStoreError> {
    request.validate_limits().map_err(ClientError::from)?;
    append_string_field(
        out,
        "pending_key_package_upload.key_package_id",
        &request.key_package_id,
        MAX_OBJECT_ID_BYTES,
    )?;
    append_device_ref(out, "pending_key_package_upload.owner", &request.owner)?;
    append_string_field(
        out,
        "pending_key_package_upload.key_package_ref",
        &request.key_package_ref,
        MAX_OBJECT_ID_BYTES,
    )?;
    append_string_field(
        out,
        "pending_key_package_upload.key_package_hash",
        &request.key_package_hash,
        MAX_OBJECT_ID_BYTES,
    )?;
    append_bytes_field(
        out,
        "pending_key_package_upload.key_package_payload",
        &request.key_package_payload,
        MAX_KEY_PACKAGE_PAYLOAD_BYTES,
    )?;
    Ok(())
}

fn encoded_upload_key_package_request_len(
    request: &UploadKeyPackageRequest,
) -> Result<usize, ClientStoreError> {
    request.validate_limits().map_err(ClientError::from)?;
    let mut len = U32_BYTES + request.key_package_id.len();
    len = checked_len_add(len, encoded_device_ref_len(&request.owner)?)?;
    len = checked_len_add(len, U32_BYTES + request.key_package_ref.len())?;
    len = checked_len_add(len, U32_BYTES + request.key_package_hash.len())?;
    len = checked_len_add(len, U32_BYTES + request.key_package_payload.len())?;
    Ok(len)
}

fn append_link_fanout_state(
    out: &mut Vec<u8>,
    fanout: &LinkFanoutState,
) -> Result<(), ClientStoreError> {
    fanout.validate_limits()?;
    append_string_field(
        out,
        "link_fanout.fanout_id",
        &fanout.fanout_id,
        MAX_OBJECT_ID_BYTES,
    )?;
    append_device_ref(out, "link_fanout.target_device", &fanout.target_device)?;
    append_optional_string(
        out,
        "link_fanout.after_room_id",
        fanout.after_room_id.as_deref(),
        MAX_ROOM_ID_BYTES,
    )?;
    append_bool(out, fanout.discovery_complete);
    append_count(
        out,
        "link_fanout.rooms",
        fanout.rooms.len(),
        MAX_LINK_FANOUT_ROOMS,
    )?;
    for room in &fanout.rooms {
        append_link_fanout_room_state(out, room)?;
    }
    Ok(())
}

fn append_link_fanout_room_state(
    out: &mut Vec<u8>,
    room: &LinkFanoutRoomState,
) -> Result<(), ClientStoreError> {
    room.validate_limits()?;
    append_link_fanout_room_plan(out, &room.plan)?;
    append_bool(out, room.claimed_key_package.is_some());
    if let Some(claimed_key_package) = &room.claimed_key_package {
        append_claimed_key_package(out, claimed_key_package)?;
    }
    match &room.status {
        LinkFanoutRoomStatus::Pending => {
            out.extend_from_slice(&LINK_FANOUT_STATUS_PENDING.to_be_bytes());
        }
        LinkFanoutRoomStatus::Prepared { prepared } => {
            out.extend_from_slice(&LINK_FANOUT_STATUS_PREPARED.to_be_bytes());
            append_prepared_commit(out, prepared)?;
        }
        LinkFanoutRoomStatus::Done { accepted_seq } => {
            out.extend_from_slice(&LINK_FANOUT_STATUS_DONE.to_be_bytes());
            out.extend_from_slice(&accepted_seq.to_be_bytes());
        }
    }
    Ok(())
}

fn append_claimed_key_package(
    out: &mut Vec<u8>,
    claimed_key_package: &ClaimKeyPackageResult,
) -> Result<(), ClientStoreError> {
    validate_claimed_key_package(claimed_key_package)?;
    append_string_field(
        out,
        "claimed_key_package.key_package_id",
        &claimed_key_package.key_package_id,
        MAX_OBJECT_ID_BYTES,
    )?;
    append_device_ref(out, "claimed_key_package.owner", &claimed_key_package.owner)?;
    append_string_field(
        out,
        "claimed_key_package.key_package_ref",
        &claimed_key_package.key_package_ref,
        MAX_OBJECT_ID_BYTES,
    )?;
    append_string_field(
        out,
        "claimed_key_package.key_package_hash",
        &claimed_key_package.key_package_hash,
        MAX_OBJECT_ID_BYTES,
    )?;
    append_bytes_field(
        out,
        "claimed_key_package.key_package_payload",
        &claimed_key_package.key_package_payload,
        MAX_KEY_PACKAGE_PAYLOAD_BYTES,
    )?;
    append_string_field(
        out,
        "claimed_key_package.lease_token",
        &claimed_key_package.lease_token,
        MAX_OBJECT_ID_BYTES,
    )?;
    Ok(())
}

fn append_link_fanout_room_plan(
    out: &mut Vec<u8>,
    plan: &LinkFanoutRoomPlan,
) -> Result<(), ClientStoreError> {
    plan.validate_limits()?;
    append_string_field(out, "link_fanout.room_id", &plan.room_id, MAX_ROOM_ID_BYTES)?;
    append_string_field(
        out,
        "link_fanout.key_package_id",
        &plan.key_package_id,
        MAX_OBJECT_ID_BYTES,
    )?;
    append_string_field(
        out,
        "link_fanout.welcome_id",
        &plan.welcome_id,
        MAX_OBJECT_ID_BYTES,
    )?;
    append_string_field(
        out,
        "link_fanout.idempotency_key",
        &plan.idempotency_key,
        MAX_IDEMPOTENCY_KEY_BYTES,
    )?;
    Ok(())
}

fn append_prepared_commit(
    out: &mut Vec<u8>,
    prepared: &PreparedCommit,
) -> Result<(), ClientStoreError> {
    prepared.validate_limits()?;
    append_submit_commit_request(out, &prepared.request)?;
    append_string_field(
        out,
        "prepared_commit.message_id",
        &prepared.message_id,
        MAX_OBJECT_ID_BYTES,
    )?;
    Ok(())
}

fn append_submit_commit_request(
    out: &mut Vec<u8>,
    request: &SubmitCommitRequest,
) -> Result<(), ClientStoreError> {
    request.validate_limits().map_err(ClientError::from)?;
    append_string_field(
        out,
        "commit_request.room_id",
        &request.room_id,
        MAX_ROOM_ID_BYTES,
    )?;
    append_device_ref(out, "commit_request.sender", &request.sender)?;
    out.extend_from_slice(&request.expected_epoch.to_be_bytes());
    append_envelope(out, &request.envelope)?;
    append_membership_delta(out, &request.membership_delta)?;
    append_count(
        out,
        "commit_request.staged_welcomes",
        request.staged_welcomes.len(),
        MAX_STAGED_WELCOMES_PER_COMMIT,
    )?;
    for welcome in &request.staged_welcomes {
        append_staged_welcome(out, welcome)?;
    }
    append_string_field(
        out,
        "commit_request.idempotency_key",
        &request.idempotency_key,
        MAX_IDEMPOTENCY_KEY_BYTES,
    )?;
    Ok(())
}

fn append_membership_delta(
    out: &mut Vec<u8>,
    delta: &MembershipDeltaV1,
) -> Result<(), ClientStoreError> {
    delta.validate_limits().map_err(ClientError::from)?;
    out.extend_from_slice(&delta.base_epoch.to_be_bytes());
    out.extend_from_slice(&delta.post_commit_epoch.to_be_bytes());
    append_string_field(
        out,
        "membership_delta.commit_message_id",
        &delta.commit_message_id,
        MAX_OBJECT_ID_BYTES,
    )?;
    append_count(
        out,
        "membership_delta.adds",
        delta.adds.len(),
        MAX_STAGED_WELCOMES_PER_COMMIT,
    )?;
    for add in &delta.adds {
        append_device_ref(out, "membership_delta.add.device", &add.device)?;
        append_string_field(
            out,
            "membership_delta.add.key_package_id",
            &add.key_package_id,
            MAX_OBJECT_ID_BYTES,
        )?;
        append_string_field(
            out,
            "membership_delta.add.key_package_ref",
            &add.key_package_ref,
            MAX_OBJECT_ID_BYTES,
        )?;
        append_string_field(
            out,
            "membership_delta.add.key_package_hash",
            &add.key_package_hash,
            MAX_OBJECT_ID_BYTES,
        )?;
        append_string_field(
            out,
            "membership_delta.add.welcome_id",
            &add.welcome_id,
            MAX_OBJECT_ID_BYTES,
        )?;
    }
    append_count(
        out,
        "membership_delta.removes",
        delta.removes.len(),
        MAX_STAGED_WELCOMES_PER_COMMIT,
    )?;
    for remove in &delta.removes {
        append_device_ref(out, "membership_delta.remove.device", &remove.device)?;
        out.extend_from_slice(&remove.removed_leaf_index.to_be_bytes());
    }
    Ok(())
}

fn append_envelope(
    out: &mut Vec<u8>,
    envelope: &finitechat_proto::FiniteEnvelope,
) -> Result<(), ClientStoreError> {
    envelope.validate_limits().map_err(ClientError::from)?;
    append_string_field(
        out,
        "envelope.room_id",
        &envelope.room_id,
        MAX_ROOM_ID_BYTES,
    )?;
    append_string_field(
        out,
        "envelope.mls_group_id",
        &envelope.mls_group_id,
        MAX_MLS_GROUP_ID_BYTES,
    )?;
    out.extend_from_slice(&envelope.epoch.to_be_bytes());
    append_device_ref(out, "envelope.sender", &envelope.sender)?;
    out.extend_from_slice(&encode_log_entry_kind(envelope.kind).to_be_bytes());
    append_bytes_field(
        out,
        "envelope.payload",
        &envelope.payload,
        MAX_ENVELOPE_PAYLOAD_BYTES,
    )?;
    Ok(())
}

fn append_staged_welcome(
    out: &mut Vec<u8>,
    welcome: &StagedWelcomeV1,
) -> Result<(), ClientStoreError> {
    welcome.validate_limits().map_err(ClientError::from)?;
    append_string_field(
        out,
        "staged_welcome.welcome_id",
        &welcome.welcome_id,
        MAX_OBJECT_ID_BYTES,
    )?;
    append_bytes_field(
        out,
        "staged_welcome.welcome_payload",
        &welcome.welcome_payload,
        MAX_WELCOME_PAYLOAD_BYTES,
    )?;
    append_bytes_field(
        out,
        "staged_welcome.ratchet_tree_payload",
        &welcome.ratchet_tree_payload,
        MAX_RATCHET_TREE_PAYLOAD_BYTES,
    )?;
    Ok(())
}

fn append_device_ref(
    out: &mut Vec<u8>,
    field: &str,
    device: &DeviceRef,
) -> Result<(), ClientStoreError> {
    device.validate_limits().map_err(ClientError::from)?;
    append_string_field(out, field, &device.account_id, MAX_ACCOUNT_ID_BYTES)?;
    append_string_field(out, field, &device.device_id, MAX_DEVICE_ID_BYTES)?;
    Ok(())
}

fn append_optional_string(
    out: &mut Vec<u8>,
    field: &str,
    value: Option<&str>,
    max_bytes: u32,
) -> Result<(), ClientStoreError> {
    append_bool(out, value.is_some());
    if let Some(value) = value {
        append_string_field(out, field, value, max_bytes)?;
    }
    Ok(())
}

fn append_bool(out: &mut Vec<u8>, value: bool) {
    let encoded = if value { 1u16 } else { 0u16 };
    out.extend_from_slice(&encoded.to_be_bytes());
}

fn encode_log_entry_kind(kind: LogEntryKind) -> u16 {
    match kind {
        LogEntryKind::Application => LOG_ENTRY_KIND_APPLICATION,
        LogEntryKind::Proposal => LOG_ENTRY_KIND_PROPOSAL,
        LogEntryKind::Commit => LOG_ENTRY_KIND_COMMIT,
    }
}

fn decode_log_entry_kind(value: u16) -> Result<LogEntryKind, ClientStoreError> {
    match value {
        LOG_ENTRY_KIND_APPLICATION => Ok(LogEntryKind::Application),
        LOG_ENTRY_KIND_PROPOSAL => Ok(LogEntryKind::Proposal),
        LOG_ENTRY_KIND_COMMIT => Ok(LogEntryKind::Commit),
        other => Err(ClientStoreError::StateSnapshotEnum {
            field: "log_entry_kind",
            value: u64::from(other),
        }),
    }
}

fn encoded_link_fanout_state_len(fanout: &LinkFanoutState) -> Result<usize, ClientStoreError> {
    let mut len = U32_BYTES + fanout.fanout_id.len();
    len = checked_len_add(len, encoded_device_ref_len(&fanout.target_device)?)?;
    len = checked_len_add(len, U16_BYTES)?;
    if let Some(after_room_id) = &fanout.after_room_id {
        len = checked_len_add(len, U32_BYTES + after_room_id.len())?;
    }
    len = checked_len_add(len, U16_BYTES)?;
    len = checked_len_add(len, U32_BYTES)?;
    for room in &fanout.rooms {
        len = checked_len_add(len, encoded_link_fanout_room_state_len(room)?)?;
    }
    Ok(len)
}

fn encoded_link_fanout_room_state_len(
    room: &LinkFanoutRoomState,
) -> Result<usize, ClientStoreError> {
    let mut len = encoded_link_fanout_room_plan_len(&room.plan)?;
    len = checked_len_add(len, U16_BYTES)?;
    if let Some(claimed_key_package) = &room.claimed_key_package {
        len = checked_len_add(len, encoded_claimed_key_package_len(claimed_key_package)?)?;
    }
    match &room.status {
        LinkFanoutRoomStatus::Pending => {}
        LinkFanoutRoomStatus::Prepared { prepared } => {
            len = checked_len_add(len, encoded_prepared_commit_len(prepared)?)?;
        }
        LinkFanoutRoomStatus::Done { .. } => {
            len = checked_len_add(len, U64_BYTES)?;
        }
    }
    Ok(len)
}

fn encoded_claimed_key_package_len(
    claimed_key_package: &ClaimKeyPackageResult,
) -> Result<usize, ClientStoreError> {
    validate_claimed_key_package(claimed_key_package)?;
    let mut len = U32_BYTES + claimed_key_package.key_package_id.len();
    len = checked_len_add(len, encoded_device_ref_len(&claimed_key_package.owner)?)?;
    len = checked_len_add(len, U32_BYTES + claimed_key_package.key_package_ref.len())?;
    len = checked_len_add(len, U32_BYTES + claimed_key_package.key_package_hash.len())?;
    len = checked_len_add(
        len,
        U32_BYTES + claimed_key_package.key_package_payload.len(),
    )?;
    len = checked_len_add(len, U32_BYTES + claimed_key_package.lease_token.len())?;
    Ok(len)
}

fn encoded_link_fanout_room_plan_len(plan: &LinkFanoutRoomPlan) -> Result<usize, ClientStoreError> {
    let mut len = U32_BYTES + plan.room_id.len();
    len = checked_len_add(len, U32_BYTES + plan.key_package_id.len())?;
    len = checked_len_add(len, U32_BYTES + plan.welcome_id.len())?;
    len = checked_len_add(len, U32_BYTES + plan.idempotency_key.len())?;
    Ok(len)
}

fn encoded_prepared_commit_len(prepared: &PreparedCommit) -> Result<usize, ClientStoreError> {
    let mut len = encoded_submit_commit_request_len(&prepared.request)?;
    len = checked_len_add(len, U32_BYTES + prepared.message_id.len())?;
    Ok(len)
}

fn encoded_submit_commit_request_len(
    request: &SubmitCommitRequest,
) -> Result<usize, ClientStoreError> {
    let mut len = U32_BYTES + request.room_id.len();
    len = checked_len_add(len, encoded_device_ref_len(&request.sender)?)?;
    len = checked_len_add(len, U64_BYTES)?;
    len = checked_len_add(len, encoded_envelope_len(&request.envelope)?)?;
    len = checked_len_add(
        len,
        encoded_membership_delta_len(&request.membership_delta)?,
    )?;
    len = checked_len_add(len, U32_BYTES)?;
    for welcome in &request.staged_welcomes {
        len = checked_len_add(len, encoded_staged_welcome_len(welcome)?)?;
    }
    len = checked_len_add(len, U32_BYTES + request.idempotency_key.len())?;
    Ok(len)
}

fn encoded_membership_delta_len(delta: &MembershipDeltaV1) -> Result<usize, ClientStoreError> {
    let mut len = U64_BYTES + U64_BYTES + U32_BYTES + delta.commit_message_id.len() + U32_BYTES;
    for add in &delta.adds {
        len = checked_len_add(len, encoded_device_ref_len(&add.device)?)?;
        len = checked_len_add(len, U32_BYTES + add.key_package_id.len())?;
        len = checked_len_add(len, U32_BYTES + add.key_package_ref.len())?;
        len = checked_len_add(len, U32_BYTES + add.key_package_hash.len())?;
        len = checked_len_add(len, U32_BYTES + add.welcome_id.len())?;
    }
    len = checked_len_add(len, U32_BYTES)?;
    for remove in &delta.removes {
        len = checked_len_add(len, encoded_device_ref_len(&remove.device)?)?;
        len = checked_len_add(len, U32_BYTES)?;
    }
    Ok(len)
}

fn encoded_envelope_len(
    envelope: &finitechat_proto::FiniteEnvelope,
) -> Result<usize, ClientStoreError> {
    let mut len = U32_BYTES + envelope.room_id.len();
    len = checked_len_add(len, U32_BYTES + envelope.mls_group_id.len())?;
    len = checked_len_add(len, U64_BYTES)?;
    len = checked_len_add(len, encoded_device_ref_len(&envelope.sender)?)?;
    len = checked_len_add(len, U16_BYTES)?;
    len = checked_len_add(len, U32_BYTES + envelope.payload.len())?;
    Ok(len)
}

fn encoded_staged_welcome_len(welcome: &StagedWelcomeV1) -> Result<usize, ClientStoreError> {
    let mut len = U32_BYTES + welcome.welcome_id.len();
    len = checked_len_add(len, U32_BYTES + welcome.welcome_payload.len())?;
    len = checked_len_add(len, U32_BYTES + welcome.ratchet_tree_payload.len())?;
    Ok(len)
}

fn encoded_device_ref_len(device: &DeviceRef) -> Result<usize, ClientStoreError> {
    let mut len = U32_BYTES + device.account_id.len();
    len = checked_len_add(len, U32_BYTES + device.device_id.len())?;
    Ok(len)
}

fn checked_len_add(left: usize, right: usize) -> Result<usize, ClientStoreError> {
    left.checked_add(right)
        .ok_or(ClientStoreError::StateSnapshotLengthOverflow)
}

fn append_string_field(
    out: &mut Vec<u8>,
    field: &str,
    value: &str,
    max_bytes: u32,
) -> Result<(), ClientStoreError> {
    validate_string_bytes(field, value, max_bytes).map_err(ClientError::from)?;
    append_raw_len_prefixed(out, value.as_bytes())
}

fn append_bytes_field(
    out: &mut Vec<u8>,
    field: &str,
    bytes: &[u8],
    max_bytes: u32,
) -> Result<(), ClientStoreError> {
    validate_bytes_len(field, bytes.len(), max_bytes).map_err(ClientError::from)?;
    append_raw_len_prefixed(out, bytes)
}

fn append_count(
    out: &mut Vec<u8>,
    field: &str,
    count: usize,
    max_items: u32,
) -> Result<(), ClientStoreError> {
    validate_item_count(field, count, max_items).map_err(ClientError::from)?;
    let count = u32::try_from(count).map_err(|_| ClientStoreError::StateSnapshotLengthOverflow)?;
    out.extend_from_slice(&count.to_be_bytes());
    Ok(())
}

fn append_raw_len_prefixed(out: &mut Vec<u8>, bytes: &[u8]) -> Result<(), ClientStoreError> {
    let len =
        u32::try_from(bytes.len()).map_err(|_| ClientStoreError::StateSnapshotLengthOverflow)?;
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(bytes);
    Ok(())
}

struct ClientStateCursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> ClientStateCursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        debug_assert!(!bytes.is_empty());
        Self { bytes, offset: 0 }
    }

    fn take_magic(&mut self) -> Result<(), ClientStoreError> {
        let magic = self.take_bytes(CLIENT_STATE_SNAPSHOT_MAGIC.len())?;
        if magic == CLIENT_STATE_SNAPSHOT_MAGIC {
            Ok(())
        } else {
            Err(ClientStoreError::StateSnapshotMagic)
        }
    }

    fn take_u16(&mut self) -> Result<u16, ClientStoreError> {
        let bytes = self.take_bytes(U16_BYTES)?;
        Ok(u16::from_be_bytes(
            bytes
                .try_into()
                .map_err(|_| ClientStoreError::StateSnapshotTruncated)?,
        ))
    }

    fn take_u32(&mut self) -> Result<u32, ClientStoreError> {
        let bytes = self.take_bytes(U32_BYTES)?;
        Ok(u32::from_be_bytes(
            bytes
                .try_into()
                .map_err(|_| ClientStoreError::StateSnapshotTruncated)?,
        ))
    }

    fn take_u64(&mut self) -> Result<u64, ClientStoreError> {
        let bytes = self.take_bytes(U64_BYTES)?;
        Ok(u64::from_be_bytes(
            bytes
                .try_into()
                .map_err(|_| ClientStoreError::StateSnapshotTruncated)?,
        ))
    }

    fn take_count(&mut self, field: &str, max_items: u32) -> Result<usize, ClientStoreError> {
        let count = self.take_u32()? as usize;
        validate_item_count(field, count, max_items).map_err(ClientError::from)?;
        Ok(count)
    }

    fn take_string(&mut self, field: &str, max_bytes: u32) -> Result<String, ClientStoreError> {
        let bytes = self.take_vec(field, max_bytes)?;
        let value = String::from_utf8(bytes).map_err(|_| ClientStoreError::StateSnapshotUtf8)?;
        validate_string_bytes(field, &value, max_bytes).map_err(ClientError::from)?;
        Ok(value)
    }

    fn take_vec(&mut self, field: &str, max_bytes: u32) -> Result<Vec<u8>, ClientStoreError> {
        let len = self.take_u32()? as usize;
        validate_bytes_len(field, len, max_bytes).map_err(ClientError::from)?;
        Ok(self.take_bytes(len)?.to_vec())
    }

    fn take_upload_key_package_request(
        &mut self,
    ) -> Result<UploadKeyPackageRequest, ClientStoreError> {
        let request = UploadKeyPackageRequest {
            key_package_id: self.take_string(
                "pending_key_package_upload.key_package_id",
                MAX_OBJECT_ID_BYTES,
            )?,
            owner: self.take_device_ref("pending_key_package_upload.owner")?,
            key_package_ref: self.take_string(
                "pending_key_package_upload.key_package_ref",
                MAX_OBJECT_ID_BYTES,
            )?,
            key_package_hash: self.take_string(
                "pending_key_package_upload.key_package_hash",
                MAX_OBJECT_ID_BYTES,
            )?,
            key_package_payload: self.take_vec(
                "pending_key_package_upload.key_package_payload",
                MAX_KEY_PACKAGE_PAYLOAD_BYTES,
            )?,
        };
        request.validate_limits().map_err(ClientError::from)?;
        Ok(request)
    }

    fn take_link_fanout_state(&mut self) -> Result<LinkFanoutState, ClientStoreError> {
        let fanout = LinkFanoutState {
            fanout_id: self.take_string("link_fanout.fanout_id", MAX_OBJECT_ID_BYTES)?,
            target_device: self.take_device_ref("link_fanout.target_device")?,
            after_room_id: self
                .take_optional_string("link_fanout.after_room_id", MAX_ROOM_ID_BYTES)?,
            discovery_complete: self.take_bool()?,
            rooms: {
                let count = self.take_count("link_fanout.rooms", MAX_LINK_FANOUT_ROOMS)?;
                let mut rooms = Vec::with_capacity(count);
                for _ in 0..count {
                    rooms.push(self.take_link_fanout_room_state()?);
                }
                rooms
            },
        };
        fanout.validate_limits()?;
        Ok(fanout)
    }

    fn take_link_fanout_room_state(&mut self) -> Result<LinkFanoutRoomState, ClientStoreError> {
        let plan = self.take_link_fanout_room_plan()?;
        let claimed_key_package = if self.take_bool()? {
            Some(self.take_claimed_key_package()?)
        } else {
            None
        };
        let status = match self.take_u16()? {
            LINK_FANOUT_STATUS_PENDING => LinkFanoutRoomStatus::Pending,
            LINK_FANOUT_STATUS_PREPARED => LinkFanoutRoomStatus::Prepared {
                prepared: Box::new(self.take_prepared_commit()?),
            },
            LINK_FANOUT_STATUS_DONE => LinkFanoutRoomStatus::Done {
                accepted_seq: self.take_u64()?,
            },
            other => {
                return Err(ClientStoreError::StateSnapshotEnum {
                    field: "link_fanout.status",
                    value: u64::from(other),
                });
            }
        };
        let room = LinkFanoutRoomState {
            plan,
            claimed_key_package,
            status,
        };
        room.validate_limits()?;
        Ok(room)
    }

    fn take_claimed_key_package(&mut self) -> Result<ClaimKeyPackageResult, ClientStoreError> {
        let claimed_key_package = ClaimKeyPackageResult {
            key_package_id: self
                .take_string("claimed_key_package.key_package_id", MAX_OBJECT_ID_BYTES)?,
            owner: self.take_device_ref("claimed_key_package.owner")?,
            key_package_ref: self
                .take_string("claimed_key_package.key_package_ref", MAX_OBJECT_ID_BYTES)?,
            key_package_hash: self
                .take_string("claimed_key_package.key_package_hash", MAX_OBJECT_ID_BYTES)?,
            key_package_payload: self.take_vec(
                "claimed_key_package.key_package_payload",
                MAX_KEY_PACKAGE_PAYLOAD_BYTES,
            )?,
            lease_token: self
                .take_string("claimed_key_package.lease_token", MAX_OBJECT_ID_BYTES)?,
        };
        validate_claimed_key_package(&claimed_key_package)?;
        Ok(claimed_key_package)
    }

    fn take_link_fanout_room_plan(&mut self) -> Result<LinkFanoutRoomPlan, ClientStoreError> {
        let plan = LinkFanoutRoomPlan {
            room_id: self.take_string("link_fanout.room_id", MAX_ROOM_ID_BYTES)?,
            key_package_id: self.take_string("link_fanout.key_package_id", MAX_OBJECT_ID_BYTES)?,
            welcome_id: self.take_string("link_fanout.welcome_id", MAX_OBJECT_ID_BYTES)?,
            idempotency_key: self
                .take_string("link_fanout.idempotency_key", MAX_IDEMPOTENCY_KEY_BYTES)?,
        };
        plan.validate_limits()?;
        Ok(plan)
    }

    fn take_prepared_commit(&mut self) -> Result<PreparedCommit, ClientStoreError> {
        let prepared = PreparedCommit {
            request: self.take_submit_commit_request()?,
            message_id: self.take_string("prepared_commit.message_id", MAX_OBJECT_ID_BYTES)?,
        };
        prepared.validate_limits()?;
        Ok(prepared)
    }

    fn take_submit_commit_request(&mut self) -> Result<SubmitCommitRequest, ClientStoreError> {
        let request = SubmitCommitRequest {
            room_id: self.take_string("commit_request.room_id", MAX_ROOM_ID_BYTES)?,
            sender: self.take_device_ref("commit_request.sender")?,
            expected_epoch: self.take_u64()?,
            envelope: self.take_envelope()?,
            membership_delta: self.take_membership_delta()?,
            staged_welcomes: {
                let count = self.take_count(
                    "commit_request.staged_welcomes",
                    MAX_STAGED_WELCOMES_PER_COMMIT,
                )?;
                let mut welcomes = Vec::with_capacity(count);
                for _ in 0..count {
                    welcomes.push(self.take_staged_welcome()?);
                }
                welcomes
            },
            idempotency_key: self
                .take_string("commit_request.idempotency_key", MAX_IDEMPOTENCY_KEY_BYTES)?,
        };
        request.validate_limits().map_err(ClientError::from)?;
        Ok(request)
    }

    fn take_membership_delta(&mut self) -> Result<MembershipDeltaV1, ClientStoreError> {
        let base_epoch = self.take_u64()?;
        let post_commit_epoch = self.take_u64()?;
        let commit_message_id =
            self.take_string("membership_delta.commit_message_id", MAX_OBJECT_ID_BYTES)?;
        let add_count = self.take_count("membership_delta.adds", MAX_STAGED_WELCOMES_PER_COMMIT)?;
        let mut adds = Vec::with_capacity(add_count);
        for _ in 0..add_count {
            adds.push(MembershipAddV1 {
                device: self.take_device_ref("membership_delta.add.device")?,
                key_package_id: self
                    .take_string("membership_delta.add.key_package_id", MAX_OBJECT_ID_BYTES)?,
                key_package_ref: self
                    .take_string("membership_delta.add.key_package_ref", MAX_OBJECT_ID_BYTES)?,
                key_package_hash: self
                    .take_string("membership_delta.add.key_package_hash", MAX_OBJECT_ID_BYTES)?,
                welcome_id: self
                    .take_string("membership_delta.add.welcome_id", MAX_OBJECT_ID_BYTES)?,
            });
        }
        let remove_count =
            self.take_count("membership_delta.removes", MAX_STAGED_WELCOMES_PER_COMMIT)?;
        let mut removes = Vec::with_capacity(remove_count);
        for _ in 0..remove_count {
            removes.push(MembershipRemoveV1 {
                device: self.take_device_ref("membership_delta.remove.device")?,
                removed_leaf_index: self.take_u32()?,
            });
        }
        let delta = MembershipDeltaV1 {
            base_epoch,
            post_commit_epoch,
            commit_message_id,
            adds,
            removes,
        };
        delta.validate_limits().map_err(ClientError::from)?;
        Ok(delta)
    }

    fn take_envelope(&mut self) -> Result<finitechat_proto::FiniteEnvelope, ClientStoreError> {
        let envelope = finitechat_proto::FiniteEnvelope {
            room_id: self.take_string("envelope.room_id", MAX_ROOM_ID_BYTES)?,
            mls_group_id: self.take_string("envelope.mls_group_id", MAX_MLS_GROUP_ID_BYTES)?,
            epoch: self.take_u64()?,
            sender: self.take_device_ref("envelope.sender")?,
            kind: self.take_log_entry_kind()?,
            payload: self.take_vec("envelope.payload", MAX_ENVELOPE_PAYLOAD_BYTES)?,
        };
        envelope.validate_limits().map_err(ClientError::from)?;
        Ok(envelope)
    }

    fn take_staged_welcome(&mut self) -> Result<StagedWelcomeV1, ClientStoreError> {
        let welcome = StagedWelcomeV1 {
            welcome_id: self.take_string("staged_welcome.welcome_id", MAX_OBJECT_ID_BYTES)?,
            welcome_payload: self
                .take_vec("staged_welcome.welcome_payload", MAX_WELCOME_PAYLOAD_BYTES)?,
            ratchet_tree_payload: self.take_vec(
                "staged_welcome.ratchet_tree_payload",
                MAX_RATCHET_TREE_PAYLOAD_BYTES,
            )?,
        };
        welcome.validate_limits().map_err(ClientError::from)?;
        Ok(welcome)
    }

    fn take_device_ref(&mut self, field: &'static str) -> Result<DeviceRef, ClientStoreError> {
        let device = DeviceRef {
            account_id: self.take_string(field, MAX_ACCOUNT_ID_BYTES)?,
            device_id: self.take_string(field, MAX_DEVICE_ID_BYTES)?,
        };
        device.validate_limits().map_err(ClientError::from)?;
        Ok(device)
    }

    fn take_optional_string(
        &mut self,
        field: &str,
        max_bytes: u32,
    ) -> Result<Option<String>, ClientStoreError> {
        if self.take_bool()? {
            Ok(Some(self.take_string(field, max_bytes)?))
        } else {
            Ok(None)
        }
    }

    fn take_bool(&mut self) -> Result<bool, ClientStoreError> {
        match self.take_u16()? {
            0 => Ok(false),
            1 => Ok(true),
            other => Err(ClientStoreError::StateSnapshotEnum {
                field: "bool",
                value: u64::from(other),
            }),
        }
    }

    fn take_log_entry_kind(&mut self) -> Result<LogEntryKind, ClientStoreError> {
        decode_log_entry_kind(self.take_u16()?)
    }

    fn take_bytes(&mut self, len: usize) -> Result<&'a [u8], ClientStoreError> {
        let end = self
            .offset
            .checked_add(len)
            .ok_or(ClientStoreError::StateSnapshotLengthOverflow)?;
        let bytes = self
            .bytes
            .get(self.offset..end)
            .ok_or(ClientStoreError::StateSnapshotTruncated)?;
        self.offset = end;
        debug_assert!(self.offset <= self.bytes.len());
        Ok(bytes)
    }

    fn finish(&self) -> Result<(), ClientStoreError> {
        if self.offset == self.bytes.len() {
            Ok(())
        } else {
            Err(ClientStoreError::StateSnapshotTrailingBytes)
        }
    }
}
