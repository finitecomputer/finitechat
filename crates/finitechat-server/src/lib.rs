use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use cgka_traits::transport::{Timestamp, TransportEnvelope, TransportMessage, TransportSource};
use cgka_traits::{EpochId, GroupId, MemberId, MessageId};
use finitechat_engine::{
    AccountRoomDevice, AccountRoomRecord, AppendEventRequest, CommitAccepted, DeviceMembership,
    EventAccepted, MembershipInterval, SubmitCommitRequest, WelcomeRecord, lease_token_for,
    staged_welcomes_by_id,
};
pub use finitechat_http::{
    AckWelcomeRequest, AckWelcomeResponse, BootstrapAccountRoomRequest,
    BootstrapAccountRoomResponse, ClaimKeyPackageRequest, ClaimKeyPackagesRequest,
    ClaimWelcomesRequest, ErrorResponse, FiniteAccountRoomCommitProjection, GetFanoutRequest,
    GroupSyncRequest, HealthResponse, HttpClaimedWelcome, HttpFanoutPlan, HttpFanoutRoomPlan,
    HttpFanoutRoomState, HttpFanoutRoomStatus, HttpKeyPackageClaim, HttpKeyPackageInventory,
    InboxSyncRequest, KeyPackageInventoryRequest, ListAccountRoomDirectoryRequest,
    ListAccountRoomDirectoryResponse, MarkFanoutDoneRequest, MarkFanoutPreparedRequest,
    PublishKeyPackageResponse, PublishMessageRequest, ReportInvalidCommitRequest,
    ReportInvalidCommitResponse, SaveAccountRoomRequest, SaveAccountRoomResponse,
    SaveFanoutRoomRequest,
};
use finitechat_proto::{
    DeviceRef, LogEntryKind, MembershipDeltaV1, RoomLogEntry, RoomStatus, WelcomeState,
};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;
use transport_http_server::{
    HTTP_SERVER_SOURCE, HttpClaimedKeyPackage, HttpCommitAdmission, HttpDeliveryService,
    HttpKeyPackageId, HttpKeyPackagePublication, HttpPublishReceipt, HttpPublishTarget,
    HttpSequence, HttpServerError, HttpSyncPage, MAX_HTTP_SYNC_PAGE_ENTRIES,
};

const MAX_HTTP_FANOUT_ROOMS: usize = MAX_HTTP_SYNC_PAGE_ENTRIES;
const MAX_HTTP_FANOUT_ID_BYTES: usize = 128;
const MAX_HTTP_IDEMPOTENCY_KEY_BYTES: usize = 128;
const MAX_HTTP_ACCOUNT_ROOM_ID_BYTES: usize = 128;

#[derive(Clone, Debug, Default)]
pub struct HttpServerState {
    service: Arc<Mutex<HttpDeliveryService>>,
    publish_idempotency: Arc<Mutex<HashMap<String, PublishIdempotencyRecord>>>,
    key_package_claim_idempotency: Arc<Mutex<HashMap<String, KeyPackageClaimIdempotencyRecord>>>,
    key_package_inventory: Arc<Mutex<HashMap<HttpKeyPackageId, KeyPackageInventoryRecord>>>,
    fanout_plans: Arc<Mutex<HashMap<String, HttpFanoutPlan>>>,
    account_rooms: Arc<Mutex<BTreeMap<String, BTreeMap<String, Value>>>>,
    room_memberships: Arc<Mutex<BTreeMap<String, HttpRoomMembershipProjection>>>,
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
            fanout_plans: Arc::new(Mutex::new(HashMap::new())),
            account_rooms: Arc::new(Mutex::new(BTreeMap::new())),
            room_memberships: Arc::new(Mutex::new(BTreeMap::new())),
            welcome_claims: Arc::new(Mutex::new(HashMap::new())),
            store: None,
        }
    }

    pub fn from_sqlite_path(path: impl AsRef<Path>) -> Result<Self, DurableStoreError> {
        let store = Arc::new(SqliteHttpDeliveryStore::open(path)?);
        let mut service = HttpDeliveryService::default();
        let operations = store.load_operations()?;
        for operation in operations.iter().cloned() {
            replay_operation(&mut service, operation)?;
        }
        let publish_idempotency = store.load_publish_idempotency()?;
        let key_package_claim_idempotency = store.load_key_package_claim_idempotency()?;
        let key_package_inventory = rebuild_key_package_inventory(&operations);
        if store.load_key_package_inventory()? != key_package_inventory {
            for record in key_package_inventory.values() {
                store.upsert_key_package_inventory(record)?;
            }
        }
        let fanout_plans = store.load_fanout_plans()?;
        let account_rooms = store.load_account_room_directory()?;
        let room_memberships = store.load_room_memberships()?;
        let welcome_claims = store.load_welcome_claims()?;
        Ok(Self {
            service: Arc::new(Mutex::new(service)),
            publish_idempotency: Arc::new(Mutex::new(publish_idempotency)),
            key_package_claim_idempotency: Arc::new(Mutex::new(key_package_claim_idempotency)),
            key_package_inventory: Arc::new(Mutex::new(key_package_inventory)),
            fanout_plans: Arc::new(Mutex::new(fanout_plans)),
            account_rooms: Arc::new(Mutex::new(account_rooms)),
            room_memberships: Arc::new(Mutex::new(room_memberships)),
            welcome_claims: Arc::new(Mutex::new(welcome_claims)),
            store: Some(store),
        })
    }

    fn apply_mutation<R>(
        &self,
        mutation: impl FnOnce(
            &mut HttpDeliveryService,
        ) -> Result<(R, Option<PersistedOperation>), HttpServerError>,
    ) -> Result<R, ServerHttpError> {
        let mut service = self.service.lock().expect("HTTP delivery service mutex");
        let Some(store) = &self.store else {
            let (result, _) = mutation(&mut service)?;
            return Ok(result);
        };

        let mut candidate = service.clone();
        let (result, operation) = mutation(&mut candidate)?;
        if let Some(operation) = operation {
            store.append_operation(&operation)?;
        }
        *service = candidate;
        Ok(result)
    }

    fn publish_message(
        &self,
        request: PublishMessageRequest,
    ) -> Result<HttpPublishReceipt, ServerHttpError> {
        self.validate_raw_commit_import(&request)?;
        let Some(idempotency_key) = request.idempotency_key.clone() else {
            return self.apply_mutation(|service| {
                let receipt = service.publish(request.target.clone(), request.message.clone())?;
                let operation =
                    (!receipt.duplicate).then_some(PersistedOperation::PublishMessage {
                        target: request.target,
                        message: request.message,
                        idempotency_key: None,
                    });
                Ok((receipt, operation))
            });
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

        let mut candidate = service.clone();
        let receipt = candidate.publish(request.target.clone(), request.message.clone())?;
        let operation = (!receipt.duplicate).then_some(PersistedOperation::PublishMessage {
            target: request.target,
            message: request.message,
            idempotency_key: Some(idempotency_key.clone()),
        });
        let record = PublishIdempotencyRecord {
            fingerprint,
            receipt: receipt.clone(),
        };
        if let Some(store) = &self.store {
            store.append_publish_mutation(operation.as_ref(), Some((&idempotency_key, &record)))?;
        }
        *service = candidate;
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

    fn publish_key_package(
        &self,
        publication: HttpKeyPackagePublication,
    ) -> Result<PublishKeyPackageResponse, ServerHttpError> {
        self.apply_mutation(|service| {
            service.publish_key_package(publication.clone())?;
            Ok((
                PublishKeyPackageResponse { published: true },
                Some(PersistedOperation::PublishKeyPackage {
                    publication: publication.clone(),
                }),
            ))
        })?;
        self.record_key_package_publication(&publication)?;
        Ok(PublishKeyPackageResponse { published: true })
    }

    fn claim_key_package(
        &self,
        request: ClaimKeyPackageRequest,
    ) -> Result<Option<HttpClaimedKeyPackage>, ServerHttpError> {
        let claimed = self.apply_mutation(|service| {
            let claimed = service.claim_key_package(&request.owner)?;
            let operation = claimed
                .is_some()
                .then_some(PersistedOperation::ClaimKeyPackage {
                    owner: request.owner,
                });
            Ok((claimed, operation))
        })?;
        self.record_claimed_key_packages(claimed.iter())?;
        Ok(claimed)
    }

    fn claim_key_packages(
        &self,
        request: ClaimKeyPackagesRequest,
    ) -> Result<Vec<HttpKeyPackageClaim>, ServerHttpError> {
        validate_key_package_claim_batch(&request.owners)?;
        let Some(idempotency_key) = request.idempotency_key.clone() else {
            let claims = self.apply_mutation(|service| {
                let claims = claim_key_packages_from_service(service, &request.owners)?;
                let operation = claims
                    .iter()
                    .any(|claim| claim.claimed.is_some())
                    .then_some(PersistedOperation::ClaimKeyPackages {
                        owners: request.owners,
                    });
                Ok((claims, operation))
            })?;
            self.record_claimed_key_packages(
                claims.iter().filter_map(|claim| claim.claimed.as_ref()),
            )?;
            return Ok(claims);
        };

        if idempotency_key.is_empty() {
            return Err(ServerHttpError::InvalidIdempotencyKey);
        }

        let fingerprint = KeyPackageClaimFingerprint {
            owners: request.owners.clone(),
        };
        let mut service = self.service.lock().expect("HTTP delivery service mutex");
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

        let mut candidate = service.clone();
        let claims = claim_key_packages_from_service(&mut candidate, &request.owners)?;
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
            )?;
        }
        *service = candidate;
        idempotency.insert(idempotency_key, record);
        self.record_claimed_key_packages(claims.iter().filter_map(|claim| claim.claimed.as_ref()))?;
        Ok(claims)
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
            }
        }
        Ok(HttpKeyPackageInventory {
            owner: request.owner,
            available: usize_to_u32("available", available)?,
            claimed: usize_to_u32("claimed", claimed)?,
        })
    }

    fn record_key_package_publication(
        &self,
        publication: &HttpKeyPackagePublication,
    ) -> Result<(), ServerHttpError> {
        let mut inventory = self
            .key_package_inventory
            .lock()
            .expect("HTTP KeyPackage inventory mutex");
        let record = inventory
            .entry(publication.key_package_id.clone())
            .or_insert_with(|| KeyPackageInventoryRecord {
                key_package_id: publication.key_package_id.clone(),
                owner: publication.owner.clone(),
                state: KeyPackageInventoryState::Available,
            });
        if record.owner != publication.owner {
            return Err(ServerHttpError::InventoryConflict {
                key_package_id: publication.key_package_id.clone(),
            });
        }
        let record = record.clone();
        if let Some(store) = &self.store {
            store.upsert_key_package_inventory(&record)?;
        }
        Ok(())
    }

    fn record_claimed_key_packages<'a>(
        &self,
        claimed: impl IntoIterator<Item = &'a HttpClaimedKeyPackage>,
    ) -> Result<(), ServerHttpError> {
        let mut inventory = self
            .key_package_inventory
            .lock()
            .expect("HTTP KeyPackage inventory mutex");
        let mut changed = Vec::new();
        for package in claimed {
            let record = inventory
                .entry(package.key_package_id.clone())
                .or_insert_with(|| KeyPackageInventoryRecord {
                    key_package_id: package.key_package_id.clone(),
                    owner: package.owner.clone(),
                    state: KeyPackageInventoryState::Available,
                });
            if record.owner != package.owner {
                return Err(ServerHttpError::InventoryConflict {
                    key_package_id: package.key_package_id.clone(),
                });
            }
            record.state = KeyPackageInventoryState::Claimed;
            changed.push(record.clone());
        }
        if let Some(store) = &self.store {
            for record in changed {
                store.upsert_key_package_inventory(&record)?;
            }
        }
        Ok(())
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

    fn record_finite_commit_projection(
        &self,
        request: &PublishMessageRequest,
        accepted_seq: HttpSequence,
    ) -> Result<(), ServerHttpError> {
        let Ok(payload) =
            serde_json::from_slice::<FiniteAccountRoomCommitProjection>(&request.message.payload)
        else {
            return Ok(());
        };
        if !matches!(&request.target, HttpPublishTarget::Group { .. })
            || request.message.id.as_slice() != payload.entry.message_id.as_bytes()
            || payload.entry.kind != LogEntryKind::Commit
            || payload.entry.envelope.kind != LogEntryKind::Commit
            || payload.entry.envelope.room_id != payload.entry.room_id
            || payload
                .membership_delta
                .validate_structure(payload.entry.epoch, &payload.entry.message_id)
                .is_err()
        {
            return Ok(());
        }

        let room_id = payload.entry.room_id.clone();
        let mls_group_id = payload.entry.envelope.mls_group_id.clone();
        let current_epoch = payload.membership_delta.post_commit_epoch;
        self.record_account_room_membership_delta(
            &room_id,
            &mls_group_id,
            current_epoch,
            &payload.membership_delta,
            accepted_seq,
        )?;
        self.record_room_membership_delta(
            &room_id,
            &mls_group_id,
            &payload.entry.sender,
            payload.entry.epoch,
            &payload.membership_delta,
            accepted_seq,
        )
    }

    fn record_room_log_entry_observation(
        &self,
        request: &PublishMessageRequest,
        accepted_seq: HttpSequence,
    ) -> Result<(), ServerHttpError> {
        if !matches!(&request.target, HttpPublishTarget::Group { .. }) {
            return Ok(());
        }
        let has_membership_delta =
            serde_json::from_slice::<FiniteAccountRoomCommitProjection>(&request.message.payload)
                .is_ok();
        let Some(entry) = room_log_entry_from_payload(&request.message.payload) else {
            return Ok(());
        };
        if request.message.id.as_slice() != entry.message_id.as_bytes()
            || entry.envelope.room_id != entry.room_id
        {
            return Ok(());
        }

        let mut rooms = self
            .room_memberships
            .lock()
            .expect("HTTP room-membership mutex");
        let Some(projection) = rooms.get_mut(&entry.room_id) else {
            return Ok(());
        };
        if projection.mls_group_id != entry.envelope.mls_group_id {
            return Ok(());
        }

        let mut changed = false;
        if entry.kind == LogEntryKind::Commit {
            let observed_epoch = entry.epoch.saturating_add(1);
            if projection.current_epoch < observed_epoch {
                projection.current_epoch = observed_epoch;
                changed = true;
            }
            if !has_membership_delta && projection.membership_complete {
                projection.membership_complete = false;
                changed = true;
            }
        }
        if projection.last_seq < accepted_seq {
            projection.last_seq = accepted_seq;
            changed = true;
        }
        if !changed {
            return Ok(());
        }
        let projection = projection.clone();
        drop(rooms);

        if let Some(store) = &self.store {
            store.upsert_room_membership(&projection)?;
        }
        Ok(())
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

        let mut candidate_service = service.clone();
        let mut candidate_publish_idempotency = publish_idempotency.clone();
        let mut candidate_account_rooms = account_rooms.clone();
        let mut candidate_room_memberships = room_memberships.clone();

        let (receipt, commit_mutation) = publish_request_to_candidate(
            &mut candidate_service,
            &mut candidate_publish_idempotency,
            commit_publish,
        )?;
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

        let welcomes = released_welcome_records_for_commit(&request, receipt.seq)?;
        let mut publish_mutations = commit_mutation.into_iter().collect::<Vec<_>>();
        for welcome in &welcomes {
            let (_, mutation) = publish_request_to_candidate(
                &mut candidate_service,
                &mut candidate_publish_idempotency,
                welcome_publish_request(welcome)?,
            )?;
            publish_mutations.extend(mutation);
        }

        if let Some(store) = &self.store {
            store.append_submit_commit_mutation(
                &publish_mutations,
                &account_room_mutation,
                &room_membership_projection,
            )?;
        }

        *service = candidate_service;
        *publish_idempotency = candidate_publish_idempotency;
        *account_rooms = candidate_account_rooms;
        *room_memberships = candidate_room_memberships;
        drop(service);
        drop(publish_idempotency);
        drop(account_rooms);
        drop(room_memberships);

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
        Ok(())
    }

    fn append_event(&self, request: AppendEventRequest) -> Result<EventAccepted, ServerHttpError> {
        validate_append_event_request(&request)?;
        let message_id = request.envelope.message_id().map_err(|error| {
            ServerHttpError::InvalidEventRequest {
                reason: error.to_string(),
            }
        })?;
        {
            let rooms = self
                .room_memberships
                .lock()
                .expect("HTTP room-membership mutex");
            let projection = rooms.get(&request.room_id).ok_or_else(|| {
                ServerHttpError::RoomMembershipConflict {
                    room_id: request.room_id.clone(),
                    reason: "typed event requires a room-membership projection".to_owned(),
                }
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
                    sender: request.sender,
                });
            }
        }

        let event_publish = event_publish_request(&request, &message_id)?;
        let receipt = self.publish_message(event_publish)?;
        self.record_room_event_acceptance(&request.room_id, receipt.seq)?;
        Ok(EventAccepted {
            seq: receipt.seq,
            message_id,
        })
    }

    fn record_room_event_acceptance(
        &self,
        room_id: &str,
        accepted_seq: HttpSequence,
    ) -> Result<(), ServerHttpError> {
        let mut rooms = self
            .room_memberships
            .lock()
            .expect("HTTP room-membership mutex");
        let Some(projection) = rooms.get_mut(room_id) else {
            return Ok(());
        };
        if projection.last_seq >= accepted_seq {
            return Ok(());
        }
        projection.last_seq = accepted_seq;
        let projection = projection.clone();
        drop(rooms);

        if let Some(store) = &self.store {
            store.upsert_room_membership(&projection)?;
        }
        Ok(())
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
        let mut activation_message = None;
        let mut claims = self
            .welcome_claims
            .lock()
            .expect("HTTP welcome claims mutex");
        let Some(record) = claims.get_mut(&request.message_id) else {
            return Err(ServerHttpError::WelcomeNotFound {
                message_id: request.message_id,
            });
        };
        let terminal_state = if request.activated {
            WelcomeClaimState::Acked
        } else {
            WelcomeClaimState::Failed
        };
        match (record.state, terminal_state) {
            (WelcomeClaimState::Claimed, _) => {
                record.state = terminal_state;
                if let Some(store) = &self.store {
                    store.upsert_welcome_claim(record)?;
                }
                if request.activated {
                    activation_message = Some(record.message.clone());
                }
            }
            (current, wanted) if current == wanted => {
                if request.activated {
                    activation_message = Some(record.message.clone());
                }
            }
            (current, wanted) => {
                return Err(ServerHttpError::WelcomeAckConflict {
                    message_id: request.message_id,
                    current,
                    wanted,
                });
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

    fn sync_group(&self, request: GroupSyncRequest) -> Result<HttpSyncPage, ServerHttpError> {
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
        .route("/messages", post(publish_message))
        .route("/events", post(append_event))
        .route("/commits", post(submit_commit))
        .route("/sync/group", post(sync_group))
        .route("/sync/inbox", post(sync_inbox))
        .route("/key-packages", post(publish_key_package))
        .route("/key-packages/inventory", post(key_package_inventory))
        .route("/key-packages/claim", post(claim_key_package))
        .route("/key-packages/claims", post(claim_key_packages))
        .route("/fanouts/get", post(get_fanout))
        .route("/fanouts/rooms", post(save_fanout_room))
        .route("/fanouts/rooms/prepared", post(mark_fanout_prepared))
        .route("/fanouts/rooms/done", post(mark_fanout_done))
        .route("/account-rooms/bootstrap", post(bootstrap_account_room))
        .route("/account-rooms", post(save_account_room))
        .route("/account-rooms/list", post(list_account_rooms))
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

async fn publish_message(
    State(state): State<HttpServerState>,
    Json(request): Json<PublishMessageRequest>,
) -> Result<Json<HttpPublishReceipt>, ServerHttpError> {
    let receipt = state.publish_message(request.clone())?;
    state.record_finite_commit_projection(&request, receipt.seq)?;
    state.record_room_log_entry_observation(&request, receipt.seq)?;
    Ok(Json(receipt))
}

async fn append_event(
    State(state): State<HttpServerState>,
    Json(request): Json<AppendEventRequest>,
) -> Result<Json<EventAccepted>, ServerHttpError> {
    Ok(Json(state.append_event(request)?))
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
    let service = state.service.lock().expect("HTTP delivery service mutex");
    let page = service.sync_inbox(&request.recipient, request.after_seq, request.limit)?;
    Ok(Json(page))
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
    ClaimKeyPackage {
        owner: MemberId,
    },
    ClaimKeyPackages {
        owners: Vec<MemberId>,
    },
}

impl PersistedOperation {
    fn kind(&self) -> &'static str {
        match self {
            Self::PublishMessage { .. } => "publish_message",
            Self::PublishKeyPackage { .. } => "publish_key_package",
            Self::ClaimKeyPackage { .. } => "claim_key_package",
            Self::ClaimKeyPackages { .. } => "claim_key_packages",
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
    state: KeyPackageInventoryState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum KeyPackageInventoryState {
    Available,
    Claimed,
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
    Failed,
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

#[derive(Clone, Debug)]
struct SqliteHttpDeliveryStore {
    path: Arc<PathBuf>,
}

impl SqliteHttpDeliveryStore {
    fn open(path: impl AsRef<Path>) -> Result<Self, rusqlite::Error> {
        let store = Self {
            path: Arc::new(path.as_ref().to_owned()),
        };
        let conn = store.connection()?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS http_delivery_ops (
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
            CREATE TABLE IF NOT EXISTS http_welcome_claims (
                message_id_json TEXT PRIMARY KEY,
                recipient_json TEXT NOT NULL,
                seq INTEGER NOT NULL,
                message_json TEXT NOT NULL,
                state_json TEXT NOT NULL
            );",
        )?;
        Ok(store)
    }

    fn append_operation(&self, operation: &PersistedOperation) -> Result<(), DurableStoreError> {
        let body_json = serde_json::to_string(operation)?;
        let conn = self.connection()?;
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
        let mut conn = self.connection()?;
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
    ) -> Result<(), DurableStoreError> {
        let mut conn = self.connection()?;
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
        transaction.commit()?;
        Ok(())
    }

    fn append_key_package_claim_mutation(
        &self,
        operation: Option<&PersistedOperation>,
        idempotency: Option<(&str, &KeyPackageClaimIdempotencyRecord)>,
    ) -> Result<(), DurableStoreError> {
        let mut conn = self.connection()?;
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
        transaction.commit()?;
        Ok(())
    }

    fn load_operations(&self) -> Result<Vec<PersistedOperation>, DurableStoreError> {
        let conn = self.connection()?;
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
        let conn = self.connection()?;
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
        let conn = self.connection()?;
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
        let conn = self.connection()?;
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
        let conn = self.connection()?;
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
                    state: serde_json::from_str(&state_json)?,
                },
            );
        }
        Ok(inventory)
    }

    fn upsert_fanout_plan(&self, plan: &HttpFanoutPlan) -> Result<(), DurableStoreError> {
        let conn = self.connection()?;
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
        let conn = self.connection()?;
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

    fn upsert_account_room(
        &self,
        record: &AccountRoomDirectoryRecord,
    ) -> Result<(), DurableStoreError> {
        let conn = self.connection()?;
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
        let conn = self.connection()?;
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
        let conn = self.connection()?;
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
        let conn = self.connection()?;
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
        let mut conn = self.connection()?;
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
        let conn = self.connection()?;
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

    fn upsert_welcome_claim(&self, record: &WelcomeClaimRecord) -> Result<(), DurableStoreError> {
        let conn = self.connection()?;
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
        let conn = self.connection()?;
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

    fn connection(&self) -> Result<Connection, rusqlite::Error> {
        Connection::open(&*self.path)
    }
}

fn publish_request_to_candidate(
    service: &mut HttpDeliveryService,
    idempotency: &mut HashMap<String, PublishIdempotencyRecord>,
    request: PublishMessageRequest,
) -> Result<(HttpPublishReceipt, Option<PublishMutation>), ServerHttpError> {
    let Some(idempotency_key) = request.idempotency_key.clone() else {
        let receipt = service.publish(request.target, request.message)?;
        return Ok((receipt, None));
    };
    if idempotency_key.is_empty() {
        return Err(ServerHttpError::InvalidIdempotencyKey);
    }

    let fingerprint = PublishMessageFingerprint::from_request(&request);
    if let Some(record) = idempotency.get(&idempotency_key) {
        if record.fingerprint == fingerprint {
            return Ok((record.receipt.clone(), None));
        }
        return Err(ServerHttpError::IdempotencyConflict { idempotency_key });
    }

    let receipt = service.publish(request.target.clone(), request.message.clone())?;
    let operation = (!receipt.duplicate).then_some(PersistedOperation::PublishMessage {
        target: request.target,
        message: request.message,
        idempotency_key: Some(idempotency_key.clone()),
    });
    let record = PublishIdempotencyRecord {
        fingerprint,
        receipt: receipt.clone(),
    };
    idempotency.insert(idempotency_key.clone(), record.clone());

    Ok((
        receipt,
        Some(PublishMutation {
            operation,
            idempotency_key,
            record,
        }),
    ))
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
        PersistedOperation::PublishKeyPackage { publication } => {
            service.publish_key_package(publication)?;
        }
        PersistedOperation::ClaimKeyPackage { owner } => {
            service.claim_key_package(&owner)?;
        }
        PersistedOperation::ClaimKeyPackages { owners } => {
            for owner in owners {
                service.claim_key_package(&owner)?;
            }
        }
    }
    Ok(())
}

fn rebuild_key_package_inventory(
    operations: &[PersistedOperation],
) -> HashMap<HttpKeyPackageId, KeyPackageInventoryRecord> {
    let mut inventory = HashMap::new();
    for operation in operations {
        match operation {
            PersistedOperation::PublishKeyPackage { publication } => {
                inventory
                    .entry(publication.key_package_id.clone())
                    .or_insert_with(|| KeyPackageInventoryRecord {
                        key_package_id: publication.key_package_id.clone(),
                        owner: publication.owner.clone(),
                        state: KeyPackageInventoryState::Available,
                    });
            }
            PersistedOperation::ClaimKeyPackage { owner } => {
                mark_next_key_package_claimed(&mut inventory, owner);
            }
            PersistedOperation::ClaimKeyPackages { owners } => {
                for owner in owners {
                    mark_next_key_package_claimed(&mut inventory, owner);
                }
            }
            PersistedOperation::PublishMessage { .. } => {}
        }
    }
    inventory
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

fn device_for_member_id(member_id: &MemberId) -> Result<DeviceRef, ServerHttpError> {
    serde_json::from_slice(member_id.as_slice()).map_err(|error| {
        ServerHttpError::InvalidGroupSyncRequest {
            reason: format!("requester must encode a Finite DeviceRef: {error}"),
        }
    })
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

fn claim_key_packages_from_service(
    service: &mut HttpDeliveryService,
    owners: &[MemberId],
) -> Result<Vec<HttpKeyPackageClaim>, HttpServerError> {
    owners
        .iter()
        .map(|owner| {
            let claimed = service.claim_key_package(owner)?;
            Ok(HttpKeyPackageClaim {
                owner: owner.clone(),
                claimed,
            })
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
    SenderNotActive {
        sender: DeviceRef,
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
    InvalidAccountRoomRequest {
        reason: String,
    },
    AccountRoomBootstrapConflict {
        account_id: String,
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
    WelcomeAckConflict {
        message_id: MessageId,
        current: WelcomeClaimState,
        wanted: WelcomeClaimState,
    },
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
            Self::SenderNotActive { sender } => (
                StatusCode::FORBIDDEN,
                "sender_not_active".to_owned(),
                format!("sender {sender:?} is not active in the room"),
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
            Self::WelcomeAckConflict {
                message_id,
                current,
                wanted,
            } => (
                StatusCode::CONFLICT,
                "welcome_ack_conflict".to_owned(),
                format!("welcome {message_id} is already {current:?}; cannot ack as {wanted:?}"),
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
