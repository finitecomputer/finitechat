use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use finitechat_client::{
    AppliedLogEntry, ClientError, ClientStoreError, CreateRoomInviteParams, FiniteChatDevice,
    FiniteChatDeviceConfig, HttpRuntimeDelivery, ReqwestHttpRuntimeTransport, RuntimeDelivery,
    RuntimeSyncOptions, SqliteClientStore, SqliteClientStoreOptions, StoredAppEvent,
    StoredAppMessage, StoredAppRoom, StoredAppRoomState, accept_pending_invite_joins,
    create_room_invite, finalize_invited_room, generate_account_secret, run_room_server_sync_tick,
    run_runtime_sync_tick, submit_invite_join_request,
};
use finitechat_hermes::{HermesAttachmentKindV1, HermesAttachmentV1, HermesMessagePayloadV1};
use finitechat_http::{SyncHintEvent, SyncStreamRequest, SyncWaitInvite, SyncWaitRoom};
use finitechat_mls::{NOSTR_SECRET_KEY_BYTES, NostrSecretKey};
use finitechat_proto::{
    ChatReactionV1, CreateRoomRequest, DecryptedApplicationEventV1, DeviceRef, DurableAppEventKind,
    InviteCodeV1, ListAccountRoomsRequest, MAX_INVITE_DISPLAY_NAME_BYTES, RoomProtocol,
    invite_current_pin, npub_decode, npub_encode,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const ACCOUNT_SECRET_FILE: &str = "account-secret.hex";
const CLIENT_STORE_FILE: &str = "client.sqlite3";
const LEGACY_APP_MESSAGES_FILE: &str = "app-messages.json";
const MAX_APP_MESSAGES: usize = 5_000;
const MAX_APP_MESSAGES_U32: u32 = 5_000;
const DEFAULT_KEY_PACKAGE_TARGET_AVAILABLE: u32 = 2;
const DEFAULT_MAX_SYNC_PAGES_PER_ROOM: u32 = 16;
const DEFAULT_CREDENTIAL_VALIDITY_SECONDS: u64 = 10 * 365 * 24 * 60 * 60;
const DEFAULT_INVITE_TTL_MS: u64 = 15 * 60 * 1000;
const DEFAULT_INVITE_MAX_JOINS: u32 = 32;
const DEFAULT_APP_UPDATE_WAIT_MILLIS: u64 = 30_000;
const MIN_APP_UPDATE_WAIT_MILLIS: u64 = 1_000;
const MAX_APP_UPDATE_WAIT_MILLIS: u64 = 60_000;

uniffi::setup_scaffolding!();

#[derive(Debug, Error, uniffi::Error)]
pub enum FiniteChatCoreError {
    #[error("filesystem error: {reason}")]
    Filesystem { reason: String },
    #[error("invalid account secret")]
    InvalidAccountSecret,
    #[error("client error: {reason}")]
    Client { reason: String },
    #[error("delivery error: {reason}")]
    Delivery { reason: String },
    #[error("store error: {reason}")]
    Store { reason: String },
    #[error("invite error: {reason}")]
    Invite { reason: String },
    #[error("lock poisoned")]
    LockPoisoned,
}

#[derive(Clone, Debug, Serialize, Deserialize, uniffi::Record)]
pub struct OpenOptions {
    pub data_dir: String,
    pub server_url: String,
    pub device_id: String,
    pub account_secret_hex: Option<String>,
    pub now_unix_seconds: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct Identity {
    pub account_id: String,
    pub device_id: String,
    pub account_secret_hex: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct BootstrapRoomResult {
    pub room_id: String,
    pub mls_group_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct InviteResult {
    pub invite_url: String,
    pub pin: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct JoinRequestResult {
    pub request_id: String,
    pub key_package_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct AcceptInvitesResult {
    pub accepted: Vec<String>,
    pub rejected: Vec<String>,
    pub rejected_reason: Option<String>,
    pub deferred_pending_commit: bool,
    pub total_requests: u32,
    pub resolved_requests: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct ChatReactionSummary {
    pub emoji: String,
    pub count: u32,
    pub reacted_by_me: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct ChatReadReceiptSummary {
    pub delivered_count: u32,
    pub read_count: u32,
    pub display_text: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
pub enum ChatMediaKind {
    Image,
    VoiceNote,
    Video,
    File,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct ChatMediaAttachment {
    pub attachment_id: String,
    pub url: Option<String>,
    pub mime_type: String,
    pub filename: String,
    pub kind: ChatMediaKind,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub local_path: Option<String>,
    /// Integer progress in 0..=1000. Kept integral so the FFI-visible
    /// projection stays Eq/Hash-friendly and deterministic across platforms.
    pub upload_progress_per_mille: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
pub enum MessageDeliveryState {
    Pending,
    #[default]
    Sent,
    Failed {
        reason: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct ChatMessage {
    pub room_id: String,
    pub seq: u64,
    pub message_id: String,
    #[serde(default)]
    pub conversation_id: Option<String>,
    pub sender_account_id: String,
    pub sender_device_id: String,
    #[serde(default)]
    pub sender_display_name: String,
    #[serde(default)]
    pub sender_npub: Option<String>,
    pub text: String,
    #[serde(default)]
    pub display_content: String,
    pub payload: Vec<u8>,
    #[serde(default)]
    pub reply_to_message_id: Option<String>,
    #[serde(default)]
    pub is_mine: bool,
    #[serde(default)]
    pub delivery: MessageDeliveryState,
    #[serde(default)]
    pub reactions: Vec<ChatReactionSummary>,
    #[serde(default)]
    pub media: Vec<ChatMediaAttachment>,
    #[serde(default)]
    pub read_receipt: Option<ChatReadReceiptSummary>,
    #[serde(default)]
    pub display_timestamp: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct SyncResult {
    pub uploaded_key_packages: u32,
    pub claimed_welcomes: u32,
    pub activated_welcome_acks_sent: u32,
    pub sync_pages: u32,
    pub messages: Vec<ChatMessage>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
pub enum AppRoomState {
    Connected,
    WaitingForApproval,
    Joining,
    NeedsAttention,
    Offline,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct AppRoomSummary {
    pub room_id: String,
    pub display_name: String,
    pub state: AppRoomState,
    pub status: String,
    pub last_message_preview: String,
    pub unread_count: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct AppInviteState {
    pub room_id: String,
    pub invite_url: String,
    pub pin: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct AppProfileSummary {
    pub account_id: String,
    pub npub: String,
    pub display_name: String,
    pub about: Option<String>,
    pub picture: Option<String>,
    pub stale: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct AppDeviceSummary {
    pub account_id: String,
    pub device_id: String,
    pub active: bool,
    pub current_device: bool,
    pub revoked: bool,
    pub room_count: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, uniffi::Record)]
pub struct AppState {
    pub rev: u64,
    pub identity: Identity,
    pub rooms: Vec<AppRoomSummary>,
    pub selected_room_id: Option<String>,
    pub active_invite: Option<AppInviteState>,
    pub active_profile_id: Option<String>,
    pub status: String,
    pub toast: Option<String>,
    pub messages: Vec<ChatMessage>,
    pub profiles: Vec<AppProfileSummary>,
    pub devices: Vec<AppDeviceSummary>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, uniffi::Enum)]
pub enum AppAction {
    StartRuntime,
    StopRuntime,
    OpenRoom {
        room_id: String,
    },
    CreateRoom {
        display_name: String,
    },
    CreateInvite {
        room_id: String,
    },
    ScanTarget {
        value: String,
    },
    SubmitInvitePin {
        pending_room_id: String,
        pin: String,
    },
    SendMessage {
        room_id: String,
        text: String,
    },
    RetryRoom {
        room_id: String,
    },
    RefreshDevices,
    RevokeDevice {
        account_id: String,
        device_id: String,
    },
}

struct CoreState {
    data_dir: PathBuf,
    server_url: String,
    account_secret: NostrSecretKey,
    config: FiniteChatDeviceConfig,
    store: SqliteClientStore,
    device: FiniteChatDevice,
}

#[derive(uniffi::Object)]
pub struct FiniteChatCore {
    state: Mutex<CoreState>,
}

#[derive(uniffi::Object)]
pub struct FiniteChatRuntime {
    state: Mutex<AppRuntimeState>,
}

struct AppRuntimeState {
    core: CoreState,
    app: AppState,
    pending_invites: BTreeMap<String, PendingInvite>,
    owned_invites: BTreeMap<String, String>,
    invite_watch_marks: BTreeMap<String, InviteWatchMark>,
    profile_cache: BTreeMap<String, AppProfileSummary>,
    revoked_devices: BTreeSet<String>,
}

#[derive(Clone, Debug)]
struct PendingInvite {
    invite_url: String,
}

#[derive(Clone, Copy, Debug, Default)]
struct InviteWatchMark {
    requests: u32,
    resolved: u32,
}

#[derive(Clone, Debug)]
struct AppRuntimeWaitPlan {
    server_url: String,
    request: SyncStreamRequest,
}

#[uniffi::export]
impl FiniteChatCore {
    #[uniffi::constructor]
    pub fn open(options: OpenOptions) -> Result<Arc<Self>, FiniteChatCoreError> {
        let state = CoreState::open(options)?;
        Ok(Arc::new(Self {
            state: Mutex::new(state),
        }))
    }

    pub fn identity(&self) -> Result<Identity, FiniteChatCoreError> {
        let state = self.lock()?;
        Ok(state.identity())
    }

    pub fn bootstrap_room(
        &self,
        room_id: String,
        display_name: Option<String>,
    ) -> Result<BootstrapRoomResult, FiniteChatCoreError> {
        let mut state = self.lock()?;
        state.bootstrap_room(&room_id, display_name)
    }

    pub fn create_invite(
        &self,
        room_id: String,
        display_name: Option<String>,
    ) -> Result<InviteResult, FiniteChatCoreError> {
        let mut state = self.lock()?;
        state.create_invite(&room_id, display_name)
    }

    pub fn current_invite_pin(&self, invite_url: String) -> Result<String, FiniteChatCoreError> {
        let state = self.lock()?;
        let code = parse_invite(&invite_url)?;
        Ok(invite_current_pin(
            &code.invite_token,
            state.now_unix_seconds()?,
        ))
    }

    pub fn join_invite(
        &self,
        invite_url: String,
        pin: String,
        display_name: Option<String>,
    ) -> Result<JoinRequestResult, FiniteChatCoreError> {
        let mut state = self.lock()?;
        state.join_invite(&invite_url, &pin, display_name)
    }

    pub fn accept_invite_joins(
        &self,
        invite_url: String,
    ) -> Result<AcceptInvitesResult, FiniteChatCoreError> {
        let mut state = self.lock()?;
        state.accept_invite_joins(&invite_url)
    }

    pub fn finalize_invite(&self, invite_url: String) -> Result<(), FiniteChatCoreError> {
        let mut state = self.lock()?;
        state.finalize_invite(&invite_url)
    }

    pub fn send_text(
        &self,
        room_id: String,
        text: String,
    ) -> Result<SyncResult, FiniteChatCoreError> {
        let mut state = self.lock()?;
        state.send_text(&room_id, &text)
    }

    pub fn sync(&self) -> Result<SyncResult, FiniteChatCoreError> {
        let mut state = self.lock()?;
        state.sync()
    }
}

impl FiniteChatCore {
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, CoreState>, FiniteChatCoreError> {
        self.state
            .lock()
            .map_err(|_| FiniteChatCoreError::LockPoisoned)
    }
}

#[uniffi::export]
impl FiniteChatRuntime {
    #[uniffi::constructor]
    pub fn open(options: OpenOptions) -> Result<Arc<Self>, FiniteChatCoreError> {
        let core = CoreState::open(options)?;
        Ok(Arc::new(Self {
            state: Mutex::new(AppRuntimeState::new(core)?),
        }))
    }

    pub fn state(&self) -> Result<AppState, FiniteChatCoreError> {
        let state = self
            .state
            .lock()
            .map_err(|_| FiniteChatCoreError::LockPoisoned)?;
        Ok(state.app.clone())
    }

    pub fn dispatch(&self, action: AppAction) -> Result<AppState, FiniteChatCoreError> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| FiniteChatCoreError::LockPoisoned)?;
        state.dispatch(action)?;
        Ok(state.app.clone())
    }

    pub fn wait_for_update(&self, timeout_millis: u64) -> Result<AppState, FiniteChatCoreError> {
        let plan = {
            let state = self
                .state
                .lock()
                .map_err(|_| FiniteChatCoreError::LockPoisoned)?;
            state.wait_plan(timeout_millis)
        };

        let event = {
            let mut delivery = delivery_for(&plan.server_url);
            let mut stream = delivery.sync_stream(&plan.request).map_err(runtime_error)?;
            stream.next_hint().map_err(runtime_error)?
        };

        let mut state = self
            .state
            .lock()
            .map_err(|_| FiniteChatCoreError::LockPoisoned)?;
        state.apply_sync_hint(event);
        state.runtime_tick()?;
        state.bump_rev();
        Ok(state.app.clone())
    }
}

impl AppRuntimeState {
    fn new(mut core: CoreState) -> Result<Self, FiniteChatCoreError> {
        let identity = core.identity();
        let owner = core.device.device_ref().clone();
        migrate_legacy_app_messages(&mut core, &owner)?;
        let stored_messages = core
            .store
            .load_app_messages(&owner, MAX_APP_MESSAGES_U32)
            .map_err(store_error)?;
        let stored_events = core
            .store
            .load_app_events(&owner, MAX_APP_MESSAGES_U32)
            .map_err(store_error)?;
        let messages = chat_messages_from_stored(stored_messages, stored_events, &owner);
        let stored_rooms = core.store.load_app_rooms(&owner).map_err(store_error)?;
        let known_room_ids = core.known_room_ids().into_iter().collect::<BTreeSet<_>>();
        let mut persisted_room_ids = BTreeSet::new();
        let mut pending_invites = BTreeMap::new();
        let mut owned_invites = BTreeMap::new();
        let mut rooms = Vec::new();
        for stored_room in stored_rooms {
            let room_id = stored_room.room_id.clone();
            let has_mls_room = known_room_ids.contains(&room_id);
            persisted_room_ids.insert(room_id.clone());
            if stored_room.state != StoredAppRoomState::Connected
                && let Some(invite_url) = stored_room.pending_invite_url.clone()
            {
                pending_invites.insert(room_id.clone(), PendingInvite { invite_url });
            }
            if let Some(invite_url) = stored_room.owned_invite_url.clone() {
                owned_invites.insert(room_id.clone(), invite_url);
            }
            rooms.push(app_room_from_stored(stored_room, has_mls_room));
        }
        for room_id in known_room_ids {
            if !persisted_room_ids.contains(&room_id) {
                rooms.push(connected_app_room(&room_id, &room_id));
            }
        }
        sort_app_rooms(&mut rooms);
        apply_message_previews(&mut rooms, &messages);
        Ok(Self {
            core,
            app: AppState {
                rev: 0,
                identity,
                selected_room_id: rooms.first().map(|room| room.room_id.clone()),
                rooms,
                active_invite: None,
                active_profile_id: None,
                status: "ready".to_owned(),
                toast: None,
                messages,
                profiles: Vec::new(),
                devices: Vec::new(),
            },
            pending_invites,
            owned_invites,
            invite_watch_marks: BTreeMap::new(),
            profile_cache: BTreeMap::new(),
            revoked_devices: BTreeSet::new(),
        })
    }

    fn dispatch(&mut self, action: AppAction) -> Result<(), FiniteChatCoreError> {
        self.app.toast = None;
        match action {
            AppAction::StartRuntime => self.start_runtime()?,
            AppAction::StopRuntime => self.app.status = "stopped".to_owned(),
            AppAction::OpenRoom { room_id } => self.open_room(room_id),
            AppAction::CreateRoom { display_name } => self.create_room(display_name)?,
            AppAction::CreateInvite { room_id } => self.create_invite(room_id)?,
            AppAction::ScanTarget { value } => self.scan_target(value)?,
            AppAction::SubmitInvitePin {
                pending_room_id,
                pin,
            } => self.submit_invite_pin(pending_room_id, pin)?,
            AppAction::SendMessage { room_id, text } => self.send_message(room_id, text)?,
            AppAction::RetryRoom { room_id } => self.retry_room(room_id)?,
            AppAction::RefreshDevices => self.refresh_devices()?,
            AppAction::RevokeDevice {
                account_id,
                device_id,
            } => self.revoke_device(account_id, device_id)?,
        }
        self.bump_rev();
        Ok(())
    }

    fn start_runtime(&mut self) -> Result<(), FiniteChatCoreError> {
        match self.runtime_tick() {
            Ok(()) => Ok(()),
            Err(FiniteChatCoreError::Delivery { .. }) => {
                self.app.status = "offline".to_owned();
                self.app.toast = Some("Showing saved chats. Connection will retry.".to_owned());
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    fn wait_plan(&self, timeout_millis: u64) -> AppRuntimeWaitPlan {
        let server_url = self.core.server_url.clone();
        let rooms = self
            .core
            .device
            .room_sync_cursors()
            .into_iter()
            .filter(|cursor| cursor.server_url.as_deref().unwrap_or(&server_url) == server_url)
            .map(|cursor| SyncWaitRoom {
                room_id: cursor.room_id,
                after_seq: cursor.after_seq,
            })
            .collect::<Vec<_>>();

        let mut invites = BTreeMap::<String, SyncWaitInvite>::new();
        for invite_url in self.owned_invites.values().chain(
            self.pending_invites
                .values()
                .map(|pending| &pending.invite_url),
        ) {
            let Ok(code) = parse_invite(invite_url) else {
                continue;
            };
            if code.server_url != server_url {
                continue;
            }
            let mark = self
                .invite_watch_marks
                .get(&code.invite_id)
                .copied()
                .unwrap_or_default();
            invites.insert(
                code.invite_id.clone(),
                SyncWaitInvite {
                    invite_id: code.invite_id,
                    seen_requests: mark.requests,
                    seen_resolved: mark.resolved,
                },
            );
        }

        AppRuntimeWaitPlan {
            server_url,
            request: SyncStreamRequest {
                rooms,
                invites: invites.into_values().collect(),
                heartbeat_ms: Some(normalize_app_update_wait_millis(timeout_millis)),
            },
        }
    }

    fn apply_sync_hint(&mut self, event: SyncHintEvent) {
        if let SyncHintEvent::InviteChanged {
            invite_id,
            requests,
            resolved,
            ..
        } = event
        {
            self.invite_watch_marks
                .insert(invite_id, InviteWatchMark { requests, resolved });
        }
    }

    fn runtime_tick(&mut self) -> Result<(), FiniteChatCoreError> {
        for invite_url in self.owned_invites.values().cloned().collect::<Vec<_>>() {
            let accepted = self.core.accept_invite_joins(&invite_url)?;
            if !accepted.accepted.is_empty() {
                self.app.toast = Some(format!("{} device(s) joined", accepted.accepted.len()));
            }
        }
        let synced = self.core.sync()?;
        self.append_messages(synced.messages)?;
        self.try_finalize_pending_rooms()?;
        self.app.status = "ready".to_owned();
        Ok(())
    }

    fn open_room(&mut self, room_id: String) {
        self.app.selected_room_id = Some(room_id.clone());
        if self.room_mut(&room_id).is_none() {
            self.upsert_room(
                &room_id,
                &room_id,
                AppRoomState::NeedsAttention,
                "room is not available on this device",
            );
        }
    }

    fn create_room(&mut self, display_name: String) -> Result<(), FiniteChatCoreError> {
        let label = display_name.trim();
        if label.len() > MAX_INVITE_DISPLAY_NAME_BYTES as usize {
            return Err(FiniteChatCoreError::Client {
                reason: format!(
                    "room display name must be at most {MAX_INVITE_DISPLAY_NAME_BYTES} bytes"
                ),
            });
        }
        let room_id = self.core.generate_object_id("room")?;
        let display_name = if label.is_empty() {
            room_id.clone()
        } else {
            label.to_owned()
        };
        self.core
            .bootstrap_room(&room_id, Some(display_name.clone()))?;
        self.upsert_room(
            &room_id,
            &display_name,
            AppRoomState::Connected,
            "connected",
        );
        self.persist_room_projection(&room_id)?;
        self.app.selected_room_id = Some(room_id);
        self.app.status = "room created".to_owned();
        Ok(())
    }

    fn create_invite(&mut self, room_id: String) -> Result<(), FiniteChatCoreError> {
        let display_name = self
            .room(&room_id)
            .map(|room| room.display_name.clone())
            .unwrap_or_else(|| room_id.clone());
        let invite = match self
            .core
            .create_invite(&room_id, Some(display_name.clone()))
        {
            Ok(invite) => invite,
            Err(error) => {
                self.app.active_invite = None;
                self.upsert_room(
                    &room_id,
                    &display_name,
                    AppRoomState::NeedsAttention,
                    &error.to_string(),
                );
                self.persist_room_projection(&room_id)?;
                self.app.status = "room needs attention".to_owned();
                self.app.toast = Some("Invite could not be created".to_owned());
                return Ok(());
            }
        };
        self.owned_invites
            .insert(room_id.clone(), invite.invite_url.clone());
        self.app.active_invite = Some(AppInviteState {
            room_id: room_id.clone(),
            invite_url: invite.invite_url,
            pin: invite.pin,
        });
        self.persist_room_projection(&room_id)?;
        self.app.status = "invite ready".to_owned();
        Ok(())
    }

    fn scan_target(&mut self, value: String) -> Result<(), FiniteChatCoreError> {
        let trimmed = value.trim();
        if trimmed.starts_with("npub1") || trimmed.starts_with("nostr:npub1") {
            let npub = trimmed.strip_prefix("nostr:").unwrap_or(trimmed);
            let account_id = npub_decode(npub).map_err(invite_error)?;
            let found = self.fetch_profiles(vec![account_id.clone()])?;
            self.app.active_profile_id = Some(account_id.clone());
            if found {
                self.app.status = "profile loaded".to_owned();
                self.app.toast = None;
            } else {
                let profile = placeholder_profile(&account_id);
                self.profile_cache.insert(account_id, profile);
                self.sync_profile_state();
                self.app.status = "profile not found".to_owned();
                self.app.toast = Some("No cached profile was available for that npub".to_owned());
            }
            return Ok(());
        }
        let code = parse_invite(trimmed)?;
        let room_id = code.room_id.clone();
        let display_name = code
            .display_name
            .clone()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| room_id.clone());
        self.app.active_profile_id = None;
        self.pending_invites.insert(
            room_id.clone(),
            PendingInvite {
                invite_url: trimmed.to_owned(),
            },
        );
        self.upsert_room(
            &room_id,
            &display_name,
            AppRoomState::WaitingForApproval,
            "enter PIN to request admission",
        );
        self.persist_room_projection(&room_id)?;
        self.app.selected_room_id = Some(room_id);
        self.app.status = "invite scanned".to_owned();
        Ok(())
    }

    fn submit_invite_pin(
        &mut self,
        pending_room_id: String,
        pin: String,
    ) -> Result<(), FiniteChatCoreError> {
        let pending = self
            .pending_invites
            .get(&pending_room_id)
            .ok_or_else(|| FiniteChatCoreError::Invite {
                reason: format!("no pending invite for room '{pending_room_id}'"),
            })?
            .clone();
        self.core.join_invite(
            &pending.invite_url,
            pin.trim(),
            Some(self.app.identity.device_id.clone()),
        )?;
        let display_name = self
            .room(&pending_room_id)
            .map(|room| room.display_name.clone())
            .unwrap_or_else(|| pending_room_id.clone());
        self.upsert_room(
            &pending_room_id,
            &display_name,
            AppRoomState::WaitingForApproval,
            "waiting for room admission",
        );
        self.persist_room_projection(&pending_room_id)?;
        self.app.selected_room_id = Some(pending_room_id);
        self.app.status = "join requested".to_owned();
        Ok(())
    }

    fn send_message(&mut self, room_id: String, text: String) -> Result<(), FiniteChatCoreError> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Ok(());
        }
        if !self.room_is_connected(&room_id) {
            return Err(FiniteChatCoreError::Client {
                reason: format!("room '{room_id}' is not ready to send"),
            });
        }
        let result = self.core.send_text(&room_id, trimmed)?;
        self.append_messages(result.messages)?;
        if let Some(room) = self.room_mut(&room_id) {
            room.last_message_preview = trimmed.to_owned();
        }
        self.app.status = "sent".to_owned();
        Ok(())
    }

    fn retry_room(&mut self, room_id: String) -> Result<(), FiniteChatCoreError> {
        if self.pending_invites.contains_key(&room_id) {
            self.try_finalize_room(&room_id)?;
        } else {
            self.runtime_tick()?;
        }
        Ok(())
    }

    fn try_finalize_pending_rooms(&mut self) -> Result<(), FiniteChatCoreError> {
        for room_id in self.pending_invites.keys().cloned().collect::<Vec<_>>() {
            match self.try_finalize_room(&room_id) {
                Ok(()) => {}
                Err(error) => {
                    if let Some(room) = self.room_mut(&room_id) {
                        room.state = AppRoomState::WaitingForApproval;
                        room.status = error.to_string();
                        self.persist_room_projection(&room_id)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn try_finalize_room(&mut self, room_id: &str) -> Result<(), FiniteChatCoreError> {
        let Some(pending) = self.pending_invites.get(room_id).cloned() else {
            return Ok(());
        };
        let display_name = parse_invite(&pending.invite_url)
            .ok()
            .and_then(|code| code.display_name)
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| room_id.to_owned());
        if let Some(room) = self.room_mut(room_id) {
            room.state = AppRoomState::Joining;
            room.status = "joining".to_owned();
        }
        self.persist_room_projection(room_id)?;
        self.core.sync()?;
        self.core.finalize_invite(&pending.invite_url)?;
        self.pending_invites.remove(room_id);
        self.upsert_room(room_id, &display_name, AppRoomState::Connected, "connected");
        self.persist_room_projection(room_id)?;
        self.app.status = "joined".to_owned();
        Ok(())
    }

    fn fetch_profiles(&mut self, account_ids: Vec<String>) -> Result<bool, FiniteChatCoreError> {
        let now_ms = self.core.now_millis()?;
        let mut delivery = self.core.home_delivery();
        let response = delivery
            .get_nostr_profiles(account_ids, now_ms)
            .map_err(runtime_error)?;
        let mut found = false;
        for entry in response.profiles {
            found = true;
            self.profile_cache.insert(
                entry.profile.account_id.clone(),
                profile_from_record(entry.profile, entry.stale),
            );
        }
        self.sync_profile_state();
        Ok(found)
    }

    fn sync_profile_state(&mut self) {
        self.app.profiles = self.profile_cache.values().cloned().collect();
    }

    fn refresh_devices(&mut self) -> Result<(), FiniteChatCoreError> {
        let account_id = self.app.identity.account_id.clone();
        let mut delivery = self.core.home_delivery();
        let mut after_room_id = None;
        let mut devices = BTreeMap::<(String, String), AppDeviceSummary>::new();
        for _ in 0..16 {
            let page = delivery
                .list_account_rooms(ListAccountRoomsRequest {
                    account_id: account_id.clone(),
                    after_room_id: after_room_id.clone(),
                    limit: 100,
                })
                .map_err(runtime_error)?;
            for room in page.rooms {
                for room_device in room.devices {
                    if room_device.device.account_id != account_id {
                        continue;
                    }
                    let key = (
                        room_device.device.account_id.clone(),
                        room_device.device.device_id.clone(),
                    );
                    let revoked_key = app_device_key(&key.0, &key.1);
                    let entry = devices
                        .entry(key.clone())
                        .or_insert_with(|| AppDeviceSummary {
                            account_id: key.0.clone(),
                            device_id: key.1.clone(),
                            active: false,
                            current_device: self.app.identity.device_id == key.1,
                            revoked: self.revoked_devices.contains(&revoked_key),
                            room_count: 0,
                        });
                    entry.active |= room_device.active;
                    entry.revoked |= self.revoked_devices.contains(&revoked_key);
                    entry.room_count = entry.room_count.saturating_add(1);
                }
            }
            if !page.has_more {
                break;
            }
            let Some(next) = page.next_after_room_id else {
                break;
            };
            after_room_id = Some(next);
        }
        self.app.devices = devices.into_values().collect();
        self.app.status = "devices refreshed".to_owned();
        Ok(())
    }

    fn revoke_device(
        &mut self,
        account_id: String,
        device_id: String,
    ) -> Result<(), FiniteChatCoreError> {
        if account_id == self.app.identity.account_id && device_id == self.app.identity.device_id {
            return Err(FiniteChatCoreError::Client {
                reason: "cannot revoke the current device from this device".to_owned(),
            });
        }
        let device = DeviceRef {
            account_id,
            device_id,
        };
        let mut delivery = self.core.home_delivery();
        delivery.revoke_device(&device).map_err(runtime_error)?;
        self.revoked_devices
            .insert(app_device_key(&device.account_id, &device.device_id));
        self.refresh_devices()?;
        self.app.status = "device revoked".to_owned();
        Ok(())
    }

    fn append_messages(&mut self, messages: Vec<ChatMessage>) -> Result<(), FiniteChatCoreError> {
        let mut existing = self
            .app
            .messages
            .iter()
            .map(message_key)
            .collect::<BTreeSet<_>>();
        let mut changed = false;
        for message in messages {
            if existing.contains(&message_key(&message)) {
                continue;
            }
            existing.insert(message_key(&message));
            if let Some(room) = self.room_mut(&message.room_id) {
                room.last_message_preview = message.text.clone();
            }
            self.app.messages.push(message);
            changed = true;
        }
        if changed && self.app.messages.len() > MAX_APP_MESSAGES {
            let drop_count = self.app.messages.len() - MAX_APP_MESSAGES;
            self.app.messages.drain(0..drop_count);
        }
        Ok(())
    }

    fn persist_room_projection(&mut self, room_id: &str) -> Result<(), FiniteChatCoreError> {
        let Some(room) = self.room(room_id).cloned() else {
            return Ok(());
        };
        let pending = self.pending_invites.get(room_id);
        let owned_invite_url = self.owned_invites.get(room_id);
        let stored = stored_room_from_app(&room, pending, owned_invite_url);
        let owner = self.core.device.device_ref().clone();
        self.core
            .store
            .save_app_rooms(&owner, std::slice::from_ref(&stored))
            .map_err(store_error)
    }

    fn upsert_room(
        &mut self,
        room_id: &str,
        display_name: &str,
        state: AppRoomState,
        status: &str,
    ) {
        if let Some(index) = self
            .app
            .rooms
            .iter()
            .position(|room| room.room_id == room_id)
        {
            self.app.rooms[index].display_name = display_name.to_owned();
            self.app.rooms[index].state = state;
            self.app.rooms[index].status = status.to_owned();
            sort_app_rooms(&mut self.app.rooms);
            return;
        }
        self.app.rooms.push(AppRoomSummary {
            room_id: room_id.to_owned(),
            display_name: display_name.to_owned(),
            state,
            status: status.to_owned(),
            last_message_preview: String::new(),
            unread_count: 0,
        });
        sort_app_rooms(&mut self.app.rooms);
    }

    fn room_is_connected(&self, room_id: &str) -> bool {
        self.room(room_id)
            .is_some_and(|room| room.state == AppRoomState::Connected)
            && self.core.has_room(room_id)
    }

    fn room(&self, room_id: &str) -> Option<&AppRoomSummary> {
        self.app.rooms.iter().find(|room| room.room_id == room_id)
    }

    fn room_mut(&mut self, room_id: &str) -> Option<&mut AppRoomSummary> {
        self.app
            .rooms
            .iter_mut()
            .find(|room| room.room_id == room_id)
    }

    fn bump_rev(&mut self) {
        self.app.rev = self.app.rev.saturating_add(1);
    }
}

fn profile_from_record(
    record: finitechat_http::NostrProfileRecord,
    stale: bool,
) -> AppProfileSummary {
    let display_name = record
        .display_name
        .clone()
        .or_else(|| record.name.clone())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| short_account_label(&record.account_id));
    AppProfileSummary {
        npub: npub_encode(&record.account_id).unwrap_or_else(|_| record.account_id.clone()),
        account_id: record.account_id,
        display_name,
        about: record.about,
        picture: record.picture,
        stale,
    }
}

fn placeholder_profile(account_id: &str) -> AppProfileSummary {
    AppProfileSummary {
        account_id: account_id.to_owned(),
        npub: npub_encode(account_id).unwrap_or_else(|_| account_id.to_owned()),
        display_name: short_account_label(account_id),
        about: None,
        picture: None,
        stale: true,
    }
}

fn short_account_label(account_id: &str) -> String {
    let prefix_len = account_id.len().min(8);
    format!("npub {}", &account_id[..prefix_len])
}

fn app_device_key(account_id: &str, device_id: &str) -> String {
    format!("{account_id}/{device_id}")
}

fn normalize_app_update_wait_millis(timeout_millis: u64) -> u64 {
    if timeout_millis == 0 {
        return DEFAULT_APP_UPDATE_WAIT_MILLIS;
    }
    timeout_millis.clamp(MIN_APP_UPDATE_WAIT_MILLIS, MAX_APP_UPDATE_WAIT_MILLIS)
}

impl CoreState {
    fn open(options: OpenOptions) -> Result<Self, FiniteChatCoreError> {
        if options.device_id.trim().is_empty() {
            return Err(FiniteChatCoreError::Client {
                reason: "device id cannot be empty".to_owned(),
            });
        }

        let data_dir = PathBuf::from(options.data_dir);
        fs::create_dir_all(&data_dir).map_err(|error| FiniteChatCoreError::Filesystem {
            reason: format!("failed to create {}: {error}", data_dir.display()),
        })?;

        let account_secret =
            load_or_create_account_secret(&data_dir, options.account_secret_hex.as_deref())?;
        let now = options
            .now_unix_seconds
            .unwrap_or_else(current_unix_seconds);
        let config = FiniteChatDeviceConfig {
            account_secret_key: account_secret.clone(),
            device_id: options.device_id,
            now_unix_seconds: now,
            credential_not_before_unix_seconds: now.saturating_sub(60),
            credential_not_after_unix_seconds: now
                .saturating_add(DEFAULT_CREDENTIAL_VALIDITY_SECONDS),
        };
        let mut store = SqliteClientStore::open(
            data_dir.join(CLIENT_STORE_FILE),
            SqliteClientStoreOptions::from_nostr_secret(&account_secret, &config.device_id)
                .map_err(store_error)?,
        )
        .map_err(store_error)?;
        let device = match store.load_device(config.clone()) {
            Ok(device) => device,
            Err(finitechat_client::ClientStoreError::DeviceStateNotFound { .. }) => {
                let device = FiniteChatDevice::new(config.clone()).map_err(client_error)?;
                store.save_device_state(&device).map_err(store_error)?;
                device
            }
            Err(error) => return Err(store_error(error)),
        };

        Ok(Self {
            data_dir,
            server_url: options.server_url,
            account_secret,
            config,
            store,
            device,
        })
    }

    fn identity(&self) -> Identity {
        let device = self.device.device_ref();
        Identity {
            account_id: device.account_id.clone(),
            device_id: device.device_id.clone(),
            account_secret_hex: hex::encode(self.account_secret.as_bytes()),
        }
    }

    fn now_unix_seconds(&self) -> Result<u64, FiniteChatCoreError> {
        Ok(self.config.now_unix_seconds)
    }

    fn now_millis(&self) -> Result<u64, FiniteChatCoreError> {
        self.now_unix_seconds()?
            .checked_mul(1000)
            .ok_or_else(|| FiniteChatCoreError::Client {
                reason: "clock overflow".to_owned(),
            })
    }

    fn home_delivery(&self) -> HttpRuntimeDelivery<ReqwestHttpRuntimeTransport> {
        delivery_for(&self.server_url)
    }

    fn generate_object_id(&mut self, prefix: &str) -> Result<String, FiniteChatCoreError> {
        self.device.generate_object_id(prefix).map_err(client_error)
    }

    fn known_room_ids(&self) -> Vec<String> {
        self.device
            .room_sync_cursors()
            .into_iter()
            .map(|cursor| cursor.room_id)
            .collect()
    }

    fn has_room(&self, room_id: &str) -> bool {
        self.device.room_mls_group_id(room_id).is_ok()
    }

    fn bootstrap_room(
        &mut self,
        room_id: &str,
        display_name: Option<String>,
    ) -> Result<BootstrapRoomResult, FiniteChatCoreError> {
        if self.has_room(room_id) {
            return Err(FiniteChatCoreError::Client {
                reason: format!("room '{room_id}' already exists on this device"),
            });
        }
        let app_room = app_room_metadata(room_id, display_name.as_deref());
        let mls_group_id = self.generate_object_id("mls")?;
        let mut delivery = self.home_delivery();
        delivery
            .bootstrap_account_room(&CreateRoomRequest {
                room_id: room_id.to_owned(),
                mls_group_id: mls_group_id.clone(),
                creator: self.device.device_ref().clone(),
                protocol: RoomProtocol::default(),
            })
            .map_err(delivery_error)?;
        self.device
            .create_group_state(room_id, &mls_group_id)
            .map_err(client_error)?;
        self.store
            .save_device_state_and_app_rooms(&self.device, std::slice::from_ref(&app_room))
            .map_err(store_error)?;
        Ok(BootstrapRoomResult {
            room_id: room_id.to_owned(),
            mls_group_id,
        })
    }

    fn create_invite(
        &mut self,
        room_id: &str,
        display_name: Option<String>,
    ) -> Result<InviteResult, FiniteChatCoreError> {
        let mut delivery = self.home_delivery();
        let code = create_room_invite(
            &self.device,
            &mut delivery,
            CreateRoomInviteParams {
                room_id,
                server_url: &self.server_url,
                display_name,
                max_joins: DEFAULT_INVITE_MAX_JOINS,
                ttl_ms: DEFAULT_INVITE_TTL_MS,
                now_ms: self.now_millis()?,
            },
        )
        .map_err(runtime_error)?;
        let pin = invite_current_pin(&code.invite_token, self.now_unix_seconds()?);
        Ok(InviteResult {
            invite_url: code.encode().map_err(invite_error)?,
            pin,
        })
    }

    fn join_invite(
        &mut self,
        invite_url: &str,
        pin: &str,
        display_name: Option<String>,
    ) -> Result<JoinRequestResult, FiniteChatCoreError> {
        let code = parse_invite(invite_url)?;
        let now_ms = self.now_millis()?;
        let mut delivery = delivery_for(&code.server_url);
        let handle = submit_invite_join_request(
            &mut self.store,
            &mut self.device,
            &mut delivery,
            &code,
            pin,
            display_name,
            now_ms,
        )
        .map_err(runtime_error)?;
        Ok(JoinRequestResult {
            request_id: handle.request_id,
            key_package_id: handle.key_package_id,
        })
    }

    fn accept_invite_joins(
        &mut self,
        invite_url: &str,
    ) -> Result<AcceptInvitesResult, FiniteChatCoreError> {
        let code = parse_invite(invite_url)?;
        let now_ms = self.now_millis()?;
        let mut delivery = delivery_for(&code.server_url);
        let report = accept_pending_invite_joins(
            &mut self.store,
            &mut self.device,
            &mut delivery,
            &code,
            now_ms,
        )
        .map_err(runtime_error)?;
        let accepted = report.accepted.iter().map(device_label).collect::<Vec<_>>();
        let accepted_set = accepted.iter().cloned().collect::<BTreeSet<_>>();
        let rejected = report
            .rejected
            .iter()
            .map(device_label)
            .filter(|device| !accepted_set.contains(device))
            .collect::<Vec<_>>();
        let rejected_reason = (!rejected.is_empty()).then(|| {
            "join proof did not verify; the PIN was probably expired/incorrect, or the join request carried malformed key material".to_owned()
        });
        if !report.deferred_pending_commit && !report.accepted.is_empty() {
            self.sync()?;
        }
        Ok(AcceptInvitesResult {
            accepted,
            rejected,
            rejected_reason,
            deferred_pending_commit: report.deferred_pending_commit,
            total_requests: report.total_requests,
            resolved_requests: report.resolved_requests,
        })
    }

    fn finalize_invite(&mut self, invite_url: &str) -> Result<(), FiniteChatCoreError> {
        let code = parse_invite(invite_url)?;
        let options = RuntimeSyncOptions {
            key_package_target_available: DEFAULT_KEY_PACKAGE_TARGET_AVAILABLE,
            max_sync_pages_per_room: DEFAULT_MAX_SYNC_PAGES_PER_ROOM,
        };
        let mut delivery = delivery_for(&code.server_url);
        run_room_server_sync_tick(
            &mut self.store,
            &mut self.device,
            &mut delivery,
            &options,
            &code.server_url,
        )
        .map_err(runtime_error)?;
        finalize_invited_room(&mut self.store, &mut self.device, &code)
            .map_err(|error| finalize_error(&code.room_id, error))?;
        let app_room = app_room_metadata(&code.room_id, code.display_name.as_deref());
        self.store
            .save_app_rooms(self.device.device_ref(), std::slice::from_ref(&app_room))
            .map_err(store_error)
    }

    fn send_text(&mut self, room_id: &str, text: &str) -> Result<SyncResult, FiniteChatCoreError> {
        let idempotency_key = self
            .device
            .generate_object_id("msg")
            .map_err(client_error)?;
        let chat_payload = encode_text_message_payload(text)?;
        let app_event_plaintext =
            encode_application_event(DurableAppEventKind::ChatMessage, None, &chat_payload)?;
        let request = self
            .device
            .create_application_request(room_id, &app_event_plaintext, idempotency_key)
            .map_err(|error| send_error(room_id, error))?;
        let sender = request.sender.clone();
        self.store
            .save_device_state(&self.device)
            .map_err(store_error)?;

        let room_server_url = self
            .device
            .room_server_url(room_id)
            .map(str::to_owned)
            .unwrap_or_else(|| self.server_url.clone());
        let mut delivery = delivery_for(&room_server_url);
        let accepted = delivery
            .append_event(&request, DurableAppEventKind::ChatMessage.delivery_policy())
            .map_err(delivery_error)?;

        let message = project_chat_message(
            room_id.to_owned(),
            accepted.seq,
            accepted.message_id,
            sender,
            app_event_plaintext,
            self.device.device_ref(),
        )
        .ok_or_else(|| FiniteChatCoreError::Client {
            reason: "sent chat message did not project as a transcript row".to_owned(),
        })?;
        self.persist_chat_messages_and_events(std::slice::from_ref(&message))?;
        let mut result = self.sync()?;
        result.messages.insert(0, message);
        Ok(result)
    }

    fn sync(&mut self) -> Result<SyncResult, FiniteChatCoreError> {
        let options = RuntimeSyncOptions {
            key_package_target_available: DEFAULT_KEY_PACKAGE_TARGET_AVAILABLE,
            max_sync_pages_per_room: DEFAULT_MAX_SYNC_PAGES_PER_ROOM,
        };
        let mut result = SyncResult::default();

        let mut home_delivery = self.home_delivery();
        let home_report = run_runtime_sync_tick(
            &mut self.store,
            &mut self.device,
            &mut home_delivery,
            &options,
        )
        .map_err(runtime_error)?;
        let owner = self.device.device_ref().clone();
        result.merge_report(home_report, &owner);

        let room_servers = self
            .device
            .room_sync_cursors()
            .into_iter()
            .filter_map(|cursor| cursor.server_url)
            .collect::<BTreeSet<_>>();
        for server_url in room_servers {
            let mut delivery = delivery_for(&server_url);
            let report = run_room_server_sync_tick(
                &mut self.store,
                &mut self.device,
                &mut delivery,
                &options,
                &server_url,
            )
            .map_err(runtime_error)?;
            result.merge_report(report, &owner);
        }

        Ok(result)
    }

    fn persist_chat_messages_and_events(
        &mut self,
        messages: &[ChatMessage],
    ) -> Result<(), FiniteChatCoreError> {
        if messages.is_empty() {
            return Ok(());
        }
        let owner = self.device.device_ref().clone();
        let stored_messages = messages
            .iter()
            .map(stored_message_from_chat)
            .collect::<Vec<_>>();
        let stored_events = messages
            .iter()
            .map(stored_event_from_chat)
            .collect::<Vec<_>>();
        self.store
            .save_app_messages_and_events(
                &owner,
                &stored_messages,
                &stored_events,
                MAX_APP_MESSAGES_U32,
            )
            .map_err(store_error)
    }
}

impl SyncResult {
    fn merge_report(&mut self, report: finitechat_client::RuntimeSyncReport, owner: &DeviceRef) {
        self.uploaded_key_packages = self
            .uploaded_key_packages
            .saturating_add(report.uploaded_key_packages);
        self.claimed_welcomes = self
            .claimed_welcomes
            .saturating_add(report.claimed_welcomes);
        self.activated_welcome_acks_sent = self
            .activated_welcome_acks_sent
            .saturating_add(report.activated_welcome_acks_sent);
        self.sync_pages = self.sync_pages.saturating_add(report.sync_pages);
        self.messages
            .extend(
                report
                    .applied_entries
                    .into_iter()
                    .filter_map(|entry| match entry.entry {
                        AppliedLogEntry::Application { plaintext, sender } => project_chat_message(
                            entry.room_id,
                            entry.seq,
                            entry.message_id,
                            sender,
                            plaintext,
                            owner,
                        ),
                        AppliedLogEntry::Commit { .. } => None,
                    }),
            );
    }
}

#[cfg(test)]
fn chat_display_text(plaintext: &[u8]) -> String {
    chat_projection_payload_from_application_plaintext(plaintext)
        .map(|payload| payload.text)
        .unwrap_or_default()
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum DecodedAppEvent {
    ChatMessage {
        conversation_id: Option<String>,
        payload: Vec<u8>,
    },
    ChatReaction(ChatReactionV1),
    Ignored,
}

struct ChatProjectionPayload {
    text: String,
    display_content: String,
    conversation_id: Option<String>,
    reply_to_message_id: Option<String>,
    sender_name: Option<String>,
    media: Vec<ChatMediaAttachment>,
}

fn project_chat_message(
    room_id: String,
    seq: u64,
    message_id: String,
    sender: DeviceRef,
    plaintext: Vec<u8>,
    owner: &DeviceRef,
) -> Option<ChatMessage> {
    let projection = chat_projection_payload_from_application_plaintext(&plaintext)?;
    let is_mine = sender == *owner;
    let sender_npub = npub_encode(&sender.account_id).ok();
    Some(ChatMessage {
        room_id,
        seq,
        message_id,
        conversation_id: projection.conversation_id,
        sender_account_id: sender.account_id.clone(),
        sender_device_id: sender.device_id.clone(),
        sender_display_name: sender_display_name(
            &sender,
            projection.sender_name.as_deref(),
            is_mine,
        ),
        sender_npub,
        text: projection.text,
        display_content: projection.display_content,
        payload: plaintext,
        reply_to_message_id: projection.reply_to_message_id,
        is_mine,
        delivery: MessageDeliveryState::Sent,
        reactions: Vec::new(),
        media: projection.media,
        read_receipt: None,
        display_timestamp: String::new(),
    })
}

fn chat_projection_payload_from_application_plaintext(
    plaintext: &[u8],
) -> Option<ChatProjectionPayload> {
    match decode_application_event(plaintext) {
        DecodedAppEvent::ChatMessage {
            conversation_id,
            payload,
        } => {
            let mut projection = chat_projection_payload(&payload);
            if projection.conversation_id.is_none() {
                projection.conversation_id = conversation_id;
            }
            Some(projection)
        }
        DecodedAppEvent::ChatReaction(_) | DecodedAppEvent::Ignored => None,
    }
}

fn chat_projection_payload(payload_bytes: &[u8]) -> ChatProjectionPayload {
    if let Ok(Some(payload)) = HermesMessagePayloadV1::decode(payload_bytes) {
        return ChatProjectionPayload {
            display_content: payload.text.clone(),
            text: payload.text,
            conversation_id: payload.conversation_id,
            reply_to_message_id: payload.reply_to_message_id,
            sender_name: payload.sender_name,
            media: payload
                .attachments
                .into_iter()
                .enumerate()
                .map(|(index, attachment)| chat_media_attachment(index, attachment))
                .collect(),
        };
    }
    let text = String::from_utf8_lossy(payload_bytes).into_owned();
    ChatProjectionPayload {
        display_content: text.clone(),
        text,
        conversation_id: None,
        reply_to_message_id: None,
        sender_name: None,
        media: Vec::new(),
    }
}

fn decode_application_event(plaintext: &[u8]) -> DecodedAppEvent {
    match serde_json::from_slice::<DecryptedApplicationEventV1>(plaintext) {
        Ok(event) => decoded_typed_application_event(event),
        Err(_) => DecodedAppEvent::ChatMessage {
            conversation_id: None,
            payload: plaintext.to_vec(),
        },
    }
}

fn decoded_typed_application_event(event: DecryptedApplicationEventV1) -> DecodedAppEvent {
    if event.validate_limits().is_err() {
        return DecodedAppEvent::Ignored;
    }
    match event.kind {
        DurableAppEventKind::ChatMessage => DecodedAppEvent::ChatMessage {
            conversation_id: event.conversation_id,
            payload: event.payload,
        },
        DurableAppEventKind::ChatReaction => {
            serde_json::from_slice::<ChatReactionV1>(&event.payload)
                .ok()
                .filter(|reaction| reaction.validate_limits().is_ok())
                .map(DecodedAppEvent::ChatReaction)
                .unwrap_or(DecodedAppEvent::Ignored)
        }
        DurableAppEventKind::ConversationCreate
        | DurableAppEventKind::ConversationUpdate
        | DurableAppEventKind::ConversationArchive
        | DurableAppEventKind::ConversationSegmentStart
        | DurableAppEventKind::ChatEdit
        | DurableAppEventKind::ChatReceipt
        | DurableAppEventKind::RuntimeStateSnapshot
        | DurableAppEventKind::RuntimeCommandRequest
        | DurableAppEventKind::RuntimeCommandResult
        | DurableAppEventKind::RuntimeCommandCancel
        | DurableAppEventKind::StreamStart
        | DurableAppEventKind::StreamFinish
        | DurableAppEventKind::Namespaced { .. } => DecodedAppEvent::Ignored,
    }
}

fn encode_text_message_payload(text: &str) -> Result<Vec<u8>, FiniteChatCoreError> {
    HermesMessagePayloadV1 {
        payload_type: finitechat_hermes::HERMES_MESSAGE_PAYLOAD_TYPE_V1.to_owned(),
        conversation_id: None,
        text: text.to_owned(),
        kind: finitechat_hermes::HermesSendKindV1::Message,
        status: finitechat_hermes::HermesMessageStatusV1::Complete,
        edit_of: None,
        attachments: Vec::new(),
        reply_to_message_id: None,
        sender_name: None,
        metadata: BTreeMap::new(),
    }
    .encode()
    .map_err(client_error)
}

fn encode_application_event(
    kind: DurableAppEventKind,
    conversation_id: Option<String>,
    payload: &[u8],
) -> Result<Vec<u8>, FiniteChatCoreError> {
    let event = DecryptedApplicationEventV1 {
        kind,
        conversation_id,
        payload: payload.to_vec(),
    };
    event.validate_limits().map_err(client_error)?;
    serde_json::to_vec(&event).map_err(client_error)
}

fn chat_media_attachment(index: usize, attachment: HermesAttachmentV1) -> ChatMediaAttachment {
    let blob = attachment.blob;
    let dimensions = blob
        .as_ref()
        .and_then(|blob| blob.metadata.dimensions.as_ref());
    let attachment_id = blob
        .as_ref()
        .map(|blob| blob.plaintext_sha256.clone())
        .or_else(|| attachment.url.clone())
        .or_else(|| attachment.path.clone())
        .unwrap_or_else(|| format!("attachment-{index}"));
    let url = blob
        .as_ref()
        .map(|blob| blob.url.clone())
        .or(attachment.url);
    let mime_type = blob
        .as_ref()
        .map(|blob| blob.metadata.mime_type.clone())
        .filter(|mime_type| !mime_type.trim().is_empty())
        .unwrap_or(attachment.mime_type);
    let filename = blob
        .as_ref()
        .map(|blob| blob.metadata.filename.clone())
        .filter(|filename| !filename.trim().is_empty())
        .unwrap_or(attachment.name);
    ChatMediaAttachment {
        attachment_id,
        url,
        mime_type,
        filename,
        kind: match attachment.kind {
            HermesAttachmentKindV1::Image => ChatMediaKind::Image,
            HermesAttachmentKindV1::Video => ChatMediaKind::Video,
            HermesAttachmentKindV1::Audio => ChatMediaKind::VoiceNote,
            HermesAttachmentKindV1::File => ChatMediaKind::File,
        },
        width: dimensions.map(|dimensions| dimensions.width),
        height: dimensions.map(|dimensions| dimensions.height),
        local_path: attachment.path,
        upload_progress_per_mille: None,
    }
}

fn sender_display_name(sender: &DeviceRef, payload_name: Option<&str>, is_mine: bool) -> String {
    if is_mine {
        return "You".to_owned();
    }
    if let Some(name) = payload_name.map(str::trim).filter(|name| !name.is_empty()) {
        return name.to_owned();
    }
    format!(
        "{} / {}",
        short_account_label(&sender.account_id),
        sender.device_id
    )
}

fn chat_message_from_stored(message: StoredAppMessage, owner: &DeviceRef) -> Option<ChatMessage> {
    project_chat_message(
        message.room_id,
        message.seq,
        message.message_id,
        message.sender,
        message.plaintext,
        owner,
    )
}

fn chat_messages_from_stored(
    stored_messages: Vec<StoredAppMessage>,
    stored_events: Vec<StoredAppEvent>,
    owner: &DeviceRef,
) -> Vec<ChatMessage> {
    let mut by_key = BTreeMap::<(String, String), ChatMessage>::new();
    for message in stored_messages {
        if let Some(projected) = chat_message_from_stored(message, owner) {
            by_key.insert(message_key(&projected), projected);
        }
    }
    for event in stored_events {
        apply_stored_app_event(&mut by_key, event, owner);
    }
    let mut messages = by_key.into_values().collect::<Vec<_>>();
    messages.sort_by(|left, right| {
        left.seq
            .cmp(&right.seq)
            .then_with(|| left.room_id.cmp(&right.room_id))
            .then_with(|| left.message_id.cmp(&right.message_id))
    });
    messages
}

fn apply_stored_app_event(
    messages: &mut BTreeMap<(String, String), ChatMessage>,
    event: StoredAppEvent,
    owner: &DeviceRef,
) {
    match decode_application_event(&event.plaintext) {
        DecodedAppEvent::ChatMessage { .. } => {
            if let Some(message) = project_chat_message(
                event.room_id,
                event.seq,
                event.message_id,
                event.sender,
                event.plaintext,
                owner,
            ) {
                messages.insert(message_key(&message), message);
            }
        }
        DecodedAppEvent::ChatReaction(reaction) => {
            apply_chat_reaction(messages, &event.room_id, &event.sender, owner, reaction);
        }
        DecodedAppEvent::Ignored => {}
    }
}

fn apply_chat_reaction(
    messages: &mut BTreeMap<(String, String), ChatMessage>,
    room_id: &str,
    sender: &DeviceRef,
    owner: &DeviceRef,
    reaction: ChatReactionV1,
) {
    let key = (room_id.to_owned(), reaction.target_message_id);
    let Some(message) = messages.get_mut(&key) else {
        return;
    };
    let emoji = reaction.emoji.trim().to_owned();
    let Some(summary) = message
        .reactions
        .iter_mut()
        .find(|summary| summary.emoji == emoji)
    else {
        message.reactions.push(ChatReactionSummary {
            emoji,
            count: 1,
            reacted_by_me: sender == owner,
        });
        return;
    };
    summary.count = summary.count.saturating_add(1);
    summary.reacted_by_me |= sender == owner;
}

fn stored_message_from_chat(message: &ChatMessage) -> StoredAppMessage {
    StoredAppMessage {
        room_id: message.room_id.clone(),
        seq: message.seq,
        message_id: message.message_id.clone(),
        sender: DeviceRef {
            account_id: message.sender_account_id.clone(),
            device_id: message.sender_device_id.clone(),
        },
        plaintext: message.payload.clone(),
    }
}

fn stored_event_from_chat(message: &ChatMessage) -> StoredAppEvent {
    StoredAppEvent {
        room_id: message.room_id.clone(),
        seq: message.seq,
        message_id: message.message_id.clone(),
        sender: DeviceRef {
            account_id: message.sender_account_id.clone(),
            device_id: message.sender_device_id.clone(),
        },
        plaintext: message.payload.clone(),
    }
}

fn app_room_from_stored(room: StoredAppRoom, has_mls_room: bool) -> AppRoomSummary {
    let mut state = app_room_state_from_stored(room.state);
    let mut status = room.status;
    if state == AppRoomState::Connected && !has_mls_room {
        state = AppRoomState::NeedsAttention;
        status = "room is not available on this device".to_owned();
    }
    AppRoomSummary {
        room_id: room.room_id,
        display_name: room.display_name,
        state,
        status,
        last_message_preview: String::new(),
        unread_count: 0,
    }
}

fn connected_app_room(room_id: &str, display_name: &str) -> AppRoomSummary {
    AppRoomSummary {
        room_id: room_id.to_owned(),
        display_name: display_name.to_owned(),
        state: AppRoomState::Connected,
        status: "connected".to_owned(),
        last_message_preview: String::new(),
        unread_count: 0,
    }
}

fn stored_room_from_app(
    room: &AppRoomSummary,
    pending: Option<&PendingInvite>,
    owned_invite_url: Option<&String>,
) -> StoredAppRoom {
    StoredAppRoom {
        room_id: room.room_id.clone(),
        display_name: room.display_name.clone(),
        state: stored_app_room_state(&room.state),
        status: room.status.clone(),
        pending_invite_url: pending.map(|pending| pending.invite_url.clone()),
        owned_invite_url: owned_invite_url.cloned(),
    }
}

fn app_room_state_from_stored(state: StoredAppRoomState) -> AppRoomState {
    match state {
        StoredAppRoomState::Connected => AppRoomState::Connected,
        StoredAppRoomState::WaitingForApproval => AppRoomState::WaitingForApproval,
        StoredAppRoomState::Joining => AppRoomState::Joining,
        StoredAppRoomState::NeedsAttention => AppRoomState::NeedsAttention,
        StoredAppRoomState::Offline => AppRoomState::Offline,
    }
}

fn stored_app_room_state(state: &AppRoomState) -> StoredAppRoomState {
    match state {
        AppRoomState::Connected => StoredAppRoomState::Connected,
        AppRoomState::WaitingForApproval => StoredAppRoomState::WaitingForApproval,
        AppRoomState::Joining => StoredAppRoomState::Joining,
        AppRoomState::NeedsAttention => StoredAppRoomState::NeedsAttention,
        AppRoomState::Offline => StoredAppRoomState::Offline,
    }
}

fn app_room_metadata(room_id: &str, display_name: Option<&str>) -> StoredAppRoom {
    let display_name = display_name
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or(room_id)
        .to_owned();
    StoredAppRoom {
        room_id: room_id.to_owned(),
        display_name,
        state: StoredAppRoomState::Connected,
        status: "connected".to_owned(),
        pending_invite_url: None,
        owned_invite_url: None,
    }
}

fn migrate_legacy_app_messages(
    core: &mut CoreState,
    owner: &DeviceRef,
) -> Result<(), FiniteChatCoreError> {
    let path = core.data_dir.join(LEGACY_APP_MESSAGES_FILE);
    if !path.exists() {
        return Ok(());
    }
    if !core
        .store
        .load_app_messages(owner, 1)
        .map_err(store_error)?
        .is_empty()
    {
        return Ok(());
    }
    let messages = load_legacy_app_messages(&core.data_dir)?;
    if messages.is_empty() {
        return Ok(());
    }
    let stored = messages
        .iter()
        .map(stored_message_from_chat)
        .collect::<Vec<_>>();
    core.store
        .save_app_messages(owner, &stored, MAX_APP_MESSAGES_U32)
        .map_err(store_error)
}

fn load_legacy_app_messages(data_dir: &Path) -> Result<Vec<ChatMessage>, FiniteChatCoreError> {
    let path = data_dir.join(LEGACY_APP_MESSAGES_FILE);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = fs::read(&path).map_err(|error| FiniteChatCoreError::Filesystem {
        reason: format!("failed to read {}: {error}", path.display()),
    })?;
    let mut messages: Vec<ChatMessage> =
        serde_json::from_slice(&raw).map_err(|error| FiniteChatCoreError::Store {
            reason: format!("failed to parse {}: {error}", path.display()),
        })?;
    if messages.len() > MAX_APP_MESSAGES {
        let drop_count = messages.len() - MAX_APP_MESSAGES;
        messages.drain(0..drop_count);
    }
    Ok(messages)
}

fn apply_message_previews(rooms: &mut [AppRoomSummary], messages: &[ChatMessage]) {
    for message in messages {
        if let Some(room) = rooms
            .iter_mut()
            .find(|room| room.room_id == message.room_id)
        {
            room.last_message_preview = message.text.clone();
        }
    }
}

fn sort_app_rooms(rooms: &mut [AppRoomSummary]) {
    rooms.sort_by(|left, right| {
        left.display_name
            .cmp(&right.display_name)
            .then_with(|| left.room_id.cmp(&right.room_id))
    });
}

fn message_key(message: &ChatMessage) -> (String, String) {
    (message.room_id.clone(), message.message_id.clone())
}

fn load_or_create_account_secret(
    data_dir: &Path,
    provided: Option<&str>,
) -> Result<NostrSecretKey, FiniteChatCoreError> {
    let secret_path = data_dir.join(ACCOUNT_SECRET_FILE);
    if let Some(secret) = provided {
        let parsed = parse_account_secret_hex(secret)?;
        write_account_secret(&secret_path, &parsed)?;
        return Ok(parsed);
    }
    if secret_path.is_file() {
        let secret =
            fs::read_to_string(&secret_path).map_err(|error| FiniteChatCoreError::Filesystem {
                reason: format!("failed to read {}: {error}", secret_path.display()),
            })?;
        return parse_account_secret_hex(secret.trim());
    }
    let secret = generate_account_secret().map_err(client_error)?;
    write_account_secret(&secret_path, &secret)?;
    Ok(secret)
}

fn write_account_secret(path: &Path, secret: &NostrSecretKey) -> Result<(), FiniteChatCoreError> {
    fs::write(path, format!("{}\n", hex::encode(secret.as_bytes()))).map_err(|error| {
        FiniteChatCoreError::Filesystem {
            reason: format!("failed to write {}: {error}", path.display()),
        }
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let permissions = fs::Permissions::from_mode(0o600);
        fs::set_permissions(path, permissions).map_err(|error| {
            FiniteChatCoreError::Filesystem {
                reason: format!("failed to chmod {}: {error}", path.display()),
            }
        })?;
    }
    Ok(())
}

fn parse_account_secret_hex(secret: &str) -> Result<NostrSecretKey, FiniteChatCoreError> {
    let bytes =
        hex::decode(secret.trim()).map_err(|_| FiniteChatCoreError::InvalidAccountSecret)?;
    let bytes: [u8; NOSTR_SECRET_KEY_BYTES] = bytes
        .try_into()
        .map_err(|_| FiniteChatCoreError::InvalidAccountSecret)?;
    NostrSecretKey::from_bytes(bytes).map_err(|_| FiniteChatCoreError::InvalidAccountSecret)
}

fn parse_invite(invite_url: &str) -> Result<InviteCodeV1, FiniteChatCoreError> {
    InviteCodeV1::parse(invite_url).map_err(invite_error)
}

fn delivery_for(server_url: &str) -> HttpRuntimeDelivery<ReqwestHttpRuntimeTransport> {
    HttpRuntimeDelivery::new(ReqwestHttpRuntimeTransport::new(server_url))
}

fn current_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn device_label(device: &DeviceRef) -> String {
    format!("{}/{}", device.account_id, device.device_id)
}

fn client_error(error: impl std::fmt::Display) -> FiniteChatCoreError {
    FiniteChatCoreError::Client {
        reason: error.to_string(),
    }
}

fn send_error(room_id: &str, error: ClientError) -> FiniteChatCoreError {
    match error {
        ClientError::GroupNotFound(_) => FiniteChatCoreError::Client {
            reason: format!(
                "this device has not created or joined room '{room_id}' yet; create the room on this device, or join and finalize an invite before sending"
            ),
        },
        other => client_error(other),
    }
}

fn finalize_error(room_id: &str, error: ClientStoreError) -> FiniteChatCoreError {
    match error {
        ClientStoreError::Client(ClientError::GroupNotFound(_)) => FiniteChatCoreError::Client {
            reason: format!(
                "this device has no accepted Welcome for room '{room_id}'; ask the room creator to accept the join and check that accepted contains this device, then finalize again"
            ),
        },
        other => store_error(other),
    }
}

fn delivery_error(error: impl std::fmt::Display) -> FiniteChatCoreError {
    FiniteChatCoreError::Delivery {
        reason: error.to_string(),
    }
}

fn runtime_error(error: impl std::fmt::Display) -> FiniteChatCoreError {
    FiniteChatCoreError::Delivery {
        reason: error.to_string(),
    }
}

fn store_error(error: impl std::fmt::Display) -> FiniteChatCoreError {
    FiniteChatCoreError::Store {
        reason: error.to_string(),
    }
}

fn invite_error(error: impl std::fmt::Display) -> FiniteChatCoreError {
    FiniteChatCoreError::Invite {
        reason: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use finitechat_http::{NostrProfileRecord, PutNostrProfileRequest};
    use finitechat_server::{HttpServerState, http_router};

    const NOW: u64 = 1_800_000_000;

    #[test]
    fn core_sessions_message_each_other_over_live_http() {
        let dir = tempfile::tempdir().unwrap();
        let server_url = spawn_live_http_server(dir.path().join("server.sqlite3"));
        let alice = FiniteChatCore::open(OpenOptions {
            data_dir: dir.path().join("alice").to_string_lossy().into_owned(),
            server_url: server_url.clone(),
            device_id: "alice-cli".to_owned(),
            account_secret_hex: None,
            now_unix_seconds: Some(NOW),
        })
        .unwrap();
        let bob = FiniteChatCore::open(OpenOptions {
            data_dir: dir.path().join("bob").to_string_lossy().into_owned(),
            server_url,
            device_id: "bob-ios".to_owned(),
            account_secret_hex: None,
            now_unix_seconds: Some(NOW),
        })
        .unwrap();

        alice
            .bootstrap_room("room-core-flow".to_owned(), Some("Core Flow".to_owned()))
            .unwrap();
        alice.sync().unwrap();
        let invite = alice
            .create_invite("room-core-flow".to_owned(), Some("Core Flow".to_owned()))
            .unwrap();
        bob.join_invite(
            invite.invite_url.clone(),
            invite.pin.clone(),
            Some("Bob iOS".to_owned()),
        )
        .unwrap();
        let accepted = alice
            .accept_invite_joins(invite.invite_url.clone())
            .unwrap();
        assert_eq!(accepted.accepted.len(), 1);
        assert_eq!(accepted.rejected.len(), 0);

        alice.sync().unwrap();
        bob.sync().unwrap();
        bob.finalize_invite(invite.invite_url).unwrap();

        let from_cli = alice
            .send_text("room-core-flow".to_owned(), "hello from cli".to_owned())
            .unwrap();
        assert_eq!(from_cli.messages.len(), 1);
        let bob_sync = bob.sync().unwrap();
        assert_eq!(texts(&bob_sync), vec!["hello from cli"]);

        bob.send_text("room-core-flow".to_owned(), "hello from ios".to_owned())
            .unwrap();
        let alice_sync = alice.sync().unwrap();
        assert_eq!(texts(&alice_sync), vec!["hello from ios"]);
    }

    #[test]
    fn chat_projection_displays_hermes_payload_text() {
        let payload = HermesMessagePayloadV1 {
            payload_type: finitechat_hermes::HERMES_MESSAGE_PAYLOAD_TYPE_V1.to_owned(),
            conversation_id: None,
            text: "echo: hello from iOS".to_owned(),
            kind: finitechat_hermes::HermesSendKindV1::Message,
            status: finitechat_hermes::HermesMessageStatusV1::Complete,
            edit_of: None,
            attachments: Vec::new(),
            reply_to_message_id: None,
            sender_name: None,
            metadata: BTreeMap::new(),
        }
        .encode()
        .unwrap();

        assert_eq!(chat_display_text(&payload), "echo: hello from iOS");
        let wrapped =
            encode_application_event(DurableAppEventKind::ChatMessage, None, &payload).unwrap();
        assert_eq!(chat_display_text(&wrapped), "echo: hello from iOS");
        assert_eq!(chat_display_text(b"plain hello"), "plain hello");
    }

    #[test]
    fn chat_projection_ignores_reaction_app_events_as_messages() {
        let reaction = ChatReactionV1 {
            target_message_id: "message-1".to_owned(),
            emoji: "+1".to_owned(),
        };
        let payload = serde_json::to_vec(&reaction).unwrap();
        let event =
            encode_application_event(DurableAppEventKind::ChatReaction, None, &payload).unwrap();
        let sender = DeviceRef {
            account_id: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                .to_owned(),
            device_id: "phone".to_owned(),
        };
        let owner = sender.clone();

        assert_eq!(chat_display_text(&event), "");
        assert!(
            project_chat_message(
                "room-main".to_owned(),
                8,
                "reaction-1".to_owned(),
                sender,
                event,
                &owner,
            )
            .is_none(),
            "typed reaction events must not become transcript rows"
        );
    }

    #[test]
    fn chat_projection_rebuilds_from_stored_app_events_without_message_cache() {
        let owner = DeviceRef {
            account_id: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                .to_owned(),
            device_id: "phone".to_owned(),
        };
        let peer = DeviceRef {
            account_id: "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"
                .to_owned(),
            device_id: "tablet".to_owned(),
        };
        let chat_payload = encode_text_message_payload("event sourced hello").unwrap();
        let chat_event =
            encode_application_event(DurableAppEventKind::ChatMessage, None, &chat_payload)
                .unwrap();
        let reaction = ChatReactionV1 {
            target_message_id: "message-1".to_owned(),
            emoji: "+1".to_owned(),
        };
        let reaction_payload = serde_json::to_vec(&reaction).unwrap();
        let reaction_event =
            encode_application_event(DurableAppEventKind::ChatReaction, None, &reaction_payload)
                .unwrap();

        let messages = chat_messages_from_stored(
            Vec::new(),
            vec![
                StoredAppEvent {
                    room_id: "room-main".to_owned(),
                    seq: 1,
                    message_id: "message-1".to_owned(),
                    sender: owner.clone(),
                    plaintext: chat_event,
                },
                StoredAppEvent {
                    room_id: "room-main".to_owned(),
                    seq: 2,
                    message_id: "reaction-1".to_owned(),
                    sender: peer,
                    plaintext: reaction_event,
                },
            ],
            &owner,
        );

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].text, "event sourced hello");
        assert_eq!(
            messages[0].reactions,
            vec![ChatReactionSummary {
                emoji: "+1".to_owned(),
                count: 1,
                reacted_by_me: false,
            }]
        );
    }

    #[test]
    fn chat_projection_maps_hermes_reply_sender_and_media() {
        use finitechat_proto::{
            AttachmentBlobEncryptionV1, AttachmentBlobMetadataV1, AttachmentBlobReferenceV1,
            AttachmentDimensionsV1,
        };

        let sender = DeviceRef {
            account_id: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                .to_owned(),
            device_id: "phone".to_owned(),
        };
        let owner = DeviceRef {
            account_id: "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"
                .to_owned(),
            device_id: "ios".to_owned(),
        };
        let payload = HermesMessagePayloadV1 {
            payload_type: finitechat_hermes::HERMES_MESSAGE_PAYLOAD_TYPE_V1.to_owned(),
            conversation_id: Some("topic-main".to_owned()),
            text: "photo from Hermes".to_owned(),
            kind: finitechat_hermes::HermesSendKindV1::Media,
            status: finitechat_hermes::HermesMessageStatusV1::Complete,
            edit_of: None,
            attachments: vec![HermesAttachmentV1 {
                kind: HermesAttachmentKindV1::Image,
                name: "ignored.jpg".to_owned(),
                mime_type: "application/octet-stream".to_owned(),
                path: Some("/tmp/local-preview.jpg".to_owned()),
                url: Some("https://cdn.invalid/fallback".to_owned()),
                blob: Some(AttachmentBlobReferenceV1 {
                    scheme: "finitechat.attachment.v1".to_owned(),
                    url: "https://blob.invalid/sha256".to_owned(),
                    ciphertext_sha256: "c".repeat(64),
                    plaintext_sha256: "p".repeat(64),
                    plaintext_size: 12,
                    ciphertext_size: 28,
                    encryption: AttachmentBlobEncryptionV1 {
                        algorithm: "AES-256-GCM".to_owned(),
                        key_hex: "00".repeat(32),
                        nonce_hex: "11".repeat(12),
                    },
                    metadata: AttachmentBlobMetadataV1 {
                        mime_type: "image/jpeg".to_owned(),
                        filename: "photo.jpg".to_owned(),
                        dimensions: Some(AttachmentDimensionsV1 {
                            width: 640,
                            height: 480,
                        }),
                    },
                }),
            }],
            reply_to_message_id: Some("message-parent".to_owned()),
            sender_name: Some("Hermes User".to_owned()),
            metadata: BTreeMap::new(),
        }
        .encode()
        .unwrap();

        let message = project_chat_message(
            "room-main".to_owned(),
            7,
            "message-7".to_owned(),
            sender,
            payload,
            &owner,
        )
        .expect("hermes chat payload should project");

        assert_eq!(message.conversation_id.as_deref(), Some("topic-main"));
        assert_eq!(message.text, "photo from Hermes");
        assert_eq!(message.display_content, "photo from Hermes");
        assert_eq!(
            message.reply_to_message_id.as_deref(),
            Some("message-parent")
        );
        assert_eq!(message.sender_display_name, "Hermes User");
        assert!(!message.is_mine);
        assert!(message.reactions.is_empty());
        assert!(message.read_receipt.is_none());
        assert_eq!(message.media.len(), 1);
        let media = &message.media[0];
        assert_eq!(media.kind, ChatMediaKind::Image);
        assert_eq!(media.url.as_deref(), Some("https://blob.invalid/sha256"));
        assert_eq!(media.mime_type, "image/jpeg");
        assert_eq!(media.filename, "photo.jpg");
        assert_eq!(media.width, Some(640));
        assert_eq!(media.height, Some(480));
        assert_eq!(media.local_path.as_deref(), Some("/tmp/local-preview.jpg"));
    }

    #[test]
    fn app_create_room_requires_durable_server_success() {
        let dir = tempfile::tempdir().unwrap();
        let app = FiniteChatRuntime::open(OpenOptions {
            data_dir: dir.path().join("alice").to_string_lossy().into_owned(),
            server_url: "http://127.0.0.1:1".to_owned(),
            device_id: "alice-ios".to_owned(),
            account_secret_hex: None,
            now_unix_seconds: Some(NOW),
        })
        .unwrap();

        let error = app
            .dispatch(AppAction::CreateRoom {
                display_name: "No Server".to_owned(),
            })
            .expect_err("server failure rejects room creation");
        assert!(
            error.to_string().contains("delivery error"),
            "unexpected error: {error}"
        );
        let state = app.state().unwrap();
        assert!(state.rooms.is_empty());
        assert_eq!(state.status, "ready");
    }

    #[test]
    fn app_create_room_rejects_oversized_display_name_before_side_effects() {
        let dir = tempfile::tempdir().unwrap();
        let app = FiniteChatRuntime::open(OpenOptions {
            data_dir: dir.path().join("alice").to_string_lossy().into_owned(),
            server_url: unavailable_http_server_url(),
            device_id: "alice-ios".to_owned(),
            account_secret_hex: None,
            now_unix_seconds: Some(NOW),
        })
        .unwrap();

        let error = app
            .dispatch(AppAction::CreateRoom {
                display_name: "x".repeat(MAX_INVITE_DISPLAY_NAME_BYTES as usize + 1),
            })
            .expect_err("oversized room labels fail before network or storage side effects");
        assert!(matches!(error, FiniteChatCoreError::Client { .. }));
        assert!(app.state().unwrap().rooms.is_empty());
    }

    #[test]
    fn app_scan_npub_loads_server_backed_profile_cache() {
        let dir = tempfile::tempdir().unwrap();
        let server_url = spawn_live_http_server(dir.path().join("server.sqlite3"));
        let account_id =
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_owned();
        let npub = npub_encode(&account_id).unwrap();
        put_profile(
            &server_url,
            NostrProfileRecord {
                account_id: account_id.clone(),
                name: Some("alice".to_owned()),
                display_name: Some("Alice Finite".to_owned()),
                about: Some("profile cache test".to_owned()),
                picture: Some("https://example.invalid/alice.png".to_owned()),
                fetched_at_ms: NOW.saturating_mul(1000).saturating_sub(1_000),
                expires_at_ms: NOW.saturating_mul(1000).saturating_add(60_000),
            },
        );
        let app = FiniteChatRuntime::open(OpenOptions {
            data_dir: dir.path().join("alice").to_string_lossy().into_owned(),
            server_url,
            device_id: "alice-ios".to_owned(),
            account_secret_hex: None,
            now_unix_seconds: Some(NOW),
        })
        .unwrap();

        let state = app
            .dispatch(AppAction::ScanTarget {
                value: npub.clone(),
            })
            .unwrap();
        assert_eq!(state.status, "profile loaded");
        assert_eq!(
            state.active_profile_id.as_deref(),
            Some(account_id.as_str())
        );
        assert_eq!(state.profiles.len(), 1);
        assert_eq!(state.profiles[0].account_id, account_id);
        assert_eq!(state.profiles[0].npub, npub);
        assert_eq!(state.profiles[0].display_name, "Alice Finite");
        assert_eq!(
            state.profiles[0].about.as_deref(),
            Some("profile cache test")
        );
        assert!(!state.profiles[0].stale);
    }

    #[test]
    fn app_scan_missing_npub_surfaces_stale_profile_placeholder() {
        let dir = tempfile::tempdir().unwrap();
        let server_url = spawn_live_http_server(dir.path().join("server.sqlite3"));
        let account_id =
            "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789".to_owned();
        let npub = npub_encode(&account_id).unwrap();
        let app = FiniteChatRuntime::open(OpenOptions {
            data_dir: dir.path().join("alice").to_string_lossy().into_owned(),
            server_url,
            device_id: "alice-ios".to_owned(),
            account_secret_hex: None,
            now_unix_seconds: Some(NOW),
        })
        .unwrap();

        let state = app
            .dispatch(AppAction::ScanTarget {
                value: format!("nostr:{npub}"),
            })
            .unwrap();
        assert_eq!(state.status, "profile not found");
        assert_eq!(
            state.toast.as_deref(),
            Some("No cached profile was available for that npub")
        );
        assert_eq!(
            state.active_profile_id.as_deref(),
            Some(account_id.as_str())
        );
        assert_eq!(state.profiles.len(), 1);
        assert_eq!(state.profiles[0].account_id, account_id);
        assert_eq!(state.profiles[0].npub, npub);
        assert!(state.profiles[0].stale);
    }

    #[test]
    fn app_runtime_auto_admits_invite_and_joiner_sends_without_protocol_actions() {
        let dir = tempfile::tempdir().unwrap();
        let server_url = spawn_live_http_server(dir.path().join("server.sqlite3"));
        let alice_dir = dir.path().join("alice");
        let alice = FiniteChatRuntime::open(OpenOptions {
            data_dir: alice_dir.to_string_lossy().into_owned(),
            server_url: server_url.clone(),
            device_id: "alice-ios".to_owned(),
            account_secret_hex: None,
            now_unix_seconds: Some(NOW),
        })
        .unwrap();
        let bob = FiniteChatRuntime::open(OpenOptions {
            data_dir: dir.path().join("bob").to_string_lossy().into_owned(),
            server_url: server_url.clone(),
            device_id: "bob-ios".to_owned(),
            account_secret_hex: None,
            now_unix_seconds: Some(NOW),
        })
        .unwrap();

        let alice_state = alice
            .dispatch(AppAction::CreateRoom {
                display_name: "Agent Room".to_owned(),
            })
            .unwrap();
        let room = alice_state.rooms.first().expect("created room");
        let room_id = room.room_id.clone();
        assert_eq!(room.display_name, "Agent Room");
        assert_eq!(room.state, AppRoomState::Connected);

        let alice_state = alice
            .dispatch(AppAction::CreateInvite {
                room_id: room_id.clone(),
            })
            .unwrap();
        let invite = alice_state.active_invite.expect("active invite");

        let bob_state = bob
            .dispatch(AppAction::ScanTarget {
                value: invite.invite_url.clone(),
            })
            .unwrap();
        let pending = app_room(&bob_state, &room_id);
        assert_eq!(pending.state, AppRoomState::WaitingForApproval);
        assert_eq!(pending.status, "enter PIN to request admission");

        let bob_state = bob
            .dispatch(AppAction::SubmitInvitePin {
                pending_room_id: room_id.clone(),
                pin: invite.pin,
            })
            .unwrap();
        let pending = app_room(&bob_state, &room_id);
        assert_eq!(pending.state, AppRoomState::WaitingForApproval);
        assert_eq!(pending.status, "waiting for room admission");

        alice.dispatch(AppAction::StartRuntime).unwrap();
        let bob_state = bob
            .dispatch(AppAction::RetryRoom {
                room_id: room_id.clone(),
            })
            .unwrap();
        assert_eq!(
            app_room(&bob_state, &room_id).state,
            AppRoomState::Connected
        );

        let sent = bob
            .dispatch(AppAction::SendMessage {
                room_id: room_id.clone(),
                text: "hello from app actor".to_owned(),
            })
            .unwrap();
        assert!(
            sent.messages
                .iter()
                .any(|message| message.text == "hello from app actor")
        );

        let alice_state = alice.dispatch(AppAction::StartRuntime).unwrap();
        assert!(
            alice_state
                .messages
                .iter()
                .any(|message| message.text == "hello from app actor")
        );
        drop(alice);

        let reopened = FiniteChatRuntime::open(OpenOptions {
            data_dir: alice_dir.to_string_lossy().into_owned(),
            server_url: server_url.clone(),
            device_id: "alice-ios".to_owned(),
            account_secret_hex: None,
            now_unix_seconds: Some(NOW),
        })
        .unwrap();
        let reopened_state = reopened.state().unwrap();
        assert!(
            reopened_state
                .messages
                .iter()
                .any(|message| message.text == "hello from app actor"),
            "message projection should survive runtime reopen"
        );
        assert_eq!(
            app_room(&reopened_state, &room_id).last_message_preview,
            "hello from app actor"
        );
    }

    #[test]
    fn app_start_runtime_returns_durable_chat_when_delivery_is_offline() {
        let dir = tempfile::tempdir().unwrap();
        let server_url = spawn_live_http_server(dir.path().join("server.sqlite3"));
        let alice_dir = dir.path().join("alice");
        let alice = FiniteChatRuntime::open(OpenOptions {
            data_dir: alice_dir.to_string_lossy().into_owned(),
            server_url,
            device_id: "alice-ios".to_owned(),
            account_secret_hex: None,
            now_unix_seconds: Some(NOW),
        })
        .unwrap();

        let alice_state = alice
            .dispatch(AppAction::CreateRoom {
                display_name: "Local First".to_owned(),
            })
            .unwrap();
        let room_id = alice_state.rooms.first().unwrap().room_id.clone();
        alice
            .dispatch(AppAction::SendMessage {
                room_id: room_id.clone(),
                text: "saved before force close".to_owned(),
            })
            .unwrap();
        drop(alice);

        let reopened = FiniteChatRuntime::open(OpenOptions {
            data_dir: alice_dir.to_string_lossy().into_owned(),
            server_url: unavailable_http_server_url(),
            device_id: "alice-ios".to_owned(),
            account_secret_hex: None,
            now_unix_seconds: Some(NOW),
        })
        .unwrap();

        let local_snapshot = reopened.state().unwrap();
        assert_eq!(
            app_room(&local_snapshot, &room_id).display_name,
            "Local First"
        );
        assert!(
            local_snapshot
                .messages
                .iter()
                .any(|message| message.text == "saved before force close"),
            "force-close reopen must render the durable local transcript before sync"
        );

        let started = reopened.dispatch(AppAction::StartRuntime).unwrap();
        assert_eq!(started.status, "offline");
        assert_eq!(
            started.toast.as_deref(),
            Some("Showing saved chats. Connection will retry.")
        );
        assert!(
            started
                .messages
                .iter()
                .any(|message| message.text == "saved before force close"),
            "startup sync failure must not hide the durable local transcript"
        );
        assert_eq!(app_room(&started, &room_id).state, AppRoomState::Connected);
    }

    #[test]
    fn app_runtime_recovers_invite_join_projection_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let server_url = spawn_live_http_server(dir.path().join("server.sqlite3"));
        let alice_dir = dir.path().join("alice");
        let bob_dir = dir.path().join("bob");
        let alice = FiniteChatRuntime::open(OpenOptions {
            data_dir: alice_dir.to_string_lossy().into_owned(),
            server_url: server_url.clone(),
            device_id: "alice-ios".to_owned(),
            account_secret_hex: None,
            now_unix_seconds: Some(NOW),
        })
        .unwrap();
        let alice_state = alice
            .dispatch(AppAction::CreateRoom {
                display_name: "Recovered Join".to_owned(),
            })
            .unwrap();
        let room_id = alice_state.rooms.first().unwrap().room_id.clone();
        let invite = alice
            .dispatch(AppAction::CreateInvite {
                room_id: room_id.clone(),
            })
            .unwrap()
            .active_invite
            .unwrap();
        drop(alice);

        let alice = FiniteChatRuntime::open(OpenOptions {
            data_dir: alice_dir.to_string_lossy().into_owned(),
            server_url: server_url.clone(),
            device_id: "alice-ios".to_owned(),
            account_secret_hex: None,
            now_unix_seconds: Some(NOW),
        })
        .unwrap();
        assert_eq!(
            app_room(&alice.state().unwrap(), &room_id).state,
            AppRoomState::Connected
        );

        let bob = FiniteChatRuntime::open(OpenOptions {
            data_dir: bob_dir.to_string_lossy().into_owned(),
            server_url: server_url.clone(),
            device_id: "bob-ios".to_owned(),
            account_secret_hex: None,
            now_unix_seconds: Some(NOW),
        })
        .unwrap();
        let scanned = bob
            .dispatch(AppAction::ScanTarget {
                value: invite.invite_url.clone(),
            })
            .unwrap();
        assert_eq!(
            app_room(&scanned, &room_id).state,
            AppRoomState::WaitingForApproval
        );
        assert_eq!(
            app_room(&scanned, &room_id).status,
            "enter PIN to request admission"
        );
        drop(bob);

        let bob = FiniteChatRuntime::open(OpenOptions {
            data_dir: bob_dir.to_string_lossy().into_owned(),
            server_url: server_url.clone(),
            device_id: "bob-ios".to_owned(),
            account_secret_hex: None,
            now_unix_seconds: Some(NOW),
        })
        .unwrap();
        let recovered = bob.state().unwrap();
        assert_eq!(
            app_room(&recovered, &room_id).state,
            AppRoomState::WaitingForApproval
        );
        assert_eq!(
            app_room(&recovered, &room_id).status,
            "enter PIN to request admission"
        );
        let requested = bob
            .dispatch(AppAction::SubmitInvitePin {
                pending_room_id: room_id.clone(),
                pin: invite.pin,
            })
            .unwrap();
        assert_eq!(
            app_room(&requested, &room_id).status,
            "waiting for room admission"
        );
        assert_eq!(
            app_room(&requested, &room_id).display_name,
            "Recovered Join"
        );
        drop(bob);

        let bob = FiniteChatRuntime::open(OpenOptions {
            data_dir: bob_dir.to_string_lossy().into_owned(),
            server_url: server_url.clone(),
            device_id: "bob-ios".to_owned(),
            account_secret_hex: None,
            now_unix_seconds: Some(NOW),
        })
        .unwrap();
        let waiting = bob.state().unwrap();
        assert_eq!(
            app_room(&waiting, &room_id).state,
            AppRoomState::WaitingForApproval
        );
        assert_eq!(
            app_room(&waiting, &room_id).status,
            "waiting for room admission"
        );
        assert_eq!(app_room(&waiting, &room_id).display_name, "Recovered Join");

        let alice_state = alice.dispatch(AppAction::StartRuntime).unwrap();
        assert_eq!(
            alice_state.toast.as_deref(),
            Some("1 device(s) joined"),
            "creator invite watch must survive relaunch and admit the joiner"
        );
        let bob_state = bob.dispatch(AppAction::StartRuntime).unwrap();
        assert_eq!(
            app_room(&bob_state, &room_id).state,
            AppRoomState::Connected
        );
    }

    #[test]
    fn app_runtime_wait_for_update_uses_sse_hints_for_admission_and_messages() {
        let dir = tempfile::tempdir().unwrap();
        let server_url = spawn_live_http_server(dir.path().join("server.sqlite3"));
        let alice = FiniteChatRuntime::open(OpenOptions {
            data_dir: dir.path().join("alice").to_string_lossy().into_owned(),
            server_url: server_url.clone(),
            device_id: "alice-ios".to_owned(),
            account_secret_hex: None,
            now_unix_seconds: Some(NOW),
        })
        .unwrap();
        let bob = FiniteChatRuntime::open(OpenOptions {
            data_dir: dir.path().join("bob").to_string_lossy().into_owned(),
            server_url,
            device_id: "bob-ios".to_owned(),
            account_secret_hex: None,
            now_unix_seconds: Some(NOW),
        })
        .unwrap();

        let alice_state = alice
            .dispatch(AppAction::CreateRoom {
                display_name: "Hint Room".to_owned(),
            })
            .unwrap();
        let room_id = alice_state.rooms.first().unwrap().room_id.clone();
        let invite = alice
            .dispatch(AppAction::CreateInvite {
                room_id: room_id.clone(),
            })
            .unwrap()
            .active_invite
            .unwrap();

        bob.dispatch(AppAction::ScanTarget {
            value: invite.invite_url.clone(),
        })
        .unwrap();
        bob.dispatch(AppAction::SubmitInvitePin {
            pending_room_id: room_id.clone(),
            pin: invite.pin,
        })
        .unwrap();

        let alice_state = alice.wait_for_update(1_000).unwrap();
        assert_eq!(
            alice_state.toast.as_deref(),
            Some("1 device(s) joined"),
            "creator should wake from invite SSE hint and admit the joiner"
        );

        let bob_state = bob.wait_for_update(1_000).unwrap();
        assert_eq!(
            app_room(&bob_state, &room_id).state,
            AppRoomState::Connected,
            "joiner should wake from invite SSE hint and finalize without a visible retry"
        );

        bob.dispatch(AppAction::SendMessage {
            room_id: room_id.clone(),
            text: "hello over app sse".to_owned(),
        })
        .unwrap();
        let alice_state = alice.wait_for_update(1_000).unwrap();
        assert!(
            alice_state
                .messages
                .iter()
                .any(|message| message.text == "hello over app sse"),
            "receiver should sync the message after the room high-watermark hint"
        );
    }

    #[test]
    fn app_runtime_lists_and_revokes_same_account_devices() {
        let dir = tempfile::tempdir().unwrap();
        let server_url = spawn_live_http_server(dir.path().join("server.sqlite3"));
        let alice = FiniteChatRuntime::open(OpenOptions {
            data_dir: dir
                .path()
                .join("alice-phone")
                .to_string_lossy()
                .into_owned(),
            server_url: server_url.clone(),
            device_id: "alice-phone".to_owned(),
            account_secret_hex: None,
            now_unix_seconds: Some(NOW),
        })
        .unwrap();
        let alice_identity = alice.state().unwrap().identity;
        let tablet = FiniteChatRuntime::open(OpenOptions {
            data_dir: dir
                .path()
                .join("alice-tablet")
                .to_string_lossy()
                .into_owned(),
            server_url,
            device_id: "alice-tablet".to_owned(),
            account_secret_hex: Some(alice_identity.account_secret_hex.clone()),
            now_unix_seconds: Some(NOW),
        })
        .unwrap();

        let alice_state = alice
            .dispatch(AppAction::CreateRoom {
                display_name: "Device Room".to_owned(),
            })
            .unwrap();
        let room_id = alice_state.rooms.first().unwrap().room_id.clone();
        let invite = alice
            .dispatch(AppAction::CreateInvite {
                room_id: room_id.clone(),
            })
            .unwrap()
            .active_invite
            .unwrap();
        tablet
            .dispatch(AppAction::ScanTarget {
                value: invite.invite_url.clone(),
            })
            .unwrap();
        tablet
            .dispatch(AppAction::SubmitInvitePin {
                pending_room_id: room_id.clone(),
                pin: invite.pin,
            })
            .unwrap();
        alice.dispatch(AppAction::StartRuntime).unwrap();
        tablet.dispatch(AppAction::RetryRoom { room_id }).unwrap();

        let devices = alice.dispatch(AppAction::RefreshDevices).unwrap();
        assert_device(&devices, "alice-phone", true, true, false);
        assert_device(&devices, "alice-tablet", true, false, false);

        let devices = alice
            .dispatch(AppAction::RevokeDevice {
                account_id: alice_identity.account_id,
                device_id: "alice-tablet".to_owned(),
            })
            .unwrap();
        assert_device(&devices, "alice-tablet", true, false, true);
    }

    #[test]
    fn app_invite_marks_stale_local_room_needs_attention() {
        let dir = tempfile::tempdir().unwrap();
        let alice_dir = dir.path().join("alice");
        let first_server_url = spawn_live_http_server(dir.path().join("server-one.sqlite3"));
        let alice = FiniteChatRuntime::open(OpenOptions {
            data_dir: alice_dir.to_string_lossy().into_owned(),
            server_url: first_server_url,
            device_id: "alice-ios".to_owned(),
            account_secret_hex: None,
            now_unix_seconds: Some(NOW),
        })
        .unwrap();
        let alice_state = alice
            .dispatch(AppAction::CreateRoom {
                display_name: "Stale Room".to_owned(),
            })
            .unwrap();
        let room_id = alice_state.rooms.first().unwrap().room_id.clone();
        drop(alice);

        let empty_server_url = spawn_live_http_server(dir.path().join("server-two.sqlite3"));
        let stale = FiniteChatRuntime::open(OpenOptions {
            data_dir: alice_dir.to_string_lossy().into_owned(),
            server_url: empty_server_url,
            device_id: "alice-ios".to_owned(),
            account_secret_hex: None,
            now_unix_seconds: Some(NOW),
        })
        .unwrap();

        let stale_state = stale
            .dispatch(AppAction::CreateInvite {
                room_id: room_id.clone(),
            })
            .unwrap();
        let room = app_room(&stale_state, &room_id);
        assert_eq!(room.state, AppRoomState::NeedsAttention);
        assert!(room.status.contains("room"));
        assert!(room.status.contains("does not exist"));
        assert_eq!(stale_state.active_invite, None);
        assert_eq!(
            stale_state.toast.as_deref(),
            Some("Invite could not be created")
        );
    }

    fn texts(result: &SyncResult) -> Vec<&str> {
        result
            .messages
            .iter()
            .map(|message| message.text.as_str())
            .collect()
    }

    fn app_room<'a>(state: &'a AppState, room_id: &str) -> &'a AppRoomSummary {
        state
            .rooms
            .iter()
            .find(|room| room.room_id == room_id)
            .unwrap_or_else(|| panic!("missing app room {room_id}"))
    }

    fn assert_device(
        state: &AppState,
        device_id: &str,
        active: bool,
        current_device: bool,
        revoked: bool,
    ) {
        let device = state
            .devices
            .iter()
            .find(|device| device.device_id == device_id)
            .unwrap_or_else(|| panic!("missing device {device_id}"));
        assert_eq!(device.active, active);
        assert_eq!(device.current_device, current_device);
        assert_eq!(device.revoked, revoked);
        assert_eq!(device.room_count, 1);
    }

    fn put_profile(server_url: &str, profile: NostrProfileRecord) {
        let response = reqwest::blocking::Client::new()
            .post(format!("{server_url}/profiles/nostr"))
            .json(&PutNostrProfileRequest { profile })
            .send()
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
    }

    fn spawn_live_http_server(path: impl AsRef<Path>) -> String {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let app = http_router(HttpServerState::from_sqlite_path(path).unwrap());
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Runtime::new().unwrap();
            runtime.block_on(async move {
                let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                axum::serve(listener, app).await.unwrap();
            });
        });
        let server_url = format!("http://{addr}");
        wait_for_live_http_server(&server_url);
        server_url
    }

    fn unavailable_http_server_url() -> String {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        format!("http://{addr}")
    }

    fn wait_for_live_http_server(server_url: &str) {
        let health_url = format!("{}/health", server_url.trim_end_matches('/'));
        let client = reqwest::blocking::Client::new();
        for _ in 0..100 {
            if client
                .get(&health_url)
                .send()
                .map(|response| response.status().is_success())
                .unwrap_or(false)
            {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("live HTTP test server did not become healthy at {health_url}");
    }
}
