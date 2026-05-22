use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

pub type AccountId = String;
pub type DeviceId = String;
pub type RoomId = String;
pub type MlsGroupId = String;
pub type MessageId = String;
pub type KeyPackageId = String;
pub type KeyPackageRef = String;
pub type KeyPackageHash = String;
pub type WelcomeId = String;
pub type LeaseToken = String;
pub type IdempotencyKey = String;
pub type ConversationId = String;
pub type RuntimeStateKey = String;
pub type AttachmentBlobUrl = String;
pub type AttachmentHash = String;
pub type Epoch = u64;
pub type Seq = u64;

pub const MESSAGE_ID_DOMAIN: &[u8] = b"finite-message-id-v1";
pub const MAX_ENVELOPE_PAYLOAD_BYTES: u32 = 256 * 1024;
pub const MAX_SYNC_PAGE_ENTRIES: u32 = 100;
pub const MAX_SYNC_PAGE_BYTES: u32 = 4 * 1024 * 1024;
pub const MAX_ACCOUNT_DEVICES_PER_ROOM: u32 = 32;
pub const MAX_DIRECT_ROOM_DEVICES_PER_ACCOUNT: u32 = 8;
pub const MAX_KEY_PACKAGES_PER_DEVICE: u32 = 64;
pub const MAX_KEY_PACKAGE_PAYLOAD_BYTES: u32 = 64 * 1024;
pub const MAX_WELCOME_CLAIMS_PER_REQUEST: u32 = 32;
pub const MAX_STAGED_WELCOMES_PER_COMMIT: u32 = 32;
pub const MAX_ACCOUNT_ROOM_DISCOVERY_RESULTS: u32 = 256;
pub const MAX_WELCOME_PAYLOAD_BYTES: u32 = 1024 * 1024;
pub const MAX_RATCHET_TREE_PAYLOAD_BYTES: u32 = 1024 * 1024;
pub const MAX_IDEMPOTENCY_RECORDS_PER_ROOM_DEVICE: u32 = 4096;
pub const MAX_LINK_SESSION_PAYLOAD_BYTES: u32 = 1024 * 1024;
pub const MAX_ATTACHMENT_PLAINTEXT_BYTES: u32 = 32 * 1024 * 1024;
pub const MAX_ATTACHMENT_CIPHERTEXT_BYTES: u32 = MAX_ATTACHMENT_PLAINTEXT_BYTES + 16;
pub const MAX_ATTACHMENT_BLOB_URL_BYTES: u32 = 2048;
pub const MAX_ATTACHMENT_FILENAME_BYTES: u32 = 255;
pub const MAX_ATTACHMENT_MIME_TYPE_BYTES: u32 = 128;
pub const MAX_ATTACHMENT_HASH_HEX_BYTES: u32 = 64;
pub const MAX_ATTACHMENT_KEY_HEX_BYTES: u32 = 64;
pub const MAX_ATTACHMENT_NONCE_HEX_BYTES: u32 = 24;
pub const MAX_RUNTIME_STATE_SNAPSHOT_PAYLOAD_BYTES: u32 = 64 * 1024;
pub const MAX_RUNTIME_STATE_KEYS_PER_ROOM_DEVICE: u32 = 128;
pub const MAX_EPHEMERAL_ACTIVITY_EXPIRY_MILLIS: u64 = 30 * 60 * 1000;
pub const MAX_EPHEMERAL_ACTIVITY_CACHE_ENTRIES_PER_ROUTE: u32 = 64;
pub const MAX_IDEMPOTENCY_KEY_BYTES: u32 = 128;
pub const MAX_ACCOUNT_ID_BYTES: u32 = 128;
pub const MAX_DEVICE_ID_BYTES: u32 = 128;
pub const MAX_ROOM_ID_BYTES: u32 = 128;
pub const MAX_MLS_GROUP_ID_BYTES: u32 = 128;
pub const MAX_OBJECT_ID_BYTES: u32 = 128;
pub const FINITECHAT_ATTACHMENT_BLOB_SCHEME_V1: &str = "finitechat.attachment.blob.v1";
pub const FINITECHAT_ATTACHMENT_BLOB_ENCRYPTION_AES256_GCM_V1: &str = "aes-256-gcm.v1";

const _: () = {
    assert!(MAX_ENVELOPE_PAYLOAD_BYTES > 0);
    assert!(MAX_SYNC_PAGE_ENTRIES > 0);
    assert!(MAX_SYNC_PAGE_BYTES >= MAX_ENVELOPE_PAYLOAD_BYTES);
    assert!(MAX_ACCOUNT_DEVICES_PER_ROOM >= MAX_DIRECT_ROOM_DEVICES_PER_ACCOUNT);
    assert!(MAX_DIRECT_ROOM_DEVICES_PER_ACCOUNT > 0);
    assert!(MAX_KEY_PACKAGES_PER_DEVICE >= MAX_DIRECT_ROOM_DEVICES_PER_ACCOUNT);
    assert!(MAX_KEY_PACKAGE_PAYLOAD_BYTES > 0);
    assert!(MAX_KEY_PACKAGE_PAYLOAD_BYTES < MAX_WELCOME_PAYLOAD_BYTES);
    assert!(MAX_WELCOME_CLAIMS_PER_REQUEST > 0);
    assert!(MAX_STAGED_WELCOMES_PER_COMMIT > 0);
    assert!(MAX_ACCOUNT_ROOM_DISCOVERY_RESULTS > 0);
    assert!(MAX_STAGED_WELCOMES_PER_COMMIT >= MAX_DIRECT_ROOM_DEVICES_PER_ACCOUNT);
    assert!(MAX_WELCOME_PAYLOAD_BYTES > 0);
    assert!(MAX_RATCHET_TREE_PAYLOAD_BYTES > 0);
    assert!(MAX_IDEMPOTENCY_RECORDS_PER_ROOM_DEVICE > 0);
    assert!(MAX_LINK_SESSION_PAYLOAD_BYTES > 0);
    assert!(MAX_ATTACHMENT_PLAINTEXT_BYTES > MAX_ENVELOPE_PAYLOAD_BYTES);
    assert!(MAX_ATTACHMENT_CIPHERTEXT_BYTES > MAX_ATTACHMENT_PLAINTEXT_BYTES);
    assert!(MAX_ATTACHMENT_BLOB_URL_BYTES >= MAX_OBJECT_ID_BYTES);
    assert!(MAX_ATTACHMENT_FILENAME_BYTES > 0);
    assert!(MAX_ATTACHMENT_MIME_TYPE_BYTES > 0);
    assert!(MAX_ATTACHMENT_HASH_HEX_BYTES == 64);
    assert!(MAX_ATTACHMENT_KEY_HEX_BYTES == 64);
    assert!(MAX_ATTACHMENT_NONCE_HEX_BYTES == 24);
    assert!(MAX_RUNTIME_STATE_SNAPSHOT_PAYLOAD_BYTES > 0);
    assert!(MAX_RUNTIME_STATE_KEYS_PER_ROOM_DEVICE > 0);
    assert!(MAX_EPHEMERAL_ACTIVITY_EXPIRY_MILLIS > 0);
    assert!(MAX_EPHEMERAL_ACTIVITY_CACHE_ENTRIES_PER_ROUTE > 0);
    assert!(MAX_IDEMPOTENCY_KEY_BYTES > 0);
};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct DeviceRef {
    pub account_id: AccountId,
    pub device_id: DeviceId,
}

impl DeviceRef {
    pub fn new(account_id: impl Into<AccountId>, device_id: impl Into<DeviceId>) -> Self {
        Self {
            account_id: account_id.into(),
            device_id: device_id.into(),
        }
    }

    pub fn validate_limits(&self) -> Result<(), ProtocolLimitError> {
        validate_string_bytes("account_id", &self.account_id, MAX_ACCOUNT_ID_BYTES)?;
        validate_string_bytes("device_id", &self.device_id, MAX_DEVICE_ID_BYTES)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoomStatus {
    Open,
    NeedsRepair,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogEntryKind {
    Application,
    Proposal,
    Commit,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FiniteEnvelope {
    pub room_id: RoomId,
    pub mls_group_id: MlsGroupId,
    pub epoch: Epoch,
    pub sender: DeviceRef,
    pub kind: LogEntryKind,
    #[serde(with = "bytes_as_vec")]
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PushPolicy {
    Default,
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnreadPolicy {
    Default,
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandInboxPolicy {
    Create,
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplicationDeliveryPolicy {
    pub push: PushPolicy,
    pub unread: UnreadPolicy,
    pub command_inbox: CommandInboxPolicy,
}

impl ApplicationDeliveryPolicy {
    pub const USER_VISIBLE_MESSAGE: Self = Self {
        push: PushPolicy::Default,
        unread: UnreadPolicy::Default,
        command_inbox: CommandInboxPolicy::Never,
    };

    pub const NON_NOTIFYING: Self = Self {
        push: PushPolicy::Never,
        unread: UnreadPolicy::Never,
        command_inbox: CommandInboxPolicy::Never,
    };

    pub const RUNTIME_COMMAND_REQUEST: Self = Self {
        push: PushPolicy::Default,
        unread: UnreadPolicy::Never,
        command_inbox: CommandInboxPolicy::Create,
    };

    pub const RUNTIME_COMMAND_RESULT: Self = Self {
        push: PushPolicy::Never,
        unread: UnreadPolicy::Never,
        command_inbox: CommandInboxPolicy::Never,
    };

    pub fn creates_push(self) -> bool {
        self.push == PushPolicy::Default
    }

    pub fn creates_unread(self) -> bool {
        self.unread == UnreadPolicy::Default
    }

    pub fn creates_command_inbox_work(self) -> bool {
        self.command_inbox == CommandInboxPolicy::Create
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DurableAppEventKind {
    ConversationCreate,
    ConversationUpdate,
    ConversationArchive,
    ConversationSegmentStart,
    ChatMessage,
    ChatEdit,
    ChatReaction,
    ChatReceipt,
    RuntimeStateSnapshot,
    RuntimeCommandRequest,
    RuntimeCommandResult,
    RuntimeCommandCancel,
    Namespaced {
        name: String,
        policy: ApplicationDeliveryPolicy,
    },
}

impl DurableAppEventKind {
    pub fn delivery_policy(&self) -> ApplicationDeliveryPolicy {
        match self {
            Self::ChatMessage => ApplicationDeliveryPolicy::USER_VISIBLE_MESSAGE,
            Self::RuntimeCommandRequest => ApplicationDeliveryPolicy::RUNTIME_COMMAND_REQUEST,
            Self::RuntimeCommandResult => ApplicationDeliveryPolicy::RUNTIME_COMMAND_RESULT,
            Self::ConversationSegmentStart
            | Self::ChatEdit
            | Self::ChatReaction
            | Self::ChatReceipt
            | Self::RuntimeStateSnapshot
            | Self::RuntimeCommandCancel => ApplicationDeliveryPolicy::NON_NOTIFYING,
            Self::ConversationCreate | Self::ConversationUpdate | Self::ConversationArchive => {
                ApplicationDeliveryPolicy::NON_NOTIFYING
            }
            Self::Namespaced { policy, .. } => *policy,
        }
    }

    pub fn validate_limits(&self) -> Result<(), ProtocolLimitError> {
        if let Self::Namespaced { name, .. } = self {
            validate_string_bytes(
                "durable_app_event.namespaced_kind",
                name,
                MAX_OBJECT_ID_BYTES,
            )?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecryptedApplicationEventV1 {
    pub kind: DurableAppEventKind,
    pub conversation_id: Option<ConversationId>,
    #[serde(with = "bytes_as_vec")]
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeStateSnapshotV1 {
    pub state_key: RuntimeStateKey,
    pub schema: String,
    pub revision: u64,
    pub observed_at_ms: u64,
    pub expires_at_ms: u64,
    #[serde(with = "bytes_as_vec")]
    pub status_payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentDimensionsV1 {
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentBlobMetadataV1 {
    pub mime_type: String,
    pub filename: String,
    pub dimensions: Option<AttachmentDimensionsV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentBlobEncryptionV1 {
    pub algorithm: String,
    pub key_hex: String,
    pub nonce_hex: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentBlobReferenceV1 {
    pub scheme: String,
    pub url: AttachmentBlobUrl,
    pub ciphertext_sha256: AttachmentHash,
    pub plaintext_sha256: AttachmentHash,
    pub plaintext_size: u64,
    pub ciphertext_size: u64,
    pub encryption: AttachmentBlobEncryptionV1,
    pub metadata: AttachmentBlobMetadataV1,
}

impl AttachmentBlobMetadataV1 {
    pub fn validate_limits(&self) -> Result<(), ProtocolLimitError> {
        validate_bytes_non_empty("attachment.mime_type", self.mime_type.len())?;
        validate_string_bytes(
            "attachment.mime_type",
            &self.mime_type,
            MAX_ATTACHMENT_MIME_TYPE_BYTES,
        )?;
        validate_bytes_non_empty("attachment.filename", self.filename.len())?;
        validate_string_bytes(
            "attachment.filename",
            &self.filename,
            MAX_ATTACHMENT_FILENAME_BYTES,
        )?;
        if let Some(dimensions) = &self.dimensions {
            dimensions.validate_limits()?;
        }
        Ok(())
    }
}

impl AttachmentDimensionsV1 {
    pub fn validate_limits(&self) -> Result<(), ProtocolLimitError> {
        if self.width == 0 {
            return Err(ProtocolLimitError::BytesEmpty {
                field: "attachment.dimensions.width".to_string(),
            });
        }
        if self.height == 0 {
            return Err(ProtocolLimitError::BytesEmpty {
                field: "attachment.dimensions.height".to_string(),
            });
        }
        Ok(())
    }
}

impl AttachmentBlobEncryptionV1 {
    pub fn validate_limits(&self) -> Result<(), ProtocolLimitError> {
        validate_bytes_non_empty("attachment.encryption.algorithm", self.algorithm.len())?;
        validate_string_bytes(
            "attachment.encryption.algorithm",
            &self.algorithm,
            MAX_OBJECT_ID_BYTES,
        )?;
        validate_string_bytes(
            "attachment.encryption.key_hex",
            &self.key_hex,
            MAX_ATTACHMENT_KEY_HEX_BYTES,
        )?;
        validate_string_bytes(
            "attachment.encryption.nonce_hex",
            &self.nonce_hex,
            MAX_ATTACHMENT_NONCE_HEX_BYTES,
        )?;
        Ok(())
    }
}

impl AttachmentBlobReferenceV1 {
    pub fn validate_limits(&self) -> Result<(), ProtocolLimitError> {
        validate_bytes_non_empty("attachment.scheme", self.scheme.len())?;
        validate_string_bytes("attachment.scheme", &self.scheme, MAX_OBJECT_ID_BYTES)?;
        validate_bytes_non_empty("attachment.url", self.url.len())?;
        validate_string_bytes("attachment.url", &self.url, MAX_ATTACHMENT_BLOB_URL_BYTES)?;
        validate_string_bytes(
            "attachment.ciphertext_sha256",
            &self.ciphertext_sha256,
            MAX_ATTACHMENT_HASH_HEX_BYTES,
        )?;
        validate_string_bytes(
            "attachment.plaintext_sha256",
            &self.plaintext_sha256,
            MAX_ATTACHMENT_HASH_HEX_BYTES,
        )?;
        validate_size_limit(
            "attachment.plaintext",
            self.plaintext_size,
            MAX_ATTACHMENT_PLAINTEXT_BYTES,
        )?;
        validate_size_limit(
            "attachment.ciphertext",
            self.ciphertext_size,
            MAX_ATTACHMENT_CIPHERTEXT_BYTES,
        )?;
        self.encryption.validate_limits()?;
        self.metadata.validate_limits()?;
        Ok(())
    }
}

impl RuntimeStateSnapshotV1 {
    pub fn validate_limits(&self) -> Result<(), ProtocolLimitError> {
        validate_bytes_non_empty("runtime_state.state_key", self.state_key.len())?;
        validate_string_bytes(
            "runtime_state.state_key",
            &self.state_key,
            MAX_OBJECT_ID_BYTES,
        )?;
        validate_bytes_non_empty("runtime_state.schema", self.schema.len())?;
        validate_string_bytes("runtime_state.schema", &self.schema, MAX_OBJECT_ID_BYTES)?;
        validate_bytes_non_empty("runtime_state.status_payload", self.status_payload.len())?;
        validate_bytes_len(
            "runtime_state.status_payload",
            self.status_payload.len(),
            MAX_RUNTIME_STATE_SNAPSHOT_PAYLOAD_BYTES,
        )?;
        Ok(())
    }

    pub fn is_expired_at(&self, now_ms: u64) -> bool {
        now_ms >= self.expires_at_ms
    }
}

#[derive(Debug, Clone, Error, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum RuntimeStateProjectionError {
    #[error(
        "runtime state snapshot is missing for room {room_id}, source {source_device:?}, key {state_key}"
    )]
    Missing {
        room_id: RoomId,
        source_device: DeviceRef,
        state_key: RuntimeStateKey,
    },
    #[error("runtime state snapshot {state_key} has schema {actual:?}, expected {expected:?}")]
    WrongSchema {
        state_key: RuntimeStateKey,
        expected: String,
        actual: String,
    },
    #[error("runtime state snapshot {state_key} expired at {expires_at_ms}, now {now_ms}")]
    Expired {
        state_key: RuntimeStateKey,
        now_ms: u64,
        expires_at_ms: u64,
    },
    #[error("runtime state snapshot {state_key} has malformed payload")]
    MalformedPayload { state_key: RuntimeStateKey },
    #[error(transparent)]
    Protocol(#[from] ProtocolLimitError),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeStateProjectionEntry {
    pub room_id: RoomId,
    pub source: DeviceRef,
    pub accepted_seq: Seq,
    pub snapshot: RuntimeStateSnapshotV1,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeStateProjection {
    entries: BTreeMap<String, RuntimeStateProjectionEntry>,
}

impl RuntimeStateProjection {
    pub fn apply(&mut self, entry: RuntimeStateProjectionEntry) -> Result<(), ProtocolLimitError> {
        validate_room_id(&entry.room_id)?;
        entry.source.validate_limits()?;
        entry.snapshot.validate_limits()?;
        let key =
            runtime_state_projection_key(&entry.room_id, &entry.source, &entry.snapshot.state_key)?;
        let should_replace = self
            .entries
            .get(&key)
            .map(|current| {
                entry.snapshot.revision > current.snapshot.revision
                    || (entry.snapshot.revision == current.snapshot.revision
                        && entry.accepted_seq > current.accepted_seq)
            })
            .unwrap_or(true);
        if should_replace {
            self.entries.insert(key, entry);
        }
        Ok(())
    }

    pub fn get(
        &self,
        room_id: &str,
        source: &DeviceRef,
        state_key: &str,
    ) -> Option<&RuntimeStateProjectionEntry> {
        let key = runtime_state_projection_key(room_id, source, state_key).ok()?;
        self.entries.get(&key)
    }

    pub fn require_fresh(
        &self,
        room_id: &str,
        source: &DeviceRef,
        state_key: &str,
        expected_schema: &str,
        now_ms: u64,
    ) -> Result<&RuntimeStateProjectionEntry, RuntimeStateProjectionError> {
        validate_room_id(room_id)?;
        source.validate_limits()?;
        validate_string_bytes("runtime_state.state_key", state_key, MAX_OBJECT_ID_BYTES)?;
        validate_bytes_non_empty("runtime_state.schema", expected_schema.len())?;
        validate_string_bytes("runtime_state.schema", expected_schema, MAX_OBJECT_ID_BYTES)?;
        let entry = self.get(room_id, source, state_key).ok_or_else(|| {
            RuntimeStateProjectionError::Missing {
                room_id: room_id.to_string(),
                source_device: source.clone(),
                state_key: state_key.to_string(),
            }
        })?;
        if entry.snapshot.schema != expected_schema {
            return Err(RuntimeStateProjectionError::WrongSchema {
                state_key: state_key.to_string(),
                expected: expected_schema.to_string(),
                actual: entry.snapshot.schema.clone(),
            });
        }
        if entry.snapshot.is_expired_at(now_ms) {
            return Err(RuntimeStateProjectionError::Expired {
                state_key: state_key.to_string(),
                now_ms,
                expires_at_ms: entry.snapshot.expires_at_ms,
            });
        }
        Ok(entry)
    }

    pub fn require_fresh_json<T: DeserializeOwned>(
        &self,
        room_id: &str,
        source: &DeviceRef,
        state_key: &str,
        expected_schema: &str,
        now_ms: u64,
    ) -> Result<T, RuntimeStateProjectionError> {
        let entry = self.require_fresh(room_id, source, state_key, expected_schema, now_ms)?;
        serde_json::from_slice(&entry.snapshot.status_payload).map_err(|_| {
            RuntimeStateProjectionError::MalformedPayload {
                state_key: state_key.to_string(),
            }
        })
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl DecryptedApplicationEventV1 {
    pub fn validate_limits(&self) -> Result<(), ProtocolLimitError> {
        self.kind.validate_limits()?;
        if let Some(conversation_id) = &self.conversation_id {
            validate_string_bytes("conversation_id", conversation_id, MAX_OBJECT_ID_BYTES)?;
        }
        let max_payload = if self.kind == DurableAppEventKind::RuntimeStateSnapshot {
            MAX_RUNTIME_STATE_SNAPSHOT_PAYLOAD_BYTES
        } else {
            MAX_ENVELOPE_PAYLOAD_BYTES
        };
        validate_bytes_len("application_event.payload", self.payload.len(), max_payload)?;
        Ok(())
    }
}

impl FiniteEnvelope {
    pub fn message_id(&self) -> Result<MessageId, serde_json::Error> {
        message_id_for_envelope(self)
    }

    pub fn validate_limits(&self) -> Result<(), ProtocolLimitError> {
        validate_room_id(&self.room_id)?;
        validate_mls_group_id(&self.mls_group_id)?;
        self.sender.validate_limits()?;
        validate_bytes_len(
            "envelope.payload",
            self.payload.len(),
            MAX_ENVELOPE_PAYLOAD_BYTES,
        )?;
        Ok(())
    }
}

pub fn message_id_for_envelope(envelope: &FiniteEnvelope) -> Result<MessageId, serde_json::Error> {
    let mut hasher = Sha256::new();
    hasher.update(MESSAGE_ID_DOMAIN);
    hasher.update(serde_json::to_vec(envelope)?);
    Ok(hex_lower(&hasher.finalize()))
}

pub fn message_id_for_bytes(bytes: &[u8]) -> MessageId {
    let mut hasher = Sha256::new();
    hasher.update(MESSAGE_ID_DOMAIN);
    hasher.update(bytes);
    hex_lower(&hasher.finalize())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MembershipDeltaV1 {
    pub base_epoch: Epoch,
    pub post_commit_epoch: Epoch,
    pub commit_message_id: MessageId,
    #[serde(default)]
    pub adds: Vec<MembershipAddV1>,
    #[serde(default)]
    pub removes: Vec<MembershipRemoveV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MembershipAddV1 {
    pub device: DeviceRef,
    pub key_package_id: KeyPackageId,
    pub key_package_ref: KeyPackageRef,
    pub key_package_hash: KeyPackageHash,
    pub welcome_id: WelcomeId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MembershipRemoveV1 {
    pub device: DeviceRef,
    pub removed_leaf_index: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StagedWelcomeV1 {
    pub welcome_id: WelcomeId,
    #[serde(with = "bytes_as_vec")]
    pub welcome_payload: Vec<u8>,
    #[serde(with = "bytes_as_vec")]
    pub ratchet_tree_payload: Vec<u8>,
}

impl StagedWelcomeV1 {
    pub fn validate_limits(&self) -> Result<(), ProtocolLimitError> {
        validate_string_bytes("welcome_id", &self.welcome_id, MAX_OBJECT_ID_BYTES)?;
        validate_bytes_non_empty("welcome_payload", self.welcome_payload.len())?;
        validate_bytes_len(
            "welcome_payload",
            self.welcome_payload.len(),
            MAX_WELCOME_PAYLOAD_BYTES,
        )?;
        validate_bytes_non_empty("ratchet_tree_payload", self.ratchet_tree_payload.len())?;
        validate_bytes_len(
            "ratchet_tree_payload",
            self.ratchet_tree_payload.len(),
            MAX_RATCHET_TREE_PAYLOAD_BYTES,
        )?;
        Ok(())
    }
}

impl MembershipDeltaV1 {
    pub fn validate_structure(
        &self,
        expected_epoch: Epoch,
        actual_commit_message_id: &str,
    ) -> Result<(), MembershipDeltaError> {
        if self.base_epoch != expected_epoch {
            return Err(MembershipDeltaError::WrongBaseEpoch {
                expected: expected_epoch,
                actual: self.base_epoch,
            });
        }
        if self.post_commit_epoch != self.base_epoch + 1 {
            return Err(MembershipDeltaError::WrongPostCommitEpoch {
                base: self.base_epoch,
                actual: self.post_commit_epoch,
            });
        }
        if self.commit_message_id != actual_commit_message_id {
            return Err(MembershipDeltaError::WrongCommitMessageId);
        }

        let mut add_devices = BTreeSet::new();
        for add in &self.adds {
            if !add_devices.insert(add.device.clone()) {
                return Err(MembershipDeltaError::DuplicateAdd(add.device.clone()));
            }
            if add.key_package_id.trim().is_empty()
                || add.key_package_ref.trim().is_empty()
                || add.key_package_hash.trim().is_empty()
                || add.welcome_id.trim().is_empty()
            {
                return Err(MembershipDeltaError::IncompleteAdd(add.device.clone()));
            }
        }

        let mut remove_devices = BTreeSet::new();
        for remove in &self.removes {
            if !remove_devices.insert(remove.device.clone()) {
                return Err(MembershipDeltaError::DuplicateRemove(remove.device.clone()));
            }
        }

        if let Some(device) = add_devices.intersection(&remove_devices).next() {
            return Err(MembershipDeltaError::AddAndRemoveSameDevice(
                (*device).clone(),
            ));
        }

        Ok(())
    }

    pub fn validate_limits(&self) -> Result<(), ProtocolLimitError> {
        for add in &self.adds {
            add.device.validate_limits()?;
            validate_string_bytes("key_package_id", &add.key_package_id, MAX_OBJECT_ID_BYTES)?;
            validate_string_bytes("key_package_ref", &add.key_package_ref, MAX_OBJECT_ID_BYTES)?;
            validate_string_bytes(
                "key_package_hash",
                &add.key_package_hash,
                MAX_OBJECT_ID_BYTES,
            )?;
            validate_string_bytes("welcome_id", &add.welcome_id, MAX_OBJECT_ID_BYTES)?;
        }
        for remove in &self.removes {
            remove.device.validate_limits()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Error, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum MembershipDeltaError {
    #[error("membership delta base epoch {actual} does not match expected epoch {expected}")]
    WrongBaseEpoch { expected: Epoch, actual: Epoch },
    #[error("membership delta post-commit epoch {actual} is not base epoch {base} + 1")]
    WrongPostCommitEpoch { base: Epoch, actual: Epoch },
    #[error("membership delta commit message id does not match submitted commit")]
    WrongCommitMessageId,
    #[error("membership delta adds device more than once: {0:?}")]
    DuplicateAdd(DeviceRef),
    #[error("membership delta removes device more than once: {0:?}")]
    DuplicateRemove(DeviceRef),
    #[error("membership delta adds and removes same device: {0:?}")]
    AddAndRemoveSameDevice(DeviceRef),
    #[error("membership delta add is missing key package or welcome fields: {0:?}")]
    IncompleteAdd(DeviceRef),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyPackageState {
    Available,
    Leased,
    Consumed,
    Released,
    Expired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WelcomeState {
    Staged,
    Released,
    Claimed,
    Acked,
    Failed,
    Expired,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomLogEntry {
    pub room_id: RoomId,
    pub seq: Seq,
    pub message_id: MessageId,
    pub sender: DeviceRef,
    pub kind: LogEntryKind,
    pub epoch: Epoch,
    pub envelope: FiniteEnvelope,
    pub idempotency_key: IdempotencyKey,
}

#[derive(Debug, Clone, Error, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum ProtocolLimitError {
    #[error("{field} is empty")]
    BytesEmpty { field: String },
    #[error("{field} has {actual_bytes} bytes, max {max_bytes}")]
    BytesTooLong {
        field: String,
        max_bytes: u64,
        actual_bytes: u64,
    },
    #[error("{field} has {actual_items} items, max {max_items}")]
    TooManyItems {
        field: String,
        max_items: u64,
        actual_items: u64,
    },
}

pub fn validate_room_id(room_id: &str) -> Result<(), ProtocolLimitError> {
    validate_string_bytes("room_id", room_id, MAX_ROOM_ID_BYTES)
}

pub fn validate_mls_group_id(mls_group_id: &str) -> Result<(), ProtocolLimitError> {
    validate_string_bytes("mls_group_id", mls_group_id, MAX_MLS_GROUP_ID_BYTES)
}

pub fn validate_idempotency_key(key: &str) -> Result<(), ProtocolLimitError> {
    validate_string_bytes("idempotency_key", key, MAX_IDEMPOTENCY_KEY_BYTES)
}

pub fn validate_string_bytes(
    field: &str,
    value: &str,
    max_bytes: u32,
) -> Result<(), ProtocolLimitError> {
    validate_bytes_len(field, value.len(), max_bytes)
}

pub fn validate_bytes_len(
    field: &str,
    actual_bytes: usize,
    max_bytes: u32,
) -> Result<(), ProtocolLimitError> {
    if actual_bytes <= max_bytes as usize {
        Ok(())
    } else {
        Err(ProtocolLimitError::BytesTooLong {
            field: field.to_string(),
            max_bytes: u64::from(max_bytes),
            actual_bytes: actual_bytes as u64,
        })
    }
}

pub fn validate_bytes_non_empty(
    field: &str,
    actual_bytes: usize,
) -> Result<(), ProtocolLimitError> {
    if actual_bytes > 0 {
        Ok(())
    } else {
        Err(ProtocolLimitError::BytesEmpty {
            field: field.to_string(),
        })
    }
}

pub fn validate_item_count(
    field: &str,
    actual_items: usize,
    max_items: u32,
) -> Result<(), ProtocolLimitError> {
    if actual_items <= max_items as usize {
        Ok(())
    } else {
        Err(ProtocolLimitError::TooManyItems {
            field: field.to_string(),
            max_items: u64::from(max_items),
            actual_items: actual_items as u64,
        })
    }
}

pub fn validate_size_limit(
    field: &str,
    actual_bytes: u64,
    max_bytes: u32,
) -> Result<(), ProtocolLimitError> {
    if actual_bytes == 0 {
        return Err(ProtocolLimitError::BytesEmpty {
            field: field.to_string(),
        });
    }
    if actual_bytes <= u64::from(max_bytes) {
        Ok(())
    } else {
        Err(ProtocolLimitError::BytesTooLong {
            field: field.to_string(),
            max_bytes: u64::from(max_bytes),
            actual_bytes,
        })
    }
}

fn runtime_state_projection_key(
    room_id: &str,
    source: &DeviceRef,
    state_key: &str,
) -> Result<String, ProtocolLimitError> {
    validate_room_id(room_id)?;
    source.validate_limits()?;
    validate_string_bytes("runtime_state.state_key", state_key, MAX_OBJECT_ID_BYTES)?;
    Ok(format!(
        "{}|{}|{}|{}",
        length_prefixed(room_id),
        length_prefixed(&source.account_id),
        length_prefixed(&source.device_id),
        length_prefixed(state_key)
    ))
}

fn length_prefixed(value: &str) -> String {
    format!("{}:{value}", value.len())
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

mod bytes_as_vec {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_bytes(bytes)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        Vec::<u8>::deserialize(deserializer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(account: &str, id: &str) -> DeviceRef {
        DeviceRef::new(account, id)
    }

    #[test]
    fn message_id_is_stable_for_same_envelope() {
        let envelope = FiniteEnvelope {
            room_id: "room_1".to_string(),
            mls_group_id: "group_1".to_string(),
            epoch: 0,
            sender: device("alice", "phone"),
            kind: LogEntryKind::Application,
            payload: b"hello".to_vec(),
        };
        assert_eq!(
            envelope.message_id().unwrap(),
            envelope.message_id().unwrap()
        );
    }

    #[test]
    fn membership_delta_rejects_duplicate_adds() {
        let delta = MembershipDeltaV1 {
            base_epoch: 0,
            post_commit_epoch: 1,
            commit_message_id: "commit".to_string(),
            adds: vec![
                MembershipAddV1 {
                    device: device("bob", "phone"),
                    key_package_id: "kp_1".to_string(),
                    key_package_ref: "ref".to_string(),
                    key_package_hash: "hash".to_string(),
                    welcome_id: "welcome_1".to_string(),
                },
                MembershipAddV1 {
                    device: device("bob", "phone"),
                    key_package_id: "kp_2".to_string(),
                    key_package_ref: "ref2".to_string(),
                    key_package_hash: "hash2".to_string(),
                    welcome_id: "welcome_2".to_string(),
                },
            ],
            removes: vec![],
        };
        assert_eq!(
            delta.validate_structure(0, "commit").unwrap_err(),
            MembershipDeltaError::DuplicateAdd(device("bob", "phone"))
        );
    }

    #[test]
    fn durable_app_event_defaults_match_push_and_inbox_policy() {
        assert_eq!(
            DurableAppEventKind::ChatMessage.delivery_policy(),
            ApplicationDeliveryPolicy::USER_VISIBLE_MESSAGE
        );
        assert_eq!(
            DurableAppEventKind::ChatReceipt.delivery_policy(),
            ApplicationDeliveryPolicy::NON_NOTIFYING
        );
        assert_eq!(
            DurableAppEventKind::ConversationSegmentStart.delivery_policy(),
            ApplicationDeliveryPolicy::NON_NOTIFYING
        );
        assert_eq!(
            DurableAppEventKind::RuntimeStateSnapshot.delivery_policy(),
            ApplicationDeliveryPolicy::NON_NOTIFYING
        );
        assert_eq!(
            DurableAppEventKind::RuntimeCommandRequest.delivery_policy(),
            ApplicationDeliveryPolicy::RUNTIME_COMMAND_REQUEST
        );
        assert_eq!(
            DurableAppEventKind::RuntimeCommandResult.delivery_policy(),
            ApplicationDeliveryPolicy::RUNTIME_COMMAND_RESULT
        );

        assert!(
            DurableAppEventKind::ChatMessage
                .delivery_policy()
                .creates_push()
        );
        assert!(
            DurableAppEventKind::ChatMessage
                .delivery_policy()
                .creates_unread()
        );
        assert!(
            DurableAppEventKind::RuntimeCommandRequest
                .delivery_policy()
                .creates_command_inbox_work()
        );
        assert!(
            !DurableAppEventKind::RuntimeStateSnapshot
                .delivery_policy()
                .creates_command_inbox_work()
        );
        assert!(
            !DurableAppEventKind::RuntimeCommandResult
                .delivery_policy()
                .creates_push()
        );
    }

    #[test]
    fn runtime_state_projection_replaces_by_revision_and_sequence() {
        let source = device("runtime_npub", "runtime_box");
        let mut projection = RuntimeStateProjection::default();

        projection
            .apply(runtime_state_entry(
                "room_1",
                source.clone(),
                "runtime.gateway",
                "finite.gateway.v1",
                1,
                10,
                br#"{"status":"down"}"#,
            ))
            .unwrap();
        projection
            .apply(runtime_state_entry(
                "room_1",
                source.clone(),
                "runtime.gateway",
                "finite.gateway.v1",
                1,
                9,
                br#"{"status":"older"}"#,
            ))
            .unwrap();
        assert_eq!(
            projection
                .get("room_1", &source, "runtime.gateway")
                .unwrap()
                .snapshot
                .status_payload,
            br#"{"status":"down"}"#
        );

        projection
            .apply(runtime_state_entry(
                "room_1",
                source.clone(),
                "runtime.gateway",
                "finite.gateway.v1",
                1,
                11,
                br#"{"status":"restarted"}"#,
            ))
            .unwrap();
        assert_eq!(
            projection
                .get("room_1", &source, "runtime.gateway")
                .unwrap()
                .snapshot
                .status_payload,
            br#"{"status":"restarted"}"#
        );

        projection
            .apply(runtime_state_entry(
                "room_1",
                source.clone(),
                "runtime.gateway",
                "finite.gateway.v1",
                2,
                8,
                br#"{"status":"live"}"#,
            ))
            .unwrap();
        let current = projection
            .get("room_1", &source, "runtime.gateway")
            .unwrap();
        assert_eq!(current.snapshot.revision, 2);
        assert_eq!(current.accepted_seq, 8);
        assert_eq!(current.snapshot.status_payload, br#"{"status":"live"}"#);
    }

    #[test]
    fn runtime_state_projection_preserves_unknown_schema_and_expiry() {
        let source = device("runtime_npub", "runtime_box");
        let mut projection = RuntimeStateProjection::default();

        projection
            .apply(runtime_state_entry(
                "room_1",
                source.clone(),
                "runtime.capabilities",
                "vendor.future-schema.v9",
                1,
                10,
                br#"{"unrecognized":true}"#,
            ))
            .unwrap();

        let current = projection
            .get("room_1", &source, "runtime.capabilities")
            .unwrap();
        assert_eq!(current.snapshot.schema, "vendor.future-schema.v9");
        assert_eq!(current.snapshot.status_payload, br#"{"unrecognized":true}"#);
        assert!(!current.snapshot.is_expired_at(1_999));
        assert!(current.snapshot.is_expired_at(2_000));
    }

    #[derive(Debug, serde::Deserialize, PartialEq, Eq)]
    struct GatewayStatus {
        status: String,
    }

    #[test]
    fn runtime_state_projection_requires_fresh_matching_schema() {
        let source = device("runtime_npub", "runtime_box");
        let mut projection = RuntimeStateProjection::default();
        projection
            .apply(runtime_state_entry(
                "room_1",
                source.clone(),
                "runtime.gateway",
                "finitecomputer.runtime.gateway.status.v1",
                1,
                10,
                br#"{"status":"down"}"#,
            ))
            .unwrap();

        let status: GatewayStatus = projection
            .require_fresh_json(
                "room_1",
                &source,
                "runtime.gateway",
                "finitecomputer.runtime.gateway.status.v1",
                1_999,
            )
            .unwrap();

        assert_eq!(
            status,
            GatewayStatus {
                status: "down".to_string()
            }
        );
    }

    #[test]
    fn runtime_state_projection_fails_loudly_for_missing_stale_wrong_or_malformed_status() {
        let source = device("runtime_npub", "runtime_box");
        let mut projection = RuntimeStateProjection::default();
        assert_eq!(
            projection
                .require_fresh(
                    "room_1",
                    &source,
                    "runtime.gateway",
                    "finitecomputer.runtime.gateway.status.v1",
                    1_500,
                )
                .unwrap_err(),
            RuntimeStateProjectionError::Missing {
                room_id: "room_1".to_string(),
                source_device: source.clone(),
                state_key: "runtime.gateway".to_string(),
            }
        );

        projection
            .apply(runtime_state_entry(
                "room_1",
                source.clone(),
                "runtime.gateway",
                "finitecomputer.runtime.gateway.status.v1",
                1,
                10,
                br#"{"status":"down"}"#,
            ))
            .unwrap();

        assert!(matches!(
            projection
                .require_fresh(
                    "room_1",
                    &source,
                    "runtime.gateway",
                    "finitecomputer.runtime.gateway.status.v1",
                    2_000,
                )
                .unwrap_err(),
            RuntimeStateProjectionError::Expired { .. }
        ));
        assert!(matches!(
            projection
                .require_fresh(
                    "room_1",
                    &source,
                    "runtime.gateway",
                    "finitecomputer.runtime.gateway.status.v2",
                    1_500,
                )
                .unwrap_err(),
            RuntimeStateProjectionError::WrongSchema { .. }
        ));

        projection
            .apply(runtime_state_entry(
                "room_1",
                source.clone(),
                "runtime.gateway",
                "finitecomputer.runtime.gateway.status.v1",
                2,
                11,
                b"not json",
            ))
            .unwrap();
        let err = projection
            .require_fresh_json::<GatewayStatus>(
                "room_1",
                &source,
                "runtime.gateway",
                "finitecomputer.runtime.gateway.status.v1",
                1_500,
            )
            .unwrap_err();
        assert_eq!(
            err,
            RuntimeStateProjectionError::MalformedPayload {
                state_key: "runtime.gateway".to_string()
            }
        );
    }

    #[test]
    fn runtime_state_snapshot_rejects_empty_key_schema_or_payload() {
        let source = device("runtime_npub", "runtime_box");
        let mut projection = RuntimeStateProjection::default();

        for (state_key, schema, payload) in [
            (
                "",
                "finitecomputer.runtime.gateway.status.v1",
                b"{}".as_slice(),
            ),
            ("runtime.gateway", "", b"{}".as_slice()),
            (
                "runtime.gateway",
                "finitecomputer.runtime.gateway.status.v1",
                b"".as_slice(),
            ),
        ] {
            assert!(matches!(
                projection
                    .apply(runtime_state_entry(
                        "room_1",
                        source.clone(),
                        state_key,
                        schema,
                        1,
                        10,
                        payload,
                    ))
                    .unwrap_err(),
                ProtocolLimitError::BytesEmpty { .. }
            ));
        }
    }

    fn runtime_state_entry(
        room_id: &str,
        source: DeviceRef,
        state_key: &str,
        schema: &str,
        revision: u64,
        accepted_seq: Seq,
        payload: &[u8],
    ) -> RuntimeStateProjectionEntry {
        RuntimeStateProjectionEntry {
            room_id: room_id.to_string(),
            source,
            accepted_seq,
            snapshot: RuntimeStateSnapshotV1 {
                state_key: state_key.to_string(),
                schema: schema.to_string(),
                revision,
                observed_at_ms: 1_000,
                expires_at_ms: 2_000,
                status_payload: payload.to_vec(),
            },
        }
    }
}
