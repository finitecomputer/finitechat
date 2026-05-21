//! Deterministic fake-MLS scenarios.
//!
//! This crate intentionally avoids real cryptography for the first slice. It
//! runs the same reducer surfaces that production storage/API code must keep
//! semantically equivalent.

use std::error::Error;
use std::fmt;

use finitechat_engine::{
    AppendEventRequest, ClaimKeyPackageResult, CreateRoomRequest, DeliveryService, EngineError,
    SubmitCommitRequest, UploadKeyPackageRequest, device, envelope,
};
use finitechat_proto::{
    DeviceRef, LogEntryKind, MembershipAddV1, MembershipDeltaV1, MembershipRemoveV1,
    StagedWelcomeV1,
};

pub type Result<T> = std::result::Result<T, SimError>;

#[derive(Debug)]
pub enum SimError {
    Engine(EngineError),
    EnvelopeJson(serde_json::Error),
    ExpectedWelcome {
        welcome_id: String,
        device: DeviceRef,
    },
}

impl fmt::Display for SimError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Engine(error) => write!(f, "{error}"),
            Self::EnvelopeJson(error) => write!(f, "failed to derive envelope message id: {error}"),
            Self::ExpectedWelcome { welcome_id, device } => write!(
                f,
                "expected welcome {welcome_id} for device {}/{}",
                device.account_id, device.device_id
            ),
        }
    }
}

impl Error for SimError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Engine(error) => Some(error),
            Self::EnvelopeJson(error) => Some(error),
            Self::ExpectedWelcome { .. } => None,
        }
    }
}

impl From<EngineError> for SimError {
    fn from(error: EngineError) -> Self {
        Self::Engine(error)
    }
}

impl From<serde_json::Error> for SimError {
    fn from(error: serde_json::Error) -> Self {
        Self::EnvelopeJson(error)
    }
}

pub struct SimWorld {
    pub server: DeliveryService,
    pub room_id: String,
    pub group_id: String,
}

impl SimWorld {
    pub fn direct_room() -> Result<Self> {
        let mut server = DeliveryService::new();
        server.create_room(CreateRoomRequest {
            room_id: "room_direct".to_string(),
            mls_group_id: "mls_direct".to_string(),
            creator: alice(),
        })?;
        Ok(Self {
            server,
            room_id: "room_direct".to_string(),
            group_id: "mls_direct".to_string(),
        })
    }

    pub fn upload_and_claim(
        &mut self,
        owner: DeviceRef,
        key_package_id: &str,
    ) -> Result<ClaimKeyPackageResult> {
        self.server.upload_key_package(UploadKeyPackageRequest {
            key_package_id: key_package_id.to_string(),
            owner,
            key_package_ref: format!("ref_{key_package_id}"),
            key_package_hash: format!("hash_{key_package_id}"),
            key_package_payload: fake_key_package_payload(key_package_id),
        })?;
        Ok(self.server.claim_key_package(key_package_id)?)
    }

    pub fn add_device_commit(
        &mut self,
        sender: DeviceRef,
        target: DeviceRef,
        key_package_id: &str,
        welcome_id: &str,
        expected_epoch: u64,
        idempotency_key: &str,
    ) -> Result<()> {
        self.upload_and_claim(target.clone(), key_package_id)?;
        let commit = envelope(
            self.room_id.clone(),
            self.group_id.clone(),
            sender.clone(),
            expected_epoch,
            LogEntryKind::Commit,
            format!("add:{}:{}", target.account_id, target.device_id).into_bytes(),
        );
        let commit_message_id = commit.message_id()?;
        self.server.submit_commit(SubmitCommitRequest {
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
        })?;
        Ok(())
    }

    pub fn add_device_request(
        &self,
        sender: DeviceRef,
        target: DeviceRef,
        key_package_id: &str,
        welcome_id: &str,
        expected_epoch: u64,
        idempotency_key: &str,
    ) -> Result<SubmitCommitRequest> {
        let commit = envelope(
            self.room_id.clone(),
            self.group_id.clone(),
            sender.clone(),
            expected_epoch,
            LogEntryKind::Commit,
            format!("add:{}:{}", target.account_id, target.device_id).into_bytes(),
        );
        let commit_message_id = commit.message_id()?;
        Ok(SubmitCommitRequest {
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
        })
    }

    pub fn remove_device_request(
        &self,
        sender: DeviceRef,
        target: DeviceRef,
        expected_epoch: u64,
        idempotency_key: &str,
    ) -> Result<SubmitCommitRequest> {
        let commit = envelope(
            self.room_id.clone(),
            self.group_id.clone(),
            sender.clone(),
            expected_epoch,
            LogEntryKind::Commit,
            format!("remove:{}:{}", target.account_id, target.device_id).into_bytes(),
        );
        let commit_message_id = commit.message_id()?;
        Ok(SubmitCommitRequest {
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
        })
    }

    pub fn app_message_request(
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

    pub fn activate_device(&mut self, welcome_id: &str, device: DeviceRef) -> Result<()> {
        let welcomes = self.server.claim_welcomes(&device);
        if !welcomes
            .iter()
            .any(|welcome| welcome.welcome_id == welcome_id)
        {
            return Err(SimError::ExpectedWelcome {
                welcome_id: welcome_id.to_string(),
                device,
            });
        }
        self.server.ack_welcome(welcome_id, true)?;
        Ok(())
    }
}

pub fn staged_welcome(welcome_id: &str) -> StagedWelcomeV1 {
    StagedWelcomeV1 {
        welcome_id: welcome_id.to_string(),
        welcome_payload: format!("welcome:{welcome_id}").into_bytes(),
        ratchet_tree_payload: format!("tree:{welcome_id}").into_bytes(),
    }
}

pub fn fake_key_package_payload(key_package_id: &str) -> Vec<u8> {
    format!("key-package:{key_package_id}").into_bytes()
}

pub fn alice() -> DeviceRef {
    device("alice_npub", "alice_browser")
}

pub fn bob() -> DeviceRef {
    device("bob_npub", "bob_runtime")
}

pub fn charlie() -> DeviceRef {
    device("charlie_npub", "charlie_phone")
}

pub fn dana() -> DeviceRef {
    device("dana_npub", "dana_tablet")
}
