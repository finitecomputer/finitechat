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
pub type RuntimeCommandRequestId = String;
pub type RuntimeCommandName = String;
pub type RuntimeCommandResourceKey = String;
pub type ActivityKind = String;
pub type ActivityId = String;
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
pub const MAX_RUNTIME_COMMAND_PAYLOAD_BYTES: u32 = 128 * 1024;
pub const MAX_RUNTIME_COMMAND_ERROR_MESSAGE_BYTES: u32 = 2048;
pub const MAX_RUNTIME_COMMAND_ACTIVITY_CLEARS: u32 = 16;
pub const MAX_RUNTIME_COMMAND_LEDGER_RECORDS: u32 = 1024;
pub const MAX_EPHEMERAL_ACTIVITY_DECRYPTED_PAYLOAD_BYTES: u32 = 64 * 1024;
pub const MAX_EPHEMERAL_ACTIVITY_PROJECTION_ENTRIES: u32 = 4096;
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
pub const FINITECHAT_DEFAULT_ACTIVITY_ID: &str = "default";

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
    assert!(MAX_RUNTIME_COMMAND_PAYLOAD_BYTES > 0);
    assert!(MAX_RUNTIME_COMMAND_PAYLOAD_BYTES < MAX_ENVELOPE_PAYLOAD_BYTES);
    assert!(MAX_RUNTIME_COMMAND_ERROR_MESSAGE_BYTES > 0);
    assert!(MAX_RUNTIME_COMMAND_ACTIVITY_CLEARS > 0);
    assert!(MAX_RUNTIME_COMMAND_LEDGER_RECORDS > 0);
    assert!(MAX_EPHEMERAL_ACTIVITY_DECRYPTED_PAYLOAD_BYTES > 0);
    assert!(MAX_EPHEMERAL_ACTIVITY_DECRYPTED_PAYLOAD_BYTES < MAX_ENVELOPE_PAYLOAD_BYTES);
    assert!(MAX_EPHEMERAL_ACTIVITY_PROJECTION_ENTRIES > 0);
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuntimeCommandPayloadKindV1 {
    #[serde(rename = "runtime.command.request")]
    Request,
    #[serde(rename = "runtime.command.result")]
    Result,
    #[serde(rename = "runtime.command.cancel")]
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeCommandTargetV1 {
    pub account_id: AccountId,
    pub device_id: Option<DeviceId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeCommandJsonPayloadV1 {
    pub schema: String,
    #[serde(with = "bytes_as_vec")]
    pub json_payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeActivityClearV1 {
    pub activity_kind: String,
    pub activity_id: Option<String>,
    pub conversation_id: Option<ConversationId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeCommandRequestV1 {
    #[serde(rename = "type")]
    pub payload_kind: RuntimeCommandPayloadKindV1,
    pub request_id: RuntimeCommandRequestId,
    pub command: RuntimeCommandName,
    pub target: RuntimeCommandTargetV1,
    pub resource_key: Option<RuntimeCommandResourceKey>,
    pub body: RuntimeCommandJsonPayloadV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeCommandTerminalStatusV1 {
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeCommandErrorV1 {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeCommandResultV1 {
    #[serde(rename = "type")]
    pub payload_kind: RuntimeCommandPayloadKindV1,
    pub request_id: RuntimeCommandRequestId,
    pub status: RuntimeCommandTerminalStatusV1,
    pub body: Option<RuntimeCommandJsonPayloadV1>,
    pub error: Option<RuntimeCommandErrorV1>,
    #[serde(default)]
    pub clears_activity: Vec<RuntimeActivityClearV1>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeCommandCancelV1 {
    #[serde(rename = "type")]
    pub payload_kind: RuntimeCommandPayloadKindV1,
    pub request_id: RuntimeCommandRequestId,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeCommandLedgerStatus {
    Pending,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeCommandLedgerDecision {
    Recorded,
    Replayed,
    IgnoredTarget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeCommandLedgerRecord {
    pub room_id: RoomId,
    pub conversation_id: Option<ConversationId>,
    pub request_id: RuntimeCommandRequestId,
    pub command: RuntimeCommandName,
    pub sender: DeviceRef,
    pub target: RuntimeCommandTargetV1,
    pub original_message_id: MessageId,
    pub accepted_seq: Seq,
    pub resource_key: Option<RuntimeCommandResourceKey>,
    pub status: RuntimeCommandLedgerStatus,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeCommandLedger {
    records: BTreeMap<String, RuntimeCommandLedgerRecord>,
}

#[derive(Debug, Clone, Copy)]
pub struct RuntimeCommandIngressContext<'a> {
    pub room_id: &'a str,
    pub conversation_id: Option<&'a str>,
    pub accepted_seq: Seq,
    pub original_message_id: &'a str,
    pub sender: &'a DeviceRef,
    pub local_device: &'a DeviceRef,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EphemeralActivityActionV1 {
    Set,
    Clear,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DecryptedEphemeralActivityV1 {
    pub activity_kind: ActivityKind,
    pub activity_id: Option<ActivityId>,
    pub action: EphemeralActivityActionV1,
    #[serde(with = "bytes_as_vec")]
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, Copy)]
pub struct EphemeralActivityIngressContext<'a> {
    pub room_id: &'a str,
    pub conversation_id: Option<&'a str>,
    pub sender: &'a DeviceRef,
    pub received_at_ms: u64,
    pub expires_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EphemeralActivityProjectionEntry {
    pub room_id: RoomId,
    pub conversation_id: Option<ConversationId>,
    pub sender: DeviceRef,
    pub activity_kind: ActivityKind,
    pub activity_id: ActivityId,
    #[serde(with = "bytes_as_vec")]
    pub payload: Vec<u8>,
    pub received_at_ms: u64,
    pub expires_at_ms: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EphemeralActivityProjection {
    entries: BTreeMap<String, EphemeralActivityProjectionEntry>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EphemeralActivityProjectionDecision {
    Set,
    Refreshed,
    Cleared,
    ClearMiss,
}

#[derive(Debug, Clone, Error, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum EphemeralActivityProjectionError {
    #[error("ephemeral activity expired before receipt")]
    AlreadyExpired,
    #[error("ephemeral activity expiry window {actual_millis}ms exceeds max {max_millis}ms")]
    ExpiryTooLong { max_millis: u64, actual_millis: u64 },
    #[error("ephemeral activity projection capacity exceeded: max {max_records}")]
    CapacityExceeded { max_records: u32 },
    #[error(transparent)]
    Protocol(#[from] ProtocolLimitError),
}

#[derive(Debug, Clone, Error, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum RuntimeCommandPayloadError {
    #[error("runtime command payload kind {actual:?} does not match expected {expected:?}")]
    WrongPayloadKind {
        expected: RuntimeCommandPayloadKindV1,
        actual: RuntimeCommandPayloadKindV1,
    },
    #[error("runtime command result {request_id} is missing a body for success")]
    SuccessMissingBody { request_id: RuntimeCommandRequestId },
    #[error("runtime command result {request_id} is missing an error for failure")]
    FailureMissingError { request_id: RuntimeCommandRequestId },
    #[error(transparent)]
    Protocol(#[from] ProtocolLimitError),
}

#[derive(Debug, Clone, Error, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum RuntimeCommandLedgerError {
    #[error("runtime command request id conflict for {request_id}")]
    ConflictingRequestId { request_id: RuntimeCommandRequestId },
    #[error("runtime command request not found for {request_id}")]
    RequestNotFound { request_id: RuntimeCommandRequestId },
    #[error("runtime command status {status:?} is not terminal")]
    NonTerminalStatus { status: RuntimeCommandLedgerStatus },
    #[error("runtime command ledger capacity exceeded: max {max_records}")]
    CapacityExceeded { max_records: u32 },
    #[error(transparent)]
    Payload(#[from] RuntimeCommandPayloadError),
    #[error(transparent)]
    Protocol(#[from] ProtocolLimitError),
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

impl RuntimeCommandTargetV1 {
    pub fn validate_limits(&self) -> Result<(), ProtocolLimitError> {
        validate_bytes_non_empty("runtime_command.target.account_id", self.account_id.len())?;
        validate_string_bytes(
            "runtime_command.target.account_id",
            &self.account_id,
            MAX_ACCOUNT_ID_BYTES,
        )?;
        if let Some(device_id) = &self.device_id {
            validate_bytes_non_empty("runtime_command.target.device_id", device_id.len())?;
            validate_string_bytes(
                "runtime_command.target.device_id",
                device_id,
                MAX_DEVICE_ID_BYTES,
            )?;
        }
        Ok(())
    }

    pub fn matches_device(&self, device: &DeviceRef) -> bool {
        self.account_id == device.account_id
            && self
                .device_id
                .as_ref()
                .is_none_or(|device_id| *device_id == device.device_id)
    }
}

impl RuntimeCommandJsonPayloadV1 {
    pub fn validate_limits(&self) -> Result<(), ProtocolLimitError> {
        validate_bytes_non_empty("runtime_command.payload.schema", self.schema.len())?;
        validate_string_bytes(
            "runtime_command.payload.schema",
            &self.schema,
            MAX_OBJECT_ID_BYTES,
        )?;
        validate_bytes_non_empty(
            "runtime_command.payload.json_payload",
            self.json_payload.len(),
        )?;
        validate_bytes_len(
            "runtime_command.payload.json_payload",
            self.json_payload.len(),
            MAX_RUNTIME_COMMAND_PAYLOAD_BYTES,
        )?;
        Ok(())
    }
}

impl RuntimeActivityClearV1 {
    pub fn validate_limits(&self) -> Result<(), ProtocolLimitError> {
        validate_bytes_non_empty("runtime_activity_clear.kind", self.activity_kind.len())?;
        validate_string_bytes(
            "runtime_activity_clear.kind",
            &self.activity_kind,
            MAX_OBJECT_ID_BYTES,
        )?;
        if let Some(activity_id) = &self.activity_id {
            validate_bytes_non_empty("runtime_activity_clear.activity_id", activity_id.len())?;
            validate_string_bytes(
                "runtime_activity_clear.activity_id",
                activity_id,
                MAX_OBJECT_ID_BYTES,
            )?;
        }
        if let Some(conversation_id) = &self.conversation_id {
            validate_bytes_non_empty(
                "runtime_activity_clear.conversation_id",
                conversation_id.len(),
            )?;
            validate_string_bytes(
                "runtime_activity_clear.conversation_id",
                conversation_id,
                MAX_OBJECT_ID_BYTES,
            )?;
        }
        Ok(())
    }
}

impl RuntimeCommandRequestV1 {
    pub fn validate_structure(&self) -> Result<(), RuntimeCommandPayloadError> {
        if self.payload_kind != RuntimeCommandPayloadKindV1::Request {
            return Err(RuntimeCommandPayloadError::WrongPayloadKind {
                expected: RuntimeCommandPayloadKindV1::Request,
                actual: self.payload_kind,
            });
        }
        self.validate_limits()?;
        Ok(())
    }

    pub fn validate_limits(&self) -> Result<(), ProtocolLimitError> {
        validate_bytes_non_empty("runtime_command.request_id", self.request_id.len())?;
        validate_string_bytes(
            "runtime_command.request_id",
            &self.request_id,
            MAX_OBJECT_ID_BYTES,
        )?;
        validate_bytes_non_empty("runtime_command.command", self.command.len())?;
        validate_string_bytes(
            "runtime_command.command",
            &self.command,
            MAX_OBJECT_ID_BYTES,
        )?;
        self.target.validate_limits()?;
        if let Some(resource_key) = &self.resource_key {
            validate_bytes_non_empty("runtime_command.resource_key", resource_key.len())?;
            validate_string_bytes(
                "runtime_command.resource_key",
                resource_key,
                MAX_OBJECT_ID_BYTES,
            )?;
        }
        self.body.validate_limits()?;
        Ok(())
    }
}

impl RuntimeCommandErrorV1 {
    pub fn validate_limits(&self) -> Result<(), ProtocolLimitError> {
        validate_bytes_non_empty("runtime_command.error.code", self.code.len())?;
        validate_string_bytes(
            "runtime_command.error.code",
            &self.code,
            MAX_OBJECT_ID_BYTES,
        )?;
        validate_bytes_non_empty("runtime_command.error.message", self.message.len())?;
        validate_string_bytes(
            "runtime_command.error.message",
            &self.message,
            MAX_RUNTIME_COMMAND_ERROR_MESSAGE_BYTES,
        )?;
        Ok(())
    }
}

impl RuntimeCommandResultV1 {
    pub fn validate_structure(&self) -> Result<(), RuntimeCommandPayloadError> {
        if self.payload_kind != RuntimeCommandPayloadKindV1::Result {
            return Err(RuntimeCommandPayloadError::WrongPayloadKind {
                expected: RuntimeCommandPayloadKindV1::Result,
                actual: self.payload_kind,
            });
        }
        self.validate_limits()?;
        match self.status {
            RuntimeCommandTerminalStatusV1::Succeeded if self.body.is_none() => {
                Err(RuntimeCommandPayloadError::SuccessMissingBody {
                    request_id: self.request_id.clone(),
                })
            }
            RuntimeCommandTerminalStatusV1::Failed if self.error.is_none() => {
                Err(RuntimeCommandPayloadError::FailureMissingError {
                    request_id: self.request_id.clone(),
                })
            }
            RuntimeCommandTerminalStatusV1::Succeeded
            | RuntimeCommandTerminalStatusV1::Failed
            | RuntimeCommandTerminalStatusV1::Cancelled => Ok(()),
        }
    }

    pub fn validate_limits(&self) -> Result<(), ProtocolLimitError> {
        validate_bytes_non_empty("runtime_command.request_id", self.request_id.len())?;
        validate_string_bytes(
            "runtime_command.request_id",
            &self.request_id,
            MAX_OBJECT_ID_BYTES,
        )?;
        if let Some(body) = &self.body {
            body.validate_limits()?;
        }
        if let Some(error) = &self.error {
            error.validate_limits()?;
        }
        validate_item_count(
            "runtime_command.clears_activity",
            self.clears_activity.len(),
            MAX_RUNTIME_COMMAND_ACTIVITY_CLEARS,
        )?;
        for clear in &self.clears_activity {
            clear.validate_limits()?;
        }
        Ok(())
    }
}

impl RuntimeCommandCancelV1 {
    pub fn validate_structure(&self) -> Result<(), RuntimeCommandPayloadError> {
        if self.payload_kind != RuntimeCommandPayloadKindV1::Cancel {
            return Err(RuntimeCommandPayloadError::WrongPayloadKind {
                expected: RuntimeCommandPayloadKindV1::Cancel,
                actual: self.payload_kind,
            });
        }
        self.validate_limits()?;
        Ok(())
    }

    pub fn validate_limits(&self) -> Result<(), ProtocolLimitError> {
        validate_bytes_non_empty("runtime_command.request_id", self.request_id.len())?;
        validate_string_bytes(
            "runtime_command.request_id",
            &self.request_id,
            MAX_OBJECT_ID_BYTES,
        )?;
        if let Some(reason) = &self.reason {
            validate_bytes_non_empty("runtime_command.cancel.reason", reason.len())?;
            validate_string_bytes("runtime_command.cancel.reason", reason, MAX_OBJECT_ID_BYTES)?;
        }
        Ok(())
    }
}

impl RuntimeCommandLedger {
    pub fn record_request(
        &mut self,
        context: RuntimeCommandIngressContext<'_>,
        request: &RuntimeCommandRequestV1,
    ) -> Result<RuntimeCommandLedgerDecision, RuntimeCommandLedgerError> {
        context.validate_limits()?;
        request.validate_structure()?;
        if !request.target.matches_device(context.local_device) {
            return Ok(RuntimeCommandLedgerDecision::IgnoredTarget);
        }

        let key = runtime_command_ledger_key(
            context.room_id,
            context.conversation_id,
            context.sender,
            &request.request_id,
        )?;
        if let Some(record) = self.records.get(&key) {
            if record.original_message_id == context.original_message_id
                && record.accepted_seq == context.accepted_seq
                && record.command == request.command
            {
                return Ok(RuntimeCommandLedgerDecision::Replayed);
            }
            return Err(RuntimeCommandLedgerError::ConflictingRequestId {
                request_id: request.request_id.clone(),
            });
        }
        if self.records.len() >= MAX_RUNTIME_COMMAND_LEDGER_RECORDS as usize {
            return Err(RuntimeCommandLedgerError::CapacityExceeded {
                max_records: MAX_RUNTIME_COMMAND_LEDGER_RECORDS,
            });
        }

        self.records.insert(
            key,
            RuntimeCommandLedgerRecord {
                room_id: context.room_id.to_string(),
                conversation_id: context.conversation_id.map(str::to_string),
                request_id: request.request_id.clone(),
                command: request.command.clone(),
                sender: context.sender.clone(),
                target: request.target.clone(),
                original_message_id: context.original_message_id.to_string(),
                accepted_seq: context.accepted_seq,
                resource_key: request.resource_key.clone(),
                status: RuntimeCommandLedgerStatus::Pending,
            },
        );
        assert!(self.records.len() <= MAX_RUNTIME_COMMAND_LEDGER_RECORDS as usize);
        Ok(RuntimeCommandLedgerDecision::Recorded)
    }

    pub fn mark_terminal(
        &mut self,
        room_id: &str,
        conversation_id: Option<&str>,
        sender: &DeviceRef,
        request_id: &str,
        status: RuntimeCommandLedgerStatus,
    ) -> Result<(), RuntimeCommandLedgerError> {
        validate_room_id(room_id)?;
        if let Some(conversation_id) = conversation_id {
            validate_bytes_non_empty("conversation_id", conversation_id.len())?;
            validate_string_bytes("conversation_id", conversation_id, MAX_OBJECT_ID_BYTES)?;
        }
        sender.validate_limits()?;
        validate_bytes_non_empty("runtime_command.request_id", request_id.len())?;
        validate_string_bytes(
            "runtime_command.request_id",
            request_id,
            MAX_OBJECT_ID_BYTES,
        )?;
        if status == RuntimeCommandLedgerStatus::Pending {
            return Err(RuntimeCommandLedgerError::NonTerminalStatus { status });
        }
        let key = runtime_command_ledger_key(room_id, conversation_id, sender, request_id)?;
        let record = self.records.get_mut(&key).ok_or_else(|| {
            RuntimeCommandLedgerError::RequestNotFound {
                request_id: request_id.to_string(),
            }
        })?;
        record.status = status;
        Ok(())
    }

    pub fn pending_requests(&self) -> Vec<&RuntimeCommandLedgerRecord> {
        self.records
            .values()
            .filter(|record| record.status == RuntimeCommandLedgerStatus::Pending)
            .collect()
    }

    pub fn get(
        &self,
        room_id: &str,
        conversation_id: Option<&str>,
        sender: &DeviceRef,
        request_id: &str,
    ) -> Option<&RuntimeCommandLedgerRecord> {
        let key = runtime_command_ledger_key(room_id, conversation_id, sender, request_id).ok()?;
        self.records.get(&key)
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

impl RuntimeCommandIngressContext<'_> {
    pub fn validate_limits(&self) -> Result<(), ProtocolLimitError> {
        validate_room_id(self.room_id)?;
        if let Some(conversation_id) = self.conversation_id {
            validate_bytes_non_empty("conversation_id", conversation_id.len())?;
            validate_string_bytes("conversation_id", conversation_id, MAX_OBJECT_ID_BYTES)?;
        }
        validate_bytes_non_empty("message_id", self.original_message_id.len())?;
        validate_string_bytes("message_id", self.original_message_id, MAX_OBJECT_ID_BYTES)?;
        self.sender.validate_limits()?;
        self.local_device.validate_limits()?;
        Ok(())
    }
}

impl DecryptedEphemeralActivityV1 {
    pub fn validate_limits(&self) -> Result<(), ProtocolLimitError> {
        validate_bytes_non_empty("ephemeral_activity.kind", self.activity_kind.len())?;
        validate_string_bytes(
            "ephemeral_activity.kind",
            &self.activity_kind,
            MAX_OBJECT_ID_BYTES,
        )?;
        if let Some(activity_id) = &self.activity_id {
            validate_bytes_non_empty("ephemeral_activity.activity_id", activity_id.len())?;
            validate_string_bytes(
                "ephemeral_activity.activity_id",
                activity_id,
                MAX_OBJECT_ID_BYTES,
            )?;
        }
        match self.action {
            EphemeralActivityActionV1::Set => {
                validate_bytes_non_empty("ephemeral_activity.payload", self.payload.len())?;
                validate_bytes_len(
                    "ephemeral_activity.payload",
                    self.payload.len(),
                    MAX_EPHEMERAL_ACTIVITY_DECRYPTED_PAYLOAD_BYTES,
                )?;
            }
            EphemeralActivityActionV1::Clear => {
                validate_bytes_len(
                    "ephemeral_activity.payload",
                    self.payload.len(),
                    MAX_EPHEMERAL_ACTIVITY_DECRYPTED_PAYLOAD_BYTES,
                )?;
            }
        }
        Ok(())
    }

    pub fn normalized_activity_id(&self) -> &str {
        self.activity_id
            .as_deref()
            .unwrap_or(FINITECHAT_DEFAULT_ACTIVITY_ID)
    }
}

impl EphemeralActivityIngressContext<'_> {
    pub fn validate_limits(&self) -> Result<(), EphemeralActivityProjectionError> {
        validate_room_id(self.room_id)?;
        if let Some(conversation_id) = self.conversation_id {
            validate_bytes_non_empty("conversation_id", conversation_id.len())?;
            validate_string_bytes("conversation_id", conversation_id, MAX_OBJECT_ID_BYTES)?;
        }
        self.sender.validate_limits()?;
        validate_ephemeral_activity_expiry(self.received_at_ms, self.expires_at_ms)?;
        Ok(())
    }
}

impl EphemeralActivityProjection {
    pub fn apply(
        &mut self,
        context: EphemeralActivityIngressContext<'_>,
        activity: &DecryptedEphemeralActivityV1,
    ) -> Result<EphemeralActivityProjectionDecision, EphemeralActivityProjectionError> {
        context.validate_limits()?;
        activity.validate_limits()?;
        let key = ephemeral_activity_projection_key(
            context.room_id,
            context.conversation_id,
            context.sender,
            &activity.activity_kind,
            activity.normalized_activity_id(),
        )?;
        match activity.action {
            EphemeralActivityActionV1::Set => {
                let existed = self.entries.contains_key(&key);
                if !existed
                    && self.entries.len() >= MAX_EPHEMERAL_ACTIVITY_PROJECTION_ENTRIES as usize
                {
                    return Err(EphemeralActivityProjectionError::CapacityExceeded {
                        max_records: MAX_EPHEMERAL_ACTIVITY_PROJECTION_ENTRIES,
                    });
                }
                self.entries.insert(
                    key,
                    EphemeralActivityProjectionEntry {
                        room_id: context.room_id.to_string(),
                        conversation_id: context.conversation_id.map(str::to_string),
                        sender: context.sender.clone(),
                        activity_kind: activity.activity_kind.clone(),
                        activity_id: activity.normalized_activity_id().to_string(),
                        payload: activity.payload.clone(),
                        received_at_ms: context.received_at_ms,
                        expires_at_ms: context.expires_at_ms,
                    },
                );
                assert!(self.entries.len() <= MAX_EPHEMERAL_ACTIVITY_PROJECTION_ENTRIES as usize);
                if existed {
                    Ok(EphemeralActivityProjectionDecision::Refreshed)
                } else {
                    Ok(EphemeralActivityProjectionDecision::Set)
                }
            }
            EphemeralActivityActionV1::Clear => {
                if self.entries.remove(&key).is_some() {
                    Ok(EphemeralActivityProjectionDecision::Cleared)
                } else {
                    Ok(EphemeralActivityProjectionDecision::ClearMiss)
                }
            }
        }
    }

    pub fn clear_from_durable_terminal(
        &mut self,
        room_id: &str,
        conversation_id: Option<&str>,
        sender: &DeviceRef,
        clear: &RuntimeActivityClearV1,
    ) -> Result<bool, EphemeralActivityProjectionError> {
        validate_room_id(room_id)?;
        if let Some(conversation_id) = conversation_id {
            validate_bytes_non_empty("conversation_id", conversation_id.len())?;
            validate_string_bytes("conversation_id", conversation_id, MAX_OBJECT_ID_BYTES)?;
        }
        sender.validate_limits()?;
        clear.validate_limits()?;
        let activity_id = clear
            .activity_id
            .as_deref()
            .unwrap_or(FINITECHAT_DEFAULT_ACTIVITY_ID);
        let key = ephemeral_activity_projection_key(
            room_id,
            clear.conversation_id.as_deref().or(conversation_id),
            sender,
            &clear.activity_kind,
            activity_id,
        )?;
        Ok(self.entries.remove(&key).is_some())
    }

    pub fn expire_at(&mut self, now_ms: u64) -> Result<u32, EphemeralActivityProjectionError> {
        let before = self.entries.len();
        self.entries.retain(|_, entry| entry.expires_at_ms > now_ms);
        let expired = before.saturating_sub(self.entries.len());
        u32::try_from(expired).map_err(|_| EphemeralActivityProjectionError::CapacityExceeded {
            max_records: u32::MAX,
        })
    }

    pub fn get(
        &self,
        room_id: &str,
        conversation_id: Option<&str>,
        sender: &DeviceRef,
        activity_kind: &str,
        activity_id: Option<&str>,
    ) -> Option<&EphemeralActivityProjectionEntry> {
        let key = ephemeral_activity_projection_key(
            room_id,
            conversation_id,
            sender,
            activity_kind,
            activity_id.unwrap_or(FINITECHAT_DEFAULT_ACTIVITY_ID),
        )
        .ok()?;
        self.entries.get(&key)
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

fn runtime_command_ledger_key(
    room_id: &str,
    conversation_id: Option<&str>,
    sender: &DeviceRef,
    request_id: &str,
) -> Result<String, ProtocolLimitError> {
    validate_room_id(room_id)?;
    if let Some(conversation_id) = conversation_id {
        validate_string_bytes("conversation_id", conversation_id, MAX_OBJECT_ID_BYTES)?;
    }
    sender.validate_limits()?;
    validate_string_bytes(
        "runtime_command.request_id",
        request_id,
        MAX_OBJECT_ID_BYTES,
    )?;
    Ok(format!(
        "{}|{}|{}|{}|{}",
        length_prefixed(room_id),
        length_prefixed(conversation_id.unwrap_or("")),
        length_prefixed(&sender.account_id),
        length_prefixed(&sender.device_id),
        length_prefixed(request_id)
    ))
}

fn ephemeral_activity_projection_key(
    room_id: &str,
    conversation_id: Option<&str>,
    sender: &DeviceRef,
    activity_kind: &str,
    activity_id: &str,
) -> Result<String, ProtocolLimitError> {
    validate_room_id(room_id)?;
    if let Some(conversation_id) = conversation_id {
        validate_string_bytes("conversation_id", conversation_id, MAX_OBJECT_ID_BYTES)?;
    }
    sender.validate_limits()?;
    validate_string_bytes(
        "ephemeral_activity.kind",
        activity_kind,
        MAX_OBJECT_ID_BYTES,
    )?;
    validate_string_bytes(
        "ephemeral_activity.activity_id",
        activity_id,
        MAX_OBJECT_ID_BYTES,
    )?;
    Ok(format!(
        "{}|{}|{}|{}|{}|{}",
        length_prefixed(room_id),
        length_prefixed(conversation_id.unwrap_or("")),
        length_prefixed(&sender.account_id),
        length_prefixed(&sender.device_id),
        length_prefixed(activity_kind),
        length_prefixed(activity_id)
    ))
}

fn validate_ephemeral_activity_expiry(
    received_at_ms: u64,
    expires_at_ms: u64,
) -> Result<(), EphemeralActivityProjectionError> {
    if expires_at_ms <= received_at_ms {
        return Err(EphemeralActivityProjectionError::AlreadyExpired);
    }
    let window = expires_at_ms - received_at_ms;
    if window > MAX_EPHEMERAL_ACTIVITY_EXPIRY_MILLIS {
        return Err(EphemeralActivityProjectionError::ExpiryTooLong {
            max_millis: MAX_EPHEMERAL_ACTIVITY_EXPIRY_MILLIS,
            actual_millis: window,
        });
    }
    Ok(())
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

    #[test]
    fn runtime_command_request_validates_kind_body_and_target_policy() {
        let local_runtime = device("runtime_npub", "runtime_box");
        let account_target = RuntimeCommandTargetV1 {
            account_id: "runtime_npub".to_string(),
            device_id: None,
        };
        let device_target = RuntimeCommandTargetV1 {
            account_id: "runtime_npub".to_string(),
            device_id: Some("runtime_box".to_string()),
        };
        let other_device_target = RuntimeCommandTargetV1 {
            account_id: "runtime_npub".to_string(),
            device_id: Some("gpu_worker".to_string()),
        };

        let request = runtime_command_request(
            "restart_1",
            "finitecomputer.runtime.gateway.restart",
            account_target.clone(),
            br#"{}"#,
        );
        request.validate_structure().unwrap();
        assert!(account_target.matches_device(&local_runtime));
        assert!(device_target.matches_device(&local_runtime));
        assert!(!other_device_target.matches_device(&local_runtime));

        let mut wrong_kind = request.clone();
        wrong_kind.payload_kind = RuntimeCommandPayloadKindV1::Result;
        assert!(matches!(
            wrong_kind.validate_structure().unwrap_err(),
            RuntimeCommandPayloadError::WrongPayloadKind { .. }
        ));

        let mut empty_schema = request;
        empty_schema.body.schema.clear();
        assert!(matches!(
            empty_schema.validate_structure().unwrap_err(),
            RuntimeCommandPayloadError::Protocol(ProtocolLimitError::BytesEmpty { field })
                if field == "runtime_command.payload.schema"
        ));
    }

    #[test]
    fn runtime_command_result_requires_terminal_shape_and_bounded_clears() {
        let ok_result = RuntimeCommandResultV1 {
            payload_kind: RuntimeCommandPayloadKindV1::Result,
            request_id: "restart_1".to_string(),
            status: RuntimeCommandTerminalStatusV1::Succeeded,
            body: Some(runtime_command_body(br#"{"status":"ok"}"#)),
            error: None,
            clears_activity: vec![RuntimeActivityClearV1 {
                activity_kind: "working".to_string(),
                activity_id: Some("restart_1".to_string()),
                conversation_id: Some("topic_1".to_string()),
            }],
        };
        ok_result.validate_structure().unwrap();

        let missing_success_body = RuntimeCommandResultV1 {
            body: None,
            ..ok_result.clone()
        };
        assert!(matches!(
            missing_success_body.validate_structure().unwrap_err(),
            RuntimeCommandPayloadError::SuccessMissingBody { .. }
        ));

        let missing_failure_error = RuntimeCommandResultV1 {
            status: RuntimeCommandTerminalStatusV1::Failed,
            body: None,
            error: None,
            clears_activity: Vec::new(),
            ..ok_result.clone()
        };
        assert!(matches!(
            missing_failure_error.validate_structure().unwrap_err(),
            RuntimeCommandPayloadError::FailureMissingError { .. }
        ));

        let too_many_clears = RuntimeCommandResultV1 {
            clears_activity: vec![
                RuntimeActivityClearV1 {
                    activity_kind: "working".to_string(),
                    activity_id: None,
                    conversation_id: None,
                };
                MAX_RUNTIME_COMMAND_ACTIVITY_CLEARS as usize + 1
            ],
            ..ok_result
        };
        assert!(matches!(
            too_many_clears.validate_structure().unwrap_err(),
            RuntimeCommandPayloadError::Protocol(ProtocolLimitError::TooManyItems { field, .. })
                if field == "runtime_command.clears_activity"
        ));
    }

    #[test]
    fn runtime_command_ledger_records_after_decrypted_target_policy() {
        let sender = device("alice_npub", "dashboard");
        let local_runtime = device("runtime_npub", "runtime_box");
        let targeted = runtime_command_request(
            "restart_1",
            "finitecomputer.runtime.gateway.restart",
            RuntimeCommandTargetV1 {
                account_id: "runtime_npub".to_string(),
                device_id: Some("runtime_box".to_string()),
            },
            br#"{}"#,
        );
        let not_for_local_device = runtime_command_request(
            "restart_2",
            "finitecomputer.runtime.gateway.restart",
            RuntimeCommandTargetV1 {
                account_id: "runtime_npub".to_string(),
                device_id: Some("other_device".to_string()),
            },
            br#"{}"#,
        );
        let mut ledger = RuntimeCommandLedger::default();

        assert_eq!(
            ledger
                .record_request(
                    RuntimeCommandIngressContext {
                        room_id: "room_1",
                        conversation_id: Some("topic_1"),
                        accepted_seq: 12,
                        original_message_id: "message_1",
                        sender: &sender,
                        local_device: &local_runtime,
                    },
                    &targeted,
                )
                .unwrap(),
            RuntimeCommandLedgerDecision::Recorded
        );
        assert_eq!(
            ledger
                .record_request(
                    RuntimeCommandIngressContext {
                        room_id: "room_1",
                        conversation_id: Some("topic_1"),
                        accepted_seq: 12,
                        original_message_id: "message_1",
                        sender: &sender,
                        local_device: &local_runtime,
                    },
                    &targeted,
                )
                .unwrap(),
            RuntimeCommandLedgerDecision::Replayed
        );
        assert_eq!(
            ledger
                .record_request(
                    RuntimeCommandIngressContext {
                        room_id: "room_1",
                        conversation_id: Some("topic_1"),
                        accepted_seq: 13,
                        original_message_id: "message_2",
                        sender: &sender,
                        local_device: &local_runtime,
                    },
                    &targeted,
                )
                .unwrap_err(),
            RuntimeCommandLedgerError::ConflictingRequestId {
                request_id: "restart_1".to_string()
            }
        );
        assert_eq!(
            ledger
                .record_request(
                    RuntimeCommandIngressContext {
                        room_id: "room_1",
                        conversation_id: Some("topic_1"),
                        accepted_seq: 14,
                        original_message_id: "message_3",
                        sender: &sender,
                        local_device: &local_runtime,
                    },
                    &not_for_local_device,
                )
                .unwrap(),
            RuntimeCommandLedgerDecision::IgnoredTarget
        );
        assert_eq!(ledger.len(), 1);
        assert_eq!(ledger.pending_requests().len(), 1);

        assert!(matches!(
            ledger
                .mark_terminal(
                    "room_1",
                    Some("topic_1"),
                    &sender,
                    "restart_1",
                    RuntimeCommandLedgerStatus::Pending,
                )
                .unwrap_err(),
            RuntimeCommandLedgerError::NonTerminalStatus { .. }
        ));
        ledger
            .mark_terminal(
                "room_1",
                Some("topic_1"),
                &sender,
                "restart_1",
                RuntimeCommandLedgerStatus::Succeeded,
            )
            .unwrap();
        assert!(ledger.pending_requests().is_empty());
        assert_eq!(
            ledger
                .get("room_1", Some("topic_1"), &sender, "restart_1")
                .unwrap()
                .status,
            RuntimeCommandLedgerStatus::Succeeded
        );
    }

    #[test]
    fn activity_projection_keeps_devices_separate_and_clear_scoped() {
        let phone = device("alice_npub", "phone");
        let laptop = device("alice_npub", "laptop");
        let mut projection = EphemeralActivityProjection::default();

        projection
            .apply(
                activity_context("room_1", Some("topic_1"), &phone, 1_000, 11_000),
                &activity_set("typing", None, br#"{"chars":3}"#),
            )
            .unwrap();
        projection
            .apply(
                activity_context("room_1", Some("topic_1"), &laptop, 1_000, 11_000),
                &activity_set("typing", None, br#"{"chars":1}"#),
            )
            .unwrap();

        assert_eq!(projection.len(), 2);
        assert_eq!(
            projection
                .apply(
                    activity_context("room_1", Some("topic_1"), &phone, 2_000, 12_000),
                    &activity_clear("typing", None),
                )
                .unwrap(),
            EphemeralActivityProjectionDecision::Cleared
        );
        assert!(
            projection
                .get("room_1", Some("topic_1"), &phone, "typing", None)
                .is_none()
        );
        assert!(
            projection
                .get("room_1", Some("topic_1"), &laptop, "typing", None)
                .is_some()
        );
    }

    #[test]
    fn activity_refresh_extends_matching_device_expiry() {
        let runtime = device("runtime_npub", "box");
        let mut projection = EphemeralActivityProjection::default();

        assert_eq!(
            projection
                .apply(
                    activity_context("room_1", Some("topic_1"), &runtime, 1_000, 11_000),
                    &activity_set("working", Some("run_1"), br#"{"pct":10}"#),
                )
                .unwrap(),
            EphemeralActivityProjectionDecision::Set
        );
        assert_eq!(
            projection
                .apply(
                    activity_context("room_1", Some("topic_1"), &runtime, 2_000, 22_000),
                    &activity_set("working", Some("run_1"), br#"{"pct":20}"#),
                )
                .unwrap(),
            EphemeralActivityProjectionDecision::Refreshed
        );
        let current = projection
            .get(
                "room_1",
                Some("topic_1"),
                &runtime,
                "working",
                Some("run_1"),
            )
            .unwrap();
        assert_eq!(current.expires_at_ms, 22_000);
        assert_eq!(current.payload, br#"{"pct":20}"#);
    }

    #[test]
    fn durable_terminal_clear_is_sender_and_activity_scoped() {
        let runtime = device("runtime_npub", "box");
        let sibling = device("runtime_npub", "gpu");
        let mut projection = EphemeralActivityProjection::default();
        for sender in [&runtime, &sibling] {
            projection
                .apply(
                    activity_context("room_1", Some("topic_1"), sender, 1_000, 11_000),
                    &activity_set("working", Some("restart_1"), br#"{}"#),
                )
                .unwrap();
        }

        let removed = projection
            .clear_from_durable_terminal(
                "room_1",
                Some("topic_1"),
                &runtime,
                &RuntimeActivityClearV1 {
                    activity_kind: "working".to_string(),
                    activity_id: Some("restart_1".to_string()),
                    conversation_id: None,
                },
            )
            .unwrap();

        assert!(removed);
        assert!(
            projection
                .get(
                    "room_1",
                    Some("topic_1"),
                    &runtime,
                    "working",
                    Some("restart_1"),
                )
                .is_none()
        );
        assert!(
            projection
                .get(
                    "room_1",
                    Some("topic_1"),
                    &sibling,
                    "working",
                    Some("restart_1"),
                )
                .is_some()
        );
    }

    #[test]
    fn activity_projection_expires_and_rejects_bad_lease_windows() {
        let runtime = device("runtime_npub", "box");
        let mut projection = EphemeralActivityProjection::default();
        projection
            .apply(
                activity_context("room_1", None, &runtime, 1_000, 11_000),
                &activity_set("finitecomputer.indexing", Some("job_1"), br#"{}"#),
            )
            .unwrap();

        assert_eq!(projection.expire_at(10_999).unwrap(), 0);
        assert_eq!(projection.expire_at(11_000).unwrap(), 1);
        assert!(projection.is_empty());

        assert_eq!(
            projection
                .apply(
                    activity_context("room_1", None, &runtime, 1_000, 1_000),
                    &activity_set("thinking", None, br#"{}"#),
                )
                .unwrap_err(),
            EphemeralActivityProjectionError::AlreadyExpired
        );
        assert!(matches!(
            projection
                .apply(
                    activity_context(
                        "room_1",
                        None,
                        &runtime,
                        1_000,
                        1_001 + MAX_EPHEMERAL_ACTIVITY_EXPIRY_MILLIS,
                    ),
                    &activity_set("thinking", None, br#"{}"#),
                )
                .unwrap_err(),
            EphemeralActivityProjectionError::ExpiryTooLong { .. }
        ));
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

    fn runtime_command_request(
        request_id: &str,
        command: &str,
        target: RuntimeCommandTargetV1,
        body: &[u8],
    ) -> RuntimeCommandRequestV1 {
        RuntimeCommandRequestV1 {
            payload_kind: RuntimeCommandPayloadKindV1::Request,
            request_id: request_id.to_string(),
            command: command.to_string(),
            target,
            resource_key: Some("hermes.config".to_string()),
            body: runtime_command_body(body),
        }
    }

    fn runtime_command_body(body: &[u8]) -> RuntimeCommandJsonPayloadV1 {
        RuntimeCommandJsonPayloadV1 {
            schema: "finitecomputer.runtime.command.body.v1".to_string(),
            json_payload: body.to_vec(),
        }
    }

    fn activity_context<'a>(
        room_id: &'a str,
        conversation_id: Option<&'a str>,
        sender: &'a DeviceRef,
        received_at_ms: u64,
        expires_at_ms: u64,
    ) -> EphemeralActivityIngressContext<'a> {
        EphemeralActivityIngressContext {
            room_id,
            conversation_id,
            sender,
            received_at_ms,
            expires_at_ms,
        }
    }

    fn activity_set(
        activity_kind: &str,
        activity_id: Option<&str>,
        payload: &[u8],
    ) -> DecryptedEphemeralActivityV1 {
        DecryptedEphemeralActivityV1 {
            activity_kind: activity_kind.to_string(),
            activity_id: activity_id.map(str::to_string),
            action: EphemeralActivityActionV1::Set,
            payload: payload.to_vec(),
        }
    }

    fn activity_clear(
        activity_kind: &str,
        activity_id: Option<&str>,
    ) -> DecryptedEphemeralActivityV1 {
        DecryptedEphemeralActivityV1 {
            activity_kind: activity_kind.to_string(),
            activity_id: activity_id.map(str::to_string),
            action: EphemeralActivityActionV1::Clear,
            payload: Vec::new(),
        }
    }
}
