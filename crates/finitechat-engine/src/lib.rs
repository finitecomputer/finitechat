use finitechat_proto::{
    AccountId, DeviceId, DeviceRef, Epoch, FiniteEnvelope, IdempotencyKey, KeyPackageHash,
    KeyPackageId, KeyPackageRef, KeyPackageState, LeaseToken, LogEntryKind,
    MAX_ACCOUNT_DEVICES_PER_ROOM, MAX_ACCOUNT_ROOM_DISCOVERY_RESULTS,
    MAX_DIRECT_ROOM_DEVICES_PER_ACCOUNT, MAX_IDEMPOTENCY_RECORDS_PER_ROOM_DEVICE,
    MAX_KEY_PACKAGE_PAYLOAD_BYTES, MAX_KEY_PACKAGES_PER_DEVICE, MAX_LINK_SESSION_PAYLOAD_BYTES,
    MAX_OBJECT_ID_BYTES, MAX_STAGED_WELCOMES_PER_COMMIT, MAX_SYNC_PAGE_BYTES,
    MAX_SYNC_PAGE_ENTRIES, MAX_WELCOME_CLAIMS_PER_REQUEST, MembershipDeltaError, MembershipDeltaV1,
    MessageId, MlsGroupId, ProtocolLimitError, RoomId, RoomLogEntry, RoomStatus, Seq,
    StagedWelcomeV1, WelcomeId, WelcomeState, validate_bytes_len, validate_bytes_non_empty,
    validate_idempotency_key, validate_mls_group_id, validate_room_id, validate_string_bytes,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use thiserror::Error;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeliveryService {
    rooms: BTreeMap<RoomId, RoomRecord>,
    direct_rooms: BTreeMap<String, RoomId>,
    #[serde(default)]
    devices: BTreeMap<String, DeviceRecord>,
    key_packages: BTreeMap<KeyPackageId, KeyPackageRecord>,
    welcomes: BTreeMap<WelcomeId, WelcomeRecord>,
    link_sessions: BTreeMap<LinkSessionId, LinkSessionRecord>,
    idempotency: BTreeMap<String, IdempotencyRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomRecord {
    pub room_id: RoomId,
    pub mls_group_id: MlsGroupId,
    pub current_epoch: Epoch,
    pub last_seq: Seq,
    pub status: RoomStatus,
    pub created_by: DeviceRef,
    pub log: Vec<RoomLogEntry>,
    pub membership: BTreeMap<String, DeviceMembership>,
    pub direct_accounts: Option<(AccountId, AccountId)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceMembership {
    pub device: DeviceRef,
    pub intervals: Vec<MembershipInterval>,
}

impl DeviceMembership {
    pub fn key(device: &DeviceRef) -> String {
        device_membership_key(device)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MembershipInterval {
    pub start_seq: Seq,
    pub end_seq: Option<Seq>,
    pub active: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceStatus {
    Active,
    Revoked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceRecord {
    pub device: DeviceRef,
    pub status: DeviceStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyPackageRecord {
    pub key_package_id: KeyPackageId,
    pub owner: DeviceRef,
    pub key_package_ref: KeyPackageRef,
    pub key_package_hash: KeyPackageHash,
    pub key_package_payload: Vec<u8>,
    pub state: KeyPackageState,
    pub lease_token: Option<LeaseToken>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyPackageInventory {
    pub owner: DeviceRef,
    pub available: u32,
    pub leased: u32,
}

impl KeyPackageInventory {
    pub fn unconsumed(&self) -> u64 {
        u64::from(self.available) + u64::from(self.leased)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WelcomeRecord {
    pub welcome_id: WelcomeId,
    pub room_id: RoomId,
    pub commit_seq: Seq,
    pub recipient: DeviceRef,
    pub sender: DeviceRef,
    pub key_package_id: KeyPackageId,
    pub join_epoch: Epoch,
    pub state: WelcomeState,
    pub lease_token: Option<LeaseToken>,
    pub welcome_payload: Vec<u8>,
    pub ratchet_tree_payload: Vec<u8>,
}

pub type LinkSessionId = String;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkSessionState {
    Created,
    PayloadUploaded,
    Claimed,
    Delivered,
    Expired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkSessionRecord {
    pub link_session_id: LinkSessionId,
    pub pairing_public_key: String,
    pub encrypted_payload: Option<Vec<u8>>,
    pub state: LinkSessionState,
    pub claim_token: Option<LeaseToken>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateRoomRequest {
    pub room_id: RoomId,
    pub mls_group_id: MlsGroupId,
    pub creator: DeviceRef,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateDirectRoomRequest {
    pub room_id: RoomId,
    pub mls_group_id: MlsGroupId,
    pub creator: DeviceRef,
    pub other_account_id: AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UploadKeyPackageRequest {
    pub key_package_id: KeyPackageId,
    pub owner: DeviceRef,
    pub key_package_ref: KeyPackageRef,
    pub key_package_hash: KeyPackageHash,
    pub key_package_payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppendEventRequest {
    pub room_id: RoomId,
    pub sender: DeviceRef,
    pub envelope: FiniteEnvelope,
    pub idempotency_key: IdempotencyKey,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubmitCommitRequest {
    pub room_id: RoomId,
    pub sender: DeviceRef,
    pub expected_epoch: Epoch,
    pub envelope: FiniteEnvelope,
    pub membership_delta: MembershipDeltaV1,
    #[serde(default)]
    pub staged_welcomes: Vec<StagedWelcomeV1>,
    pub idempotency_key: IdempotencyKey,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClaimKeyPackageResult {
    pub key_package_id: KeyPackageId,
    pub owner: DeviceRef,
    pub key_package_ref: KeyPackageRef,
    pub key_package_hash: KeyPackageHash,
    pub key_package_payload: Vec<u8>,
    pub lease_token: LeaseToken,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommitAccepted {
    pub seq: Seq,
    pub message_id: MessageId,
    pub released_welcomes: Vec<WelcomeId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventAccepted {
    pub seq: Seq,
    pub message_id: MessageId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncEventsPage {
    pub entries: Vec<RoomLogEntry>,
    pub next_after_seq: Seq,
    pub has_more: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListAccountRoomsRequest {
    pub account_id: AccountId,
    pub after_room_id: Option<RoomId>,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountRoomDevice {
    pub device: DeviceRef,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountRoomRecord {
    pub room_id: RoomId,
    pub mls_group_id: MlsGroupId,
    pub current_epoch: Epoch,
    pub last_seq: Seq,
    pub status: RoomStatus,
    pub devices: Vec<AccountRoomDevice>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListAccountRoomsPage {
    pub rooms: Vec<AccountRoomRecord>,
    pub next_after_room_id: Option<RoomId>,
    pub has_more: bool,
}

impl CreateRoomRequest {
    pub fn validate_limits(&self) -> Result<(), ProtocolLimitError> {
        validate_room_id(&self.room_id)?;
        validate_mls_group_id(&self.mls_group_id)?;
        self.creator.validate_limits()?;
        Ok(())
    }
}

impl CreateDirectRoomRequest {
    pub fn validate_limits(&self) -> Result<(), ProtocolLimitError> {
        validate_room_id(&self.room_id)?;
        validate_mls_group_id(&self.mls_group_id)?;
        self.creator.validate_limits()?;
        validate_string_bytes(
            "other_account_id",
            &self.other_account_id,
            finitechat_proto::MAX_ACCOUNT_ID_BYTES,
        )?;
        Ok(())
    }
}

impl ListAccountRoomsRequest {
    pub fn validate_limits(&self) -> Result<(), ProtocolLimitError> {
        validate_string_bytes(
            "account_id",
            &self.account_id,
            finitechat_proto::MAX_ACCOUNT_ID_BYTES,
        )?;
        if let Some(after_room_id) = &self.after_room_id {
            validate_room_id(after_room_id)?;
        }
        finitechat_proto::validate_item_count(
            "account_room_discovery.limit",
            self.limit as usize,
            MAX_ACCOUNT_ROOM_DISCOVERY_RESULTS,
        )?;
        validate_bytes_non_empty("account_room_discovery.limit", self.limit as usize)?;
        Ok(())
    }
}

impl AccountRoomDevice {
    pub fn validate_limits(&self) -> Result<(), ProtocolLimitError> {
        self.device.validate_limits()?;
        Ok(())
    }
}

impl AccountRoomRecord {
    pub fn validate_limits(&self) -> Result<(), ProtocolLimitError> {
        validate_room_id(&self.room_id)?;
        validate_mls_group_id(&self.mls_group_id)?;
        finitechat_proto::validate_item_count(
            "account_room.devices",
            self.devices.len(),
            MAX_ACCOUNT_DEVICES_PER_ROOM,
        )?;
        validate_bytes_non_empty("account_room.devices", self.devices.len())?;
        for device in &self.devices {
            device.validate_limits()?;
        }
        Ok(())
    }
}

impl ListAccountRoomsPage {
    pub fn validate_limits(&self) -> Result<(), ProtocolLimitError> {
        finitechat_proto::validate_item_count(
            "account_room_discovery.rooms",
            self.rooms.len(),
            MAX_ACCOUNT_ROOM_DISCOVERY_RESULTS,
        )?;
        for room in &self.rooms {
            room.validate_limits()?;
        }
        if let Some(next_after_room_id) = &self.next_after_room_id {
            validate_room_id(next_after_room_id)?;
        }
        Ok(())
    }
}

impl UploadKeyPackageRequest {
    pub fn validate_limits(&self) -> Result<(), ProtocolLimitError> {
        validate_string_bytes("key_package_id", &self.key_package_id, MAX_OBJECT_ID_BYTES)?;
        self.owner.validate_limits()?;
        validate_string_bytes(
            "key_package_ref",
            &self.key_package_ref,
            MAX_OBJECT_ID_BYTES,
        )?;
        validate_string_bytes(
            "key_package_hash",
            &self.key_package_hash,
            MAX_OBJECT_ID_BYTES,
        )?;
        validate_bytes_non_empty("key_package_payload", self.key_package_payload.len())?;
        validate_bytes_len(
            "key_package_payload",
            self.key_package_payload.len(),
            MAX_KEY_PACKAGE_PAYLOAD_BYTES,
        )?;
        Ok(())
    }
}

impl AppendEventRequest {
    pub fn validate_limits(&self) -> Result<(), ProtocolLimitError> {
        validate_room_id(&self.room_id)?;
        self.sender.validate_limits()?;
        self.envelope.validate_limits()?;
        validate_idempotency_key(&self.idempotency_key)?;
        Ok(())
    }
}

impl SubmitCommitRequest {
    pub fn validate_limits(&self) -> Result<(), ProtocolLimitError> {
        validate_room_id(&self.room_id)?;
        self.sender.validate_limits()?;
        self.envelope.validate_limits()?;
        self.membership_delta.validate_limits()?;
        finitechat_proto::validate_item_count(
            "staged_welcomes",
            self.staged_welcomes.len(),
            MAX_STAGED_WELCOMES_PER_COMMIT,
        )?;
        for staged_welcome in &self.staged_welcomes {
            staged_welcome.validate_limits()?;
        }
        validate_idempotency_key(&self.idempotency_key)?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
enum IdempotencyResponse {
    Event(Result<EventAccepted, EngineError>),
    Commit(Result<CommitAccepted, EngineError>),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct IdempotencyRecord {
    room_id: RoomId,
    sender: DeviceRef,
    operation: String,
    request_hash: String,
    response: IdempotencyResponse,
}

impl DeliveryService {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn room(&self, room_id: &str) -> Option<&RoomRecord> {
        self.rooms.get(room_id)
    }

    pub fn key_package(&self, key_package_id: &str) -> Option<&KeyPackageRecord> {
        self.key_packages.get(key_package_id)
    }

    pub fn key_package_inventory(
        &self,
        owner: &DeviceRef,
    ) -> Result<KeyPackageInventory, EngineError> {
        owner.validate_limits()?;
        let inventory = key_package_inventory_for(owner, self.key_packages.values())?;
        debug_assert_eq!(inventory.owner, *owner);
        Ok(inventory)
    }

    pub fn device(&self, device: &DeviceRef) -> Option<&DeviceRecord> {
        self.devices.get(&device_registry_key(device))
    }

    pub fn welcome(&self, welcome_id: &str) -> Option<&WelcomeRecord> {
        self.welcomes.get(welcome_id)
    }

    pub fn link_session(&self, link_session_id: &str) -> Option<&LinkSessionRecord> {
        self.link_sessions.get(link_session_id)
    }

    pub fn create_room(&mut self, request: CreateRoomRequest) -> Result<(), EngineError> {
        request.validate_limits()?;
        self.observe_active_device(&request.creator)?;
        if self.rooms.contains_key(&request.room_id) {
            return Err(EngineError::RoomAlreadyExists(request.room_id));
        }

        let mut membership = BTreeMap::new();
        membership.insert(
            device_membership_key(&request.creator),
            DeviceMembership {
                device: request.creator.clone(),
                intervals: vec![MembershipInterval {
                    start_seq: 0,
                    end_seq: None,
                    active: true,
                }],
            },
        );

        let room = RoomRecord {
            room_id: request.room_id.clone(),
            mls_group_id: request.mls_group_id,
            current_epoch: 0,
            last_seq: 0,
            status: RoomStatus::Open,
            created_by: request.creator,
            log: Vec::new(),
            membership,
            direct_accounts: None,
        };
        self.rooms.insert(request.room_id, room);
        Ok(())
    }

    pub fn create_or_get_direct_room(
        &mut self,
        request: CreateDirectRoomRequest,
    ) -> Result<RoomId, EngineError> {
        request.validate_limits()?;
        self.observe_active_device(&request.creator)?;
        let key = direct_room_key(&request.creator.account_id, &request.other_account_id);
        let direct_key = direct_room_key_string(&key);
        if let Some(room_id) = self.direct_rooms.get(&direct_key) {
            return Ok(room_id.clone());
        }
        if self.rooms.contains_key(&request.room_id) {
            return Err(EngineError::RoomAlreadyExists(request.room_id));
        }

        let mut membership = BTreeMap::new();
        membership.insert(
            device_membership_key(&request.creator),
            DeviceMembership {
                device: request.creator.clone(),
                intervals: vec![MembershipInterval {
                    start_seq: 0,
                    end_seq: None,
                    active: true,
                }],
            },
        );

        let room = RoomRecord {
            room_id: request.room_id.clone(),
            mls_group_id: request.mls_group_id,
            current_epoch: 0,
            last_seq: 0,
            status: RoomStatus::Open,
            created_by: request.creator,
            log: Vec::new(),
            membership,
            direct_accounts: Some(key.clone()),
        };
        self.direct_rooms
            .insert(direct_key, request.room_id.clone());
        self.rooms.insert(request.room_id.clone(), room);
        Ok(request.room_id)
    }

    pub fn register_device(&mut self, device: DeviceRef) -> Result<(), EngineError> {
        device.validate_limits()?;
        let key = device_registry_key(&device);
        if let Some(record) = self.devices.get(&key) {
            debug_assert_eq!(record.device, device);
            if record.status == DeviceStatus::Revoked {
                return Err(EngineError::DeviceRevoked(device));
            }
            return Ok(());
        }
        self.devices.insert(
            key,
            DeviceRecord {
                device,
                status: DeviceStatus::Active,
            },
        );
        Ok(())
    }

    pub fn revoke_device(&mut self, device: DeviceRef) -> Result<(), EngineError> {
        device.validate_limits()?;
        let key = device_registry_key(&device);
        self.devices.insert(
            key,
            DeviceRecord {
                device,
                status: DeviceStatus::Revoked,
            },
        );
        Ok(())
    }

    pub fn upload_key_package(
        &mut self,
        request: UploadKeyPackageRequest,
    ) -> Result<(), EngineError> {
        request.validate_limits()?;
        self.observe_active_device(&request.owner)?;
        if let Some(existing) = self.key_packages.get(&request.key_package_id) {
            if key_package_request_matches_record(&request, existing) {
                return Ok(());
            }
            return Err(EngineError::KeyPackageAlreadyExists(request.key_package_id));
        }
        let inventory = self.key_package_inventory(&request.owner)?;
        if inventory.unconsumed() >= u64::from(MAX_KEY_PACKAGES_PER_DEVICE) {
            return Err(EngineError::KeyPackageInventoryFull {
                owner: request.owner,
                available: inventory.available,
                leased: inventory.leased,
                max: MAX_KEY_PACKAGES_PER_DEVICE,
            });
        }
        self.key_packages.insert(
            request.key_package_id.clone(),
            KeyPackageRecord {
                key_package_id: request.key_package_id,
                owner: request.owner,
                key_package_ref: request.key_package_ref,
                key_package_hash: request.key_package_hash,
                key_package_payload: request.key_package_payload,
                state: KeyPackageState::Available,
                lease_token: None,
            },
        );
        Ok(())
    }

    pub fn claim_key_package(
        &mut self,
        key_package_id: &str,
    ) -> Result<ClaimKeyPackageResult, EngineError> {
        validate_string_bytes("key_package_id", key_package_id, MAX_OBJECT_ID_BYTES)?;
        let package = self
            .key_packages
            .get_mut(key_package_id)
            .ok_or_else(|| EngineError::KeyPackageNotFound(key_package_id.to_string()))?;
        if package.state != KeyPackageState::Available {
            return Err(EngineError::KeyPackageUnavailable {
                key_package_id: key_package_id.to_string(),
                state: package.state,
            });
        }
        ensure_device_not_revoked(&self.devices, &package.owner)?;
        validate_key_package_payload(&package.key_package_payload)?;
        let lease_token = lease_token_for(key_package_id, &package.owner);
        package.state = KeyPackageState::Leased;
        package.lease_token = Some(lease_token.clone());
        Ok(claimed_key_package_result(package, lease_token))
    }

    pub fn claim_key_packages_for_account(
        &mut self,
        account_id: &str,
    ) -> Result<Vec<ClaimKeyPackageResult>, EngineError> {
        validate_string_bytes(
            "account_id",
            account_id,
            finitechat_proto::MAX_ACCOUNT_ID_BYTES,
        )?;

        let mut available_packages = self
            .key_packages
            .iter()
            .filter(|(_, package)| {
                package.owner.account_id == account_id
                    && package.state == KeyPackageState::Available
                    && !device_is_revoked(&self.devices, &package.owner)
            })
            .collect::<Vec<_>>();
        available_packages.sort_by(|(left_id, left), (right_id, right)| {
            left.owner
                .device_id
                .cmp(&right.owner.device_id)
                .then_with(|| left_id.cmp(right_id))
        });

        let mut key_package_ids = Vec::new();
        let mut seen_devices = BTreeSet::<DeviceId>::new();
        for (key_package_id, package) in available_packages {
            if key_package_ids.len() >= MAX_DIRECT_ROOM_DEVICES_PER_ACCOUNT as usize {
                break;
            }
            if !seen_devices.insert(package.owner.device_id.clone()) {
                continue;
            }
            validate_key_package_payload(&package.key_package_payload)?;
            key_package_ids.push(key_package_id.clone());
        }

        let mut claimed = Vec::with_capacity(key_package_ids.len());
        for key_package_id in key_package_ids {
            let package = self
                .key_packages
                .get_mut(&key_package_id)
                .expect("available KeyPackage was selected before mutation");
            let lease_token = lease_token_for(&package.key_package_id, &package.owner);
            package.state = KeyPackageState::Leased;
            package.lease_token = Some(lease_token.clone());
            claimed.push(claimed_key_package_result(package, lease_token));
        }
        debug_assert!(claimed.len() <= MAX_DIRECT_ROOM_DEVICES_PER_ACCOUNT as usize);
        Ok(claimed)
    }

    pub fn release_key_package_lease(&mut self, key_package_id: &str) -> Result<(), EngineError> {
        validate_string_bytes("key_package_id", key_package_id, MAX_OBJECT_ID_BYTES)?;
        let package = self
            .key_packages
            .get_mut(key_package_id)
            .ok_or_else(|| EngineError::KeyPackageNotFound(key_package_id.to_string()))?;
        if package.state != KeyPackageState::Leased {
            return Err(EngineError::KeyPackageUnavailable {
                key_package_id: key_package_id.to_string(),
                state: package.state,
            });
        }
        package.state = KeyPackageState::Available;
        package.lease_token = None;
        Ok(())
    }

    pub fn expire_key_package_lease(&mut self, key_package_id: &str) -> Result<(), EngineError> {
        self.release_key_package_lease(key_package_id)
    }

    pub fn append_event(
        &mut self,
        request: AppendEventRequest,
    ) -> Result<EventAccepted, EngineError> {
        request.validate_limits()?;
        if request.envelope.kind == LogEntryKind::Commit {
            return Err(EngineError::WrongEnvelopeKind {
                expected: LogEntryKind::Application,
                actual: request.envelope.kind,
            });
        }

        let request_hash = request_hash(&request)?;
        let scope = idempotency_scope_key(
            &request.room_id,
            &request.sender,
            "append_event",
            &request.idempotency_key,
        );
        if let Some(record) = self.idempotency.get(&scope) {
            if record.request_hash != request_hash {
                return Err(EngineError::ConflictingIdempotencyKey);
            }
            return match &record.response {
                IdempotencyResponse::Event(result) => result.clone(),
                IdempotencyResponse::Commit(_) => Err(EngineError::ConflictingIdempotencyKey),
            };
        }

        self.ensure_idempotency_capacity(&request.room_id, &request.sender)?;
        let room_id = request.room_id.clone();
        let sender = request.sender.clone();
        let result = self.append_event_inner(request);
        self.idempotency.insert(
            scope,
            IdempotencyRecord {
                room_id,
                sender,
                operation: "append_event".to_string(),
                request_hash,
                response: IdempotencyResponse::Event(result.clone()),
            },
        );
        result
    }

    pub fn submit_commit(
        &mut self,
        request: SubmitCommitRequest,
    ) -> Result<CommitAccepted, EngineError> {
        request.validate_limits()?;
        let request_hash = request_hash(&request)?;
        let scope = idempotency_scope_key(
            &request.room_id,
            &request.sender,
            "submit_commit",
            &request.idempotency_key,
        );
        if let Some(record) = self.idempotency.get(&scope) {
            if record.request_hash != request_hash {
                return Err(EngineError::ConflictingIdempotencyKey);
            }
            return match &record.response {
                IdempotencyResponse::Commit(result) => result.clone(),
                IdempotencyResponse::Event(_) => Err(EngineError::ConflictingIdempotencyKey),
            };
        }

        self.ensure_idempotency_capacity(&request.room_id, &request.sender)?;
        let room_id = request.room_id.clone();
        let sender = request.sender.clone();
        let result = self.submit_commit_inner(request);
        self.idempotency.insert(
            scope,
            IdempotencyRecord {
                room_id,
                sender,
                operation: "submit_commit".to_string(),
                request_hash,
                response: IdempotencyResponse::Commit(result.clone()),
            },
        );
        result
    }

    fn append_event_inner(
        &mut self,
        request: AppendEventRequest,
    ) -> Result<EventAccepted, EngineError> {
        ensure_device_not_revoked(&self.devices, &request.sender)?;
        let room = self
            .rooms
            .get_mut(&request.room_id)
            .ok_or_else(|| EngineError::RoomNotFound(request.room_id.clone()))?;
        validate_room_open(room)?;
        validate_envelope(room, &request.envelope, request.envelope.kind)?;
        if request.envelope.epoch != room.current_epoch {
            return Err(EngineError::WrongEpoch {
                expected: room.current_epoch,
                actual: request.envelope.epoch,
            });
        }
        if request.envelope.sender != request.sender {
            return Err(EngineError::EnvelopeSenderMismatch);
        }
        if !room.device_active_at_head(&request.sender) {
            return Err(EngineError::SenderNotActive(request.sender));
        }

        let message_id = request.envelope.message_id()?;
        if room.log.iter().any(|entry| entry.message_id == message_id) {
            return Err(EngineError::DuplicateMessageId(message_id));
        }
        let seq = room.last_seq + 1;
        room.last_seq = seq;
        room.log.push(RoomLogEntry {
            room_id: request.room_id.clone(),
            seq,
            message_id: message_id.clone(),
            sender: request.sender.clone(),
            kind: request.envelope.kind,
            epoch: request.envelope.epoch,
            envelope: request.envelope,
            idempotency_key: request.idempotency_key,
        });

        Ok(EventAccepted { seq, message_id })
    }

    fn submit_commit_inner(
        &mut self,
        request: SubmitCommitRequest,
    ) -> Result<CommitAccepted, EngineError> {
        ensure_device_not_revoked(&self.devices, &request.sender)?;
        let actual_commit_message_id = request.envelope.message_id()?;
        request
            .membership_delta
            .validate_structure(request.expected_epoch, &actual_commit_message_id)?;
        let staged_welcomes =
            staged_welcomes_by_id(&request.membership_delta, &request.staged_welcomes)?;

        let room = self
            .rooms
            .get(&request.room_id)
            .ok_or_else(|| EngineError::RoomNotFound(request.room_id.clone()))?;
        validate_room_open(room)?;
        validate_envelope(room, &request.envelope, LogEntryKind::Commit)?;
        if request.expected_epoch != room.current_epoch {
            return Err(EngineError::WrongEpoch {
                expected: room.current_epoch,
                actual: request.expected_epoch,
            });
        }
        if request.envelope.epoch != request.expected_epoch {
            return Err(EngineError::WrongEpoch {
                expected: request.expected_epoch,
                actual: request.envelope.epoch,
            });
        }
        if request.envelope.sender != request.sender {
            return Err(EngineError::EnvelopeSenderMismatch);
        }
        if !room.device_active_at_head(&request.sender) {
            return Err(EngineError::SenderNotActive(request.sender));
        }
        if room
            .log
            .iter()
            .any(|entry| entry.message_id == actual_commit_message_id)
        {
            return Err(EngineError::DuplicateMessageId(actual_commit_message_id));
        }

        self.validate_commit_key_packages(room, &request.membership_delta)?;
        self.validate_commit_welcomes(&request.membership_delta)?;

        let room = self
            .rooms
            .get_mut(&request.room_id)
            .ok_or_else(|| EngineError::RoomNotFound(request.room_id.clone()))?;
        let seq = room.last_seq + 1;
        room.last_seq = seq;
        room.current_epoch += 1;
        room.log.push(RoomLogEntry {
            room_id: request.room_id.clone(),
            seq,
            message_id: actual_commit_message_id.clone(),
            sender: request.sender.clone(),
            kind: LogEntryKind::Commit,
            epoch: request.expected_epoch,
            envelope: request.envelope.clone(),
            idempotency_key: request.idempotency_key.clone(),
        });

        for remove in &request.membership_delta.removes {
            room.close_active_interval(&remove.device, seq);
        }

        for add in &request.membership_delta.adds {
            room.membership
                .entry(device_membership_key(&add.device))
                .or_insert_with(|| DeviceMembership {
                    device: add.device.clone(),
                    intervals: Vec::new(),
                })
                .intervals
                .push(MembershipInterval {
                    start_seq: seq,
                    end_seq: None,
                    active: false,
                });
        }

        let mut released_welcomes = Vec::new();
        for add in &request.membership_delta.adds {
            let package = self
                .key_packages
                .get_mut(&add.key_package_id)
                .ok_or_else(|| EngineError::KeyPackageNotFound(add.key_package_id.clone()))?;
            package.state = KeyPackageState::Consumed;
            package.lease_token = None;
            let staged_welcome = staged_welcomes
                .get(&add.welcome_id)
                .expect("staged welcome was validated");

            self.welcomes.insert(
                add.welcome_id.clone(),
                WelcomeRecord {
                    welcome_id: add.welcome_id.clone(),
                    room_id: request.room_id.clone(),
                    commit_seq: seq,
                    recipient: add.device.clone(),
                    sender: request.sender.clone(),
                    key_package_id: add.key_package_id.clone(),
                    join_epoch: room.current_epoch,
                    state: WelcomeState::Released,
                    lease_token: Some(lease_token_for(&add.welcome_id, &add.device)),
                    welcome_payload: staged_welcome.welcome_payload.clone(),
                    ratchet_tree_payload: staged_welcome.ratchet_tree_payload.clone(),
                },
            );
            released_welcomes.push(add.welcome_id.clone());
        }

        Ok(CommitAccepted {
            seq,
            message_id: actual_commit_message_id,
            released_welcomes,
        })
    }

    pub fn claim_welcomes(
        &mut self,
        device: &DeviceRef,
    ) -> Result<Vec<WelcomeRecord>, EngineError> {
        device.validate_limits()?;
        ensure_device_not_revoked(&self.devices, device)?;
        let mut claimed = Vec::new();
        for welcome in self.welcomes.values_mut() {
            if claimed.len() >= MAX_WELCOME_CLAIMS_PER_REQUEST as usize {
                break;
            }
            if &welcome.recipient == device && welcome.state == WelcomeState::Released {
                welcome.state = WelcomeState::Claimed;
                claimed.push(welcome.clone());
            }
        }
        Ok(claimed)
    }

    pub fn ack_welcome(&mut self, welcome_id: &str, activated: bool) -> Result<(), EngineError> {
        if activated {
            let recipient = self
                .welcomes
                .get(welcome_id)
                .ok_or_else(|| EngineError::WelcomeNotFound(welcome_id.to_string()))?
                .recipient
                .clone();
            ensure_device_not_revoked(&self.devices, &recipient)?;
        }
        let welcome = self
            .welcomes
            .get_mut(welcome_id)
            .ok_or_else(|| EngineError::WelcomeNotFound(welcome_id.to_string()))?;
        match (welcome.state, activated) {
            (WelcomeState::Acked, true) | (WelcomeState::Failed, false) => return Ok(()),
            (WelcomeState::Claimed, _) => {}
            _ => return Err(EngineError::WelcomeNotClaimed(welcome_id.to_string())),
        }
        welcome.state = if activated {
            WelcomeState::Acked
        } else {
            WelcomeState::Failed
        };

        if activated {
            let room = self
                .rooms
                .get_mut(&welcome.room_id)
                .ok_or_else(|| EngineError::RoomNotFound(welcome.room_id.clone()))?;
            room.activate_interval(&welcome.recipient, welcome.commit_seq);
        }
        Ok(())
    }

    pub fn release_welcome_claim(&mut self, welcome_id: &str) -> Result<(), EngineError> {
        let welcome = self
            .welcomes
            .get_mut(welcome_id)
            .ok_or_else(|| EngineError::WelcomeNotFound(welcome_id.to_string()))?;
        if welcome.state != WelcomeState::Claimed {
            return Err(EngineError::WelcomeNotClaimed(welcome_id.to_string()));
        }
        welcome.state = WelcomeState::Released;
        Ok(())
    }

    pub fn report_invalid_commit(
        &mut self,
        room_id: &str,
        reporter: &DeviceRef,
        offending_seq: Seq,
    ) -> Result<(), EngineError> {
        let room = self
            .rooms
            .get_mut(room_id)
            .ok_or_else(|| EngineError::RoomNotFound(room_id.to_string()))?;
        if !room.device_was_member_for_seq(reporter, offending_seq) {
            return Err(EngineError::ReporterNotInInterval(reporter.clone()));
        }
        room.status = RoomStatus::NeedsRepair;
        Ok(())
    }

    pub fn sync_events(
        &self,
        room_id: &str,
        requester: &DeviceRef,
        after_seq: Seq,
    ) -> Result<SyncEventsPage, EngineError> {
        validate_room_id(room_id)?;
        requester.validate_limits()?;
        let room = self
            .rooms
            .get(room_id)
            .ok_or_else(|| EngineError::RoomNotFound(room_id.to_string()))?;
        Ok(sync_events_page_for_room(room, requester, after_seq))
    }

    pub fn list_account_rooms(
        &self,
        request: ListAccountRoomsRequest,
    ) -> Result<ListAccountRoomsPage, EngineError> {
        request.validate_limits()?;
        let mut rooms = Vec::new();
        let limit = request.limit as usize;
        for room in self.rooms.values() {
            if let Some(after_room_id) = &request.after_room_id
                && room.room_id <= *after_room_id
            {
                continue;
            }
            let devices = room.current_devices_for_account(&request.account_id);
            if devices.is_empty() {
                continue;
            }
            if rooms.len() == limit {
                let next_after_room_id = rooms
                    .last()
                    .map(|room: &AccountRoomRecord| room.room_id.clone());
                let page = ListAccountRoomsPage {
                    rooms,
                    next_after_room_id,
                    has_more: true,
                };
                page.validate_limits()?;
                return Ok(page);
            }
            rooms.push(AccountRoomRecord {
                room_id: room.room_id.clone(),
                mls_group_id: room.mls_group_id.clone(),
                current_epoch: room.current_epoch,
                last_seq: room.last_seq,
                status: room.status,
                devices,
            });
        }
        let next_after_room_id = rooms.last().map(|room| room.room_id.clone());
        let page = ListAccountRoomsPage {
            rooms,
            next_after_room_id,
            has_more: false,
        };
        page.validate_limits()?;
        Ok(page)
    }

    pub fn create_link_session(
        &mut self,
        link_session_id: impl Into<LinkSessionId>,
        pairing_public_key: impl Into<String>,
    ) -> Result<(), EngineError> {
        let link_session_id = link_session_id.into();
        let pairing_public_key = pairing_public_key.into();
        validate_string_bytes("link_session_id", &link_session_id, MAX_OBJECT_ID_BYTES)?;
        validate_string_bytes(
            "pairing_public_key",
            &pairing_public_key,
            MAX_OBJECT_ID_BYTES,
        )?;
        if self.link_sessions.contains_key(&link_session_id) {
            return Err(EngineError::LinkSessionAlreadyExists(link_session_id));
        }
        self.link_sessions.insert(
            link_session_id.clone(),
            LinkSessionRecord {
                link_session_id,
                pairing_public_key,
                encrypted_payload: None,
                state: LinkSessionState::Created,
                claim_token: None,
            },
        );
        Ok(())
    }

    pub fn upload_link_payload(
        &mut self,
        link_session_id: &str,
        encrypted_payload: Vec<u8>,
    ) -> Result<(), EngineError> {
        validate_string_bytes("link_session_id", link_session_id, MAX_OBJECT_ID_BYTES)?;
        validate_bytes_len(
            "link_session.encrypted_payload",
            encrypted_payload.len(),
            MAX_LINK_SESSION_PAYLOAD_BYTES,
        )?;
        let session = self
            .link_sessions
            .get_mut(link_session_id)
            .ok_or_else(|| EngineError::LinkSessionNotFound(link_session_id.to_string()))?;
        match session.state {
            LinkSessionState::Created => {
                session.encrypted_payload = Some(encrypted_payload);
                session.state = LinkSessionState::PayloadUploaded;
                Ok(())
            }
            LinkSessionState::PayloadUploaded
                if session.encrypted_payload.as_deref() == Some(encrypted_payload.as_slice()) =>
            {
                Ok(())
            }
            LinkSessionState::PayloadUploaded => Err(EngineError::LinkSessionConflict),
            LinkSessionState::Claimed | LinkSessionState::Delivered | LinkSessionState::Expired => {
                Err(EngineError::LinkSessionClosed)
            }
        }
    }

    pub fn claim_link_payload(
        &mut self,
        link_session_id: &str,
    ) -> Result<(Vec<u8>, LeaseToken), EngineError> {
        let session = self
            .link_sessions
            .get_mut(link_session_id)
            .ok_or_else(|| EngineError::LinkSessionNotFound(link_session_id.to_string()))?;
        if session.state != LinkSessionState::PayloadUploaded {
            return Err(EngineError::LinkSessionNotReady);
        }
        let payload = session
            .encrypted_payload
            .clone()
            .ok_or(EngineError::LinkSessionNotReady)?;
        let token = lease_token_for(
            &session.link_session_id,
            &DeviceRef {
                account_id: "link".to_string(),
                device_id: session.pairing_public_key.clone(),
            },
        );
        session.state = LinkSessionState::Claimed;
        session.claim_token = Some(token.clone());
        Ok((payload, token))
    }

    pub fn ack_link_payload(
        &mut self,
        link_session_id: &str,
        claim_token: &str,
    ) -> Result<(), EngineError> {
        let session = self
            .link_sessions
            .get_mut(link_session_id)
            .ok_or_else(|| EngineError::LinkSessionNotFound(link_session_id.to_string()))?;
        if session.state != LinkSessionState::Claimed {
            return Err(EngineError::LinkSessionNotReady);
        }
        if session.claim_token.as_deref() != Some(claim_token) {
            return Err(EngineError::BadLinkSessionClaimToken);
        }
        session.state = LinkSessionState::Delivered;
        Ok(())
    }

    pub fn release_link_claim(&mut self, link_session_id: &str) -> Result<(), EngineError> {
        let session = self
            .link_sessions
            .get_mut(link_session_id)
            .ok_or_else(|| EngineError::LinkSessionNotFound(link_session_id.to_string()))?;
        if session.state != LinkSessionState::Claimed {
            return Err(EngineError::LinkSessionNotReady);
        }
        session.state = LinkSessionState::PayloadUploaded;
        session.claim_token = None;
        Ok(())
    }

    pub fn expire_link_session(&mut self, link_session_id: &str) -> Result<(), EngineError> {
        let session = self
            .link_sessions
            .get_mut(link_session_id)
            .ok_or_else(|| EngineError::LinkSessionNotFound(link_session_id.to_string()))?;
        if session.state == LinkSessionState::Delivered {
            return Err(EngineError::LinkSessionClosed);
        }
        session.state = LinkSessionState::Expired;
        Ok(())
    }

    fn validate_commit_key_packages(
        &self,
        room: &RoomRecord,
        delta: &MembershipDeltaV1,
    ) -> Result<(), EngineError> {
        let mut seen_packages = BTreeSet::new();
        let mut added_devices_by_account = BTreeMap::<AccountId, usize>::new();
        for add in &delta.adds {
            ensure_device_not_revoked(&self.devices, &add.device)?;
            if let Some((left, right)) = &room.direct_accounts
                && add.device.account_id != *left
                && add.device.account_id != *right
            {
                return Err(EngineError::DirectRoomThirdAccount(
                    add.device.account_id.clone(),
                ));
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
            )?;
            if room.direct_accounts.is_some() {
                finitechat_proto::validate_item_count(
                    "direct_room.devices_per_account",
                    current_devices + *added_devices,
                    MAX_DIRECT_ROOM_DEVICES_PER_ACCOUNT,
                )?;
            }
            if room.device_current_or_pending_at_head(&add.device) {
                return Err(EngineError::DeviceAlreadyInRoom(add.device.clone()));
            }
            if !seen_packages.insert(add.key_package_id.clone()) {
                return Err(EngineError::DuplicateKeyPackage(add.key_package_id.clone()));
            }
            let package = self
                .key_packages
                .get(&add.key_package_id)
                .ok_or_else(|| EngineError::KeyPackageNotFound(add.key_package_id.clone()))?;
            if package.state != KeyPackageState::Leased {
                return Err(EngineError::KeyPackageUnavailable {
                    key_package_id: add.key_package_id.clone(),
                    state: package.state,
                });
            }
            if package.owner != add.device {
                return Err(EngineError::KeyPackageOwnerMismatch(
                    add.key_package_id.clone(),
                ));
            }
            if package.key_package_ref != add.key_package_ref
                || package.key_package_hash != add.key_package_hash
            {
                return Err(EngineError::KeyPackageRefMismatch(
                    add.key_package_id.clone(),
                ));
            }
        }
        Ok(())
    }

    fn validate_commit_welcomes(&self, delta: &MembershipDeltaV1) -> Result<(), EngineError> {
        for add in &delta.adds {
            if self.welcomes.contains_key(&add.welcome_id) {
                return Err(EngineError::WelcomeAlreadyExists(add.welcome_id.clone()));
            }
        }
        Ok(())
    }

    fn ensure_idempotency_capacity(
        &self,
        room_id: &str,
        sender: &DeviceRef,
    ) -> Result<(), EngineError> {
        let records = self
            .idempotency
            .values()
            .filter(|record| record.room_id == room_id)
            .filter(|record| record.sender == *sender)
            .count();
        if records < MAX_IDEMPOTENCY_RECORDS_PER_ROOM_DEVICE as usize {
            Ok(())
        } else {
            Err(EngineError::IdempotencyCapacityExceeded {
                room_id: room_id.to_string(),
                sender: sender.clone(),
                max_records: MAX_IDEMPOTENCY_RECORDS_PER_ROOM_DEVICE,
            })
        }
    }

    fn observe_active_device(&mut self, device: &DeviceRef) -> Result<(), EngineError> {
        ensure_device_not_revoked(&self.devices, device)?;
        let key = device_registry_key(device);
        self.devices.entry(key).or_insert_with(|| DeviceRecord {
            device: device.clone(),
            status: DeviceStatus::Active,
        });
        Ok(())
    }
}

impl RoomRecord {
    pub fn device_active_at_head(&self, device: &DeviceRef) -> bool {
        self.membership
            .get(&device_membership_key(device))
            .map(|membership| {
                membership.intervals.iter().any(|interval| {
                    interval.active
                        && interval.start_seq <= self.last_seq
                        && interval.end_seq.is_none()
                })
            })
            .unwrap_or(false)
    }

    pub fn device_was_member_for_seq(&self, device: &DeviceRef, seq: Seq) -> bool {
        self.membership
            .get(&device_membership_key(device))
            .map(|membership| {
                membership.intervals.iter().any(|interval| {
                    interval.start_seq <= seq && interval.end_seq.is_none_or(|end| seq <= end)
                })
            })
            .unwrap_or(false)
    }

    pub fn current_or_pending_device_count_for_account(&self, account_id: &str) -> usize {
        self.membership
            .values()
            .filter(|membership| membership.device.account_id == account_id)
            .filter(|membership| {
                membership
                    .intervals
                    .iter()
                    .any(|interval| interval.end_seq.is_none())
            })
            .count()
    }

    pub fn device_current_or_pending_at_head(&self, device: &DeviceRef) -> bool {
        self.membership
            .get(&device_membership_key(device))
            .map(|membership| {
                membership
                    .intervals
                    .iter()
                    .any(|interval| interval.end_seq.is_none())
            })
            .unwrap_or(false)
    }

    pub fn current_devices_for_account(&self, account_id: &str) -> Vec<AccountRoomDevice> {
        let mut devices = self
            .membership
            .values()
            .filter(|membership| membership.device.account_id == account_id)
            .filter_map(|membership| {
                let mut is_current = false;
                let mut is_active = false;
                for interval in &membership.intervals {
                    if interval.end_seq.is_none() {
                        is_current = true;
                        is_active = is_active || interval.active;
                    }
                }
                if is_current {
                    Some(AccountRoomDevice {
                        device: membership.device.clone(),
                        active: is_active,
                    })
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        devices.sort_by(|left, right| left.device.device_id.cmp(&right.device.device_id));
        devices
    }

    fn close_active_interval(&mut self, device: &DeviceRef, seq: Seq) {
        if let Some(membership) = self.membership.get_mut(&device_membership_key(device))
            && let Some(interval) = membership
                .intervals
                .iter_mut()
                .rev()
                .find(|interval| interval.active && interval.end_seq.is_none())
        {
            interval.end_seq = Some(seq);
        }
    }

    fn activate_interval(&mut self, device: &DeviceRef, start_seq: Seq) {
        if let Some(membership) = self.membership.get_mut(&device_membership_key(device))
            && let Some(interval) = membership
                .intervals
                .iter_mut()
                .find(|interval| interval.start_seq == start_seq && !interval.active)
        {
            interval.active = true;
        }
    }
}

#[derive(Debug, Clone, Error, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum EngineError {
    #[error("room already exists: {0}")]
    RoomAlreadyExists(RoomId),
    #[error("room not found: {0}")]
    RoomNotFound(RoomId),
    #[error("room is not open")]
    RoomNotOpen,
    #[error("key package already exists: {0}")]
    KeyPackageAlreadyExists(KeyPackageId),
    #[error("key package not found: {0}")]
    KeyPackageNotFound(KeyPackageId),
    #[error("key package {key_package_id} is {state:?}")]
    KeyPackageUnavailable {
        key_package_id: KeyPackageId,
        state: KeyPackageState,
    },
    #[error(
        "key package inventory is full for {owner:?}: {available} available and {leased} leased, max {max}"
    )]
    KeyPackageInventoryFull {
        owner: DeviceRef,
        available: u32,
        leased: u32,
        max: u32,
    },
    #[error("key package owner mismatch: {0}")]
    KeyPackageOwnerMismatch(KeyPackageId),
    #[error("key package ref or hash mismatch: {0}")]
    KeyPackageRefMismatch(KeyPackageId),
    #[error("duplicate key package in commit: {0}")]
    DuplicateKeyPackage(KeyPackageId),
    #[error("device is already current or pending in room: {0:?}")]
    DeviceAlreadyInRoom(DeviceRef),
    #[error("device is revoked: {0:?}")]
    DeviceRevoked(DeviceRef),
    #[error("duplicate message id in room log: {0}")]
    DuplicateMessageId(MessageId),
    #[error("welcome not found: {0}")]
    WelcomeNotFound(WelcomeId),
    #[error("welcome already exists: {0}")]
    WelcomeAlreadyExists(WelcomeId),
    #[error("welcome is not claimed: {0}")]
    WelcomeNotClaimed(WelcomeId),
    #[error("duplicate welcome id in commit: {0}")]
    DuplicateWelcomeId(WelcomeId),
    #[error("commit add is missing staged Welcome bytes: {0}")]
    MissingStagedWelcome(WelcomeId),
    #[error("staged Welcome does not match any commit add: {0}")]
    UnexpectedStagedWelcome(WelcomeId),
    #[error("wrong epoch: expected {expected}, actual {actual}")]
    WrongEpoch { expected: Epoch, actual: Epoch },
    #[error("wrong envelope kind: expected {expected:?}, actual {actual:?}")]
    WrongEnvelopeKind {
        expected: LogEntryKind,
        actual: LogEntryKind,
    },
    #[error("envelope room does not match request")]
    EnvelopeRoomMismatch,
    #[error("envelope MLS group does not match room")]
    EnvelopeGroupMismatch,
    #[error("envelope sender does not match request")]
    EnvelopeSenderMismatch,
    #[error("sender is not active: {0:?}")]
    SenderNotActive(DeviceRef),
    #[error("reporter was not a member for offending seq: {0:?}")]
    ReporterNotInInterval(DeviceRef),
    #[error("conflicting idempotency key")]
    ConflictingIdempotencyKey,
    #[error(
        "idempotency capacity exceeded for room {room_id} and sender {sender:?}: max {max_records}"
    )]
    IdempotencyCapacityExceeded {
        room_id: RoomId,
        sender: DeviceRef,
        max_records: u32,
    },
    #[error("link session already exists: {0}")]
    LinkSessionAlreadyExists(LinkSessionId),
    #[error("link session not found: {0}")]
    LinkSessionNotFound(LinkSessionId),
    #[error("link session has a conflicting payload")]
    LinkSessionConflict,
    #[error("link session is closed")]
    LinkSessionClosed,
    #[error("link session is not ready")]
    LinkSessionNotReady,
    #[error("bad link session claim token")]
    BadLinkSessionClaimToken,
    #[error("direct room cannot add third account: {0}")]
    DirectRoomThirdAccount(AccountId),
    #[error(transparent)]
    ProtocolLimit(#[from] ProtocolLimitError),
    #[error(transparent)]
    MembershipDelta(#[from] MembershipDeltaError),
    #[error("json serialization failed: {0}")]
    Json(String),
}

impl From<serde_json::Error> for EngineError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error.to_string())
    }
}

fn validate_room_open(room: &RoomRecord) -> Result<(), EngineError> {
    if room.status != RoomStatus::Open {
        return Err(EngineError::RoomNotOpen);
    }
    Ok(())
}

fn validate_key_package_payload(payload: &[u8]) -> Result<(), EngineError> {
    validate_bytes_non_empty("key_package_payload", payload.len())?;
    validate_bytes_len(
        "key_package_payload",
        payload.len(),
        MAX_KEY_PACKAGE_PAYLOAD_BYTES,
    )?;
    Ok(())
}

fn claimed_key_package_result(
    package: &KeyPackageRecord,
    lease_token: LeaseToken,
) -> ClaimKeyPackageResult {
    ClaimKeyPackageResult {
        key_package_id: package.key_package_id.clone(),
        owner: package.owner.clone(),
        key_package_ref: package.key_package_ref.clone(),
        key_package_hash: package.key_package_hash.clone(),
        key_package_payload: package.key_package_payload.clone(),
        lease_token,
    }
}

pub fn staged_welcomes_by_id<'a>(
    delta: &MembershipDeltaV1,
    staged_welcomes: &'a [StagedWelcomeV1],
) -> Result<BTreeMap<WelcomeId, &'a StagedWelcomeV1>, EngineError> {
    finitechat_proto::validate_item_count(
        "staged_welcomes",
        staged_welcomes.len(),
        MAX_STAGED_WELCOMES_PER_COMMIT,
    )?;

    let mut by_id = BTreeMap::new();
    for staged_welcome in staged_welcomes {
        staged_welcome.validate_limits()?;
        if by_id
            .insert(staged_welcome.welcome_id.clone(), staged_welcome)
            .is_some()
        {
            return Err(EngineError::DuplicateWelcomeId(
                staged_welcome.welcome_id.clone(),
            ));
        }
    }

    let mut expected_ids = BTreeSet::new();
    for add in &delta.adds {
        if !expected_ids.insert(add.welcome_id.clone()) {
            return Err(EngineError::DuplicateWelcomeId(add.welcome_id.clone()));
        }
        if !by_id.contains_key(&add.welcome_id) {
            return Err(EngineError::MissingStagedWelcome(add.welcome_id.clone()));
        }
    }

    for welcome_id in by_id.keys() {
        if !expected_ids.contains(welcome_id) {
            return Err(EngineError::UnexpectedStagedWelcome(welcome_id.clone()));
        }
    }

    debug_assert_eq!(by_id.len(), expected_ids.len());
    debug_assert!(
        delta
            .adds
            .iter()
            .all(|add| by_id.contains_key(&add.welcome_id))
    );
    Ok(by_id)
}

pub fn direct_room_key(left: &str, right: &str) -> (AccountId, AccountId) {
    if left <= right {
        (left.to_string(), right.to_string())
    } else {
        (right.to_string(), left.to_string())
    }
}

pub fn direct_room_key_string((left, right): &(AccountId, AccountId)) -> String {
    format!("{left}\u{1f}{right}")
}

fn device_membership_key(device: &DeviceRef) -> String {
    format!(
        "{}\u{1f}{}",
        length_prefixed(&device.account_id),
        length_prefixed(&device.device_id)
    )
}

fn device_registry_key(device: &DeviceRef) -> String {
    device_membership_key(device)
}

fn key_package_inventory_for<'a>(
    owner: &DeviceRef,
    packages: impl Iterator<Item = &'a KeyPackageRecord>,
) -> Result<KeyPackageInventory, EngineError> {
    let mut available = 0usize;
    let mut leased = 0usize;
    for package in packages {
        if package.owner != *owner {
            continue;
        }
        match package.state {
            KeyPackageState::Available => available += 1,
            KeyPackageState::Leased => leased += 1,
            KeyPackageState::Consumed | KeyPackageState::Released | KeyPackageState::Expired => {}
        }
    }

    let inventory = KeyPackageInventory {
        owner: owner.clone(),
        available: inventory_count_to_u32("key_package_inventory.available", available)?,
        leased: inventory_count_to_u32("key_package_inventory.leased", leased)?,
    };
    debug_assert_eq!(inventory.owner, *owner);
    Ok(inventory)
}

fn inventory_count_to_u32(field: &str, value: usize) -> Result<u32, EngineError> {
    u32::try_from(value).map_err(|_| {
        ProtocolLimitError::TooManyItems {
            field: field.to_string(),
            max_items: u64::from(u32::MAX),
            actual_items: u64::try_from(value).unwrap_or(u64::MAX),
        }
        .into()
    })
}

fn key_package_request_matches_record(
    request: &UploadKeyPackageRequest,
    record: &KeyPackageRecord,
) -> bool {
    request.key_package_id == record.key_package_id
        && request.owner == record.owner
        && request.key_package_ref == record.key_package_ref
        && request.key_package_hash == record.key_package_hash
        && request.key_package_payload == record.key_package_payload
}

fn device_is_revoked(devices: &BTreeMap<String, DeviceRecord>, device: &DeviceRef) -> bool {
    devices
        .get(&device_registry_key(device))
        .is_some_and(|record| record.status == DeviceStatus::Revoked)
}

fn ensure_device_not_revoked(
    devices: &BTreeMap<String, DeviceRecord>,
    device: &DeviceRef,
) -> Result<(), EngineError> {
    debug_assert!(device.validate_limits().is_ok());
    if device_is_revoked(devices, device) {
        Err(EngineError::DeviceRevoked(device.clone()))
    } else {
        Ok(())
    }
}

pub fn idempotency_scope_key(
    room_id: &str,
    sender: &DeviceRef,
    operation: &str,
    key: &str,
) -> String {
    [
        length_prefixed(room_id),
        length_prefixed(&sender.account_id),
        length_prefixed(&sender.device_id),
        length_prefixed(operation),
        length_prefixed(key),
    ]
    .join("|")
}

fn length_prefixed(value: &str) -> String {
    format!("{}:{value}", value.len())
}

fn validate_envelope(
    room: &RoomRecord,
    envelope: &FiniteEnvelope,
    expected_kind: LogEntryKind,
) -> Result<(), EngineError> {
    if envelope.kind != expected_kind {
        return Err(EngineError::WrongEnvelopeKind {
            expected: expected_kind,
            actual: envelope.kind,
        });
    }
    if envelope.room_id != room.room_id {
        return Err(EngineError::EnvelopeRoomMismatch);
    }
    if envelope.mls_group_id != room.mls_group_id {
        return Err(EngineError::EnvelopeGroupMismatch);
    }
    Ok(())
}

pub fn request_hash<T: Serialize>(value: &T) -> Result<String, serde_json::Error> {
    let mut hasher = Sha256::new();
    hasher.update(serde_json::to_vec(value)?);
    Ok(hex_lower(&hasher.finalize()))
}

pub fn lease_token_for(id: &str, device: &DeviceRef) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"finitechat-lease-v1");
    hasher.update(id.as_bytes());
    hasher.update(device.account_id.as_bytes());
    hasher.update(device.device_id.as_bytes());
    hex_lower(&hasher.finalize())
}

pub fn sync_events_page_for_room(
    room: &RoomRecord,
    requester: &DeviceRef,
    after_seq: Seq,
) -> SyncEventsPage {
    let mut entries = Vec::with_capacity(MAX_SYNC_PAGE_ENTRIES as usize);
    let mut page_bytes = 0usize;
    let mut scanned_to_seq = after_seq;
    let mut has_more = false;

    for entry in room.log.iter().filter(|entry| entry.seq > after_seq) {
        scanned_to_seq = entry.seq;
        if room.device_was_member_for_seq(requester, entry.seq) {
            if entries.len() >= MAX_SYNC_PAGE_ENTRIES as usize {
                has_more = true;
                break;
            }
            let next_page_bytes = page_bytes.saturating_add(entry.envelope.payload.len());
            if next_page_bytes > MAX_SYNC_PAGE_BYTES as usize {
                has_more = true;
                break;
            }
            page_bytes = next_page_bytes;
            entries.push(entry.clone());
        }
    }

    let next_after_seq = entries
        .last()
        .map(|entry| entry.seq)
        .unwrap_or(scanned_to_seq);
    debug_assert!(entries.len() <= MAX_SYNC_PAGE_ENTRIES as usize);
    debug_assert!(page_bytes <= MAX_SYNC_PAGE_BYTES as usize);
    debug_assert!(next_after_seq >= after_seq);
    SyncEventsPage {
        entries,
        next_after_seq,
        has_more,
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

pub fn device(account_id: impl Into<AccountId>, device_id: impl Into<DeviceId>) -> DeviceRef {
    DeviceRef {
        account_id: account_id.into(),
        device_id: device_id.into(),
    }
}

pub fn envelope(
    room_id: impl Into<RoomId>,
    group_id: impl Into<MlsGroupId>,
    sender: DeviceRef,
    epoch: Epoch,
    kind: LogEntryKind,
    payload: impl Into<Vec<u8>>,
) -> FiniteEnvelope {
    FiniteEnvelope {
        room_id: room_id.into(),
        mls_group_id: group_id.into(),
        epoch,
        sender,
        kind,
        payload: payload.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use finitechat_proto::{MembershipAddV1, MembershipRemoveV1, StagedWelcomeV1};

    fn service_with_room() -> DeliveryService {
        let mut service = DeliveryService::new();
        service
            .create_room(CreateRoomRequest {
                room_id: "room_1".to_string(),
                mls_group_id: "group_1".to_string(),
                creator: device("alice", "phone"),
            })
            .unwrap();
        service
    }

    fn bob_keypackage(service: &mut DeliveryService) {
        service
            .upload_key_package(UploadKeyPackageRequest {
                key_package_id: "kp_bob".to_string(),
                owner: device("bob", "phone"),
                key_package_ref: "ref_bob".to_string(),
                key_package_hash: "hash_bob".to_string(),
                key_package_payload: b"key-package:kp_bob".to_vec(),
            })
            .unwrap();
        service.claim_key_package("kp_bob").unwrap();
    }

    fn staged_welcome(welcome_id: &str) -> StagedWelcomeV1 {
        StagedWelcomeV1 {
            welcome_id: welcome_id.to_string(),
            welcome_payload: format!("welcome:{welcome_id}").into_bytes(),
            ratchet_tree_payload: format!("tree:{welcome_id}").into_bytes(),
        }
    }

    fn add_bob_commit(service: &mut DeliveryService, idempotency_key: &str) -> CommitAccepted {
        bob_keypackage(service);
        let commit = envelope(
            "room_1",
            "group_1",
            device("alice", "phone"),
            0,
            LogEntryKind::Commit,
            b"add-bob".to_vec(),
        );
        let commit_id = commit.message_id().unwrap();
        service
            .submit_commit(SubmitCommitRequest {
                room_id: "room_1".to_string(),
                sender: device("alice", "phone"),
                expected_epoch: 0,
                envelope: commit,
                membership_delta: MembershipDeltaV1 {
                    base_epoch: 0,
                    post_commit_epoch: 1,
                    commit_message_id: commit_id,
                    adds: vec![MembershipAddV1 {
                        device: device("bob", "phone"),
                        key_package_id: "kp_bob".to_string(),
                        key_package_ref: "ref_bob".to_string(),
                        key_package_hash: "hash_bob".to_string(),
                        welcome_id: "welcome_bob".to_string(),
                    }],
                    removes: vec![],
                },
                idempotency_key: idempotency_key.to_string(),
                staged_welcomes: vec![staged_welcome("welcome_bob")],
            })
            .unwrap()
    }

    #[test]
    fn duplicate_commit_retry_returns_same_result() {
        let mut service = service_with_room();
        let first = add_bob_commit(&mut service, "idem_1");

        let commit = envelope(
            "room_1",
            "group_1",
            device("alice", "phone"),
            0,
            LogEntryKind::Commit,
            b"add-bob".to_vec(),
        );
        let commit_id = commit.message_id().unwrap();
        let second = service
            .submit_commit(SubmitCommitRequest {
                room_id: "room_1".to_string(),
                sender: device("alice", "phone"),
                expected_epoch: 0,
                envelope: commit,
                membership_delta: MembershipDeltaV1 {
                    base_epoch: 0,
                    post_commit_epoch: 1,
                    commit_message_id: commit_id,
                    adds: vec![MembershipAddV1 {
                        device: device("bob", "phone"),
                        key_package_id: "kp_bob".to_string(),
                        key_package_ref: "ref_bob".to_string(),
                        key_package_hash: "hash_bob".to_string(),
                        welcome_id: "welcome_bob".to_string(),
                    }],
                    removes: vec![],
                },
                idempotency_key: "idem_1".to_string(),
                staged_welcomes: vec![staged_welcome("welcome_bob")],
            })
            .unwrap();

        assert_eq!(first, second);
        assert_eq!(service.room("room_1").unwrap().log.len(), 1);
    }

    #[test]
    fn same_epoch_commit_race_accepts_one() {
        let mut service = service_with_room();
        add_bob_commit(&mut service, "idem_1");

        service
            .upload_key_package(UploadKeyPackageRequest {
                key_package_id: "kp_charlie".to_string(),
                owner: device("charlie", "phone"),
                key_package_ref: "ref_charlie".to_string(),
                key_package_hash: "hash_charlie".to_string(),
                key_package_payload: b"key-package:kp_charlie".to_vec(),
            })
            .unwrap();
        service.claim_key_package("kp_charlie").unwrap();
        let losing_commit = envelope(
            "room_1",
            "group_1",
            device("alice", "phone"),
            0,
            LogEntryKind::Commit,
            b"add-charlie".to_vec(),
        );
        let losing_commit_id = losing_commit.message_id().unwrap();
        let err = service
            .submit_commit(SubmitCommitRequest {
                room_id: "room_1".to_string(),
                sender: device("alice", "phone"),
                expected_epoch: 0,
                envelope: losing_commit,
                membership_delta: MembershipDeltaV1 {
                    base_epoch: 0,
                    post_commit_epoch: 1,
                    commit_message_id: losing_commit_id,
                    adds: vec![MembershipAddV1 {
                        device: device("charlie", "phone"),
                        key_package_id: "kp_charlie".to_string(),
                        key_package_ref: "ref_charlie".to_string(),
                        key_package_hash: "hash_charlie".to_string(),
                        welcome_id: "welcome_charlie".to_string(),
                    }],
                    removes: vec![],
                },
                idempotency_key: "idem_2".to_string(),
                staged_welcomes: vec![staged_welcome("welcome_charlie")],
            })
            .unwrap_err();

        assert!(matches!(
            err,
            EngineError::WrongEpoch {
                expected: 1,
                actual: 0
            }
        ));
        assert!(service.welcome("welcome_charlie").is_none());
        assert_eq!(
            service.key_package("kp_charlie").unwrap().state,
            KeyPackageState::Leased
        );
    }

    #[test]
    fn removed_device_can_sync_through_removal_commit() {
        let mut service = service_with_room();
        add_bob_commit(&mut service, "add_bob");
        let claimed = service.claim_welcomes(&device("bob", "phone")).unwrap();
        assert_eq!(claimed.len(), 1);
        service.ack_welcome("welcome_bob", true).unwrap();

        let remove = envelope(
            "room_1",
            "group_1",
            device("alice", "phone"),
            1,
            LogEntryKind::Commit,
            b"remove-bob".to_vec(),
        );
        let remove_id = remove.message_id().unwrap();
        let accepted = service
            .submit_commit(SubmitCommitRequest {
                room_id: "room_1".to_string(),
                sender: device("alice", "phone"),
                expected_epoch: 1,
                envelope: remove,
                membership_delta: MembershipDeltaV1 {
                    base_epoch: 1,
                    post_commit_epoch: 2,
                    commit_message_id: remove_id,
                    adds: vec![],
                    removes: vec![MembershipRemoveV1 {
                        device: device("bob", "phone"),
                        removed_leaf_index: 2,
                    }],
                },
                idempotency_key: "remove_bob".to_string(),
                staged_welcomes: vec![],
            })
            .unwrap();

        let bob_page = service
            .sync_events("room_1", &device("bob", "phone"), 1)
            .unwrap();
        assert_eq!(bob_page.entries.len(), 1);
        assert_eq!(bob_page.entries[0].seq, accepted.seq);
        assert_eq!(bob_page.next_after_seq, accepted.seq);
        assert!(!bob_page.has_more);
    }

    #[test]
    fn invalid_commit_report_blocks_room() {
        let mut service = service_with_room();
        let accepted = add_bob_commit(&mut service, "add_bob");
        service
            .report_invalid_commit("room_1", &device("alice", "phone"), accepted.seq)
            .unwrap();
        assert_eq!(
            service.room("room_1").unwrap().status,
            RoomStatus::NeedsRepair
        );

        let err = service
            .append_event(AppendEventRequest {
                room_id: "room_1".to_string(),
                sender: device("alice", "phone"),
                envelope: envelope(
                    "room_1",
                    "group_1",
                    device("alice", "phone"),
                    1,
                    LogEntryKind::Application,
                    b"hello".to_vec(),
                ),
                idempotency_key: "msg_1".to_string(),
            })
            .unwrap_err();
        assert!(matches!(err, EngineError::RoomNotOpen));
    }
}
