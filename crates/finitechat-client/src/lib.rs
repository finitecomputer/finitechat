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
    DeviceRef, KeyPackageId, LogEntryKind, MAX_OBJECT_ID_BYTES, MAX_STAGED_WELCOMES_PER_COMMIT,
    MembershipAddV1, MembershipDeltaV1, MlsGroupId, ProtocolLimitError, RoomId, RoomLogEntry,
    StagedWelcomeV1, WelcomeId, validate_bytes_non_empty, validate_idempotency_key,
    validate_mls_group_id, validate_room_id, validate_string_bytes,
};
use openmls::prelude::tls_codec::{Deserialize as _, Serialize as _};
use openmls::prelude::{
    Ciphersuite, CredentialWithKey, GroupId, KeyPackage, KeyPackageIn, MlsGroup,
    MlsGroupCreateConfig, MlsMessageBodyIn, MlsMessageIn, MlsMessageOut, OpenMlsProvider,
    ProcessedMessageContent, ProtocolMessage, ProtocolVersion, RatchetTreeIn, StagedCommit,
    StagedWelcome, Welcome,
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

const _: () = {
    assert!(NOSTR_PUBLIC_KEY_BYTES == 32);
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

        let device = Self {
            provider,
            device_ref,
            now_unix_seconds: config.now_unix_seconds,
            credential,
            credential_with_key,
            signer,
            groups,
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
        self.groups.insert(room_id, group);
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
        self.groups.insert(room_id, group);
        Ok(())
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
}

impl FiniteChatDeviceState {
    fn validate_limits(&self) -> Result<(), ClientError> {
        self.device_ref.validate_limits()?;
        validate_bytes_non_empty("signer_public_key", self.signer_public_key.len())?;
        validate_bytes_non_empty("credential_identity", self.credential_identity.len())?;
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
        validate_bytes_non_empty("openmls_storage.value", self.value.len())?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct SqliteClientStore {
    db_path: PathBuf,
}

impl SqliteClientStore {
    pub fn open(db_path: impl AsRef<Path>) -> Result<Self, ClientStoreError> {
        let db_path = db_path.as_ref().to_path_buf();
        if let Some(parent) = db_path.parent() {
            fs::create_dir_all(parent).map_err(|source| ClientStoreError::CreateDir {
                path: parent.to_path_buf(),
                source,
            })?;
        }

        let store = Self { db_path };
        let conn = store.connect()?;
        migrate_client_store(&conn)?;
        Ok(store)
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub fn save_device_state(&mut self, device: &FiniteChatDevice) -> Result<(), ClientStoreError> {
        let state = device.export_state()?;
        self.with_transaction(|tx| save_device_state_tx(tx, &state))
    }

    pub fn load_device(
        &self,
        config: FiniteChatDeviceConfig,
    ) -> Result<FiniteChatDevice, ClientStoreError> {
        let account_id = hex_lower(config.account_secret_key.public_key().as_bytes());
        let device_id = config.device_id.clone();
        let conn = self.connect()?;
        let state = load_device_state(&conn, &account_id, &device_id)?.ok_or(
            ClientStoreError::DeviceStateNotFound {
                account_id,
                device_id,
            },
        )?;
        Ok(FiniteChatDevice::from_state(config, state)?)
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
    #[error("corrupt finitechat client state: {0}")]
    CorruptState(String),
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
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS client_profiles (
          account_id TEXT NOT NULL,
          device_id TEXT NOT NULL,
          signer_public_key BLOB NOT NULL,
          credential_identity BLOB NOT NULL,
          PRIMARY KEY (account_id, device_id)
        );

        CREATE TABLE IF NOT EXISTS client_rooms (
          account_id TEXT NOT NULL,
          device_id TEXT NOT NULL,
          room_id TEXT NOT NULL,
          mls_group_id TEXT NOT NULL,
          PRIMARY KEY (account_id, device_id, room_id),
          FOREIGN KEY (account_id, device_id)
            REFERENCES client_profiles(account_id, device_id)
            ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS client_openmls_storage (
          account_id TEXT NOT NULL,
          device_id TEXT NOT NULL,
          storage_key BLOB NOT NULL,
          storage_value BLOB NOT NULL,
          PRIMARY KEY (account_id, device_id, storage_key),
          FOREIGN KEY (account_id, device_id)
            REFERENCES client_profiles(account_id, device_id)
            ON DELETE CASCADE
        );
        "#,
    )?;
    Ok(())
}

fn save_device_state_tx(
    tx: &Transaction<'_>,
    state: &FiniteChatDeviceState,
) -> Result<(), ClientStoreError> {
    state.validate_limits()?;
    tx.execute(
        r#"
        INSERT INTO client_profiles (
          account_id, device_id, signer_public_key, credential_identity
        ) VALUES (?1, ?2, ?3, ?4)
        ON CONFLICT(account_id, device_id) DO UPDATE SET
          signer_public_key = excluded.signer_public_key,
          credential_identity = excluded.credential_identity
        "#,
        params![
            state.device_ref.account_id,
            state.device_ref.device_id,
            state.signer_public_key,
            state.credential_identity,
        ],
    )?;
    tx.execute(
        "DELETE FROM client_rooms WHERE account_id = ?1 AND device_id = ?2",
        params![state.device_ref.account_id, state.device_ref.device_id],
    )?;
    tx.execute(
        "DELETE FROM client_openmls_storage WHERE account_id = ?1 AND device_id = ?2",
        params![state.device_ref.account_id, state.device_ref.device_id],
    )?;
    for room in &state.rooms {
        tx.execute(
            r#"
            INSERT INTO client_rooms (account_id, device_id, room_id, mls_group_id)
            VALUES (?1, ?2, ?3, ?4)
            "#,
            params![
                state.device_ref.account_id,
                state.device_ref.device_id,
                room.room_id,
                room.mls_group_id,
            ],
        )?;
    }
    for record in &state.openmls_storage_records {
        tx.execute(
            r#"
            INSERT INTO client_openmls_storage (
              account_id, device_id, storage_key, storage_value
            ) VALUES (?1, ?2, ?3, ?4)
            "#,
            params![
                state.device_ref.account_id,
                state.device_ref.device_id,
                record.key,
                record.value,
            ],
        )?;
    }
    Ok(())
}

fn load_device_state(
    conn: &Connection,
    account_id: &str,
    device_id: &str,
) -> Result<Option<FiniteChatDeviceState>, ClientStoreError> {
    validate_string_bytes(
        "account_id",
        account_id,
        finitechat_proto::MAX_ACCOUNT_ID_BYTES,
    )
    .map_err(ClientError::from)?;
    validate_string_bytes(
        "device_id",
        device_id,
        finitechat_proto::MAX_DEVICE_ID_BYTES,
    )
    .map_err(ClientError::from)?;
    let profile = conn
        .query_row(
            r#"
            SELECT signer_public_key, credential_identity
            FROM client_profiles
            WHERE account_id = ?1 AND device_id = ?2
            "#,
            params![account_id, device_id],
            |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, Vec<u8>>(1)?)),
        )
        .optional()?;
    let Some((signer_public_key, credential_identity)) = profile else {
        return Ok(None);
    };

    let rooms = load_persisted_rooms(conn, account_id, device_id)?;
    let openmls_storage_records = load_openmls_storage_records(conn, account_id, device_id)?;
    let state = FiniteChatDeviceState {
        device_ref: DeviceRef {
            account_id: account_id.to_string(),
            device_id: device_id.to_string(),
        },
        signer_public_key,
        credential_identity,
        rooms,
        openmls_storage_records,
    };
    state.validate_limits()?;
    Ok(Some(state))
}

fn load_persisted_rooms(
    conn: &Connection,
    account_id: &str,
    device_id: &str,
) -> Result<Vec<PersistedRoomState>, ClientStoreError> {
    let mut statement = conn.prepare(
        r#"
        SELECT room_id, mls_group_id
        FROM client_rooms
        WHERE account_id = ?1 AND device_id = ?2
        ORDER BY room_id
        "#,
    )?;
    let mut rows = statement.query(params![account_id, device_id])?;
    let mut rooms = Vec::new();
    while let Some(row) = rows.next()? {
        rooms.push(PersistedRoomState {
            room_id: row.get(0)?,
            mls_group_id: row.get(1)?,
        });
    }
    Ok(rooms)
}

fn load_openmls_storage_records(
    conn: &Connection,
    account_id: &str,
    device_id: &str,
) -> Result<Vec<OpenMlsStorageRecord>, ClientStoreError> {
    let mut statement = conn.prepare(
        r#"
        SELECT storage_key, storage_value
        FROM client_openmls_storage
        WHERE account_id = ?1 AND device_id = ?2
        ORDER BY storage_key
        "#,
    )?;
    let mut rows = statement.query(params![account_id, device_id])?;
    let mut records = Vec::new();
    while let Some(row) = rows.next()? {
        records.push(OpenMlsStorageRecord {
            key: row.get(0)?,
            value: row.get(1)?,
        });
    }
    if records.is_empty() {
        return Err(ClientStoreError::CorruptState(format!(
            "device {account_id}/{device_id} has no OpenMLS storage rows"
        )));
    }
    Ok(records)
}
