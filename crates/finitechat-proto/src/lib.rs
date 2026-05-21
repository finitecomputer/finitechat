use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
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
pub type Epoch = u64;
pub type Seq = u64;

pub const MESSAGE_ID_DOMAIN: &[u8] = b"finite-message-id-v1";
pub const MAX_ENVELOPE_PAYLOAD_BYTES: u32 = 256 * 1024;
pub const MAX_SYNC_PAGE_ENTRIES: u32 = 100;
pub const MAX_SYNC_PAGE_BYTES: u32 = 4 * 1024 * 1024;
pub const MAX_DIRECT_ROOM_DEVICES_PER_ACCOUNT: u32 = 8;
pub const MAX_WELCOME_CLAIMS_PER_REQUEST: u32 = 32;
pub const MAX_STAGED_WELCOMES_PER_COMMIT: u32 = 32;
pub const MAX_WELCOME_PAYLOAD_BYTES: u32 = 1024 * 1024;
pub const MAX_RATCHET_TREE_PAYLOAD_BYTES: u32 = 1024 * 1024;
pub const MAX_IDEMPOTENCY_RECORDS_PER_ROOM_DEVICE: u32 = 4096;
pub const MAX_LINK_SESSION_PAYLOAD_BYTES: u32 = 1024 * 1024;
pub const MAX_IDEMPOTENCY_KEY_BYTES: u32 = 128;
pub const MAX_ACCOUNT_ID_BYTES: u32 = 128;
pub const MAX_DEVICE_ID_BYTES: u32 = 128;
pub const MAX_ROOM_ID_BYTES: u32 = 128;
pub const MAX_MLS_GROUP_ID_BYTES: u32 = 128;
pub const MAX_OBJECT_ID_BYTES: u32 = 128;

const _: () = {
    assert!(MAX_ENVELOPE_PAYLOAD_BYTES > 0);
    assert!(MAX_SYNC_PAGE_ENTRIES > 0);
    assert!(MAX_SYNC_PAGE_BYTES >= MAX_ENVELOPE_PAYLOAD_BYTES);
    assert!(MAX_DIRECT_ROOM_DEVICES_PER_ACCOUNT > 0);
    assert!(MAX_WELCOME_CLAIMS_PER_REQUEST > 0);
    assert!(MAX_STAGED_WELCOMES_PER_COMMIT > 0);
    assert!(MAX_STAGED_WELCOMES_PER_COMMIT >= MAX_DIRECT_ROOM_DEVICES_PER_ACCOUNT);
    assert!(MAX_WELCOME_PAYLOAD_BYTES > 0);
    assert!(MAX_RATCHET_TREE_PAYLOAD_BYTES > 0);
    assert!(MAX_IDEMPOTENCY_RECORDS_PER_ROOM_DEVICE > 0);
    assert!(MAX_LINK_SESSION_PAYLOAD_BYTES > 0);
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
}
