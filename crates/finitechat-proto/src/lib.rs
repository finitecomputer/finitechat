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
