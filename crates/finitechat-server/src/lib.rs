use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use cgka_traits::engine::KeyPackage;
use cgka_traits::transport::{Timestamp, TransportEnvelope, TransportMessage, TransportSource};
use cgka_traits::{EpochId, GroupId, MemberId, MessageId};
use finitechat_proto::{
    AccountRoomDevice, AccountRoomRecord, AppendApplicationEventRequest,
    AppendEphemeralActivityRequest, AppendEventRequest, CommitAccepted, DeviceMembership,
    EphemeralActivityAccepted, EphemeralActivityRecord, EventAccepted, MembershipInterval,
    SubmitCommitRequest, UploadKeyPackageRequest, WelcomeRecord, lease_token_for,
    staged_welcomes_by_id, validate_activity_expiry,
};
pub use finitechat_http::{
    AckLinkPayloadRequest, AckLinkPayloadResponse, AckWelcomeRequest, AckWelcomeResponse,
    ApplicationEffectCountsResponse, ApplicationEffectRequest, BootstrapAccountRoomRequest,
    BootstrapAccountRoomResponse, ClaimKeyPackageRequest, ClaimKeyPackagesRequest,
    ClaimLinkPayloadRequest, ClaimLinkPayloadResponse, ClaimWelcomesRequest,
    CreateLinkSessionRequest,
    DeviceLivenessRecord, ErrorResponse, ExpireKeyPackageLeaseRequest,
    ExpireKeyPackageLeaseResponse, ExpireLinkSessionRequest, ExpireLinkSessionResponse,
    FiniteAccountRoomCommitProjection, GetDeviceLivenessRequest, GetDeviceLivenessResponse,
    GetFanoutRequest, GetLinkSessionRequest, GroupSyncRequest, HealthResponse,
    HttpApplicationDeliveryEffect, HttpClaimedWelcome, HttpFanoutPlan, HttpFanoutRoomPlan,
    HttpFanoutRoomState, HttpFanoutRoomStatus, HttpKeyPackageClaim, HttpKeyPackageInventory,
    HttpLinkSessionRecord, HttpLinkSessionState, InboxSyncRequest, KeyPackageInventoryRequest,
    ListAccountRoomDirectoryRequest, ListAccountRoomDirectoryResponse, MarkFanoutDoneRequest,
    MarkFanoutPreparedRequest, ObserveDeviceLivenessRequest, PublishKeyPackageResponse,
    PublishMessageRequest, ReleaseLinkClaimRequest, ReleaseLinkClaimResponse,
    ReportInvalidCommitRequest, ReportInvalidCommitResponse, RevokeDeviceRequest,
    LeaveRoomRequest, LeaveRoomResponse, UpdateRoomAdminsRequest, UpdateRoomAdminsResponse,
    RevokeDeviceResponse, SaveAccountRoomRequest, SaveAccountRoomResponse, SaveFanoutRoomRequest,
    UploadLinkPayloadRequest,
};
use finitechat_proto::{
    DeviceRef, LogEntryKind, MAX_ACCOUNT_DEVICES_PER_ROOM, MAX_DEVICE_LIVENESS_EXPIRY_MILLIS,
    MAX_EPHEMERAL_ACTIVITY_CACHE_ENTRIES_PER_ROUTE,
    MAX_KEY_PACKAGES_PER_DEVICE, MIN_SUPPORTED_PROTOCOL_VERSION, PROTOCOL_VERSION_V1,
    RoomProtocol,
    MAX_LINK_SESSION_PAYLOAD_BYTES, MAX_OBJECT_ID_BYTES, MembershipAddV1, MembershipDeltaV1,
    RoomLogEntry, RoomStatus, WelcomeState, validate_bytes_len, validate_string_bytes,
};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use transport_http_server::{
    HTTP_SERVER_SOURCE, HttpClaimedKeyPackage, HttpCommitAdmission, HttpDeliveryLimits,
    HttpDeliveryService, HttpKeyPackageId, HttpKeyPackagePublication, HttpPublishCheck,
    HttpPublishReceipt, HttpPublishTarget, HttpSequence, HttpServerError, HttpSyncPage,
    MAX_HTTP_SYNC_PAGE_ENTRIES,
};

const MAX_HTTP_FANOUT_ROOMS: usize = MAX_HTTP_SYNC_PAGE_ENTRIES;
const MAX_HTTP_FANOUT_ID_BYTES: usize = 128;
const MAX_HTTP_IDEMPOTENCY_KEY_BYTES: usize = 128;
const MAX_HTTP_ACCOUNT_ROOM_ID_BYTES: usize = 128;

/// Capacity limits for the durable finite chat server.
///
/// The upstream defaults are sized for tests. These are sized for the current
/// product phase (hundreds of active users, dozens of long chats each); they
/// must be applied before op-log replay so reopening a large server never
/// trips a smaller cap than the one it was written under.
pub fn finite_delivery_limits() -> HttpDeliveryLimits {
    HttpDeliveryLimits {
        max_groups: 65_536,
        max_recipient_inboxes: 65_536,
        max_queue_entries_per_route: 262_144,
        max_key_packages_per_account: 4_096,
    }
}

#[derive(Clone, Debug, Default)]
pub struct HttpServerState {
    service: Arc<Mutex<HttpDeliveryService>>,
    publish_idempotency: Arc<Mutex<HashMap<String, PublishIdempotencyRecord>>>,
    key_package_claim_idempotency: Arc<Mutex<HashMap<String, KeyPackageClaimIdempotencyRecord>>>,
    key_package_inventory: Arc<Mutex<HashMap<HttpKeyPackageId, KeyPackageInventoryRecord>>>,
    revoked_devices: Arc<Mutex<BTreeSet<String>>>,
    fanout_plans: Arc<Mutex<HashMap<String, HttpFanoutPlan>>>,
    link_sessions: Arc<Mutex<BTreeMap<String, HttpLinkSessionRecord>>>,
    account_rooms: Arc<Mutex<BTreeMap<String, BTreeMap<String, Value>>>>,
    room_memberships: Arc<Mutex<BTreeMap<String, HttpRoomMembershipProjection>>>,
    application_effects: Arc<Mutex<BTreeMap<String, HttpApplicationDeliveryEffect>>>,
    ephemeral_activity: Arc<Mutex<BTreeMap<String, Vec<EphemeralActivityRecord>>>>,
    device_liveness: Arc<Mutex<BTreeMap<String, DeviceLivenessRecord>>>,
    welcome_claims: Arc<Mutex<HashMap<MessageId, WelcomeClaimRecord>>>,
    store: Option<Arc<SqliteHttpDeliveryStore>>,
}

impl HttpServerState {
    pub fn new(service: HttpDeliveryService) -> Self {
        Self {
            service: Arc::new(Mutex::new(service)),
            publish_idempotency: Arc::new(Mutex::new(HashMap::new())),
            key_package_claim_idempotency: Arc::new(Mutex::new(HashMap::new())),
            key_package_inventory: Arc::new(Mutex::new(HashMap::new())),
            revoked_devices: Arc::new(Mutex::new(BTreeSet::new())),
            fanout_plans: Arc::new(Mutex::new(HashMap::new())),
            link_sessions: Arc::new(Mutex::new(BTreeMap::new())),
            account_rooms: Arc::new(Mutex::new(BTreeMap::new())),
            room_memberships: Arc::new(Mutex::new(BTreeMap::new())),
            application_effects: Arc::new(Mutex::new(BTreeMap::new())),
            ephemeral_activity: Arc::new(Mutex::new(BTreeMap::new())),
            device_liveness: Arc::new(Mutex::new(BTreeMap::new())),
            welcome_claims: Arc::new(Mutex::new(HashMap::new())),
            store: None,
        }
    }

    pub fn from_sqlite_path(path: impl AsRef<Path>) -> Result<Self, DurableStoreError> {
        let store = Arc::new(SqliteHttpDeliveryStore::open(path)?);
        let mut service = HttpDeliveryService::with_limits(finite_delivery_limits());
        let operations = store.load_operations()?;
        for operation in operations.iter().cloned() {
            replay_operation(&mut service, operation)?;
        }
        let publish_idempotency = store.load_publish_idempotency()?;
        let key_package_claim_idempotency = store.load_key_package_claim_idempotency()?;
        let key_package_inventory = rebuild_key_package_inventory(&operations);
        let revoked_devices = rebuild_revoked_devices(&operations);
        if !key_package_inventory_cache_matches(
            &store.load_key_package_inventory()?,
            &key_package_inventory,
        ) {
            for record in key_package_inventory.values() {
                store.upsert_key_package_inventory(record)?;
            }
        }
        let fanout_plans = store.load_fanout_plans()?;
        let link_sessions = store.load_link_sessions()?;
        let account_rooms = store.load_account_room_directory()?;
        let room_memberships = store.load_room_memberships()?;
        let application_effects = store.load_application_effects()?;
        let welcome_claims = store.load_welcome_claims()?;
        Ok(Self {
            service: Arc::new(Mutex::new(service)),
            publish_idempotency: Arc::new(Mutex::new(publish_idempotency)),
            key_package_claim_idempotency: Arc::new(Mutex::new(key_package_claim_idempotency)),
            key_package_inventory: Arc::new(Mutex::new(key_package_inventory)),
            revoked_devices: Arc::new(Mutex::new(revoked_devices)),
            fanout_plans: Arc::new(Mutex::new(fanout_plans)),
            link_sessions: Arc::new(Mutex::new(link_sessions)),
            account_rooms: Arc::new(Mutex::new(account_rooms)),
            room_memberships: Arc::new(Mutex::new(room_memberships)),
            application_effects: Arc::new(Mutex::new(application_effects)),
            ephemeral_activity: Arc::new(Mutex::new(BTreeMap::new())),
            device_liveness: Arc::new(Mutex::new(BTreeMap::new())),
            welcome_claims: Arc::new(Mutex::new(welcome_claims)),
            store: Some(store),
        })
    }


    /// Raw delivery-contract publish, also used by the upstream
    /// `transport-http-server` conformance suite against this durable server.
    pub fn publish_message(
        &self,
        request: PublishMessageRequest,
    ) -> Result<HttpPublishReceipt, ServerHttpError> {
        self.validate_raw_commit_import(&request)?;
        let Some(idempotency_key) = request.idempotency_key.clone() else {
            let mut service = self.service.lock().expect("HTTP delivery service mutex");
            let receipt = match service.check_publish(&request.target, &request.message)? {
                HttpPublishCheck::DuplicateReplay(receipt) => return Ok(receipt),
                HttpPublishCheck::Fresh(receipt) => receipt,
            };
            if let Some(store) = &self.store {
                store.append_operation(&PersistedOperation::PublishMessage {
                    target: request.target.clone(),
                    message: request.message.clone(),
                    idempotency_key: None,
                })?;
            }
            // The dry run admitted this publish under the held lock, so the
            // apply cannot fail; `?` keeps the impossible path a 500 instead
            // of a panic.
            let published = service.publish(request.target, request.message)?;
            debug_assert_eq!(published, receipt);
            return Ok(published);
        };

        if idempotency_key.is_empty() {
            return Err(ServerHttpError::InvalidIdempotencyKey);
        }

        let fingerprint = PublishMessageFingerprint::from_request(&request);
        let mut service = self.service.lock().expect("HTTP delivery service mutex");
        let mut idempotency = self
            .publish_idempotency
            .lock()
            .expect("HTTP publish idempotency mutex");
        if let Some(record) = idempotency.get(&idempotency_key) {
            if record.fingerprint == fingerprint {
                return Ok(record.receipt.clone());
            }
            return Err(ServerHttpError::IdempotencyConflict { idempotency_key });
        }

        let receipt = match service.check_publish(&request.target, &request.message)? {
            HttpPublishCheck::DuplicateReplay(receipt) => receipt,
            HttpPublishCheck::Fresh(receipt) => receipt,
        };
        let operation = (!receipt.duplicate).then_some(PersistedOperation::PublishMessage {
            target: request.target.clone(),
            message: request.message.clone(),
            idempotency_key: Some(idempotency_key.clone()),
        });
        let record = PublishIdempotencyRecord {
            fingerprint,
            receipt: receipt.clone(),
        };
        if let Some(store) = &self.store {
            store.append_publish_mutation(operation.as_ref(), Some((&idempotency_key, &record)))?;
        }
        if !receipt.duplicate {
            let published = service.publish(request.target, request.message)?;
            debug_assert_eq!(published, receipt);
        }
        idempotency.insert(idempotency_key, record);
        Ok(receipt)
    }


    fn validate_raw_commit_import(
        &self,
        request: &PublishMessageRequest,
    ) -> Result<(), ServerHttpError> {
        if !matches!(&request.target, HttpPublishTarget::Group { .. })
            || serde_json::from_slice::<FiniteAccountRoomCommitProjection>(&request.message.payload)
                .is_ok()
        {
            return Ok(());
        }
        let Some(entry) = room_log_entry_from_payload(&request.message.payload) else {
            return Ok(());
        };
        if entry.kind != LogEntryKind::Commit
            || entry.envelope.kind != LogEntryKind::Commit
            || entry.envelope.room_id != entry.room_id
            || request.message.id.as_slice() != entry.message_id.as_bytes()
        {
            return Ok(());
        }

        let rooms = self
            .room_memberships
            .lock()
            .expect("HTTP room-membership mutex");
        let Some(projection) = rooms.get(&entry.room_id) else {
            return Ok(());
        };
        if projection.mls_group_id == entry.envelope.mls_group_id && projection.membership_complete
        {
            return Err(ServerHttpError::InvalidRawCommitImport {
                room_id: entry.room_id,
                reason: "raw commit import for a typed room must carry membership_delta projection"
                    .to_owned(),
            });
        }
        Ok(())
    }

    pub fn publish_key_package(
        &self,
        publication: HttpKeyPackagePublication,
    ) -> Result<PublishKeyPackageResponse, ServerHttpError> {
        self.ensure_member_not_revoked(&publication.owner)?;
        let mut inventory = self
            .key_package_inventory
            .lock()
            .expect("HTTP KeyPackage inventory mutex");
        let mut candidate = inventory.clone();
        let Some(record) = record_key_package_publication(&mut candidate, &publication)? else {
            return Ok(PublishKeyPackageResponse { published: true });
        };
        let operation = PersistedOperation::PublishKeyPackage { publication };
        if let Some(store) = &self.store {
            store.append_key_package_inventory_operation(&operation, &record)?;
        }
        *inventory = candidate;
        Ok(PublishKeyPackageResponse { published: true })
    }

    pub fn claim_key_package(
        &self,
        request: ClaimKeyPackageRequest,
    ) -> Result<Option<HttpClaimedKeyPackage>, ServerHttpError> {
        self.ensure_member_not_revoked(&request.owner)?;
        let mut inventory = self
            .key_package_inventory
            .lock()
            .expect("HTTP KeyPackage inventory mutex");
        let mut candidate = inventory.clone();
        let claimed = claim_next_key_package_from_inventory(&mut candidate, &request.owner);
        let changed = claimed
            .as_ref()
            .and_then(|package| candidate.get(&package.key_package_id).cloned());
        let changed = changed.into_iter().collect::<Vec<_>>();
        let operation = claimed
            .is_some()
            .then_some(PersistedOperation::ClaimKeyPackage {
                owner: request.owner,
            });
        if let Some(store) = &self.store {
            store.append_key_package_claim_mutation(
                operation.as_ref(),
                None,
                changed.as_slice(),
            )?;
        }
        *inventory = candidate;
        Ok(claimed)
    }

    fn claim_key_packages(
        &self,
        request: ClaimKeyPackagesRequest,
    ) -> Result<Vec<HttpKeyPackageClaim>, ServerHttpError> {
        validate_key_package_claim_batch(&request.owners)?;
        let Some(idempotency_key) = request.idempotency_key.clone() else {
            let mut inventory = self
                .key_package_inventory
                .lock()
                .expect("HTTP KeyPackage inventory mutex");
            let revoked_devices = self.revoked_device_keys();
            let mut candidate = inventory.clone();
            let claims = claim_key_packages_from_inventory(
                &mut candidate,
                &request.owners,
                &revoked_devices,
            );
            let changed = key_package_claim_inventory_records(&candidate, &claims);
            let operation = claims
                .iter()
                .any(|claim| claim.claimed.is_some())
                .then_some(PersistedOperation::ClaimKeyPackages {
                    owners: request.owners,
                });
            if let Some(store) = &self.store {
                store.append_key_package_claim_mutation(
                    operation.as_ref(),
                    None,
                    changed.as_slice(),
                )?;
            }
            *inventory = candidate;
            return Ok(claims);
        };

        if idempotency_key.is_empty() {
            return Err(ServerHttpError::InvalidIdempotencyKey);
        }

        let fingerprint = KeyPackageClaimFingerprint {
            owners: request.owners.clone(),
        };
        let mut inventory = self
            .key_package_inventory
            .lock()
            .expect("HTTP KeyPackage inventory mutex");
        let revoked_devices = self.revoked_device_keys();
        let mut idempotency = self
            .key_package_claim_idempotency
            .lock()
            .expect("HTTP KeyPackage claim idempotency mutex");
        if let Some(record) = idempotency.get(&idempotency_key) {
            if record.fingerprint == fingerprint {
                return Ok(record.response.clone());
            }
            return Err(ServerHttpError::IdempotencyConflict { idempotency_key });
        }

        let mut candidate = inventory.clone();
        let claims =
            claim_key_packages_from_inventory(&mut candidate, &request.owners, &revoked_devices);
        let changed = key_package_claim_inventory_records(&candidate, &claims);
        let operation = claims
            .iter()
            .any(|claim| claim.claimed.is_some())
            .then_some(PersistedOperation::ClaimKeyPackages {
                owners: request.owners,
            });
        let record = KeyPackageClaimIdempotencyRecord {
            fingerprint,
            response: claims.clone(),
        };
        if let Some(store) = &self.store {
            store.append_key_package_claim_mutation(
                operation.as_ref(),
                Some((&idempotency_key, &record)),
                changed.as_slice(),
            )?;
        }
        *inventory = candidate;
        idempotency.insert(idempotency_key, record);
        Ok(claims)
    }

    fn expire_key_package_lease(
        &self,
        request: ExpireKeyPackageLeaseRequest,
    ) -> Result<ExpireKeyPackageLeaseResponse, ServerHttpError> {
        let mut inventory = self
            .key_package_inventory
            .lock()
            .expect("HTTP KeyPackage inventory mutex");
        let mut candidate = inventory.clone();
        let record = candidate.get_mut(&request.key_package_id).ok_or_else(|| {
            ServerHttpError::InvalidKeyPackageLeaseRequest {
                reason: format!("KeyPackage {:?} was not published", request.key_package_id),
            }
        })?;
        match record.state {
            KeyPackageInventoryState::Claimed => {
                record.state = KeyPackageInventoryState::Available;
            }
            KeyPackageInventoryState::Available => {
                return Err(ServerHttpError::InvalidKeyPackageLeaseRequest {
                    reason: format!("KeyPackage {:?} is not claimed", request.key_package_id),
                });
            }
            KeyPackageInventoryState::Consumed => {
                return Err(ServerHttpError::InvalidKeyPackageLeaseRequest {
                    reason: format!(
                        "KeyPackage {:?} is already consumed",
                        request.key_package_id
                    ),
                });
            }
        }
        let changed = record.clone();
        let operation = PersistedOperation::ExpireKeyPackageLease {
            key_package_id: request.key_package_id,
        };
        if let Some(store) = &self.store {
            store.append_key_package_inventory_operation(&operation, &changed)?;
        }
        *inventory = candidate;
        Ok(ExpireKeyPackageLeaseResponse { expired: true })
    }

    fn revoke_device(
        &self,
        request: RevokeDeviceRequest,
    ) -> Result<RevokeDeviceResponse, ServerHttpError> {
        request.device.validate_limits().map_err(|error| {
            ServerHttpError::InvalidDeviceRequest {
                reason: error.to_string(),
            }
        })?;
        let device_key = DeviceMembership::key(&request.device);
        let mut revoked_devices = self.revoked_devices.lock().expect("HTTP device mutex");
        if !revoked_devices.contains(&device_key) {
            let operation = PersistedOperation::RevokeDevice {
                device: request.device.clone(),
            };
            if let Some(store) = &self.store {
                store.append_operation(&operation)?;
            }
            revoked_devices.insert(device_key);
        }
        Ok(RevokeDeviceResponse { revoked: true })
    }

    fn observe_device_liveness(
        &self,
        request: ObserveDeviceLivenessRequest,
    ) -> Result<DeviceLivenessRecord, ServerHttpError> {
        validate_device_liveness_request(&request)?;
        self.ensure_device_not_revoked(&request.device)?;
        if !self.device_active_in_any_room(&request.device) {
            return Err(ServerHttpError::DeviceNotActive {
                device: request.device,
            });
        }

        let key = DeviceMembership::key(&request.device);
        let mut records = self
            .device_liveness
            .lock()
            .expect("HTTP device-liveness mutex");
        if let Some(current) = records.get(&key)
            && request.observed_at_ms <= current.observed_at_ms
        {
            return Ok(current.clone());
        }

        let record = DeviceLivenessRecord {
            device: request.device,
            observed_at_ms: request.observed_at_ms,
            expires_at_ms: request.expires_at_ms,
        };
        records.insert(key, record.clone());
        Ok(record)
    }

    fn get_device_liveness(
        &self,
        request: GetDeviceLivenessRequest,
    ) -> Result<GetDeviceLivenessResponse, ServerHttpError> {
        request.device.validate_limits().map_err(|error| {
            ServerHttpError::InvalidDeviceLivenessRequest {
                reason: error.to_string(),
            }
        })?;
        let key = DeviceMembership::key(&request.device);
        let record = self
            .device_liveness
            .lock()
            .expect("HTTP device-liveness mutex")
            .get(&key)
            .cloned();
        let live = record
            .as_ref()
            .is_some_and(|record| request.now_ms < record.expires_at_ms)
            && self.device_active_in_any_room(&request.device)
            && self.ensure_device_not_revoked(&request.device).is_ok();
        Ok(GetDeviceLivenessResponse { record, live })
    }

    fn device_active_in_any_room(&self, device: &DeviceRef) -> bool {
        self.room_memberships
            .lock()
            .expect("HTTP room-membership mutex")
            .values()
            .any(|projection| projection.device_active_at_head(device))
    }

    fn revoked_device_keys(&self) -> BTreeSet<String> {
        self.revoked_devices
            .lock()
            .expect("HTTP device mutex")
            .clone()
    }

    fn ensure_device_not_revoked(&self, device: &DeviceRef) -> Result<(), ServerHttpError> {
        let revoked_devices = self.revoked_devices.lock().expect("HTTP device mutex");
        ensure_device_not_revoked_in(&revoked_devices, device)
    }

    fn ensure_member_not_revoked(&self, member: &MemberId) -> Result<(), ServerHttpError> {
        if let Some(device) = finite_device_for_member_id(member) {
            self.ensure_device_not_revoked(&device)?;
        }
        Ok(())
    }

    fn key_package_inventory(
        &self,
        request: KeyPackageInventoryRequest,
    ) -> Result<HttpKeyPackageInventory, ServerHttpError> {
        let inventory = self
            .key_package_inventory
            .lock()
            .expect("HTTP KeyPackage inventory mutex");
        let mut available = 0usize;
        let mut claimed = 0usize;
        for record in inventory.values() {
            if record.owner != request.owner {
                continue;
            }
            match record.state {
                KeyPackageInventoryState::Available => available += 1,
                KeyPackageInventoryState::Claimed => claimed += 1,
                KeyPackageInventoryState::Consumed => {}
            }
        }
        Ok(HttpKeyPackageInventory {
            owner: request.owner,
            available: usize_to_u32("available", available)?,
            claimed: usize_to_u32("claimed", claimed)?,
        })
    }

    fn save_fanout_room(
        &self,
        request: SaveFanoutRoomRequest,
    ) -> Result<HttpFanoutPlan, ServerHttpError> {
        validate_fanout_id(&request.fanout_id)?;
        validate_fanout_room_plan(&request.room)?;
        let mut fanouts = self.fanout_plans.lock().expect("HTTP fanout mutex");
        let plan = fanouts
            .entry(request.fanout_id.clone())
            .or_insert_with(|| HttpFanoutPlan {
                fanout_id: request.fanout_id.clone(),
                target_owner: request.target_owner.clone(),
                rooms: Vec::new(),
            });
        if plan.target_owner != request.target_owner {
            return Err(ServerHttpError::FanoutConflict {
                fanout_id: request.fanout_id,
                reason: "target owner differs from existing fanout".to_owned(),
            });
        }
        match plan
            .rooms
            .iter()
            .position(|room| room.plan.room_id == request.room.room_id)
        {
            Some(index) if plan.rooms[index].plan == request.room => {}
            Some(_) => {
                return Err(ServerHttpError::FanoutConflict {
                    fanout_id: request.fanout_id,
                    reason: "room plan differs from existing fanout room".to_owned(),
                });
            }
            None => {
                if plan.rooms.len() >= MAX_HTTP_FANOUT_ROOMS {
                    return Err(ServerHttpError::FanoutLimitExceeded {
                        fanout_id: request.fanout_id,
                        actual: plan.rooms.len() + 1,
                        max: MAX_HTTP_FANOUT_ROOMS,
                    });
                }
                plan.rooms.push(HttpFanoutRoomState {
                    plan: request.room,
                    status: HttpFanoutRoomStatus::Pending,
                });
                plan.rooms.sort_by(|left, right| {
                    left.plan
                        .room_id
                        .as_slice()
                        .cmp(right.plan.room_id.as_slice())
                });
            }
        }
        let plan = plan.clone();
        if let Some(store) = &self.store {
            store.upsert_fanout_plan(&plan)?;
        }
        Ok(plan)
    }

    fn get_fanout(
        &self,
        request: GetFanoutRequest,
    ) -> Result<Option<HttpFanoutPlan>, ServerHttpError> {
        validate_fanout_id(&request.fanout_id)?;
        let fanouts = self.fanout_plans.lock().expect("HTTP fanout mutex");
        Ok(fanouts.get(&request.fanout_id).cloned())
    }

    fn mark_fanout_prepared(
        &self,
        request: MarkFanoutPreparedRequest,
    ) -> Result<HttpFanoutPlan, ServerHttpError> {
        validate_fanout_id(&request.fanout_id)?;
        let mut fanouts = self.fanout_plans.lock().expect("HTTP fanout mutex");
        let plan =
            fanouts
                .get_mut(&request.fanout_id)
                .ok_or_else(|| ServerHttpError::FanoutNotFound {
                    fanout_id: request.fanout_id.clone(),
                })?;
        let room = plan
            .rooms
            .iter_mut()
            .find(|room| room.plan.room_id == request.room_id)
            .ok_or_else(|| ServerHttpError::FanoutRoomNotFound {
                fanout_id: request.fanout_id.clone(),
                room_id: request.room_id.clone(),
            })?;
        match &room.status {
            HttpFanoutRoomStatus::Done { .. } => {
                return Err(ServerHttpError::FanoutConflict {
                    fanout_id: request.fanout_id,
                    reason: "cannot mark a completed fanout room prepared".to_owned(),
                });
            }
            HttpFanoutRoomStatus::Pending | HttpFanoutRoomStatus::Prepared { .. } => {
                room.status = HttpFanoutRoomStatus::Prepared {
                    prepared_message_id: request.prepared_message_id,
                };
            }
        }
        let plan = plan.clone();
        if let Some(store) = &self.store {
            store.upsert_fanout_plan(&plan)?;
        }
        Ok(plan)
    }

    fn mark_fanout_done(
        &self,
        request: MarkFanoutDoneRequest,
    ) -> Result<HttpFanoutPlan, ServerHttpError> {
        validate_fanout_id(&request.fanout_id)?;
        let mut fanouts = self.fanout_plans.lock().expect("HTTP fanout mutex");
        let plan =
            fanouts
                .get_mut(&request.fanout_id)
                .ok_or_else(|| ServerHttpError::FanoutNotFound {
                    fanout_id: request.fanout_id.clone(),
                })?;
        let room = plan
            .rooms
            .iter_mut()
            .find(|room| room.plan.room_id == request.room_id)
            .ok_or_else(|| ServerHttpError::FanoutRoomNotFound {
                fanout_id: request.fanout_id.clone(),
                room_id: request.room_id.clone(),
            })?;
        match &room.status {
            HttpFanoutRoomStatus::Pending => {
                return Err(ServerHttpError::FanoutConflict {
                    fanout_id: request.fanout_id,
                    reason: "cannot complete a fanout room before it is prepared".to_owned(),
                });
            }
            HttpFanoutRoomStatus::Prepared {
                prepared_message_id,
            } if *prepared_message_id == request.prepared_message_id => {
                room.status = HttpFanoutRoomStatus::Done {
                    prepared_message_id: request.prepared_message_id,
                    accepted_seq: request.accepted_seq,
                };
            }
            HttpFanoutRoomStatus::Prepared { .. } => {
                return Err(ServerHttpError::FanoutConflict {
                    fanout_id: request.fanout_id,
                    reason: "prepared message id does not match fanout room state".to_owned(),
                });
            }
            HttpFanoutRoomStatus::Done {
                prepared_message_id,
                accepted_seq,
            } if *prepared_message_id == request.prepared_message_id
                && *accepted_seq == request.accepted_seq => {}
            HttpFanoutRoomStatus::Done { .. } => {
                return Err(ServerHttpError::FanoutConflict {
                    fanout_id: request.fanout_id,
                    reason: "completed fanout room differs from request".to_owned(),
                });
            }
        }
        let plan = plan.clone();
        if let Some(store) = &self.store {
            store.upsert_fanout_plan(&plan)?;
        }
        Ok(plan)
    }

    fn create_link_session(
        &self,
        request: CreateLinkSessionRequest,
    ) -> Result<HttpLinkSessionRecord, ServerHttpError> {
        validate_link_session_id(&request.link_session_id)?;
        validate_link_pairing_public_key(&request.pairing_public_key)?;
        let mut sessions = self.link_sessions.lock().expect("HTTP link-session mutex");
        if sessions.contains_key(&request.link_session_id) {
            return Err(ServerHttpError::LinkSessionAlreadyExists {
                link_session_id: request.link_session_id,
            });
        }
        let record = HttpLinkSessionRecord {
            link_session_id: request.link_session_id,
            pairing_public_key: request.pairing_public_key,
            encrypted_payload: None,
            state: HttpLinkSessionState::Created,
            claim_token: None,
        };
        sessions.insert(record.link_session_id.clone(), record.clone());
        drop(sessions);

        if let Some(store) = &self.store {
            store.upsert_link_session(&record)?;
        }
        Ok(record)
    }

    fn get_link_session(
        &self,
        request: GetLinkSessionRequest,
    ) -> Result<Option<HttpLinkSessionRecord>, ServerHttpError> {
        validate_link_session_id(&request.link_session_id)?;
        let sessions = self.link_sessions.lock().expect("HTTP link-session mutex");
        Ok(sessions.get(&request.link_session_id).cloned())
    }

    fn upload_link_payload(
        &self,
        request: UploadLinkPayloadRequest,
    ) -> Result<HttpLinkSessionRecord, ServerHttpError> {
        validate_link_session_id(&request.link_session_id)?;
        validate_link_payload(&request.encrypted_payload)?;
        let mut sessions = self.link_sessions.lock().expect("HTTP link-session mutex");
        let session = sessions.get_mut(&request.link_session_id).ok_or_else(|| {
            ServerHttpError::LinkSessionNotFound {
                link_session_id: request.link_session_id.clone(),
            }
        })?;
        match session.state {
            HttpLinkSessionState::Created => {
                session.encrypted_payload = Some(request.encrypted_payload);
                session.state = HttpLinkSessionState::PayloadUploaded;
            }
            HttpLinkSessionState::PayloadUploaded
                if session.encrypted_payload.as_deref()
                    == Some(request.encrypted_payload.as_slice()) => {}
            HttpLinkSessionState::PayloadUploaded => {
                return Err(ServerHttpError::LinkSessionConflict {
                    link_session_id: request.link_session_id,
                    reason: "encrypted payload differs from existing payload".to_owned(),
                });
            }
            HttpLinkSessionState::Claimed
            | HttpLinkSessionState::Delivered
            | HttpLinkSessionState::Expired => {
                return Err(ServerHttpError::LinkSessionClosed {
                    link_session_id: request.link_session_id,
                });
            }
        }
        let record = session.clone();
        drop(sessions);

        if let Some(store) = &self.store {
            store.upsert_link_session(&record)?;
        }
        Ok(record)
    }

    fn claim_link_payload(
        &self,
        request: ClaimLinkPayloadRequest,
    ) -> Result<ClaimLinkPayloadResponse, ServerHttpError> {
        validate_link_session_id(&request.link_session_id)?;
        let mut sessions = self.link_sessions.lock().expect("HTTP link-session mutex");
        let session = sessions.get_mut(&request.link_session_id).ok_or_else(|| {
            ServerHttpError::LinkSessionNotFound {
                link_session_id: request.link_session_id.clone(),
            }
        })?;
        if session.state != HttpLinkSessionState::PayloadUploaded {
            return Err(ServerHttpError::LinkSessionNotReady {
                link_session_id: request.link_session_id,
            });
        }
        let encrypted_payload = session.encrypted_payload.clone().ok_or_else(|| {
            ServerHttpError::LinkSessionNotReady {
                link_session_id: request.link_session_id.clone(),
            }
        })?;
        let claim_token = link_session_claim_token(session);
        session.state = HttpLinkSessionState::Claimed;
        session.claim_token = Some(claim_token.clone());
        let record = session.clone();
        drop(sessions);

        if let Some(store) = &self.store {
            store.upsert_link_session(&record)?;
        }
        Ok(ClaimLinkPayloadResponse {
            encrypted_payload,
            claim_token,
        })
    }

    fn ack_link_payload(
        &self,
        request: AckLinkPayloadRequest,
    ) -> Result<AckLinkPayloadResponse, ServerHttpError> {
        validate_link_session_id(&request.link_session_id)?;
        validate_link_claim_token(&request.claim_token)?;
        let mut sessions = self.link_sessions.lock().expect("HTTP link-session mutex");
        let session = sessions.get_mut(&request.link_session_id).ok_or_else(|| {
            ServerHttpError::LinkSessionNotFound {
                link_session_id: request.link_session_id.clone(),
            }
        })?;
        if session.state != HttpLinkSessionState::Claimed {
            return Err(ServerHttpError::LinkSessionNotReady {
                link_session_id: request.link_session_id,
            });
        }
        if session.claim_token.as_deref() != Some(request.claim_token.as_str()) {
            return Err(ServerHttpError::BadLinkSessionClaimToken {
                link_session_id: request.link_session_id,
            });
        }
        session.state = HttpLinkSessionState::Delivered;
        let record = session.clone();
        drop(sessions);

        if let Some(store) = &self.store {
            store.upsert_link_session(&record)?;
        }
        Ok(AckLinkPayloadResponse { acked: true })
    }

    fn release_link_claim(
        &self,
        request: ReleaseLinkClaimRequest,
    ) -> Result<ReleaseLinkClaimResponse, ServerHttpError> {
        validate_link_session_id(&request.link_session_id)?;
        let mut sessions = self.link_sessions.lock().expect("HTTP link-session mutex");
        let session = sessions.get_mut(&request.link_session_id).ok_or_else(|| {
            ServerHttpError::LinkSessionNotFound {
                link_session_id: request.link_session_id.clone(),
            }
        })?;
        if session.state != HttpLinkSessionState::Claimed {
            return Err(ServerHttpError::LinkSessionNotReady {
                link_session_id: request.link_session_id,
            });
        }
        session.state = HttpLinkSessionState::PayloadUploaded;
        session.claim_token = None;
        let record = session.clone();
        drop(sessions);

        if let Some(store) = &self.store {
            store.upsert_link_session(&record)?;
        }
        Ok(ReleaseLinkClaimResponse { released: true })
    }

    fn expire_link_session(
        &self,
        request: ExpireLinkSessionRequest,
    ) -> Result<ExpireLinkSessionResponse, ServerHttpError> {
        validate_link_session_id(&request.link_session_id)?;
        let mut sessions = self.link_sessions.lock().expect("HTTP link-session mutex");
        let session = sessions.get_mut(&request.link_session_id).ok_or_else(|| {
            ServerHttpError::LinkSessionNotFound {
                link_session_id: request.link_session_id.clone(),
            }
        })?;
        if session.state == HttpLinkSessionState::Delivered {
            return Err(ServerHttpError::LinkSessionClosed {
                link_session_id: request.link_session_id,
            });
        }
        session.state = HttpLinkSessionState::Expired;
        let record = session.clone();
        drop(sessions);

        if let Some(store) = &self.store {
            store.upsert_link_session(&record)?;
        }
        Ok(ExpireLinkSessionResponse { expired: true })
    }

    fn save_account_room(
        &self,
        request: SaveAccountRoomRequest,
    ) -> Result<SaveAccountRoomResponse, ServerHttpError> {
        validate_account_room_id("account_id", &request.account_id)?;
        validate_account_room_id("room_id", &request.room_id)?;
        let Some(record) = account_scoped_account_room_record(
            &request.account_id,
            &request.room_id,
            &request.record,
        )?
        else {
            return Err(ServerHttpError::InvalidAccountRoomRequest {
                reason: format!(
                    "record has no current devices for account {}",
                    request.account_id
                ),
            });
        };
        let value = serde_json::to_value(&record)
            .map_err(|error| ServerHttpError::ProjectionJson(error.to_string()))?;

        let mut directory = self
            .account_rooms
            .lock()
            .expect("HTTP account-room directory mutex");
        directory
            .entry(request.account_id.clone())
            .or_default()
            .insert(request.room_id.clone(), value.clone());
        if let Some(store) = &self.store {
            store.upsert_account_room(&AccountRoomDirectoryRecord {
                account_id: request.account_id,
                room_id: request.room_id,
                record: value,
            })?;
        }
        Ok(SaveAccountRoomResponse { saved: true })
    }

    fn bootstrap_account_room(
        &self,
        request: BootstrapAccountRoomRequest,
    ) -> Result<BootstrapAccountRoomResponse, ServerHttpError> {
        validate_account_room_id("room_id", &request.room_id)?;
        validate_account_room_id("mls_group_id", &request.mls_group_id)?;
        request.creator.validate_limits().map_err(|error| {
            ServerHttpError::InvalidAccountRoomRequest {
                reason: error.to_string(),
            }
        })?;

        request.protocol.validate_limits().map_err(|error| {
            ServerHttpError::InvalidAccountRoomRequest {
                reason: error.to_string(),
            }
        })?;
        if request.protocol.protocol_version < MIN_SUPPORTED_PROTOCOL_VERSION
            || request.protocol.protocol_version > PROTOCOL_VERSION_V1
        {
            return Err(ServerHttpError::UnsupportedProtocolVersion {
                requested: request.protocol.protocol_version,
                min: MIN_SUPPORTED_PROTOCOL_VERSION,
                max: PROTOCOL_VERSION_V1,
            });
        }
        let account_id = request.creator.account_id.clone();
        validate_account_room_id("account_id", &account_id)?;
        let mut bootstrapped = false;
        {
            let mut directory = self
                .account_rooms
                .lock()
                .expect("HTTP account-room directory mutex");
            if let Some(existing_value) = directory
                .get(&account_id)
                .and_then(|rooms| rooms.get(&request.room_id))
            {
                let existing_record =
                    serde_json::from_value::<AccountRoomRecord>(existing_value.clone()).map_err(
                        |error| ServerHttpError::AccountRoomBootstrapConflict {
                            account_id: account_id.clone(),
                            room_id: request.room_id.clone(),
                            reason: format!(
                                "existing record is not a Finite account-room record: {error}"
                            ),
                        },
                    )?;
                let has_creator = existing_record
                    .devices
                    .iter()
                    .any(|device| device.device == request.creator && device.active);
                if existing_record.mls_group_id != request.mls_group_id || !has_creator {
                    return Err(ServerHttpError::AccountRoomBootstrapConflict {
                        account_id,
                        room_id: request.room_id,
                        reason: "existing account-room record differs from bootstrap request"
                            .to_owned(),
                    });
                }
            } else {
                let record = AccountRoomRecord {
                    room_id: request.room_id.clone(),
                    mls_group_id: request.mls_group_id.clone(),
                    current_epoch: 0,
                    last_seq: 0,
                    status: RoomStatus::Open,
                    devices: vec![AccountRoomDevice {
                        device: request.creator.clone(),
                        active: true,
                    }],
                };
                record.validate_limits().map_err(|error| {
                    ServerHttpError::InvalidAccountRoomRequest {
                        reason: error.to_string(),
                    }
                })?;
                let value = serde_json::to_value(&record)
                    .map_err(|error| ServerHttpError::ProjectionJson(error.to_string()))?;
                directory
                    .entry(account_id.clone())
                    .or_default()
                    .insert(request.room_id.clone(), value.clone());
                if let Some(store) = &self.store {
                    store.upsert_account_room(&AccountRoomDirectoryRecord {
                        account_id: account_id.clone(),
                        room_id: request.room_id.clone(),
                        record: value,
                    })?;
                }
                bootstrapped = true;
            }
        }

        self.bootstrap_room_membership(&request)?;
        Ok(BootstrapAccountRoomResponse { bootstrapped })
    }


    fn list_account_rooms(
        &self,
        request: ListAccountRoomDirectoryRequest,
    ) -> Result<ListAccountRoomDirectoryResponse, ServerHttpError> {
        validate_account_room_id("account_id", &request.account_id)?;
        if let Some(after_room_id) = &request.after_room_id {
            validate_account_room_id("after_room_id", after_room_id)?;
        }
        if request.limit == 0 || request.limit > MAX_HTTP_SYNC_PAGE_ENTRIES {
            return Err(ServerHttpError::InvalidAccountRoomListLimit {
                actual: request.limit,
                max: MAX_HTTP_SYNC_PAGE_ENTRIES,
            });
        }

        let directory = self
            .account_rooms
            .lock()
            .expect("HTTP account-room directory mutex");
        let mut rooms = Vec::new();
        let mut next_after_room_id = None;
        let mut has_more = false;
        if let Some(account_rooms) = directory.get(&request.account_id) {
            for (room_id, record) in account_rooms {
                if let Some(after_room_id) = &request.after_room_id
                    && room_id <= after_room_id
                {
                    continue;
                }
                let Some(record) =
                    account_scoped_account_room_record(&request.account_id, room_id, record)?
                else {
                    continue;
                };
                if rooms.len() == request.limit {
                    has_more = true;
                    break;
                }
                rooms.push(
                    serde_json::to_value(&record)
                        .map_err(|error| ServerHttpError::ProjectionJson(error.to_string()))?,
                );
                next_after_room_id = Some(room_id.clone());
            }
        }
        Ok(ListAccountRoomDirectoryResponse {
            rooms,
            next_after_room_id,
            has_more,
        })
    }

    fn report_invalid_commit(
        &self,
        request: ReportInvalidCommitRequest,
    ) -> Result<ReportInvalidCommitResponse, ServerHttpError> {
        validate_account_room_id("room_id", &request.room_id)?;
        request.reporter.validate_limits().map_err(|error| {
            ServerHttpError::InvalidRepairReport {
                reason: error.to_string(),
            }
        })?;

        let mut projection = {
            let rooms = self
                .room_memberships
                .lock()
                .expect("HTTP room-membership mutex");
            rooms.get(&request.room_id).cloned().ok_or_else(|| {
                ServerHttpError::RoomMembershipConflict {
                    room_id: request.room_id.clone(),
                    reason: "invalid commit report requires a room-membership projection"
                        .to_owned(),
                }
            })?
        };
        if !projection.device_was_member_for_seq(&request.reporter, request.offending_seq) {
            return Err(ServerHttpError::ReporterNotInInterval {
                reporter: request.reporter,
                offending_seq: request.offending_seq,
            });
        }
        projection.status = RoomStatus::NeedsRepair;

        let account_records = self.account_room_repair_records(&request.room_id)?;
        if let Some(store) = &self.store {
            store.upsert_room_repair_state(&projection, &account_records)?;
        }

        let mut rooms = self
            .room_memberships
            .lock()
            .expect("HTTP room-membership mutex");
        rooms.insert(request.room_id.clone(), projection);
        drop(rooms);

        let mut directory = self
            .account_rooms
            .lock()
            .expect("HTTP account-room directory mutex");
        for record in account_records {
            directory
                .entry(record.account_id)
                .or_default()
                .insert(record.room_id, record.record);
        }

        Ok(ReportInvalidCommitResponse { reported: true })
    }

    fn account_room_repair_records(
        &self,
        room_id: &str,
    ) -> Result<Vec<AccountRoomDirectoryRecord>, ServerHttpError> {
        let directory = self
            .account_rooms
            .lock()
            .expect("HTTP account-room directory mutex");
        let mut records = Vec::new();
        for (account_id, rooms) in directory.iter() {
            let Some(value) = rooms.get(room_id) else {
                continue;
            };
            let Some(mut record) = account_scoped_account_room_record(account_id, room_id, value)?
            else {
                continue;
            };
            record.status = RoomStatus::NeedsRepair;
            let value = serde_json::to_value(&record)
                .map_err(|error| ServerHttpError::ProjectionJson(error.to_string()))?;
            records.push(AccountRoomDirectoryRecord {
                account_id: account_id.clone(),
                room_id: room_id.to_owned(),
                record: value,
            });
        }
        Ok(records)
    }

    fn bootstrap_room_membership(
        &self,
        request: &BootstrapAccountRoomRequest,
    ) -> Result<(), ServerHttpError> {
        let mut rooms = self
            .room_memberships
            .lock()
            .expect("HTTP room-membership mutex");
        if let Some(existing) = rooms.get(&request.room_id) {
            let creator_is_active = existing
                .membership
                .get(&DeviceMembership::key(&request.creator))
                .is_some_and(|membership| {
                    membership.intervals.iter().any(|interval| {
                        interval.active && interval.start_seq == 0 && interval.end_seq.is_none()
                    })
                });
            if existing.mls_group_id != request.mls_group_id || !creator_is_active {
                return Err(ServerHttpError::RoomMembershipConflict {
                    room_id: request.room_id.clone(),
                    reason: "existing room-membership projection differs from bootstrap request"
                        .to_owned(),
                });
            }
            return Ok(());
        }

        let observed = self.observed_room_head(&request.room_id, &request.mls_group_id)?;
        if observed.raw_commit_without_projection {
            return Err(ServerHttpError::RoomMembershipConflict {
                room_id: request.room_id.clone(),
                reason: "typed bootstrap requires existing raw commit history to carry membership_delta projection wrappers".to_owned(),
            });
        }
        let projection = initial_room_membership_projection(
            &request.room_id,
            &request.mls_group_id,
            &request.creator,
            observed.current_epoch,
            observed.last_seq,
            true,
            request.protocol.clone(),
        );
        rooms.insert(request.room_id.clone(), projection.clone());
        drop(rooms);

        if let Some(store) = &self.store {
            store.upsert_room_membership(&projection)?;
        }
        Ok(())
    }

    fn observed_room_head(
        &self,
        room_id: &str,
        mls_group_id: &str,
    ) -> Result<ObservedRoomHead, ServerHttpError> {
        let group_id = group_id_for_room(room_id);
        let service = self.service.lock().expect("HTTP delivery service mutex");
        let mut current_epoch = 0;
        let mut last_seq = 0;
        let mut after_seq = 0;
        let mut raw_commit_without_projection = false;
        loop {
            let page = service.sync_group(&group_id, after_seq, MAX_HTTP_SYNC_PAGE_ENTRIES)?;
            for queued in &page.entries {
                last_seq = last_seq.max(queued.seq);
                let has_membership_delta = serde_json::from_slice::<
                    FiniteAccountRoomCommitProjection,
                >(&queued.message.payload)
                .is_ok();
                let Some(entry) = room_log_entry_from_payload(&queued.message.payload) else {
                    continue;
                };
                if entry.room_id == room_id
                    && entry.envelope.mls_group_id == mls_group_id
                    && entry.kind == LogEntryKind::Commit
                {
                    current_epoch = current_epoch.max(entry.epoch.saturating_add(1));
                    if !has_membership_delta {
                        raw_commit_without_projection = true;
                    }
                }
            }
            if !page.has_more || page.next_after_seq <= after_seq {
                break;
            }
            after_seq = page.next_after_seq;
        }
        Ok(ObservedRoomHead {
            current_epoch,
            last_seq,
            raw_commit_without_projection,
        })
    }



    fn record_submit_commit_projection(
        &self,
        request: &SubmitCommitRequest,
        accepted_seq: HttpSequence,
    ) -> Result<(), ServerHttpError> {
        self.record_account_room_membership_delta(
            &request.room_id,
            &request.envelope.mls_group_id,
            request.membership_delta.post_commit_epoch,
            &request.membership_delta,
            accepted_seq,
        )?;
        self.record_room_membership_delta(
            &request.room_id,
            &request.envelope.mls_group_id,
            &request.sender,
            request.expected_epoch,
            &request.membership_delta,
            accepted_seq,
        )
    }

    fn ensure_submit_commit_projection(
        &self,
        request: &SubmitCommitRequest,
        accepted_seq: HttpSequence,
    ) -> Result<(), ServerHttpError> {
        let rooms = self
            .room_memberships
            .lock()
            .expect("HTTP room-membership mutex");
        let projection_is_current = rooms.get(&request.room_id).is_some_and(|projection| {
            projection.mls_group_id == request.envelope.mls_group_id
                && projection.current_epoch >= request.membership_delta.post_commit_epoch
                && projection.last_seq >= accepted_seq
        });
        drop(rooms);

        if projection_is_current {
            return Ok(());
        }

        self.record_submit_commit_projection(request, accepted_seq)
    }

    fn record_account_room_membership_delta(
        &self,
        room_id: &str,
        mls_group_id: &str,
        current_epoch: u64,
        membership_delta: &MembershipDeltaV1,
        accepted_seq: HttpSequence,
    ) -> Result<(), ServerHttpError> {
        let mut directory = self
            .account_rooms
            .lock()
            .expect("HTTP account-room directory mutex");
        let mutation = apply_account_room_membership_delta(
            &mut directory,
            room_id,
            mls_group_id,
            current_epoch,
            membership_delta,
            accepted_seq,
        )?;
        drop(directory);

        if let Some(store) = &self.store {
            for (account_id, room_id) in mutation.deletes {
                store.delete_account_room(&account_id, &room_id)?;
            }
            for record in mutation.upserts {
                store.upsert_account_room(&record)?;
            }
        }
        Ok(())
    }

    fn record_room_membership_delta(
        &self,
        room_id: &str,
        mls_group_id: &str,
        sender: &DeviceRef,
        expected_epoch: u64,
        membership_delta: &MembershipDeltaV1,
        accepted_seq: HttpSequence,
    ) -> Result<(), ServerHttpError> {
        let mut rooms = self
            .room_memberships
            .lock()
            .expect("HTTP room-membership mutex");
        let projection = apply_room_membership_delta(
            &mut rooms,
            room_id,
            mls_group_id,
            sender,
            expected_epoch,
            membership_delta,
            accepted_seq,
        )?;
        drop(rooms);

        if let Some(store) = &self.store {
            store.upsert_room_membership(&projection)?;
        }
        Ok(())
    }

    fn submit_commit(
        &self,
        request: SubmitCommitRequest,
    ) -> Result<CommitAccepted, ServerHttpError> {
        validate_submit_commit_request(&request)?;
        let message_id = request.envelope.message_id().map_err(|error| {
            ServerHttpError::InvalidCommitRequest {
                reason: error.to_string(),
            }
        })?;
        let commit_publish = commit_publish_request(&request, &message_id)?;
        if let Some(receipt) = self.replayed_publish_receipt(&commit_publish) {
            self.ensure_submit_commit_projection(&request, receipt.seq)?;
            let welcomes = released_welcome_records_for_commit(&request, receipt.seq)?;
            for welcome in &welcomes {
                self.publish_message(welcome_publish_request(welcome)?)?;
            }
            return Ok(CommitAccepted {
                seq: receipt.seq,
                message_id,
                released_welcomes: welcomes
                    .into_iter()
                    .map(|welcome| welcome.welcome_id)
                    .collect(),
            });
        }

        self.ensure_device_not_revoked(&request.sender)?;
        for add in &request.membership_delta.adds {
            self.ensure_device_not_revoked(&add.device)?;
        }
        self.validate_commit_room_membership(&request)?;

        // Fresh typed commits must publish the commit, release Welcomes, and update
        // Finite projections as one candidate snapshot before the durable swap.
        let mut service = self.service.lock().expect("HTTP delivery service mutex");
        let mut publish_idempotency = self
            .publish_idempotency
            .lock()
            .expect("HTTP publish idempotency mutex");
        let mut account_rooms = self
            .account_rooms
            .lock()
            .expect("HTTP account-room directory mutex");
        let mut room_memberships = self
            .room_memberships
            .lock()
            .expect("HTTP room-membership mutex");
        let mut key_package_inventory = self
            .key_package_inventory
            .lock()
            .expect("HTTP KeyPackage inventory mutex");

        // Commit and Welcome publishes are dry-run checked against live
        // state (the delivery service is never cloned); only the small
        // projection maps keep the candidate pattern.
        let mut candidate_account_rooms = account_rooms.clone();
        let mut candidate_room_memberships = room_memberships.clone();
        let mut candidate_key_package_inventory = key_package_inventory.clone();

        let commit_check =
            check_publish_request(&service, &publish_idempotency, &commit_publish)?;
        let receipt = commit_check.receipt.clone();
        let mut checked_publishes = vec![(commit_publish, commit_check)];
        let account_room_mutation = apply_account_room_membership_delta(
            &mut candidate_account_rooms,
            &request.room_id,
            &request.envelope.mls_group_id,
            request.membership_delta.post_commit_epoch,
            &request.membership_delta,
            receipt.seq,
        )?;
        let room_membership_projection = apply_room_membership_delta(
            &mut candidate_room_memberships,
            &request.room_id,
            &request.envelope.mls_group_id,
            &request.sender,
            request.expected_epoch,
            &request.membership_delta,
            receipt.seq,
        )?;
        let key_package_inventory_mutation = consume_claimed_key_packages_for_commit(
            &mut candidate_key_package_inventory,
            &request,
        )?;

        let welcomes = released_welcome_records_for_commit(&request, receipt.seq)?;
        for welcome in &welcomes {
            let publish = welcome_publish_request(welcome)?;
            let check = check_publish_request(&service, &publish_idempotency, &publish)?;
            checked_publishes.push((publish, check));
        }
        let publish_mutations = checked_publishes
            .iter()
            .filter_map(|(_, check)| check.mutation.clone())
            .collect::<Vec<_>>();

        if let Some(store) = &self.store {
            store.append_submit_commit_mutation(
                &publish_mutations,
                &account_room_mutation,
                &room_membership_projection,
                &key_package_inventory_mutation,
            )?;
        }

        for (publish, check) in checked_publishes {
            if check.fresh {
                let published = service.publish(publish.target, publish.message)?;
                debug_assert_eq!(published, check.receipt);
            }
            if let Some(mutation) = check.mutation {
                publish_idempotency.insert(mutation.idempotency_key, mutation.record);
            }
        }
        *account_rooms = candidate_account_rooms;
        *room_memberships = candidate_room_memberships;
        *key_package_inventory = candidate_key_package_inventory;
        drop(service);
        drop(publish_idempotency);
        drop(account_rooms);
        drop(room_memberships);
        drop(key_package_inventory);

        Ok(CommitAccepted {
            seq: receipt.seq,
            message_id,
            released_welcomes: welcomes
                .into_iter()
                .map(|welcome| welcome.welcome_id)
                .collect(),
        })
    }

    fn replayed_publish_receipt(
        &self,
        request: &PublishMessageRequest,
    ) -> Option<HttpPublishReceipt> {
        let idempotency_key = request.idempotency_key.as_ref()?;
        let fingerprint = PublishMessageFingerprint::from_request(request);
        let idempotency = self
            .publish_idempotency
            .lock()
            .expect("HTTP publish idempotency mutex");
        idempotency
            .get(idempotency_key)
            .filter(|record| record.fingerprint == fingerprint)
            .map(|record| record.receipt.clone())
    }

    fn validate_commit_room_membership(
        &self,
        request: &SubmitCommitRequest,
    ) -> Result<(), ServerHttpError> {
        let rooms = self
            .room_memberships
            .lock()
            .expect("HTTP room-membership mutex");
        let Some(projection) = rooms.get(&request.room_id) else {
            return Ok(());
        };
        if projection.mls_group_id != request.envelope.mls_group_id {
            return Err(ServerHttpError::InvalidCommitRequest {
                reason: "commit envelope MLS group does not match room projection".to_owned(),
            });
        }
        if projection.status != RoomStatus::Open {
            return Err(ServerHttpError::RoomNotOpen {
                room_id: request.room_id.clone(),
                status: projection.status,
            });
        }
        if request.expected_epoch != projection.current_epoch {
            return Err(ServerHttpError::InvalidCommitRequest {
                reason: format!(
                    "commit expected epoch {} does not match room epoch {}",
                    request.expected_epoch, projection.current_epoch
                ),
            });
        }
        let tracks_sender = projection.tracks_device(&request.sender);
        if (tracks_sender || projection.membership_complete)
            && !projection.device_active_at_head(&request.sender)
        {
            return Err(ServerHttpError::SenderNotActive {
                sender: request.sender.clone(),
            });
        }
        // Authority: changing another account's membership requires admin
        // (ADR 0003 §2). Same-account linking and removal stay open to any
        // active member, which keeps the device-link fanout admin-free.
        if projection.membership_complete {
            let touches_other_account = request
                .membership_delta
                .adds
                .iter()
                .map(|add| &add.device)
                .chain(request.membership_delta.removes.iter().map(|remove| &remove.device))
                .any(|device| device.account_id != request.sender.account_id);
            if touches_other_account && !projection.admins.contains(&request.sender.account_id) {
                return Err(ServerHttpError::CommitAuthorityRequired {
                    sender: request.sender.clone(),
                });
            }
        }
        validate_membership_adds_for_projection(projection, &request.membership_delta.adds)?;
        Ok(())
    }

    fn append_application_event(
        &self,
        request: AppendApplicationEventRequest,
    ) -> Result<EventAccepted, ServerHttpError> {
        validate_append_event_request(&request.event)?;
        if request.event.envelope.kind != LogEntryKind::Application {
            return Err(ServerHttpError::InvalidEventRequest {
                reason: "/events accepts only application envelopes".to_owned(),
            });
        }
        self.ensure_device_not_revoked(&request.event.sender)?;
        self.validate_event_room_membership(&request.event)?;
        let message_id = request.event.envelope.message_id().map_err(|error| {
            ServerHttpError::InvalidEventRequest {
                reason: error.to_string(),
            }
        })?;
        let event_publish = event_publish_request(&request.event, &message_id)?;

        let mut service = self.service.lock().expect("HTTP delivery service mutex");
        let mut idempotency = self
            .publish_idempotency
            .lock()
            .expect("HTTP publish idempotency mutex");
        let mut room_memberships = self
            .room_memberships
            .lock()
            .expect("HTTP room-membership mutex");
        let mut application_effects = self
            .application_effects
            .lock()
            .expect("HTTP application-effects mutex");

        // Check phase: every admission rule runs read-only against live
        // state, producing exactly the rows to persist.
        let (receipt, publish_mutation) =
            check_typed_event_publish(&service, &idempotency, &event_publish, &message_id)?;
        let room_membership_projection = check_room_event_acceptance(
            &room_memberships,
            &request.event.room_id,
            receipt.seq,
        );
        let effect = HttpApplicationDeliveryEffect {
            room_id: request.event.room_id.clone(),
            seq: receipt.seq,
            message_id: message_id.clone(),
            sender: request.event.sender,
            delivery_policy: request.delivery_policy,
        };
        let effect_mutation = check_application_delivery_effect(
            &application_effects,
            effect,
            &request.event.idempotency_key,
        )?;

        // Persist phase: one SQLite transaction, before any in-memory state
        // changes, so an injected failure rolls back with nothing to undo.
        if let Some(store) = &self.store {
            store.append_application_event_mutation(
                publish_mutation.as_ref(),
                room_membership_projection.as_ref(),
                effect_mutation.as_ref(),
            )?;
        }

        // Apply phase: infallible given the checks above ran under the held
        // locks.
        if let Some(mutation) = publish_mutation {
            let published = service.publish(
                event_publish.target.clone(),
                event_publish.message.clone(),
            )?;
            debug_assert_eq!(published, receipt);
            idempotency.insert(mutation.idempotency_key, mutation.record);
        }
        if let Some(projection) = room_membership_projection {
            room_memberships.insert(request.event.room_id.clone(), projection);
        }
        if let Some(effect) = effect_mutation {
            application_effects.insert(effect.message_id.clone(), effect);
        }
        Ok(EventAccepted {
            seq: receipt.seq,
            message_id,
        })
    }

    fn validate_event_room_membership(
        &self,
        request: &AppendEventRequest,
    ) -> Result<(), ServerHttpError> {
        let rooms = self
            .room_memberships
            .lock()
            .expect("HTTP room-membership mutex");
        let projection =
            rooms
                .get(&request.room_id)
                .ok_or_else(|| ServerHttpError::RoomMembershipConflict {
                    room_id: request.room_id.clone(),
                    reason: "typed event requires a room-membership projection".to_owned(),
                })?;
        if projection.mls_group_id != request.envelope.mls_group_id {
            return Err(ServerHttpError::InvalidEventRequest {
                reason: "event envelope MLS group does not match room projection".to_owned(),
            });
        }
        if projection.status != RoomStatus::Open {
            return Err(ServerHttpError::RoomNotOpen {
                room_id: request.room_id.clone(),
                status: projection.status,
            });
        }
        if request.envelope.epoch != projection.current_epoch {
            return Err(ServerHttpError::InvalidEventRequest {
                reason: format!(
                    "event envelope epoch {} does not match room epoch {}",
                    request.envelope.epoch, projection.current_epoch
                ),
            });
        }
        let tracks_sender = projection.tracks_device(&request.sender);
        if (tracks_sender || projection.membership_complete)
            && !projection.device_active_at_head(&request.sender)
        {
            return Err(ServerHttpError::SenderNotActive {
                sender: request.sender.clone(),
            });
        }
        Ok(())
    }

    fn application_effect(
        &self,
        request: ApplicationEffectRequest,
    ) -> Result<Option<HttpApplicationDeliveryEffect>, ServerHttpError> {
        validate_string_bytes("message_id", &request.message_id, MAX_OBJECT_ID_BYTES).map_err(
            |error| ServerHttpError::InvalidEventRequest {
                reason: error.to_string(),
            },
        )?;
        let effects = self
            .application_effects
            .lock()
            .expect("HTTP application-effects mutex");
        Ok(effects.get(&request.message_id).cloned())
    }

    fn application_effect_counts(
        &self,
    ) -> Result<ApplicationEffectCountsResponse, ServerHttpError> {
        let effects = self
            .application_effects
            .lock()
            .expect("HTTP application-effects mutex");
        let mut push_outbox = 0usize;
        let mut unread = 0usize;
        let mut command_inbox = 0usize;
        for effect in effects.values() {
            if effect.delivery_policy.creates_push() {
                push_outbox += 1;
            }
            if effect.delivery_policy.creates_unread() {
                unread += 1;
            }
            if effect.delivery_policy.creates_command_inbox_work() {
                command_inbox += 1;
            }
        }
        Ok(ApplicationEffectCountsResponse {
            push_outbox: usize_to_u32("push_outbox", push_outbox)?,
            unread: usize_to_u32("unread", unread)?,
            command_inbox: usize_to_u32("command_inbox", command_inbox)?,
        })
    }

    fn append_ephemeral_activity(
        &self,
        request: AppendEphemeralActivityRequest,
    ) -> Result<EphemeralActivityAccepted, ServerHttpError> {
        validate_append_ephemeral_activity_request(&request)?;
        self.ensure_device_not_revoked(&request.sender)?;
        {
            let rooms = self
                .room_memberships
                .lock()
                .expect("HTTP room-membership mutex");
            let projection = rooms.get(&request.room_id).ok_or_else(|| {
                ServerHttpError::RoomMembershipConflict {
                    room_id: request.room_id.clone(),
                    reason: "ephemeral activity requires a room-membership projection".to_owned(),
                }
            })?;
            if projection.mls_group_id != request.mls_group_id {
                return Err(ServerHttpError::InvalidActivityRequest {
                    reason: "activity MLS group does not match room projection".to_owned(),
                });
            }
            if projection.status != RoomStatus::Open {
                return Err(ServerHttpError::RoomNotOpen {
                    room_id: request.room_id.clone(),
                    status: projection.status,
                });
            }
            if request.epoch != projection.current_epoch {
                return Err(ServerHttpError::InvalidActivityRequest {
                    reason: format!(
                        "activity epoch {} does not match room epoch {}",
                        request.epoch, projection.current_epoch
                    ),
                });
            }
            let tracks_sender = projection.tracks_device(&request.sender);
            if (tracks_sender || projection.membership_complete)
                && !projection.device_active_at_head(&request.sender)
            {
                return Err(ServerHttpError::SenderNotActive {
                    sender: request.sender.clone(),
                });
            }
        }

        let route_key = finitechat_proto::ephemeral_activity_route_key(
            &request.room_id,
            request.conversation_id.as_deref(),
            &request.sender,
        );
        let record = EphemeralActivityRecord {
            room_id: request.room_id,
            mls_group_id: request.mls_group_id,
            epoch: request.epoch,
            sender: request.sender,
            conversation_id: request.conversation_id,
            payload: request.payload,
            received_at_ms: request.received_at_ms,
            expires_at_ms: request.expires_at_ms,
        };
        let mut activity = self
            .ephemeral_activity
            .lock()
            .expect("HTTP ephemeral activity mutex");
        let records = activity.entry(route_key.clone()).or_default();
        records.retain(|record| record.expires_at_ms > record.received_at_ms);
        records.push(record);
        while records.len() > MAX_EPHEMERAL_ACTIVITY_CACHE_ENTRIES_PER_ROUTE as usize {
            records.remove(0);
        }
        let cached_events_for_route =
            u32::try_from(records.len()).map_err(|_| ServerHttpError::CounterOverflow)?;
        Ok(EphemeralActivityAccepted {
            route_key,
            cached_events_for_route,
        })
    }


    fn claim_welcomes(
        &self,
        request: ClaimWelcomesRequest,
    ) -> Result<Vec<HttpClaimedWelcome>, ServerHttpError> {
        if request.limit == 0 || request.limit > MAX_HTTP_SYNC_PAGE_ENTRIES {
            return Err(ServerHttpError::InvalidWelcomeClaimLimit {
                actual: request.limit,
                max: MAX_HTTP_SYNC_PAGE_ENTRIES,
            });
        }
        self.ensure_member_not_revoked(&request.recipient)?;

        let service = self.service.lock().expect("HTTP delivery service mutex");
        let mut claims = self
            .welcome_claims
            .lock()
            .expect("HTTP welcome claims mutex");
        let mut claimed = Vec::new();
        let mut after_seq = 0;
        loop {
            let page =
                service.sync_inbox(&request.recipient, after_seq, MAX_HTTP_SYNC_PAGE_ENTRIES)?;
            for entry in page.entries {
                if claimed.len() >= request.limit {
                    break;
                }
                if !matches!(entry.message.envelope, TransportEnvelope::Welcome { .. }) {
                    continue;
                }
                if claims.contains_key(&entry.message.id) {
                    continue;
                }
                let record = WelcomeClaimRecord {
                    recipient: request.recipient.clone(),
                    seq: entry.seq,
                    message: entry.message,
                    state: WelcomeClaimState::Claimed,
                };
                if let Some(store) = &self.store {
                    store.upsert_welcome_claim(&record)?;
                }
                claims.insert(record.message.id.clone(), record.clone());
                claimed.push(record.into_claimed_welcome());
            }
            if claimed.len() >= request.limit || !page.has_more {
                break;
            }
            after_seq = page.next_after_seq;
        }
        Ok(claimed)
    }

    fn ack_welcome(
        &self,
        request: AckWelcomeRequest,
    ) -> Result<AckWelcomeResponse, ServerHttpError> {
        let activation_message;
        let mut claims = self
            .welcome_claims
            .lock()
            .expect("HTTP welcome claims mutex");
        let Some(record) = claims.get_mut(&request.message_id) else {
            return Err(ServerHttpError::WelcomeNotFound {
                message_id: request.message_id,
            });
        };
        ensure_welcome_message_recipient_not_revoked(
            &self.revoked_device_keys(),
            &record.message,
        )?;
        match record.state {
            WelcomeClaimState::Claimed => {
                record.state = WelcomeClaimState::Acked;
                if let Some(store) = &self.store {
                    store.upsert_welcome_claim(record)?;
                }
                activation_message = Some(record.message.clone());
            }
            // A failed activation never reaches the server: the device simply
            // retries, so a repeated ack is an idempotent activation replay.
            WelcomeClaimState::Acked => {
                activation_message = Some(record.message.clone());
            }
        }
        drop(claims);

        if let Some(message) = activation_message {
            self.activate_account_room_from_welcome(&message)?;
            self.activate_room_membership_from_welcome(&message)?;
        }
        Ok(AckWelcomeResponse { acked: true })
    }

    fn activate_account_room_from_welcome(
        &self,
        message: &TransportMessage,
    ) -> Result<(), ServerHttpError> {
        let Ok(welcome) = serde_json::from_slice::<WelcomeRecord>(&message.payload) else {
            return Ok(());
        };
        if message.id.as_slice() != welcome.welcome_id.as_bytes() {
            return Ok(());
        }
        validate_account_room_id("room_id", &welcome.room_id)?;
        welcome.recipient.validate_limits().map_err(|error| {
            ServerHttpError::InvalidAccountRoomRequest {
                reason: error.to_string(),
            }
        })?;

        let account_id = welcome.recipient.account_id.clone();
        let mut directory = self
            .account_rooms
            .lock()
            .expect("HTTP account-room directory mutex");
        let Some(existing_value) = directory
            .get(&account_id)
            .and_then(|rooms| rooms.get(&welcome.room_id))
            .cloned()
        else {
            return Ok(());
        };
        let Some(mut record) =
            account_scoped_account_room_record(&account_id, &welcome.room_id, &existing_value)?
        else {
            return Ok(());
        };

        let mut changed = false;
        for device in &mut record.devices {
            if device.device == welcome.recipient && !device.active {
                device.active = true;
                changed = true;
            }
        }
        if !changed {
            return Ok(());
        }
        let value = serde_json::to_value(&record)
            .map_err(|error| ServerHttpError::ProjectionJson(error.to_string()))?;
        directory
            .entry(account_id.clone())
            .or_default()
            .insert(welcome.room_id.clone(), value.clone());
        drop(directory);

        if let Some(store) = &self.store {
            store.upsert_account_room(&AccountRoomDirectoryRecord {
                account_id,
                room_id: welcome.room_id,
                record: value,
            })?;
        }
        Ok(())
    }

    fn leave_room(&self, request: LeaveRoomRequest) -> Result<LeaveRoomResponse, ServerHttpError> {
        self.ensure_device_not_revoked(&request.sender)?;
        let mut rooms = self
            .room_memberships
            .lock()
            .expect("HTTP room-membership mutex");
        let Some(projection) = rooms.get_mut(&request.room_id) else {
            return Err(ServerHttpError::RoomMembershipConflict {
                room_id: request.room_id.clone(),
                reason: "leave requires a room-membership projection".to_owned(),
            });
        };
        if projection.status != RoomStatus::Open {
            return Err(ServerHttpError::RoomNotOpen {
                room_id: request.room_id.clone(),
                status: projection.status,
            });
        }
        let account_id = request.sender.account_id.clone();
        let departed_at_seq = projection.last_seq;
        if projection.departed.contains(&account_id)
            || projection
                .current_or_pending_device_count_for_account(&account_id)
                == 0
        {
            // Idempotent replay: the account already left (or was removed).
            return Ok(LeaveRoomResponse {
                left: false,
                departed_at_seq,
            });
        }
        if !projection.device_active_at_head(&request.sender) {
            return Err(ServerHttpError::SenderNotActive {
                sender: request.sender.clone(),
            });
        }

        // Whole-account leave (ADR 0003 §3): close every open interval the
        // account holds; delivery filtering takes over immediately. The MLS
        // removal commit follows asynchronously from an admin device.
        for membership in projection.membership.values_mut() {
            if membership.device.account_id != account_id {
                continue;
            }
            for interval in membership.intervals.iter_mut() {
                if interval.end_seq.is_none() {
                    interval.end_seq = Some(departed_at_seq);
                }
            }
        }
        projection.departed.insert(account_id.clone());
        // The last admin cannot leave a room that still has other members —
        // that would strand the room with no one able to manage membership.
        // They must grant another admin first (or remove everyone).
        if projection.admins.contains(&account_id) && projection.admins.len() == 1 {
            let remaining_accounts = projection
                .membership
                .values()
                .filter(|membership| membership.device.account_id != account_id)
                .filter(|membership| {
                    membership
                        .intervals
                        .iter()
                        .any(|interval| interval.end_seq.is_none())
                })
                .count();
            if remaining_accounts > 0 {
                // Re-open the intervals we just closed and refuse: the last
                // admin must hand off (or remove everyone) before leaving.
                for membership in projection.membership.values_mut() {
                    if membership.device.account_id != account_id {
                        continue;
                    }
                    for interval in membership.intervals.iter_mut() {
                        if interval.end_seq == Some(departed_at_seq) {
                            interval.end_seq = None;
                        }
                    }
                }
                projection.departed.remove(&account_id);
                return Err(ServerHttpError::InvalidAdminChange {
                    reason: "the last admin must grant another admin before leaving".to_owned(),
                });
            }
        }
        projection.admins.remove(&account_id);
        let updated = projection.clone();
        drop(rooms);

        // Drop the room from the departing account's directory.
        {
            let mut directory = self
                .account_rooms
                .lock()
                .expect("HTTP account-room directory mutex");
            if let Some(rooms_for_account) = directory.get_mut(&account_id) {
                rooms_for_account.remove(&request.room_id);
            }
        }
        if let Some(store) = &self.store {
            store.upsert_room_membership(&updated)?;
            store.delete_account_room(&account_id, &request.room_id)?;
        }
        Ok(LeaveRoomResponse {
            left: true,
            departed_at_seq,
        })
    }

    fn update_room_admins(
        &self,
        request: UpdateRoomAdminsRequest,
    ) -> Result<UpdateRoomAdminsResponse, ServerHttpError> {
        let (grant, target) = match (&request.grant, &request.revoke) {
            (Some(account), None) => (true, account.clone()),
            (None, Some(account)) => (false, account.clone()),
            _ => {
                return Err(ServerHttpError::InvalidAdminChange {
                    reason: "exactly one of grant or revoke is required".to_owned(),
                });
            }
        };
        self.ensure_device_not_revoked(&request.sender)?;

        let mut rooms = self
            .room_memberships
            .lock()
            .expect("HTTP room-membership mutex");
        let Some(projection) = rooms.get_mut(&request.room_id) else {
            return Err(ServerHttpError::RoomMembershipConflict {
                room_id: request.room_id.clone(),
                reason: "admin change requires a room-membership projection".to_owned(),
            });
        };
        if projection.status != RoomStatus::Open {
            return Err(ServerHttpError::RoomNotOpen {
                room_id: request.room_id.clone(),
                status: projection.status,
            });
        }
        if !projection.device_active_at_head(&request.sender) {
            return Err(ServerHttpError::SenderNotActive {
                sender: request.sender.clone(),
            });
        }
        if !projection.admins.contains(&request.sender.account_id) {
            return Err(ServerHttpError::CommitAuthorityRequired {
                sender: request.sender.clone(),
            });
        }

        if grant {
            if projection
                .current_or_pending_device_count_for_account(&target)
                == 0
            {
                return Err(ServerHttpError::InvalidAdminChange {
                    reason: format!("account {target} has no devices in the room"),
                });
            }
            projection.admins.insert(target);
        } else {
            if !projection.admins.contains(&target) {
                return Err(ServerHttpError::InvalidAdminChange {
                    reason: format!("account {target} is not an admin"),
                });
            }
            if projection.admins.len() == 1 {
                return Err(ServerHttpError::InvalidAdminChange {
                    reason: "cannot revoke the last admin".to_owned(),
                });
            }
            projection.admins.remove(&target);
        }
        let updated = projection.clone();
        drop(rooms);

        if let Some(store) = &self.store {
            store.upsert_room_membership(&updated)?;
        }
        Ok(UpdateRoomAdminsResponse {
            admins: updated.admins.iter().cloned().collect(),
        })
    }

    pub fn sync_inbox(
        &self,
        recipient: &MemberId,
        after_seq: u64,
        limit: usize,
    ) -> Result<HttpSyncPage, ServerHttpError> {
        let service = self.service.lock().expect("HTTP delivery service mutex");
        Ok(service.sync_inbox(recipient, after_seq, limit)?)
    }

    pub fn sync_group(&self, request: GroupSyncRequest) -> Result<HttpSyncPage, ServerHttpError> {
        if request.limit == 0 || request.limit > MAX_HTTP_SYNC_PAGE_ENTRIES {
            return Err(ServerHttpError::InvalidGroupSyncLimit {
                actual: request.limit,
                max: MAX_HTTP_SYNC_PAGE_ENTRIES,
            });
        }
        let service = self.service.lock().expect("HTTP delivery service mutex");
        let page = service.sync_group(&request.group_id, request.after_seq, request.limit)?;
        drop(service);

        let Some(requester) = &request.requester else {
            return Ok(page);
        };
        let requester = device_for_member_id(requester)?;
        let room_id = room_id_for_group_id(&request.group_id)?;
        let rooms = self
            .room_memberships
            .lock()
            .expect("HTTP room-membership mutex");
        let Some(projection) = rooms.get(&room_id) else {
            return Ok(page);
        };
        if !projection.membership_complete && !projection.tracks_device(&requester) {
            return Ok(page);
        }

        let mut entries = Vec::new();
        let mut scanned_to_seq = request.after_seq;
        for entry in page.entries {
            scanned_to_seq = entry.seq;
            if projection.device_was_member_for_seq(&requester, entry.seq) {
                entries.push(entry);
            }
        }
        let next_after_seq = entries
            .last()
            .map(|entry| entry.seq)
            .unwrap_or(scanned_to_seq);
        Ok(HttpSyncPage {
            entries,
            next_after_seq,
            has_more: page.has_more,
        })
    }

    fn activate_room_membership_from_welcome(
        &self,
        message: &TransportMessage,
    ) -> Result<(), ServerHttpError> {
        let Ok(welcome) = serde_json::from_slice::<WelcomeRecord>(&message.payload) else {
            return Ok(());
        };
        if message.id.as_slice() != welcome.welcome_id.as_bytes() {
            return Ok(());
        }
        let mut rooms = self
            .room_memberships
            .lock()
            .expect("HTTP room-membership mutex");
        let Some(projection) = rooms.get_mut(&welcome.room_id) else {
            return Ok(());
        };
        if !projection.activate_interval(&welcome.recipient, welcome.commit_seq) {
            return Ok(());
        }
        let projection = projection.clone();
        drop(rooms);

        if let Some(store) = &self.store {
            store.upsert_room_membership(&projection)?;
        }
        Ok(())
    }
}

pub fn http_router(state: HttpServerState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/events", post(append_application_event))
        .route("/application-effects/get", post(get_application_effect))
        .route(
            "/application-effects/counts",
            post(get_application_effect_counts),
        )
        .route("/activities", post(append_ephemeral_activity))
        .route("/commits", post(submit_commit))
        .route("/sync/group", post(sync_group))
        .route("/sync/inbox", post(sync_inbox))
        .route("/devices/revoke", post(revoke_device))
        .route("/devices/liveness", post(observe_device_liveness))
        .route("/devices/liveness/get", post(get_device_liveness))
        .route("/key-packages", post(publish_key_package))
        .route("/key-packages/inventory", post(key_package_inventory))
        .route("/key-packages/claim", post(claim_key_package))
        .route("/key-packages/claims", post(claim_key_packages))
        .route(
            "/key-packages/leases/expire",
            post(expire_key_package_lease),
        )
        .route("/fanouts/get", post(get_fanout))
        .route("/fanouts/rooms", post(save_fanout_room))
        .route("/fanouts/rooms/prepared", post(mark_fanout_prepared))
        .route("/fanouts/rooms/done", post(mark_fanout_done))
        .route("/link-sessions", post(create_link_session))
        .route("/link-sessions/get", post(get_link_session))
        .route("/link-sessions/payload", post(upload_link_payload))
        .route("/link-sessions/claim", post(claim_link_payload))
        .route("/link-sessions/ack", post(ack_link_payload))
        .route("/link-sessions/release", post(release_link_claim))
        .route("/link-sessions/expire", post(expire_link_session))
        .route("/account-rooms/bootstrap", post(bootstrap_account_room))
        .route("/account-rooms", post(save_account_room))
        .route("/account-rooms/list", post(list_account_rooms))
        .route("/rooms/leave", post(leave_room))
        .route("/rooms/admins", post(update_room_admins))
        .route("/rooms/report-invalid-commit", post(report_invalid_commit))
        .route("/welcomes/claim", post(claim_welcomes))
        .route("/welcomes/ack", post(ack_welcome))
        .with_state(state)
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_owned(),
    })
}



async fn append_application_event(
    State(state): State<HttpServerState>,
    Json(request): Json<AppendApplicationEventRequest>,
) -> Result<Json<EventAccepted>, ServerHttpError> {
    Ok(Json(state.append_application_event(request)?))
}

async fn get_application_effect(
    State(state): State<HttpServerState>,
    Json(request): Json<ApplicationEffectRequest>,
) -> Result<Json<Option<HttpApplicationDeliveryEffect>>, ServerHttpError> {
    Ok(Json(state.application_effect(request)?))
}

async fn get_application_effect_counts(
    State(state): State<HttpServerState>,
) -> Result<Json<ApplicationEffectCountsResponse>, ServerHttpError> {
    Ok(Json(state.application_effect_counts()?))
}

async fn append_ephemeral_activity(
    State(state): State<HttpServerState>,
    Json(request): Json<AppendEphemeralActivityRequest>,
) -> Result<Json<EphemeralActivityAccepted>, ServerHttpError> {
    Ok(Json(state.append_ephemeral_activity(request)?))
}

async fn submit_commit(
    State(state): State<HttpServerState>,
    Json(request): Json<SubmitCommitRequest>,
) -> Result<Json<CommitAccepted>, ServerHttpError> {
    Ok(Json(state.submit_commit(request)?))
}

async fn sync_group(
    State(state): State<HttpServerState>,
    Json(request): Json<GroupSyncRequest>,
) -> Result<Json<HttpSyncPage>, ServerHttpError> {
    Ok(Json(state.sync_group(request)?))
}

async fn sync_inbox(
    State(state): State<HttpServerState>,
    Json(request): Json<InboxSyncRequest>,
) -> Result<Json<HttpSyncPage>, ServerHttpError> {
    let page = state.sync_inbox(&request.recipient, request.after_seq, request.limit)?;
    Ok(Json(page))
}

async fn revoke_device(
    State(state): State<HttpServerState>,
    Json(request): Json<RevokeDeviceRequest>,
) -> Result<Json<RevokeDeviceResponse>, ServerHttpError> {
    let response = state.revoke_device(request)?;
    Ok(Json(response))
}

async fn observe_device_liveness(
    State(state): State<HttpServerState>,
    Json(request): Json<ObserveDeviceLivenessRequest>,
) -> Result<Json<DeviceLivenessRecord>, ServerHttpError> {
    let response = state.observe_device_liveness(request)?;
    Ok(Json(response))
}

async fn get_device_liveness(
    State(state): State<HttpServerState>,
    Json(request): Json<GetDeviceLivenessRequest>,
) -> Result<Json<GetDeviceLivenessResponse>, ServerHttpError> {
    let response = state.get_device_liveness(request)?;
    Ok(Json(response))
}

async fn publish_key_package(
    State(state): State<HttpServerState>,
    Json(publication): Json<HttpKeyPackagePublication>,
) -> Result<Json<PublishKeyPackageResponse>, ServerHttpError> {
    let response = state.publish_key_package(publication)?;
    Ok(Json(response))
}

async fn claim_key_package(
    State(state): State<HttpServerState>,
    Json(request): Json<ClaimKeyPackageRequest>,
) -> Result<Json<Option<HttpClaimedKeyPackage>>, ServerHttpError> {
    let claimed = state.claim_key_package(request)?;
    Ok(Json(claimed))
}

async fn key_package_inventory(
    State(state): State<HttpServerState>,
    Json(request): Json<KeyPackageInventoryRequest>,
) -> Result<Json<HttpKeyPackageInventory>, ServerHttpError> {
    let inventory = state.key_package_inventory(request)?;
    Ok(Json(inventory))
}

async fn claim_key_packages(
    State(state): State<HttpServerState>,
    Json(request): Json<ClaimKeyPackagesRequest>,
) -> Result<Json<Vec<HttpKeyPackageClaim>>, ServerHttpError> {
    let claimed = state.claim_key_packages(request)?;
    Ok(Json(claimed))
}

async fn expire_key_package_lease(
    State(state): State<HttpServerState>,
    Json(request): Json<ExpireKeyPackageLeaseRequest>,
) -> Result<Json<ExpireKeyPackageLeaseResponse>, ServerHttpError> {
    let response = state.expire_key_package_lease(request)?;
    Ok(Json(response))
}

async fn save_fanout_room(
    State(state): State<HttpServerState>,
    Json(request): Json<SaveFanoutRoomRequest>,
) -> Result<Json<HttpFanoutPlan>, ServerHttpError> {
    let fanout = state.save_fanout_room(request)?;
    Ok(Json(fanout))
}

async fn get_fanout(
    State(state): State<HttpServerState>,
    Json(request): Json<GetFanoutRequest>,
) -> Result<Json<Option<HttpFanoutPlan>>, ServerHttpError> {
    let fanout = state.get_fanout(request)?;
    Ok(Json(fanout))
}

async fn mark_fanout_prepared(
    State(state): State<HttpServerState>,
    Json(request): Json<MarkFanoutPreparedRequest>,
) -> Result<Json<HttpFanoutPlan>, ServerHttpError> {
    let fanout = state.mark_fanout_prepared(request)?;
    Ok(Json(fanout))
}

async fn mark_fanout_done(
    State(state): State<HttpServerState>,
    Json(request): Json<MarkFanoutDoneRequest>,
) -> Result<Json<HttpFanoutPlan>, ServerHttpError> {
    let fanout = state.mark_fanout_done(request)?;
    Ok(Json(fanout))
}

async fn create_link_session(
    State(state): State<HttpServerState>,
    Json(request): Json<CreateLinkSessionRequest>,
) -> Result<Json<HttpLinkSessionRecord>, ServerHttpError> {
    let record = state.create_link_session(request)?;
    Ok(Json(record))
}

async fn get_link_session(
    State(state): State<HttpServerState>,
    Json(request): Json<GetLinkSessionRequest>,
) -> Result<Json<Option<HttpLinkSessionRecord>>, ServerHttpError> {
    let record = state.get_link_session(request)?;
    Ok(Json(record))
}

async fn upload_link_payload(
    State(state): State<HttpServerState>,
    Json(request): Json<UploadLinkPayloadRequest>,
) -> Result<Json<HttpLinkSessionRecord>, ServerHttpError> {
    let record = state.upload_link_payload(request)?;
    Ok(Json(record))
}

async fn claim_link_payload(
    State(state): State<HttpServerState>,
    Json(request): Json<ClaimLinkPayloadRequest>,
) -> Result<Json<ClaimLinkPayloadResponse>, ServerHttpError> {
    let response = state.claim_link_payload(request)?;
    Ok(Json(response))
}

async fn ack_link_payload(
    State(state): State<HttpServerState>,
    Json(request): Json<AckLinkPayloadRequest>,
) -> Result<Json<AckLinkPayloadResponse>, ServerHttpError> {
    let response = state.ack_link_payload(request)?;
    Ok(Json(response))
}

async fn release_link_claim(
    State(state): State<HttpServerState>,
    Json(request): Json<ReleaseLinkClaimRequest>,
) -> Result<Json<ReleaseLinkClaimResponse>, ServerHttpError> {
    let response = state.release_link_claim(request)?;
    Ok(Json(response))
}

async fn expire_link_session(
    State(state): State<HttpServerState>,
    Json(request): Json<ExpireLinkSessionRequest>,
) -> Result<Json<ExpireLinkSessionResponse>, ServerHttpError> {
    let response = state.expire_link_session(request)?;
    Ok(Json(response))
}


async fn save_account_room(
    State(state): State<HttpServerState>,
    Json(request): Json<SaveAccountRoomRequest>,
) -> Result<Json<SaveAccountRoomResponse>, ServerHttpError> {
    let response = state.save_account_room(request)?;
    Ok(Json(response))
}

async fn bootstrap_account_room(
    State(state): State<HttpServerState>,
    Json(request): Json<BootstrapAccountRoomRequest>,
) -> Result<Json<BootstrapAccountRoomResponse>, ServerHttpError> {
    let response = state.bootstrap_account_room(request)?;
    Ok(Json(response))
}

async fn list_account_rooms(
    State(state): State<HttpServerState>,
    Json(request): Json<ListAccountRoomDirectoryRequest>,
) -> Result<Json<ListAccountRoomDirectoryResponse>, ServerHttpError> {
    let page = state.list_account_rooms(request)?;
    Ok(Json(page))
}

async fn leave_room(
    State(state): State<HttpServerState>,
    Json(request): Json<LeaveRoomRequest>,
) -> Result<Json<LeaveRoomResponse>, ServerHttpError> {
    let response = state.leave_room(request)?;
    Ok(Json(response))
}

async fn update_room_admins(
    State(state): State<HttpServerState>,
    Json(request): Json<UpdateRoomAdminsRequest>,
) -> Result<Json<UpdateRoomAdminsResponse>, ServerHttpError> {
    let response = state.update_room_admins(request)?;
    Ok(Json(response))
}

async fn report_invalid_commit(
    State(state): State<HttpServerState>,
    Json(request): Json<ReportInvalidCommitRequest>,
) -> Result<Json<ReportInvalidCommitResponse>, ServerHttpError> {
    let response = state.report_invalid_commit(request)?;
    Ok(Json(response))
}

async fn claim_welcomes(
    State(state): State<HttpServerState>,
    Json(request): Json<ClaimWelcomesRequest>,
) -> Result<Json<Vec<HttpClaimedWelcome>>, ServerHttpError> {
    let claimed = state.claim_welcomes(request)?;
    Ok(Json(claimed))
}

async fn ack_welcome(
    State(state): State<HttpServerState>,
    Json(request): Json<AckWelcomeRequest>,
) -> Result<Json<AckWelcomeResponse>, ServerHttpError> {
    let acked = state.ack_welcome(request)?;
    Ok(Json(acked))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum PersistedOperation {
    PublishMessage {
        target: HttpPublishTarget,
        message: cgka_traits::transport::TransportMessage,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        idempotency_key: Option<String>,
    },
    PublishKeyPackage {
        publication: HttpKeyPackagePublication,
    },
    RevokeDevice {
        device: DeviceRef,
    },
    ClaimKeyPackage {
        owner: MemberId,
    },
    ClaimKeyPackages {
        owners: Vec<MemberId>,
    },
    ExpireKeyPackageLease {
        key_package_id: HttpKeyPackageId,
    },
}

impl PersistedOperation {
    fn kind(&self) -> &'static str {
        match self {
            Self::PublishMessage { .. } => "publish_message",
            Self::PublishKeyPackage { .. } => "publish_key_package",
            Self::RevokeDevice { .. } => "revoke_device",
            Self::ClaimKeyPackage { .. } => "claim_key_package",
            Self::ClaimKeyPackages { .. } => "claim_key_packages",
            Self::ExpireKeyPackageLease { .. } => "expire_key_package_lease",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct PublishMessageFingerprint {
    target: HttpPublishTarget,
    message: cgka_traits::transport::TransportMessage,
}

impl PublishMessageFingerprint {
    fn from_request(request: &PublishMessageRequest) -> Self {
        Self {
            target: request.target.clone(),
            message: request.message.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct PublishIdempotencyRecord {
    fingerprint: PublishMessageFingerprint,
    receipt: HttpPublishReceipt,
}


#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct KeyPackageClaimFingerprint {
    owners: Vec<MemberId>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct KeyPackageClaimIdempotencyRecord {
    fingerprint: KeyPackageClaimFingerprint,
    response: Vec<HttpKeyPackageClaim>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct KeyPackageInventoryRecord {
    key_package_id: HttpKeyPackageId,
    owner: MemberId,
    key_package: KeyPackage,
    state: KeyPackageInventoryState,
    finite_metadata: Option<FiniteKeyPackageMetadata>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum KeyPackageInventoryState {
    Available,
    Claimed,
    Consumed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct FiniteKeyPackageMetadata {
    owner: DeviceRef,
    key_package_ref: String,
    key_package_hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct WelcomeClaimRecord {
    recipient: MemberId,
    seq: HttpSequence,
    message: TransportMessage,
    state: WelcomeClaimState,
}

impl WelcomeClaimRecord {
    fn into_claimed_welcome(self) -> HttpClaimedWelcome {
        HttpClaimedWelcome {
            seq: self.seq,
            message: self.message,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WelcomeClaimState {
    Claimed,
    Acked,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct AccountRoomDirectoryRecord {
    account_id: String,
    room_id: String,
    record: Value,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct AccountRoomDirectoryMutation {
    deletes: Vec<(String, String)>,
    upserts: Vec<AccountRoomDirectoryRecord>,
}

#[derive(Clone, Debug, PartialEq)]
struct PublishMutation {
    operation: Option<PersistedOperation>,
    idempotency_key: String,
    record: PublishIdempotencyRecord,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct HttpRoomMembershipProjection {
    room_id: String,
    mls_group_id: String,
    current_epoch: u64,
    last_seq: HttpSequence,
    status: RoomStatus,
    #[serde(default = "default_membership_complete")]
    membership_complete: bool,
    /// Accounts allowed to change membership for other accounts (ADR 0003 §2
    /// as amended by ADR 0004 §4). Creator-initialized at typed bootstrap.
    #[serde(default)]
    admins: BTreeSet<String>,
    /// Accounts that left (ADR 0003 §3) and still await the MLS removal
    /// commit. The server already filters their delivery; this marker lets
    /// member workers discover the pending cryptographic cleanup.
    #[serde(default)]
    departed: BTreeSet<String>,
    /// Per-room protocol slots (ADR 0003 §1).
    #[serde(default)]
    protocol: RoomProtocol,
    #[serde(default)]
    membership: BTreeMap<String, DeviceMembership>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ObservedRoomHead {
    current_epoch: u64,
    last_seq: HttpSequence,
    raw_commit_without_projection: bool,
}

impl HttpRoomMembershipProjection {
    fn tracks_device(&self, device: &DeviceRef) -> bool {
        self.membership.contains_key(&DeviceMembership::key(device))
    }

    fn device_active_at_head(&self, device: &DeviceRef) -> bool {
        self.membership
            .get(&DeviceMembership::key(device))
            .map(|membership| {
                membership.intervals.iter().any(|interval| {
                    interval.active
                        && interval.start_seq <= self.last_seq
                        && interval.end_seq.is_none()
                })
            })
            .unwrap_or(false)
    }

    fn device_was_member_for_seq(&self, device: &DeviceRef, seq: HttpSequence) -> bool {
        self.membership
            .get(&DeviceMembership::key(device))
            .map(|membership| {
                membership.intervals.iter().any(|interval| {
                    interval.start_seq <= seq && interval.end_seq.is_none_or(|end| seq <= end)
                })
            })
            .unwrap_or(false)
    }

    fn current_or_pending_device_count_for_account(&self, account_id: &str) -> usize {
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

    fn device_current_or_pending_at_head(&self, device: &DeviceRef) -> bool {
        self.membership
            .get(&DeviceMembership::key(device))
            .map(|membership| {
                membership
                    .intervals
                    .iter()
                    .any(|interval| interval.end_seq.is_none())
            })
            .unwrap_or(false)
    }

    fn activate_interval(&mut self, device: &DeviceRef, start_seq: HttpSequence) -> bool {
        let Some(membership) = self.membership.get_mut(&DeviceMembership::key(device)) else {
            return false;
        };
        let Some(interval) = membership
            .intervals
            .iter_mut()
            .find(|interval| interval.start_seq == start_seq && !interval.active)
        else {
            return false;
        };
        interval.active = true;
        true
    }
}

#[derive(Debug)]
struct SqliteHttpDeliveryStore {
    conn: Mutex<Connection>,
}

impl SqliteHttpDeliveryStore {
    fn open(path: impl AsRef<Path>) -> Result<Self, rusqlite::Error> {
        let store = Self {
            conn: Mutex::new(Connection::open(path.as_ref())?),
        };
        let conn = store.connection();
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
            PRAGMA synchronous = FULL;
            PRAGMA busy_timeout = 5000;
            CREATE TABLE IF NOT EXISTS http_delivery_ops (
                seq INTEGER PRIMARY KEY AUTOINCREMENT,
                kind TEXT NOT NULL,
                body_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS http_publish_idempotency (
                idempotency_key TEXT PRIMARY KEY,
                fingerprint_json TEXT NOT NULL,
                receipt_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS http_key_package_claim_idempotency (
                idempotency_key TEXT PRIMARY KEY,
                fingerprint_json TEXT NOT NULL,
                response_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS http_key_package_inventory (
                key_package_id_json TEXT PRIMARY KEY,
                owner_json TEXT NOT NULL,
                state_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS http_fanout_plans (
                fanout_id TEXT PRIMARY KEY,
                plan_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS http_link_sessions (
                link_session_id TEXT PRIMARY KEY,
                record_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS http_account_rooms (
                account_id TEXT NOT NULL,
                room_id TEXT NOT NULL,
                record_json TEXT NOT NULL,
                PRIMARY KEY(account_id, room_id)
            );
            CREATE TABLE IF NOT EXISTS http_room_memberships (
                room_id TEXT PRIMARY KEY,
                projection_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS http_application_delivery_effects (
                message_id TEXT PRIMARY KEY,
                room_id TEXT NOT NULL,
                seq INTEGER NOT NULL,
                sender_json TEXT NOT NULL,
                delivery_policy_json TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS http_welcome_claims (
                message_id_json TEXT PRIMARY KEY,
                recipient_json TEXT NOT NULL,
                seq INTEGER NOT NULL,
                message_json TEXT NOT NULL,
                state_json TEXT NOT NULL
            );",
        )?;
        drop(conn);
        Ok(store)
    }

    fn append_operation(&self, operation: &PersistedOperation) -> Result<(), DurableStoreError> {
        let body_json = serde_json::to_string(operation)?;
        let conn = self.connection();
        conn.execute(
            "INSERT INTO http_delivery_ops (kind, body_json) VALUES (?1, ?2)",
            params![operation.kind(), body_json],
        )?;
        Ok(())
    }

    fn append_publish_mutation(
        &self,
        operation: Option<&PersistedOperation>,
        idempotency: Option<(&str, &PublishIdempotencyRecord)>,
    ) -> Result<(), DurableStoreError> {
        let mut conn = self.connection();
        let transaction = conn.transaction()?;
        if let Some(operation) = operation {
            transaction.execute(
                "INSERT INTO http_delivery_ops (kind, body_json) VALUES (?1, ?2)",
                params![operation.kind(), serde_json::to_string(operation)?],
            )?;
        }
        if let Some((idempotency_key, record)) = idempotency {
            transaction.execute(
                "INSERT INTO http_publish_idempotency (
                    idempotency_key,
                    fingerprint_json,
                    receipt_json
                ) VALUES (?1, ?2, ?3)",
                params![
                    idempotency_key,
                    serde_json::to_string(&record.fingerprint)?,
                    serde_json::to_string(&record.receipt)?,
                ],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    fn append_submit_commit_mutation(
        &self,
        publish_mutations: &[PublishMutation],
        account_room_mutation: &AccountRoomDirectoryMutation,
        room_membership_projection: &HttpRoomMembershipProjection,
        key_package_inventory_mutation: &[KeyPackageInventoryRecord],
    ) -> Result<(), DurableStoreError> {
        let mut conn = self.connection();
        let transaction = conn.transaction()?;
        for mutation in publish_mutations {
            if let Some(operation) = &mutation.operation {
                transaction.execute(
                    "INSERT INTO http_delivery_ops (kind, body_json) VALUES (?1, ?2)",
                    params![operation.kind(), serde_json::to_string(operation)?],
                )?;
            }
            transaction.execute(
                "INSERT INTO http_publish_idempotency (
                    idempotency_key,
                    fingerprint_json,
                    receipt_json
                ) VALUES (?1, ?2, ?3)",
                params![
                    mutation.idempotency_key,
                    serde_json::to_string(&mutation.record.fingerprint)?,
                    serde_json::to_string(&mutation.record.receipt)?,
                ],
            )?;
        }
        for (account_id, room_id) in &account_room_mutation.deletes {
            transaction.execute(
                "DELETE FROM http_account_rooms WHERE account_id = ?1 AND room_id = ?2",
                params![account_id, room_id],
            )?;
        }
        for record in &account_room_mutation.upserts {
            transaction.execute(
                "INSERT INTO http_account_rooms (account_id, room_id, record_json)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(account_id, room_id) DO UPDATE SET
                    record_json = excluded.record_json",
                params![
                    record.account_id,
                    record.room_id,
                    serde_json::to_string(&record.record)?,
                ],
            )?;
        }
        transaction.execute(
            "INSERT INTO http_room_memberships (room_id, projection_json)
             VALUES (?1, ?2)
             ON CONFLICT(room_id) DO UPDATE SET
                projection_json = excluded.projection_json",
            params![
                room_membership_projection.room_id,
                serde_json::to_string(room_membership_projection)?,
            ],
        )?;
        for record in key_package_inventory_mutation {
            upsert_key_package_inventory_in_transaction(&transaction, record)?;
        }
        transaction.commit()?;
        Ok(())
    }

    fn append_application_event_mutation(
        &self,
        publish_mutation: Option<&PublishMutation>,
        room_membership_projection: Option<&HttpRoomMembershipProjection>,
        effect: Option<&HttpApplicationDeliveryEffect>,
    ) -> Result<(), DurableStoreError> {
        let mut conn = self.connection();
        let transaction = conn.transaction()?;
        if let Some(mutation) = publish_mutation {
            if let Some(operation) = &mutation.operation {
                transaction.execute(
                    "INSERT INTO http_delivery_ops (kind, body_json) VALUES (?1, ?2)",
                    params![operation.kind(), serde_json::to_string(operation)?],
                )?;
            }
            transaction.execute(
                "INSERT INTO http_publish_idempotency (
                    idempotency_key,
                    fingerprint_json,
                    receipt_json
                ) VALUES (?1, ?2, ?3)",
                params![
                    mutation.idempotency_key,
                    serde_json::to_string(&mutation.record.fingerprint)?,
                    serde_json::to_string(&mutation.record.receipt)?,
                ],
            )?;
        }
        if let Some(projection) = room_membership_projection {
            transaction.execute(
                "INSERT INTO http_room_memberships (room_id, projection_json)
                 VALUES (?1, ?2)
                 ON CONFLICT(room_id) DO UPDATE SET
                    projection_json = excluded.projection_json",
                params![projection.room_id, serde_json::to_string(projection)?],
            )?;
        }
        if let Some(effect) = effect {
            upsert_application_effect_in_transaction(&transaction, effect)?;
        }
        transaction.commit()?;
        Ok(())
    }

    fn append_key_package_claim_mutation(
        &self,
        operation: Option<&PersistedOperation>,
        idempotency: Option<(&str, &KeyPackageClaimIdempotencyRecord)>,
        inventory_records: &[KeyPackageInventoryRecord],
    ) -> Result<(), DurableStoreError> {
        let mut conn = self.connection();
        let transaction = conn.transaction()?;
        if let Some(operation) = operation {
            transaction.execute(
                "INSERT INTO http_delivery_ops (kind, body_json) VALUES (?1, ?2)",
                params![operation.kind(), serde_json::to_string(operation)?],
            )?;
        }
        if let Some((idempotency_key, record)) = idempotency {
            transaction.execute(
                "INSERT INTO http_key_package_claim_idempotency (
                    idempotency_key,
                    fingerprint_json,
                    response_json
                ) VALUES (?1, ?2, ?3)",
                params![
                    idempotency_key,
                    serde_json::to_string(&record.fingerprint)?,
                    serde_json::to_string(&record.response)?,
                ],
            )?;
        }
        for record in inventory_records {
            upsert_key_package_inventory_in_transaction(&transaction, record)?;
        }
        transaction.commit()?;
        Ok(())
    }

    fn append_key_package_inventory_operation(
        &self,
        operation: &PersistedOperation,
        inventory_record: &KeyPackageInventoryRecord,
    ) -> Result<(), DurableStoreError> {
        let mut conn = self.connection();
        let transaction = conn.transaction()?;
        transaction.execute(
            "INSERT INTO http_delivery_ops (kind, body_json) VALUES (?1, ?2)",
            params![operation.kind(), serde_json::to_string(operation)?],
        )?;
        upsert_key_package_inventory_in_transaction(&transaction, inventory_record)?;
        transaction.commit()?;
        Ok(())
    }

    fn load_operations(&self) -> Result<Vec<PersistedOperation>, DurableStoreError> {
        let conn = self.connection();
        let mut statement =
            conn.prepare("SELECT body_json FROM http_delivery_ops ORDER BY seq ASC")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        let mut operations = Vec::new();
        for row in rows {
            operations.push(serde_json::from_str(&row?)?);
        }
        Ok(operations)
    }

    fn load_publish_idempotency(
        &self,
    ) -> Result<HashMap<String, PublishIdempotencyRecord>, DurableStoreError> {
        let conn = self.connection();
        let mut statement = conn.prepare(
            "SELECT idempotency_key, fingerprint_json, receipt_json FROM http_publish_idempotency",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut idempotency = HashMap::new();
        for row in rows {
            let (key, fingerprint_json, receipt_json) = row?;
            idempotency.insert(
                key,
                PublishIdempotencyRecord {
                    fingerprint: serde_json::from_str(&fingerprint_json)?,
                    receipt: serde_json::from_str(&receipt_json)?,
                },
            );
        }
        Ok(idempotency)
    }

    fn load_key_package_claim_idempotency(
        &self,
    ) -> Result<HashMap<String, KeyPackageClaimIdempotencyRecord>, DurableStoreError> {
        let conn = self.connection();
        let mut statement = conn.prepare(
            "SELECT idempotency_key, fingerprint_json, response_json
             FROM http_key_package_claim_idempotency",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut idempotency = HashMap::new();
        for row in rows {
            let (key, fingerprint_json, response_json) = row?;
            idempotency.insert(
                key,
                KeyPackageClaimIdempotencyRecord {
                    fingerprint: serde_json::from_str(&fingerprint_json)?,
                    response: serde_json::from_str(&response_json)?,
                },
            );
        }
        Ok(idempotency)
    }

    fn upsert_key_package_inventory(
        &self,
        record: &KeyPackageInventoryRecord,
    ) -> Result<(), DurableStoreError> {
        let conn = self.connection();
        conn.execute(
            "INSERT INTO http_key_package_inventory (
                key_package_id_json,
                owner_json,
                state_json
            ) VALUES (?1, ?2, ?3)
            ON CONFLICT(key_package_id_json) DO UPDATE SET
                owner_json = excluded.owner_json,
                state_json = excluded.state_json",
            params![
                serde_json::to_string(&record.key_package_id)?,
                serde_json::to_string(&record.owner)?,
                serde_json::to_string(&record.state)?,
            ],
        )?;
        Ok(())
    }

    fn load_key_package_inventory(
        &self,
    ) -> Result<HashMap<HttpKeyPackageId, KeyPackageInventoryRecord>, DurableStoreError> {
        let conn = self.connection();
        let mut statement = conn.prepare(
            "SELECT key_package_id_json, owner_json, state_json FROM http_key_package_inventory",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut inventory = HashMap::new();
        for row in rows {
            let (key_package_id_json, owner_json, state_json) = row?;
            let key_package_id: HttpKeyPackageId = serde_json::from_str(&key_package_id_json)?;
            inventory.insert(
                key_package_id.clone(),
                KeyPackageInventoryRecord {
                    key_package_id,
                    owner: serde_json::from_str(&owner_json)?,
                    key_package: KeyPackage::new(Vec::new()),
                    state: serde_json::from_str(&state_json)?,
                    finite_metadata: None,
                },
            );
        }
        Ok(inventory)
    }

    fn upsert_fanout_plan(&self, plan: &HttpFanoutPlan) -> Result<(), DurableStoreError> {
        let conn = self.connection();
        conn.execute(
            "INSERT INTO http_fanout_plans (fanout_id, plan_json)
             VALUES (?1, ?2)
             ON CONFLICT(fanout_id) DO UPDATE SET
                plan_json = excluded.plan_json",
            params![plan.fanout_id, serde_json::to_string(plan)?],
        )?;
        Ok(())
    }

    fn load_fanout_plans(&self) -> Result<HashMap<String, HttpFanoutPlan>, DurableStoreError> {
        let conn = self.connection();
        let mut statement = conn.prepare("SELECT fanout_id, plan_json FROM http_fanout_plans")?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut fanouts = HashMap::new();
        for row in rows {
            let (fanout_id, plan_json) = row?;
            fanouts.insert(fanout_id, serde_json::from_str(&plan_json)?);
        }
        Ok(fanouts)
    }

    fn upsert_link_session(&self, record: &HttpLinkSessionRecord) -> Result<(), DurableStoreError> {
        let conn = self.connection();
        conn.execute(
            "INSERT INTO http_link_sessions (link_session_id, record_json)
             VALUES (?1, ?2)
             ON CONFLICT(link_session_id) DO UPDATE SET
                record_json = excluded.record_json",
            params![record.link_session_id, serde_json::to_string(record)?],
        )?;
        Ok(())
    }

    fn load_link_sessions(
        &self,
    ) -> Result<BTreeMap<String, HttpLinkSessionRecord>, DurableStoreError> {
        let conn = self.connection();
        let mut statement = conn.prepare(
            "SELECT link_session_id, record_json
             FROM http_link_sessions
             ORDER BY link_session_id ASC",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut sessions = BTreeMap::new();
        for row in rows {
            let (link_session_id, record_json) = row?;
            sessions.insert(link_session_id, serde_json::from_str(&record_json)?);
        }
        Ok(sessions)
    }

    fn upsert_account_room(
        &self,
        record: &AccountRoomDirectoryRecord,
    ) -> Result<(), DurableStoreError> {
        let conn = self.connection();
        conn.execute(
            "INSERT INTO http_account_rooms (account_id, room_id, record_json)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(account_id, room_id) DO UPDATE SET
                record_json = excluded.record_json",
            params![
                record.account_id,
                record.room_id,
                serde_json::to_string(&record.record)?,
            ],
        )?;
        Ok(())
    }

    fn load_account_room_directory(
        &self,
    ) -> Result<BTreeMap<String, BTreeMap<String, Value>>, DurableStoreError> {
        let conn = self.connection();
        let mut statement = conn.prepare(
            "SELECT account_id, room_id, record_json
             FROM http_account_rooms
             ORDER BY account_id ASC, room_id ASC",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut directory = BTreeMap::new();
        for row in rows {
            let (account_id, room_id, record_json) = row?;
            directory
                .entry(account_id)
                .or_insert_with(BTreeMap::new)
                .insert(room_id, serde_json::from_str(&record_json)?);
        }
        Ok(directory)
    }

    fn delete_account_room(
        &self,
        account_id: &str,
        room_id: &str,
    ) -> Result<(), DurableStoreError> {
        let conn = self.connection();
        conn.execute(
            "DELETE FROM http_account_rooms WHERE account_id = ?1 AND room_id = ?2",
            params![account_id, room_id],
        )?;
        Ok(())
    }

    fn upsert_room_membership(
        &self,
        projection: &HttpRoomMembershipProjection,
    ) -> Result<(), DurableStoreError> {
        let conn = self.connection();
        conn.execute(
            "INSERT INTO http_room_memberships (room_id, projection_json)
             VALUES (?1, ?2)
             ON CONFLICT(room_id) DO UPDATE SET
                projection_json = excluded.projection_json",
            params![&projection.room_id, serde_json::to_string(projection)?,],
        )?;
        Ok(())
    }

    fn upsert_room_repair_state(
        &self,
        projection: &HttpRoomMembershipProjection,
        account_records: &[AccountRoomDirectoryRecord],
    ) -> Result<(), DurableStoreError> {
        let mut conn = self.connection();
        let transaction = conn.transaction()?;
        transaction.execute(
            "INSERT INTO http_room_memberships (room_id, projection_json)
             VALUES (?1, ?2)
             ON CONFLICT(room_id) DO UPDATE SET
                projection_json = excluded.projection_json",
            params![projection.room_id, serde_json::to_string(projection)?],
        )?;
        for record in account_records {
            transaction.execute(
                "INSERT INTO http_account_rooms (account_id, room_id, record_json)
                 VALUES (?1, ?2, ?3)
                 ON CONFLICT(account_id, room_id) DO UPDATE SET
                    record_json = excluded.record_json",
                params![
                    record.account_id,
                    record.room_id,
                    serde_json::to_string(&record.record)?,
                ],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    fn load_room_memberships(
        &self,
    ) -> Result<BTreeMap<String, HttpRoomMembershipProjection>, DurableStoreError> {
        let conn = self.connection();
        let mut statement = conn.prepare(
            "SELECT room_id, projection_json
             FROM http_room_memberships
             ORDER BY room_id ASC",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut rooms = BTreeMap::new();
        for row in rows {
            let (room_id, projection_json) = row?;
            rooms.insert(room_id, serde_json::from_str(&projection_json)?);
        }
        Ok(rooms)
    }

    fn load_application_effects(
        &self,
    ) -> Result<BTreeMap<String, HttpApplicationDeliveryEffect>, DurableStoreError> {
        let conn = self.connection();
        let mut statement = conn.prepare(
            "SELECT message_id, room_id, seq, sender_json, delivery_policy_json
             FROM http_application_delivery_effects
             ORDER BY message_id ASC",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, u64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?;
        let mut effects = BTreeMap::new();
        for row in rows {
            let (message_id, room_id, seq, sender_json, delivery_policy_json) = row?;
            effects.insert(
                message_id.clone(),
                HttpApplicationDeliveryEffect {
                    room_id,
                    seq,
                    message_id,
                    sender: serde_json::from_str(&sender_json)?,
                    delivery_policy: serde_json::from_str(&delivery_policy_json)?,
                },
            );
        }
        Ok(effects)
    }

    fn upsert_welcome_claim(&self, record: &WelcomeClaimRecord) -> Result<(), DurableStoreError> {
        let conn = self.connection();
        conn.execute(
            "INSERT INTO http_welcome_claims (
                message_id_json,
                recipient_json,
                seq,
                message_json,
                state_json
            ) VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(message_id_json) DO UPDATE SET
                recipient_json = excluded.recipient_json,
                seq = excluded.seq,
                message_json = excluded.message_json,
                state_json = excluded.state_json",
            params![
                serde_json::to_string(&record.message.id)?,
                serde_json::to_string(&record.recipient)?,
                record.seq,
                serde_json::to_string(&record.message)?,
                serde_json::to_string(&record.state)?,
            ],
        )?;
        Ok(())
    }

    fn load_welcome_claims(
        &self,
    ) -> Result<HashMap<MessageId, WelcomeClaimRecord>, DurableStoreError> {
        let conn = self.connection();
        let mut statement = conn.prepare(
            "SELECT message_id_json, recipient_json, seq, message_json, state_json
             FROM http_welcome_claims",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, u64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?;
        let mut claims = HashMap::new();
        for row in rows {
            let (message_id_json, recipient_json, seq, message_json, state_json) = row?;
            let message_id = serde_json::from_str(&message_id_json)?;
            claims.insert(
                message_id,
                WelcomeClaimRecord {
                    recipient: serde_json::from_str(&recipient_json)?,
                    seq,
                    message: serde_json::from_str(&message_json)?,
                    state: serde_json::from_str(&state_json)?,
                },
            );
        }
        Ok(claims)
    }

    fn connection(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().expect("HTTP delivery store connection mutex")
    }
}

fn upsert_key_package_inventory_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    record: &KeyPackageInventoryRecord,
) -> Result<(), DurableStoreError> {
    transaction.execute(
        "INSERT INTO http_key_package_inventory (
            key_package_id_json,
            owner_json,
            state_json
        ) VALUES (?1, ?2, ?3)
        ON CONFLICT(key_package_id_json) DO UPDATE SET
            owner_json = excluded.owner_json,
            state_json = excluded.state_json",
        params![
            serde_json::to_string(&record.key_package_id)?,
            serde_json::to_string(&record.owner)?,
            serde_json::to_string(&record.state)?,
        ],
    )?;
    Ok(())
}

fn upsert_application_effect_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    effect: &HttpApplicationDeliveryEffect,
) -> Result<(), DurableStoreError> {
    transaction.execute(
        "INSERT INTO http_application_delivery_effects (
            message_id,
            room_id,
            seq,
            sender_json,
            delivery_policy_json
        ) VALUES (?1, ?2, ?3, ?4, ?5)
        ON CONFLICT(message_id) DO NOTHING",
        params![
            &effect.message_id,
            &effect.room_id,
            effect.seq,
            serde_json::to_string(&effect.sender)?,
            serde_json::to_string(&effect.delivery_policy)?,
        ],
    )?;
    Ok(())
}

/// Result of a read-only publish admission check inside a typed commit.
struct CheckedPublish {
    receipt: HttpPublishReceipt,
    /// True when the publish must be applied to the live service after the
    /// durable rows are persisted; false for exact replays.
    fresh: bool,
    mutation: Option<PublishMutation>,
}

/// Read-only form of the old candidate publish: validates one publish inside
/// a typed commit against live state and returns the receipt it would
/// produce, whether it still needs applying, and the durable rows to
/// persist. Distinct queues and idempotency keys per publish are guaranteed
/// by typed-commit validation (duplicate adds are rejected), so a batch of
/// these checks against the same live state predicts seqs correctly.
fn check_publish_request(
    service: &HttpDeliveryService,
    idempotency: &HashMap<String, PublishIdempotencyRecord>,
    request: &PublishMessageRequest,
) -> Result<CheckedPublish, ServerHttpError> {
    let Some(idempotency_key) = request.idempotency_key.clone() else {
        let (receipt, fresh) = match service.check_publish(&request.target, &request.message)? {
            HttpPublishCheck::DuplicateReplay(receipt) => (receipt, false),
            HttpPublishCheck::Fresh(receipt) => (receipt, true),
        };
        return Ok(CheckedPublish {
            receipt,
            fresh,
            mutation: None,
        });
    };
    if idempotency_key.is_empty() {
        return Err(ServerHttpError::InvalidIdempotencyKey);
    }

    let fingerprint = PublishMessageFingerprint::from_request(request);
    if let Some(record) = idempotency.get(&idempotency_key) {
        if record.fingerprint == fingerprint {
            return Ok(CheckedPublish {
                receipt: record.receipt.clone(),
                fresh: false,
                mutation: None,
            });
        }
        return Err(ServerHttpError::IdempotencyConflict { idempotency_key });
    }

    let (receipt, fresh) = match service.check_publish(&request.target, &request.message)? {
        HttpPublishCheck::DuplicateReplay(receipt) => (receipt, false),
        HttpPublishCheck::Fresh(receipt) => (receipt, true),
    };
    let operation = fresh.then(|| PersistedOperation::PublishMessage {
        target: request.target.clone(),
        message: request.message.clone(),
        idempotency_key: Some(idempotency_key.clone()),
    });
    let record = PublishIdempotencyRecord {
        fingerprint,
        receipt: receipt.clone(),
    };

    Ok(CheckedPublish {
        receipt,
        fresh,
        mutation: Some(PublishMutation {
            operation,
            idempotency_key,
            record,
        }),
    })
}

/// Read-only admission check for a typed event publish. Returns the receipt
/// the publish would produce plus the durable mutation to persist before
/// applying. Returns `(receipt, None)` for an exact idempotent replay.
fn check_typed_event_publish(
    service: &HttpDeliveryService,
    idempotency: &HashMap<String, PublishIdempotencyRecord>,
    request: &PublishMessageRequest,
    message_id: &str,
) -> Result<(HttpPublishReceipt, Option<PublishMutation>), ServerHttpError> {
    let Some(idempotency_key) = request.idempotency_key.clone() else {
        return Err(ServerHttpError::InvalidIdempotencyKey);
    };
    if idempotency_key.is_empty() {
        return Err(ServerHttpError::InvalidIdempotencyKey);
    }

    let fingerprint = PublishMessageFingerprint::from_request(request);
    if let Some(record) = idempotency.get(&idempotency_key) {
        if record.fingerprint == fingerprint {
            return Ok((record.receipt.clone(), None));
        }
        return Err(ServerHttpError::IdempotencyConflict { idempotency_key });
    }

    let typed_message_id = MessageId::new(message_id.as_bytes().to_vec());
    let receipt = match service.check_publish(&request.target, &request.message) {
        Ok(HttpPublishCheck::Fresh(receipt)) => receipt,
        Ok(HttpPublishCheck::DuplicateReplay(_))
        | Err(HttpServerError::ConflictingMessageId { .. }) => {
            return Err(ServerHttpError::DuplicateMessageId {
                message_id: typed_message_id,
            });
        }
        Err(error) => return Err(error.into()),
    };

    let operation = PersistedOperation::PublishMessage {
        target: request.target.clone(),
        message: request.message.clone(),
        idempotency_key: Some(idempotency_key.clone()),
    };
    let record = PublishIdempotencyRecord {
        fingerprint,
        receipt: receipt.clone(),
    };

    Ok((
        receipt,
        Some(PublishMutation {
            operation: Some(operation),
            idempotency_key,
            record,
        }),
    ))
}

/// Compute the room-membership `last_seq` advance for an accepted typed
/// event: returns the updated projection to persist and later insert,
/// without touching the map.
fn check_room_event_acceptance(
    rooms: &BTreeMap<String, HttpRoomMembershipProjection>,
    room_id: &str,
    accepted_seq: HttpSequence,
) -> Option<HttpRoomMembershipProjection> {
    let projection = rooms.get(room_id)?;
    if projection.last_seq >= accepted_seq {
        return None;
    }
    let mut updated = projection.clone();
    updated.last_seq = accepted_seq;
    Some(updated)
}

/// Validate a delivery effect against the stored projection and return the
/// row to persist and later insert, without touching the map. Exact replays
/// return `None`; conflicting policies for the same message id are rejected.
fn check_application_delivery_effect(
    effects: &BTreeMap<String, HttpApplicationDeliveryEffect>,
    effect: HttpApplicationDeliveryEffect,
    idempotency_key: &str,
) -> Result<Option<HttpApplicationDeliveryEffect>, ServerHttpError> {
    if let Some(existing) = effects.get(&effect.message_id) {
        if existing == &effect {
            return Ok(None);
        }
        return Err(ServerHttpError::IdempotencyConflict {
            idempotency_key: idempotency_key.to_owned(),
        });
    }
    Ok(Some(effect))
}


fn apply_account_room_membership_delta(
    directory: &mut BTreeMap<String, BTreeMap<String, Value>>,
    room_id: &str,
    mls_group_id: &str,
    current_epoch: u64,
    membership_delta: &MembershipDeltaV1,
    accepted_seq: HttpSequence,
) -> Result<AccountRoomDirectoryMutation, ServerHttpError> {
    let mut account_ids = BTreeSet::new();
    for (account_id, rooms) in directory.iter() {
        if rooms.contains_key(room_id) {
            account_ids.insert(account_id.clone());
        }
    }
    for add in &membership_delta.adds {
        account_ids.insert(add.device.account_id.clone());
    }
    for remove in &membership_delta.removes {
        account_ids.insert(remove.device.account_id.clone());
    }

    let mut mutation = AccountRoomDirectoryMutation::default();
    for account_id in account_ids {
        let empty_record = || AccountRoomRecord {
            room_id: room_id.to_owned(),
            mls_group_id: mls_group_id.to_owned(),
            current_epoch,
            last_seq: accepted_seq,
            status: RoomStatus::Open,
            devices: Vec::new(),
        };
        let existing_record = directory
            .get(&account_id)
            .and_then(|rooms| rooms.get(room_id))
            .cloned();
        let mut record = match existing_record {
            Some(value) => match account_scoped_account_room_record(&account_id, room_id, &value) {
                Ok(Some(record)) => record,
                Ok(None) => empty_record(),
                Err(_) => continue,
            },
            None => empty_record(),
        };

        if record.room_id != room_id {
            continue;
        }
        record.mls_group_id = mls_group_id.to_owned();
        record.current_epoch = current_epoch;
        record.last_seq = accepted_seq;
        for remove in membership_delta
            .removes
            .iter()
            .filter(|remove| remove.device.account_id == account_id)
        {
            record
                .devices
                .retain(|device| device.device != remove.device);
        }
        for add in membership_delta
            .adds
            .iter()
            .filter(|add| add.device.account_id == account_id)
        {
            if !record
                .devices
                .iter()
                .any(|device| device.device == add.device)
            {
                record.devices.push(AccountRoomDevice {
                    device: add.device.clone(),
                    active: false,
                });
            }
        }
        record
            .devices
            .sort_by(|left, right| left.device.device_id.cmp(&right.device.device_id));

        if record.devices.is_empty() {
            if let Some(rooms) = directory.get_mut(&account_id) {
                rooms.remove(room_id);
                if rooms.is_empty() {
                    directory.remove(&account_id);
                }
            }
            mutation.deletes.push((account_id, room_id.to_owned()));
            continue;
        }

        let value = serde_json::to_value(&record)
            .map_err(|error| ServerHttpError::ProjectionJson(error.to_string()))?;
        directory
            .entry(account_id.clone())
            .or_default()
            .insert(room_id.to_owned(), value.clone());
        mutation.upserts.push(AccountRoomDirectoryRecord {
            account_id,
            room_id: room_id.to_owned(),
            record: value,
        });
    }
    Ok(mutation)
}

fn validate_membership_adds_for_projection(
    projection: &HttpRoomMembershipProjection,
    adds: &[MembershipAddV1],
) -> Result<(), ServerHttpError> {
    let mut added_devices_by_account = BTreeMap::<String, usize>::new();
    for add in adds {
        let current_devices =
            projection.current_or_pending_device_count_for_account(&add.device.account_id);
        let added_devices = added_devices_by_account
            .entry(add.device.account_id.clone())
            .or_insert(0);
        *added_devices += 1;
        let proposed = current_devices + *added_devices;
        if proposed > MAX_ACCOUNT_DEVICES_PER_ROOM as usize {
            return Err(ServerHttpError::InvalidCommitRequest {
                reason: format!(
                    "room.devices_per_account has {proposed} items, max {MAX_ACCOUNT_DEVICES_PER_ROOM}"
                ),
            });
        }
        if projection.device_current_or_pending_at_head(&add.device) {
            return Err(ServerHttpError::InvalidCommitRequest {
                reason: format!(
                    "device {:?} is already current or pending in room",
                    add.device
                ),
            });
        }
    }
    Ok(())
}

fn apply_room_membership_delta(
    rooms: &mut BTreeMap<String, HttpRoomMembershipProjection>,
    room_id: &str,
    mls_group_id: &str,
    sender: &DeviceRef,
    expected_epoch: u64,
    membership_delta: &MembershipDeltaV1,
    accepted_seq: HttpSequence,
) -> Result<HttpRoomMembershipProjection, ServerHttpError> {
    let projection = rooms.entry(room_id.to_owned()).or_insert_with(|| {
        initial_room_membership_projection(
            room_id,
            mls_group_id,
            sender,
            expected_epoch,
            0,
            expected_epoch == 0,
            RoomProtocol::default(),
        )
    });
    if projection.room_id != room_id || projection.mls_group_id != mls_group_id {
        return Err(ServerHttpError::RoomMembershipConflict {
            room_id: room_id.to_owned(),
            reason: "membership delta targets a different room or MLS group".to_owned(),
        });
    }
    if projection.current_epoch != expected_epoch {
        return Err(ServerHttpError::RoomMembershipConflict {
            room_id: room_id.to_owned(),
            reason: format!(
                "membership delta expected epoch {expected_epoch}, projection is at {}",
                projection.current_epoch
            ),
        });
    }

    validate_membership_adds_for_projection(projection, &membership_delta.adds)?;

    for remove in &membership_delta.removes {
        if let Some(membership) = projection
            .membership
            .get_mut(&DeviceMembership::key(&remove.device))
            && let Some(interval) = membership
                .intervals
                .iter_mut()
                .rev()
                .find(|interval| interval.active && interval.end_seq.is_none())
        {
            interval.end_seq = Some(accepted_seq);
        }
        // The MLS removal commit for a departed account completes the leave.
        projection.departed.remove(&remove.device.account_id);
    }
    for add in &membership_delta.adds {
        projection
            .membership
            .entry(DeviceMembership::key(&add.device))
            .or_insert_with(|| DeviceMembership {
                device: add.device.clone(),
                intervals: Vec::new(),
            })
            .intervals
            .push(MembershipInterval {
                start_seq: accepted_seq,
                end_seq: None,
                active: false,
            });
    }
    projection.current_epoch = membership_delta.post_commit_epoch;
    projection.last_seq = accepted_seq;
    Ok(projection.clone())
}

fn replay_operation(
    service: &mut HttpDeliveryService,
    operation: PersistedOperation,
) -> Result<(), DurableStoreError> {
    match operation {
        PersistedOperation::PublishMessage {
            target, message, ..
        } => {
            service.publish(target, message)?;
        }
        // KeyPackage lease/reclaim/consume state is rebuilt in the finite wrapper
        // inventory; Darkmatter's core store has no claimed lease state.
        PersistedOperation::PublishKeyPackage { .. } => {}
        PersistedOperation::RevokeDevice { .. } => {}
        PersistedOperation::ClaimKeyPackage { .. }
        | PersistedOperation::ClaimKeyPackages { .. }
        | PersistedOperation::ExpireKeyPackageLease { .. } => {}
    }
    Ok(())
}

fn rebuild_revoked_devices(operations: &[PersistedOperation]) -> BTreeSet<String> {
    operations
        .iter()
        .filter_map(|operation| match operation {
            PersistedOperation::RevokeDevice { device } => Some(DeviceMembership::key(device)),
            _ => None,
        })
        .collect()
}

fn rebuild_key_package_inventory(
    operations: &[PersistedOperation],
) -> HashMap<HttpKeyPackageId, KeyPackageInventoryRecord> {
    let mut inventory = HashMap::new();
    for operation in operations {
        match operation {
            PersistedOperation::PublishKeyPackage { publication } => {
                let record = inventory
                    .entry(publication.key_package_id.clone())
                    .or_insert_with(|| KeyPackageInventoryRecord {
                        key_package_id: publication.key_package_id.clone(),
                        owner: publication.owner.clone(),
                        key_package: publication.key_package.clone(),
                        state: KeyPackageInventoryState::Available,
                        finite_metadata: finite_key_package_metadata(publication),
                    });
                if record.key_package.bytes().is_empty() {
                    record.key_package = publication.key_package.clone();
                }
                if record.finite_metadata.is_none() {
                    record.finite_metadata = finite_key_package_metadata(publication);
                }
            }
            PersistedOperation::ClaimKeyPackage { owner } => {
                mark_next_key_package_claimed(&mut inventory, owner);
            }
            PersistedOperation::ClaimKeyPackages { owners } => {
                for owner in owners {
                    mark_next_key_package_claimed(&mut inventory, owner);
                }
            }
            PersistedOperation::ExpireKeyPackageLease { key_package_id } => {
                if let Some(record) = inventory.get_mut(key_package_id)
                    && record.state == KeyPackageInventoryState::Claimed
                {
                    record.state = KeyPackageInventoryState::Available;
                }
            }
            PersistedOperation::PublishMessage { message, .. } => {
                consume_key_packages_from_persisted_message(&mut inventory, message);
            }
            PersistedOperation::RevokeDevice { .. } => {}
        }
    }
    inventory
}

fn key_package_inventory_cache_matches(
    cached: &HashMap<HttpKeyPackageId, KeyPackageInventoryRecord>,
    rebuilt: &HashMap<HttpKeyPackageId, KeyPackageInventoryRecord>,
) -> bool {
    cached.len() == rebuilt.len()
        && rebuilt.iter().all(|(key_package_id, rebuilt_record)| {
            cached.get(key_package_id).is_some_and(|cached_record| {
                cached_record.owner == rebuilt_record.owner
                    && cached_record.state == rebuilt_record.state
            })
        })
}

fn mark_next_key_package_claimed(
    inventory: &mut HashMap<HttpKeyPackageId, KeyPackageInventoryRecord>,
    owner: &MemberId,
) {
    let selected = inventory
        .iter()
        .filter(|(_, record)| {
            record.owner == *owner && record.state == KeyPackageInventoryState::Available
        })
        .map(|(key_package_id, _)| key_package_id.clone())
        .min_by(|left, right| left.as_slice().cmp(right.as_slice()));
    if let Some(key_package_id) = selected {
        inventory
            .get_mut(&key_package_id)
            .expect("selected KeyPackage must exist before claim")
            .state = KeyPackageInventoryState::Claimed;
    }
}

fn consume_claimed_key_packages_for_commit(
    inventory: &mut HashMap<HttpKeyPackageId, KeyPackageInventoryRecord>,
    request: &SubmitCommitRequest,
) -> Result<Vec<KeyPackageInventoryRecord>, ServerHttpError> {
    let mut changed = Vec::new();
    for add in &request.membership_delta.adds {
        let record = validate_claimed_key_package_for_add(inventory, add)?;
        record.state = KeyPackageInventoryState::Consumed;
        changed.push(record.clone());
    }
    Ok(changed)
}

fn validate_claimed_key_package_for_add<'a>(
    inventory: &'a mut HashMap<HttpKeyPackageId, KeyPackageInventoryRecord>,
    add: &MembershipAddV1,
) -> Result<&'a mut KeyPackageInventoryRecord, ServerHttpError> {
    let key_package_id = HttpKeyPackageId::new(add.key_package_id.as_bytes().to_vec());
    let Some(record) = inventory.get_mut(&key_package_id) else {
        return Err(ServerHttpError::InvalidCommitRequest {
            reason: format!(
                "KeyPackage {} must be published and claimed before a typed commit can add {:?}",
                add.key_package_id, add.device
            ),
        });
    };
    match record.state {
        KeyPackageInventoryState::Claimed => {}
        KeyPackageInventoryState::Available => {
            return Err(ServerHttpError::InvalidCommitRequest {
                reason: format!(
                    "KeyPackage {} must be claimed before a typed commit can add {:?}",
                    add.key_package_id, add.device
                ),
            });
        }
        KeyPackageInventoryState::Consumed => {
            return Err(ServerHttpError::InvalidCommitRequest {
                reason: format!("KeyPackage {} is already consumed", add.key_package_id),
            });
        }
    }

    let expected_owner = member_id_for_device(&add.device)?;
    if record.owner != expected_owner {
        return Err(ServerHttpError::InvalidCommitRequest {
            reason: format!(
                "KeyPackage {} owner does not match added device",
                add.key_package_id
            ),
        });
    }
    let Some(metadata) = &record.finite_metadata else {
        return Err(ServerHttpError::InvalidCommitRequest {
            reason: format!(
                "KeyPackage {} does not contain Finite upload metadata",
                add.key_package_id
            ),
        });
    };
    if metadata.owner != add.device {
        return Err(ServerHttpError::InvalidCommitRequest {
            reason: format!(
                "KeyPackage {} metadata owner does not match added device",
                add.key_package_id
            ),
        });
    }
    if metadata.key_package_ref != add.key_package_ref
        || metadata.key_package_hash != add.key_package_hash
    {
        return Err(ServerHttpError::InvalidCommitRequest {
            reason: format!(
                "KeyPackage {} metadata does not match membership add",
                add.key_package_id
            ),
        });
    }
    Ok(record)
}

fn consume_key_packages_from_persisted_message(
    inventory: &mut HashMap<HttpKeyPackageId, KeyPackageInventoryRecord>,
    message: &TransportMessage,
) {
    let Ok(projection) =
        serde_json::from_slice::<FiniteAccountRoomCommitProjection>(&message.payload)
    else {
        return;
    };
    for add in &projection.membership_delta.adds {
        let key_package_id = HttpKeyPackageId::new(add.key_package_id.as_bytes().to_vec());
        let Ok(owner) = member_id_for_device(&add.device) else {
            continue;
        };
        let record =
            inventory
                .entry(key_package_id.clone())
                .or_insert_with(|| KeyPackageInventoryRecord {
                    key_package_id,
                    owner: owner.clone(),
                    key_package: KeyPackage::new(Vec::new()),
                    state: KeyPackageInventoryState::Claimed,
                    finite_metadata: Some(FiniteKeyPackageMetadata {
                        owner: add.device.clone(),
                        key_package_ref: add.key_package_ref.clone(),
                        key_package_hash: add.key_package_hash.clone(),
                    }),
                });
        if record.owner != owner {
            continue;
        }
        if record.finite_metadata.is_none() {
            record.finite_metadata = Some(FiniteKeyPackageMetadata {
                owner: add.device.clone(),
                key_package_ref: add.key_package_ref.clone(),
                key_package_hash: add.key_package_hash.clone(),
            });
        }
        record.state = KeyPackageInventoryState::Consumed;
    }
}

fn finite_key_package_metadata(
    publication: &HttpKeyPackagePublication,
) -> Option<FiniteKeyPackageMetadata> {
    let request =
        serde_json::from_slice::<UploadKeyPackageRequest>(publication.key_package.bytes()).ok()?;
    if publication.key_package_id.as_slice() != request.key_package_id.as_bytes() {
        return None;
    }
    if member_id_for_device(&request.owner).ok()? != publication.owner {
        return None;
    }
    Some(FiniteKeyPackageMetadata {
        owner: request.owner,
        key_package_ref: request.key_package_ref,
        key_package_hash: request.key_package_hash,
    })
}

fn validate_submit_commit_request(request: &SubmitCommitRequest) -> Result<(), ServerHttpError> {
    request
        .validate_limits()
        .map_err(|error| ServerHttpError::InvalidCommitRequest {
            reason: error.to_string(),
        })?;
    let message_id =
        request
            .envelope
            .message_id()
            .map_err(|error| ServerHttpError::InvalidCommitRequest {
                reason: error.to_string(),
            })?;
    if request.envelope.kind != LogEntryKind::Commit {
        return Err(ServerHttpError::InvalidCommitRequest {
            reason: "commit request envelope must be a commit".to_owned(),
        });
    }
    if request.envelope.room_id != request.room_id {
        return Err(ServerHttpError::InvalidCommitRequest {
            reason: format!(
                "commit envelope room_id {} does not match request room_id {}",
                request.envelope.room_id, request.room_id
            ),
        });
    }
    if request.envelope.epoch != request.expected_epoch {
        return Err(ServerHttpError::InvalidCommitRequest {
            reason: format!(
                "commit envelope epoch {} does not match expected epoch {}",
                request.envelope.epoch, request.expected_epoch
            ),
        });
    }
    if request.envelope.sender != request.sender {
        return Err(ServerHttpError::InvalidCommitRequest {
            reason: "commit envelope sender does not match request sender".to_owned(),
        });
    }
    request
        .membership_delta
        .validate_structure(request.expected_epoch, &message_id)
        .map_err(|error| ServerHttpError::InvalidCommitRequest {
            reason: error.to_string(),
        })?;
    staged_welcomes_by_id(&request.membership_delta, &request.staged_welcomes).map_err(
        |error| ServerHttpError::InvalidCommitRequest {
            reason: error.to_string(),
        },
    )?;
    Ok(())
}

fn validate_append_event_request(request: &AppendEventRequest) -> Result<(), ServerHttpError> {
    request
        .validate_limits()
        .map_err(|error| ServerHttpError::InvalidEventRequest {
            reason: error.to_string(),
        })?;
    if request.envelope.kind == LogEntryKind::Commit {
        return Err(ServerHttpError::InvalidEventRequest {
            reason: "commit events must be submitted through /commits".to_owned(),
        });
    }
    if request.envelope.room_id != request.room_id {
        return Err(ServerHttpError::InvalidEventRequest {
            reason: format!(
                "event envelope room_id {} does not match request room_id {}",
                request.envelope.room_id, request.room_id
            ),
        });
    }
    if request.envelope.sender != request.sender {
        return Err(ServerHttpError::InvalidEventRequest {
            reason: "event envelope sender does not match request sender".to_owned(),
        });
    }
    request
        .envelope
        .message_id()
        .map_err(|error| ServerHttpError::InvalidEventRequest {
            reason: error.to_string(),
        })?;
    Ok(())
}

fn validate_append_ephemeral_activity_request(
    request: &AppendEphemeralActivityRequest,
) -> Result<(), ServerHttpError> {
    request
        .validate_limits()
        .map_err(|error| ServerHttpError::InvalidActivityRequest {
            reason: error.to_string(),
        })?;
    validate_activity_expiry(request.received_at_ms, request.expires_at_ms).map_err(|error| {
        ServerHttpError::InvalidActivityRequest {
            reason: error.to_string(),
        }
    })
}

fn validate_device_liveness_request(
    request: &ObserveDeviceLivenessRequest,
) -> Result<(), ServerHttpError> {
    request.device.validate_limits().map_err(|error| {
        ServerHttpError::InvalidDeviceLivenessRequest {
            reason: error.to_string(),
        }
    })?;
    if request.expires_at_ms <= request.observed_at_ms {
        return Err(ServerHttpError::InvalidDeviceLivenessRequest {
            reason:
                "device_liveness.expires_at_ms must be greater than device_liveness.observed_at_ms"
                    .to_owned(),
        });
    }
    let window = request.expires_at_ms - request.observed_at_ms;
    if window > MAX_DEVICE_LIVENESS_EXPIRY_MILLIS {
        return Err(ServerHttpError::InvalidDeviceLivenessRequest {
            reason: format!(
                "device_liveness.expiry_window_millis has {window} ms, max {MAX_DEVICE_LIVENESS_EXPIRY_MILLIS}"
            ),
        });
    }
    Ok(())
}

fn commit_publish_request(
    request: &SubmitCommitRequest,
    message_id: &str,
) -> Result<PublishMessageRequest, ServerHttpError> {
    let transport_group_id = transport_group_id_for_room(&request.room_id);
    let placeholder_entry = RoomLogEntry {
        room_id: request.room_id.clone(),
        seq: 0,
        message_id: message_id.to_owned(),
        sender: request.sender.clone(),
        kind: LogEntryKind::Commit,
        epoch: request.expected_epoch,
        envelope: request.envelope.clone(),
        idempotency_key: request.idempotency_key.clone(),
    };
    Ok(PublishMessageRequest {
        target: HttpPublishTarget::Group {
            group_id: group_id_for_room(&request.room_id),
            transport_group_id: transport_group_id.clone(),
            commit_admission: Some(HttpCommitAdmission {
                source_epoch: EpochId(request.expected_epoch),
            }),
        },
        message: TransportMessage {
            id: MessageId::new(message_id.as_bytes().to_vec()),
            payload: serde_json::to_vec(&FiniteAccountRoomCommitProjection {
                entry: placeholder_entry,
                membership_delta: request.membership_delta.clone(),
            })
            .map_err(|error| ServerHttpError::ProjectionJson(error.to_string()))?,
            timestamp: Timestamp(0),
            causal_deps: Vec::new(),
            source: TransportSource(HTTP_SERVER_SOURCE.to_owned()),
            envelope: TransportEnvelope::GroupMessage { transport_group_id },
        },
        idempotency_key: Some(format!(
            "commit:{}:{}",
            request.room_id, request.idempotency_key
        )),
    })
}

fn event_publish_request(
    request: &AppendEventRequest,
    message_id: &str,
) -> Result<PublishMessageRequest, ServerHttpError> {
    let transport_group_id = transport_group_id_for_room(&request.room_id);
    let placeholder_entry = RoomLogEntry {
        room_id: request.room_id.clone(),
        seq: 0,
        message_id: message_id.to_owned(),
        sender: request.sender.clone(),
        kind: request.envelope.kind,
        epoch: request.envelope.epoch,
        envelope: request.envelope.clone(),
        idempotency_key: request.idempotency_key.clone(),
    };
    Ok(PublishMessageRequest {
        target: HttpPublishTarget::Group {
            group_id: group_id_for_room(&request.room_id),
            transport_group_id: transport_group_id.clone(),
            commit_admission: None,
        },
        message: TransportMessage {
            id: MessageId::new(message_id.as_bytes().to_vec()),
            payload: serde_json::to_vec(&placeholder_entry)
                .map_err(|error| ServerHttpError::ProjectionJson(error.to_string()))?,
            timestamp: Timestamp(0),
            causal_deps: Vec::new(),
            source: TransportSource(HTTP_SERVER_SOURCE.to_owned()),
            envelope: TransportEnvelope::GroupMessage { transport_group_id },
        },
        idempotency_key: Some(format!(
            "event:{}:{}",
            request.room_id, request.idempotency_key
        )),
    })
}

fn room_log_entry_from_payload(payload: &[u8]) -> Option<RoomLogEntry> {
    if let Ok(projection) = serde_json::from_slice::<FiniteAccountRoomCommitProjection>(payload) {
        return Some(projection.entry);
    }
    serde_json::from_slice(payload).ok()
}

fn released_welcome_records_for_commit(
    request: &SubmitCommitRequest,
    commit_seq: u64,
) -> Result<Vec<WelcomeRecord>, ServerHttpError> {
    let staged = staged_welcomes_by_id(&request.membership_delta, &request.staged_welcomes)
        .map_err(|error| ServerHttpError::InvalidCommitRequest {
            reason: error.to_string(),
        })?;
    request
        .membership_delta
        .adds
        .iter()
        .map(|add| {
            let staged = staged
                .get(&add.welcome_id)
                .expect("validated staged welcome must exist");
            Ok(WelcomeRecord {
                welcome_id: add.welcome_id.clone(),
                room_id: request.room_id.clone(),
                commit_seq,
                recipient: add.device.clone(),
                sender: request.sender.clone(),
                key_package_id: add.key_package_id.clone(),
                join_epoch: request.membership_delta.post_commit_epoch,
                state: WelcomeState::Released,
                lease_token: Some(lease_token_for(&add.welcome_id, &add.device)),
                welcome_payload: staged.welcome_payload.clone(),
                ratchet_tree_payload: staged.ratchet_tree_payload.clone(),
            })
        })
        .collect()
}

fn welcome_publish_request(
    welcome: &WelcomeRecord,
) -> Result<PublishMessageRequest, ServerHttpError> {
    let recipient = member_id_for_device(&welcome.recipient)?;
    Ok(PublishMessageRequest {
        target: HttpPublishTarget::Inbox {
            recipient: recipient.clone(),
        },
        message: TransportMessage {
            id: MessageId::new(welcome.welcome_id.as_bytes().to_vec()),
            payload: serde_json::to_vec(welcome)
                .map_err(|error| ServerHttpError::ProjectionJson(error.to_string()))?,
            timestamp: Timestamp(0),
            causal_deps: Vec::new(),
            source: TransportSource(HTTP_SERVER_SOURCE.to_owned()),
            envelope: TransportEnvelope::Welcome { recipient },
        },
        idempotency_key: Some(format!("welcome:{}", welcome.welcome_id)),
    })
}

fn member_id_for_device(device: &DeviceRef) -> Result<MemberId, ServerHttpError> {
    serde_json::to_vec(device)
        .map(MemberId::new)
        .map_err(|error| ServerHttpError::InvalidCommitRequest {
            reason: error.to_string(),
        })
}

fn finite_device_for_member_id(member_id: &MemberId) -> Option<DeviceRef> {
    serde_json::from_slice(member_id.as_slice()).ok()
}

fn device_for_member_id(member_id: &MemberId) -> Result<DeviceRef, ServerHttpError> {
    serde_json::from_slice(member_id.as_slice()).map_err(|error| {
        ServerHttpError::InvalidGroupSyncRequest {
            reason: format!("requester must encode a Finite DeviceRef: {error}"),
        }
    })
}

fn ensure_device_not_revoked_in(
    revoked_devices: &BTreeSet<String>,
    device: &DeviceRef,
) -> Result<(), ServerHttpError> {
    if revoked_devices.contains(&DeviceMembership::key(device)) {
        Err(ServerHttpError::DeviceRevoked {
            device: device.clone(),
        })
    } else {
        Ok(())
    }
}

fn member_id_is_revoked(member_id: &MemberId, revoked_devices: &BTreeSet<String>) -> bool {
    finite_device_for_member_id(member_id)
        .as_ref()
        .is_some_and(|device| revoked_devices.contains(&DeviceMembership::key(device)))
}

fn ensure_welcome_message_recipient_not_revoked(
    revoked_devices: &BTreeSet<String>,
    message: &TransportMessage,
) -> Result<(), ServerHttpError> {
    let Ok(welcome) = serde_json::from_slice::<WelcomeRecord>(&message.payload) else {
        return Ok(());
    };
    ensure_device_not_revoked_in(revoked_devices, &welcome.recipient)
}

fn group_id_for_room(room_id: &str) -> GroupId {
    GroupId::new(room_id.as_bytes().to_vec())
}

fn room_id_for_group_id(group_id: &GroupId) -> Result<String, ServerHttpError> {
    String::from_utf8(group_id.as_slice().to_vec()).map_err(|error| {
        ServerHttpError::InvalidGroupSyncRequest {
            reason: format!("group_id must be a UTF-8 Finite room_id: {error}"),
        }
    })
}

fn transport_group_id_for_room(room_id: &str) -> Vec<u8> {
    room_id.as_bytes().to_vec()
}

fn initial_room_membership_projection(
    room_id: &str,
    mls_group_id: &str,
    creator: &DeviceRef,
    current_epoch: u64,
    last_seq: HttpSequence,
    membership_complete: bool,
    protocol: RoomProtocol,
) -> HttpRoomMembershipProjection {
    let mut membership = BTreeMap::new();
    membership.insert(
        DeviceMembership::key(creator),
        DeviceMembership {
            device: creator.clone(),
            intervals: vec![MembershipInterval {
                start_seq: 0,
                end_seq: None,
                active: true,
            }],
        },
    );
    HttpRoomMembershipProjection {
        room_id: room_id.to_owned(),
        mls_group_id: mls_group_id.to_owned(),
        current_epoch,
        last_seq,
        status: RoomStatus::Open,
        membership_complete,
        admins: BTreeSet::from([creator.account_id.clone()]),
        departed: BTreeSet::new(),
        protocol,
        membership,
    }
}

fn default_membership_complete() -> bool {
    true
}

fn validate_fanout_id(fanout_id: &str) -> Result<(), ServerHttpError> {
    validate_string_id("fanout_id", fanout_id, MAX_HTTP_FANOUT_ID_BYTES)
}

fn validate_fanout_room_plan(plan: &HttpFanoutRoomPlan) -> Result<(), ServerHttpError> {
    if plan.commit_idempotency_key.is_empty() {
        return Err(ServerHttpError::InvalidFanoutRequest {
            reason: "commit idempotency key must not be empty".to_owned(),
        });
    }
    validate_string_id(
        "commit_idempotency_key",
        &plan.commit_idempotency_key,
        MAX_HTTP_IDEMPOTENCY_KEY_BYTES,
    )
}

fn validate_link_session_id(link_session_id: &str) -> Result<(), ServerHttpError> {
    validate_string_bytes("link_session_id", link_session_id, MAX_OBJECT_ID_BYTES).map_err(
        |error| ServerHttpError::InvalidLinkSessionRequest {
            reason: error.to_string(),
        },
    )
}

fn validate_link_pairing_public_key(pairing_public_key: &str) -> Result<(), ServerHttpError> {
    validate_string_bytes(
        "pairing_public_key",
        pairing_public_key,
        MAX_OBJECT_ID_BYTES,
    )
    .map_err(|error| ServerHttpError::InvalidLinkSessionRequest {
        reason: error.to_string(),
    })
}

fn validate_link_payload(payload: &[u8]) -> Result<(), ServerHttpError> {
    validate_bytes_len(
        "link_session.encrypted_payload",
        payload.len(),
        MAX_LINK_SESSION_PAYLOAD_BYTES,
    )
    .map_err(|error| ServerHttpError::InvalidLinkSessionRequest {
        reason: error.to_string(),
    })
}

fn validate_link_claim_token(claim_token: &str) -> Result<(), ServerHttpError> {
    validate_string_bytes("link_session.claim_token", claim_token, MAX_OBJECT_ID_BYTES).map_err(
        |error| ServerHttpError::InvalidLinkSessionRequest {
            reason: error.to_string(),
        },
    )
}

fn link_session_claim_token(session: &HttpLinkSessionRecord) -> String {
    lease_token_for(
        &session.link_session_id,
        &DeviceRef {
            account_id: "link".to_owned(),
            device_id: session.pairing_public_key.clone(),
        },
    )
}

fn validate_account_room_id(field: &'static str, value: &str) -> Result<(), ServerHttpError> {
    if value.is_empty() || value.len() > MAX_HTTP_ACCOUNT_ROOM_ID_BYTES {
        return Err(ServerHttpError::InvalidAccountRoomRequest {
            reason: format!(
                "{field} must contain between 1 and {MAX_HTTP_ACCOUNT_ROOM_ID_BYTES} bytes"
            ),
        });
    }
    Ok(())
}

fn account_scoped_account_room_record(
    account_id: &str,
    room_id: &str,
    value: &Value,
) -> Result<Option<AccountRoomRecord>, ServerHttpError> {
    let mut record =
        serde_json::from_value::<AccountRoomRecord>(value.clone()).map_err(|error| {
            ServerHttpError::InvalidAccountRoomRequest {
                reason: format!("record must be a Finite account-room record: {error}"),
            }
        })?;
    record
        .validate_limits()
        .map_err(|error| ServerHttpError::InvalidAccountRoomRequest {
            reason: error.to_string(),
        })?;
    if record.room_id != room_id {
        return Err(ServerHttpError::InvalidAccountRoomRequest {
            reason: format!(
                "record room_id {} does not match directory room_id {room_id}",
                record.room_id
            ),
        });
    }

    record
        .devices
        .retain(|device| device.device.account_id == account_id);
    record
        .devices
        .sort_by(|left, right| left.device.device_id.cmp(&right.device.device_id));
    if record.devices.is_empty() {
        return Ok(None);
    }
    record
        .validate_limits()
        .map_err(|error| ServerHttpError::InvalidAccountRoomRequest {
            reason: error.to_string(),
        })?;
    Ok(Some(record))
}

fn validate_string_id(field: &'static str, value: &str, max: usize) -> Result<(), ServerHttpError> {
    if value.is_empty() || value.len() > max {
        return Err(ServerHttpError::InvalidFanoutRequest {
            reason: format!("{field} must contain between 1 and {max} bytes"),
        });
    }
    Ok(())
}

fn claim_key_packages_from_inventory(
    inventory: &mut HashMap<HttpKeyPackageId, KeyPackageInventoryRecord>,
    owners: &[MemberId],
    revoked_devices: &BTreeSet<String>,
) -> Vec<HttpKeyPackageClaim> {
    owners
        .iter()
        .map(|owner| {
            let claimed = if member_id_is_revoked(owner, revoked_devices) {
                None
            } else {
                claim_next_key_package_from_inventory(inventory, owner)
            };
            HttpKeyPackageClaim {
                owner: owner.clone(),
                claimed,
            }
        })
        .collect()
}

fn record_key_package_publication(
    inventory: &mut HashMap<HttpKeyPackageId, KeyPackageInventoryRecord>,
    publication: &HttpKeyPackagePublication,
) -> Result<Option<KeyPackageInventoryRecord>, ServerHttpError> {
    if let Some(record) = inventory.get_mut(&publication.key_package_id) {
        if record.owner != publication.owner || record.key_package != publication.key_package {
            return Err(HttpServerError::ConflictingKeyPackage {
                key_package_id: publication.key_package_id.clone(),
            }
            .into());
        }
        if record.finite_metadata.is_none() {
            record.finite_metadata = finite_key_package_metadata(publication);
            return Ok(Some(record.clone()));
        }
        return Ok(None);
    }

    let unconsumed = inventory
        .values()
        .filter(|record| {
            record.owner == publication.owner
                && matches!(
                    record.state,
                    KeyPackageInventoryState::Available | KeyPackageInventoryState::Claimed
                )
        })
        .count();
    if unconsumed >= MAX_KEY_PACKAGES_PER_DEVICE as usize {
        return Err(HttpServerError::KeyPackageInventoryFull {
            owner: publication.owner.clone(),
            max: MAX_KEY_PACKAGES_PER_DEVICE as usize,
        }
        .into());
    }

    let record = KeyPackageInventoryRecord {
        key_package_id: publication.key_package_id.clone(),
        owner: publication.owner.clone(),
        key_package: publication.key_package.clone(),
        state: KeyPackageInventoryState::Available,
        finite_metadata: finite_key_package_metadata(publication),
    };
    inventory.insert(publication.key_package_id.clone(), record.clone());
    Ok(Some(record))
}

fn claim_next_key_package_from_inventory(
    inventory: &mut HashMap<HttpKeyPackageId, KeyPackageInventoryRecord>,
    owner: &MemberId,
) -> Option<HttpClaimedKeyPackage> {
    let selected = inventory
        .iter()
        .filter(|(_, record)| {
            record.owner == *owner && record.state == KeyPackageInventoryState::Available
        })
        .map(|(key_package_id, _)| key_package_id.clone())
        .min_by(|left, right| left.as_slice().cmp(right.as_slice()));
    let key_package_id = selected?;
    let record = inventory
        .get_mut(&key_package_id)
        .expect("selected KeyPackage must exist before claim");
    record.state = KeyPackageInventoryState::Claimed;
    Some(HttpClaimedKeyPackage {
        key_package_id,
        owner: record.owner.clone(),
        key_package: record.key_package.clone(),
    })
}

fn key_package_claim_inventory_records(
    inventory: &HashMap<HttpKeyPackageId, KeyPackageInventoryRecord>,
    claims: &[HttpKeyPackageClaim],
) -> Vec<KeyPackageInventoryRecord> {
    claims
        .iter()
        .filter_map(|claim| {
            claim
                .claimed
                .as_ref()
                .and_then(|package| inventory.get(&package.key_package_id))
                .cloned()
        })
        .collect()
}

fn validate_key_package_claim_batch(owners: &[MemberId]) -> Result<(), ServerHttpError> {
    if owners.is_empty() || owners.len() > MAX_HTTP_SYNC_PAGE_ENTRIES {
        return Err(ServerHttpError::InvalidKeyPackageClaimBatch {
            actual: owners.len(),
            max: MAX_HTTP_SYNC_PAGE_ENTRIES,
        });
    }

    let mut seen = HashSet::new();
    for owner in owners {
        if !seen.insert(owner) {
            return Err(ServerHttpError::DuplicateKeyPackageClaimOwner {
                owner: owner.clone(),
            });
        }
    }
    Ok(())
}

fn usize_to_u32(field: &'static str, value: usize) -> Result<u32, ServerHttpError> {
    u32::try_from(value)
        .map_err(|_| ServerHttpError::KeyPackageInventoryCountOverflow { field, value })
}

#[derive(Debug, Error)]
pub enum DurableStoreError {
    #[error("SQLite delivery store error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("delivery store JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("persisted delivery operation failed replay: {0}")]
    Replay(#[from] HttpServerError),
}

#[derive(Debug)]
pub enum ServerHttpError {
    Delivery(HttpServerError),
    IdempotencyConflict {
        idempotency_key: String,
    },
    InvalidIdempotencyKey,
    InvalidKeyPackageClaimBatch {
        actual: usize,
        max: usize,
    },
    InvalidKeyPackageLeaseRequest {
        reason: String,
    },
    InvalidDeviceRequest {
        reason: String,
    },
    DeviceRevoked {
        device: DeviceRef,
    },
    InvalidDeviceLivenessRequest {
        reason: String,
    },
    DeviceNotActive {
        device: DeviceRef,
    },
    DuplicateKeyPackageClaimOwner {
        owner: MemberId,
    },
    InventoryConflict {
        key_package_id: HttpKeyPackageId,
    },
    KeyPackageInventoryCountOverflow {
        field: &'static str,
        value: usize,
    },
    CounterOverflow,
    InvalidCommitRequest {
        reason: String,
    },
    InvalidRawCommitImport {
        room_id: String,
        reason: String,
    },
    InvalidEventRequest {
        reason: String,
    },
    DuplicateMessageId {
        message_id: MessageId,
    },
    InvalidActivityRequest {
        reason: String,
    },
    SenderNotActive {
        sender: DeviceRef,
    },
    CommitAuthorityRequired {
        sender: DeviceRef,
    },
    InvalidAdminChange {
        reason: String,
    },
    UnsupportedProtocolVersion {
        requested: u32,
        min: u32,
        max: u32,
    },
    InvalidRepairReport {
        reason: String,
    },
    ReporterNotInInterval {
        reporter: DeviceRef,
        offending_seq: HttpSequence,
    },
    RoomNotOpen {
        room_id: String,
        status: RoomStatus,
    },
    InvalidFanoutRequest {
        reason: String,
    },
    FanoutLimitExceeded {
        fanout_id: String,
        actual: usize,
        max: usize,
    },
    FanoutConflict {
        fanout_id: String,
        reason: String,
    },
    FanoutNotFound {
        fanout_id: String,
    },
    FanoutRoomNotFound {
        fanout_id: String,
        room_id: GroupId,
    },
    InvalidLinkSessionRequest {
        reason: String,
    },
    LinkSessionAlreadyExists {
        link_session_id: String,
    },
    LinkSessionNotFound {
        link_session_id: String,
    },
    LinkSessionConflict {
        link_session_id: String,
        reason: String,
    },
    LinkSessionClosed {
        link_session_id: String,
    },
    LinkSessionNotReady {
        link_session_id: String,
    },
    BadLinkSessionClaimToken {
        link_session_id: String,
    },
    InvalidAccountRoomRequest {
        reason: String,
    },
    AccountRoomBootstrapConflict {
        account_id: String,
        room_id: String,
        reason: String,
    },
    DirectRoomConflict {
        room_id: String,
        reason: String,
    },
    ProjectionJson(String),
    InvalidGroupSyncRequest {
        reason: String,
    },
    InvalidGroupSyncLimit {
        actual: usize,
        max: usize,
    },
    RoomMembershipConflict {
        room_id: String,
        reason: String,
    },
    InvalidAccountRoomListLimit {
        actual: usize,
        max: usize,
    },
    InvalidWelcomeClaimLimit {
        actual: usize,
        max: usize,
    },
    Store(DurableStoreError),
    WelcomeNotFound {
        message_id: MessageId,
    },
}

impl From<HttpServerError> for ServerHttpError {
    fn from(error: HttpServerError) -> Self {
        Self::Delivery(error)
    }
}

impl From<DurableStoreError> for ServerHttpError {
    fn from(error: DurableStoreError) -> Self {
        Self::Store(error)
    }
}

impl IntoResponse for ServerHttpError {
    fn into_response(self) -> Response {
        let (status, kind, error) = match self {
            Self::Delivery(error) => (
                status_for_error(&error),
                kind_for_error(&error).to_owned(),
                error.to_string(),
            ),
            Self::Store(error) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "delivery_store".to_owned(),
                error.to_string(),
            ),
            Self::IdempotencyConflict { idempotency_key } => (
                StatusCode::CONFLICT,
                "idempotency_conflict".to_owned(),
                format!("conflicting request for idempotency key '{idempotency_key}'"),
            ),
            Self::InvalidIdempotencyKey => (
                StatusCode::BAD_REQUEST,
                "invalid_idempotency_key".to_owned(),
                "idempotency key must not be empty".to_owned(),
            ),
            Self::InvalidKeyPackageClaimBatch { actual, max } => (
                StatusCode::BAD_REQUEST,
                "invalid_key_package_claim_batch".to_owned(),
                format!(
                    "KeyPackage claim batch must contain between 1 and {max} owners, got {actual}"
                ),
            ),
            Self::InvalidKeyPackageLeaseRequest { reason } => (
                StatusCode::BAD_REQUEST,
                "invalid_key_package_lease_request".to_owned(),
                reason,
            ),
            Self::InvalidDeviceRequest { reason } => (
                StatusCode::BAD_REQUEST,
                "invalid_device_request".to_owned(),
                reason,
            ),
            Self::DeviceRevoked { device } => (
                StatusCode::FORBIDDEN,
                "device_revoked".to_owned(),
                format!("device {device:?} is revoked"),
            ),
            Self::InvalidDeviceLivenessRequest { reason } => (
                StatusCode::BAD_REQUEST,
                "invalid_device_liveness_request".to_owned(),
                reason,
            ),
            Self::DeviceNotActive { device } => (
                StatusCode::FORBIDDEN,
                "device_not_active".to_owned(),
                format!("device {device:?} is not active in any room"),
            ),
            Self::DuplicateKeyPackageClaimOwner { owner } => (
                StatusCode::BAD_REQUEST,
                "duplicate_key_package_claim_owner".to_owned(),
                format!("KeyPackage claim batch contains duplicate owner {owner:?}"),
            ),
            Self::InventoryConflict { key_package_id } => (
                StatusCode::CONFLICT,
                "key_package_inventory_conflict".to_owned(),
                format!("KeyPackage inventory has a conflicting owner for {key_package_id:?}"),
            ),
            Self::KeyPackageInventoryCountOverflow { field, value } => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "key_package_inventory_count_overflow".to_owned(),
                format!("KeyPackage inventory field {field} does not fit in u32: {value}"),
            ),
            Self::CounterOverflow => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "counter_overflow".to_owned(),
                "counter value does not fit in u32".to_owned(),
            ),
            Self::InvalidCommitRequest { reason } => (
                StatusCode::BAD_REQUEST,
                "invalid_commit_request".to_owned(),
                reason,
            ),
            Self::InvalidRawCommitImport { room_id, reason } => (
                StatusCode::BAD_REQUEST,
                "invalid_raw_commit_import".to_owned(),
                format!("raw commit import for {room_id} is invalid: {reason}"),
            ),
            Self::InvalidEventRequest { reason } => (
                StatusCode::BAD_REQUEST,
                "invalid_event_request".to_owned(),
                reason,
            ),
            Self::DuplicateMessageId { message_id } => (
                StatusCode::CONFLICT,
                "duplicate_message_id".to_owned(),
                format!("duplicate typed event message id {message_id}"),
            ),
            Self::InvalidActivityRequest { reason } => (
                StatusCode::BAD_REQUEST,
                "invalid_activity_request".to_owned(),
                reason,
            ),
            Self::SenderNotActive { sender } => (
                StatusCode::FORBIDDEN,
                "sender_not_active".to_owned(),
                format!("sender {sender:?} is not active in the room"),
            ),
            Self::CommitAuthorityRequired { sender } => (
                StatusCode::FORBIDDEN,
                "commit_authority_required".to_owned(),
                format!(
                    "sender {sender:?} must be a room admin to change another account's membership"
                ),
            ),
            Self::InvalidAdminChange { reason } => (
                StatusCode::BAD_REQUEST,
                "invalid_admin_change".to_owned(),
                reason,
            ),
            Self::UnsupportedProtocolVersion {
                requested,
                min,
                max,
            } => (
                StatusCode::UPGRADE_REQUIRED,
                "unsupported_protocol_version".to_owned(),
                format!(
                    "room protocol version {requested} is outside the supported range {min}..={max}"
                ),
            ),
            Self::InvalidRepairReport { reason } => (
                StatusCode::BAD_REQUEST,
                "invalid_repair_report".to_owned(),
                reason,
            ),
            Self::ReporterNotInInterval {
                reporter,
                offending_seq,
            } => (
                StatusCode::FORBIDDEN,
                "reporter_not_in_interval".to_owned(),
                format!("reporter {reporter:?} was not a member for seq {offending_seq}"),
            ),
            Self::RoomNotOpen { room_id, status } => (
                StatusCode::CONFLICT,
                "room_not_open".to_owned(),
                format!("room {room_id} is {status:?}"),
            ),
            Self::InvalidFanoutRequest { reason } => (
                StatusCode::BAD_REQUEST,
                "invalid_fanout_request".to_owned(),
                reason,
            ),
            Self::FanoutLimitExceeded {
                fanout_id,
                actual,
                max,
            } => (
                StatusCode::TOO_MANY_REQUESTS,
                "fanout_limit_exceeded".to_owned(),
                format!("fanout {fanout_id} has {actual} rooms, max {max}"),
            ),
            Self::FanoutConflict { fanout_id, reason } => (
                StatusCode::CONFLICT,
                "fanout_conflict".to_owned(),
                format!("fanout {fanout_id} conflict: {reason}"),
            ),
            Self::FanoutNotFound { fanout_id } => (
                StatusCode::NOT_FOUND,
                "fanout_not_found".to_owned(),
                format!("fanout {fanout_id} was not found"),
            ),
            Self::FanoutRoomNotFound { fanout_id, room_id } => (
                StatusCode::NOT_FOUND,
                "fanout_room_not_found".to_owned(),
                format!("fanout {fanout_id} has no room {room_id:?}"),
            ),
            Self::InvalidLinkSessionRequest { reason } => (
                StatusCode::BAD_REQUEST,
                "invalid_link_session_request".to_owned(),
                reason,
            ),
            Self::LinkSessionAlreadyExists { link_session_id } => (
                StatusCode::CONFLICT,
                "link_session_already_exists".to_owned(),
                format!("link session {link_session_id} already exists"),
            ),
            Self::LinkSessionNotFound { link_session_id } => (
                StatusCode::NOT_FOUND,
                "link_session_not_found".to_owned(),
                format!("link session {link_session_id} was not found"),
            ),
            Self::LinkSessionConflict {
                link_session_id,
                reason,
            } => (
                StatusCode::CONFLICT,
                "link_session_conflict".to_owned(),
                format!("link session {link_session_id} conflict: {reason}"),
            ),
            Self::LinkSessionClosed { link_session_id } => (
                StatusCode::BAD_REQUEST,
                "link_session_closed".to_owned(),
                format!("link session {link_session_id} is closed"),
            ),
            Self::LinkSessionNotReady { link_session_id } => (
                StatusCode::BAD_REQUEST,
                "link_session_not_ready".to_owned(),
                format!("link session {link_session_id} is not ready"),
            ),
            Self::BadLinkSessionClaimToken { link_session_id } => (
                StatusCode::BAD_REQUEST,
                "bad_link_session_claim_token".to_owned(),
                format!("link session {link_session_id} claim token does not match"),
            ),
            Self::InvalidAccountRoomRequest { reason } => (
                StatusCode::BAD_REQUEST,
                "invalid_account_room_request".to_owned(),
                reason,
            ),
            Self::AccountRoomBootstrapConflict {
                account_id,
                room_id,
                reason,
            } => (
                StatusCode::CONFLICT,
                "account_room_bootstrap_conflict".to_owned(),
                format!("account-room bootstrap conflict for {account_id}/{room_id}: {reason}"),
            ),
            Self::DirectRoomConflict { room_id, reason } => (
                StatusCode::CONFLICT,
                "direct_room_conflict".to_owned(),
                format!("direct-room conflict for {room_id}: {reason}"),
            ),
            Self::ProjectionJson(error) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "finite_projection_json".to_owned(),
                error,
            ),
            Self::InvalidGroupSyncRequest { reason } => (
                StatusCode::BAD_REQUEST,
                "invalid_group_sync_request".to_owned(),
                reason,
            ),
            Self::InvalidGroupSyncLimit { actual, max } => (
                StatusCode::BAD_REQUEST,
                "invalid_group_sync_limit".to_owned(),
                format!("group sync limit must be between 1 and {max}, got {actual}"),
            ),
            Self::RoomMembershipConflict { room_id, reason } => (
                StatusCode::CONFLICT,
                "room_membership_conflict".to_owned(),
                format!("room-membership projection conflict for {room_id}: {reason}"),
            ),
            Self::InvalidAccountRoomListLimit { actual, max } => (
                StatusCode::BAD_REQUEST,
                "invalid_account_room_list_limit".to_owned(),
                format!("account-room list limit must be between 1 and {max}, got {actual}"),
            ),
            Self::InvalidWelcomeClaimLimit { actual, max } => (
                StatusCode::BAD_REQUEST,
                "invalid_welcome_claim_limit".to_owned(),
                format!("welcome claim limit must be between 1 and {max}, got {actual}"),
            ),
            Self::WelcomeNotFound { message_id } => (
                StatusCode::NOT_FOUND,
                "welcome_not_found".to_owned(),
                format!("welcome {message_id} was not claimed"),
            ),
        };
        let body = ErrorResponse { kind, error };
        (status, Json(body)).into_response()
    }
}

fn status_for_error(error: &HttpServerError) -> StatusCode {
    match error {
        HttpServerError::ConflictingMessageId { .. }
        | HttpServerError::StaleEpoch { .. }
        | HttpServerError::ConflictingKeyPackage { .. } => StatusCode::CONFLICT,
        HttpServerError::QueueFull { .. }
        | HttpServerError::GroupLimitExceeded { .. }
        | HttpServerError::InboxLimitExceeded { .. }
        | HttpServerError::KeyPackageInventoryFull { .. } => StatusCode::TOO_MANY_REQUESTS,
        HttpServerError::Empty { .. }
        | HttpServerError::TooLarge { .. }
        | HttpServerError::PublishTargetMismatch
        | HttpServerError::InvalidPageLimit { .. } => StatusCode::BAD_REQUEST,
    }
}

fn kind_for_error(error: &HttpServerError) -> &'static str {
    match error {
        HttpServerError::Empty { .. } => "empty",
        HttpServerError::TooLarge { .. } => "too_large",
        HttpServerError::PublishTargetMismatch => "publish_target_mismatch",
        HttpServerError::ConflictingMessageId { .. } => "conflicting_message_id",
        HttpServerError::StaleEpoch { .. } => "stale_epoch",
        HttpServerError::QueueFull { .. } => "queue_full",
        HttpServerError::GroupLimitExceeded { .. } => "group_limit_exceeded",
        HttpServerError::InboxLimitExceeded { .. } => "inbox_limit_exceeded",
        HttpServerError::InvalidPageLimit { .. } => "invalid_page_limit",
        HttpServerError::ConflictingKeyPackage { .. } => "conflicting_key_package",
        HttpServerError::KeyPackageInventoryFull { .. } => "key_package_inventory_full",
    }
}
