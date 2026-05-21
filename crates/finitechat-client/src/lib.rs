use finitechat_engine::{
    AppendEventRequest, ClaimKeyPackageResult, CreateDirectRoomRequest, EngineError,
    SubmitCommitRequest, UploadKeyPackageRequest, envelope,
};
use finitechat_mls::{
    ExpectedDeviceCredential, FiniteDeviceCredentialV1, MlsCredentialError, NOSTR_PUBLIC_KEY_BYTES,
    NostrPublicKey, NostrSecretKey,
};
use finitechat_proto::message_id_for_bytes;
use finitechat_proto::{
    DeviceRef, KeyPackageId, LogEntryKind, MAX_ACCOUNT_ID_BYTES, MAX_DEVICE_ID_BYTES,
    MAX_OBJECT_ID_BYTES, MAX_STAGED_WELCOMES_PER_COMMIT, MembershipAddV1, MembershipDeltaV1,
    MembershipRemoveV1, MlsGroupId, ProtocolLimitError, RoomId, RoomLogEntry, StagedWelcomeV1,
    WelcomeId, validate_bytes_len, validate_bytes_non_empty, validate_idempotency_key,
    validate_item_count, validate_mls_group_id, validate_room_id, validate_string_bytes,
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
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

pub const FINITECHAT_CIPHERSUITE: Ciphersuite =
    Ciphersuite::MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519;

const CLIENT_STORE_KEY_DERIVATION_DOMAIN: &[u8] = b"finitechat.client-store-key.v1";
const CLIENT_STATE_SNAPSHOT_MAGIC: &[u8] = b"finitechat.client-state-snapshot.v1";
const CLIENT_STATE_SNAPSHOT_VERSION: u16 = 2;
const CLIENT_STORE_KEY_BYTES: usize = 32;
const CLIENT_STORE_NONCE_BYTES: usize = 12;
const CLIENT_STORE_AEAD_TAG_BYTES: u32 = 16;
const MAX_PERSISTED_ROOMS: u32 = 1024;
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

const _: () = {
    assert!(NOSTR_PUBLIC_KEY_BYTES == 32);
    assert!(CLIENT_STORE_KEY_BYTES == 32);
    assert!(CLIENT_STORE_NONCE_BYTES == 12);
    assert!(CLIENT_STORE_AEAD_TAG_BYTES == 16);
    assert!(MAX_PERSISTED_ROOMS > 0);
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
    pub openmls_storage_records: Vec<OpenMlsStorageRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedRoomState {
    pub room_id: RoomId,
    pub mls_group_id: MlsGroupId,
    pub last_applied_seq: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenMlsStorageRecord {
    pub key: Vec<u8>,
    pub value: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppliedLogEntry {
    Application(Vec<u8>),
    Commit { sender: DeviceRef, epoch: u64 },
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
        let room_cursors = state
            .rooms
            .iter()
            .map(|room| (room.room_id.clone(), room.last_applied_seq))
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

        let mut rooms = self
            .groups
            .iter()
            .map(|(room_id, group)| {
                Ok(PersistedRoomState {
                    room_id: room_id.clone(),
                    mls_group_id: mls_group_id_string(group.group_id())?,
                    last_applied_seq: *self.room_cursors.get(room_id).unwrap_or(&0),
                })
            })
            .collect::<Result<Vec<_>, ClientError>>()?;
        rooms.sort_by(|left, right| left.room_id.cmp(&right.room_id));

        let state = FiniteChatDeviceState {
            device_ref: self.device_ref.clone(),
            signer_public_key: self.signer.public().to_vec(),
            credential_identity: self.credential.identity_bytes(),
            rooms,
            openmls_storage_records,
        };
        state.validate_limits()?;
        Ok(state)
    }

    pub fn device_ref(&self) -> &DeviceRef {
        &self.device_ref
    }

    pub fn create_direct_room_request(
        &self,
        room_id: impl Into<String>,
        mls_group_id: impl Into<String>,
        other_account_id: impl Into<String>,
    ) -> CreateDirectRoomRequest {
        CreateDirectRoomRequest {
            room_id: room_id.into(),
            mls_group_id: mls_group_id.into(),
            creator: self.device_ref.clone(),
            other_account_id: other_account_id.into(),
        }
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
        let key_package_id = key_package_id.into();
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

        let request = UploadKeyPackageRequest {
            key_package_id,
            owner: self.device_ref.clone(),
            key_package_ref: hex_lower(key_package_ref.as_slice()),
            key_package_hash: message_id_for_bytes(&payload),
            key_package_payload: payload,
        };
        request.validate_limits()?;
        Ok(request)
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
                let plaintext = self.decrypt_application_entry(room_id, entry)?;
                Ok(AppliedLogEntry::Application(plaintext))
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
    ) -> Result<Vec<u8>, ClientError> {
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
        let ProcessedMessageContent::ApplicationMessage(message) = processed.into_content() else {
            return Err(ClientError::UnexpectedMessage);
        };
        Ok(message.into_bytes())
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
            if credential.device_id() == device.device_id {
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
            if credential.device_id() == device.device_id {
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
        for room in &self.rooms {
            if !seen_rooms.insert(room.room_id.clone()) {
                return Err(ClientError::DuplicatePersistedRoom(room.room_id.clone()));
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
        Ok(())
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

#[derive(Debug, Clone)]
pub struct SqliteClientStore {
    db_path: PathBuf,
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

        let store = Self { db_path, options };
        let conn = store.connect()?;
        migrate_client_store(&conn)?;
        Ok(store)
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
        let conn = self.connect()?;
        let state =
            load_device_state(&conn, &self.options.encryption_key, &account_id, &device_id)?
                .ok_or(ClientStoreError::DeviceStateNotFound {
                    account_id,
                    device_id,
                })?;
        Ok(FiniteChatDevice::from_state(config, state)?)
    }

    pub fn activate_welcome_and_save(
        &mut self,
        device: &mut FiniteChatDevice,
        room_id: impl Into<RoomId>,
        welcome_payload: &[u8],
        ratchet_tree_payload: &[u8],
        commit_seq: u64,
    ) -> Result<(), ClientStoreError> {
        let room_id = room_id.into();
        device.activate_welcome(room_id.clone(), welcome_payload, ratchet_tree_payload)?;
        device.set_last_applied_seq(&room_id, commit_seq)?;
        self.save_device_state(device)
    }

    pub fn apply_log_entry_and_save(
        &mut self,
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
        let applied = device.apply_log_entry(room_id, entry)?;
        device.set_last_applied_seq(room_id, entry.seq)?;
        self.save_device_state(device)?;
        Ok(Some(applied))
    }

    fn connect(&self) -> Result<Connection, ClientStoreError> {
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
        f: impl FnOnce(&Transaction<'_>) -> Result<T, ClientStoreError>,
    ) -> Result<T, ClientStoreError> {
        let mut conn = self.connect()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let value = f(&tx)?;
        tx.commit()?;
        Ok(value)
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
    #[error("failed to serialize OpenMLS KeyPackage")]
    SerializeKeyPackage,
    #[error("failed to parse OpenMLS KeyPackage")]
    ParseKeyPackage,
    #[error("claimed KeyPackage ref does not match payload")]
    KeyPackageRefMismatch,
    #[error("claimed KeyPackage hash does not match payload")]
    KeyPackageHashMismatch,
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
        rooms.push(PersistedRoomState {
            room_id: cursor.take_string("room_id", finitechat_proto::MAX_ROOM_ID_BYTES)?,
            mls_group_id: cursor
                .take_string("mls_group_id", finitechat_proto::MAX_MLS_GROUP_ID_BYTES)?,
            last_applied_seq: cursor.take_u64()?,
        });
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
