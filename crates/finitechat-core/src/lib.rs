use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use finitechat_blob::{
    BlobDescriptor, BlossomDownloadHttpResponse, BlossomUploadHttpResponse,
    finish_blossom_download_http_response, finish_blossom_upload_http_response,
    prepare_attachment_upload, prepare_blossom_download_http_request,
    prepare_blossom_upload_http_request,
};
use finitechat_client::{
    AppliedLogEntry, ClientError, ClientStoreError, CreateRoomInviteParams, FiniteChatDevice,
    FiniteChatDeviceConfig, HttpRuntimeDelivery, ReqwestHttpRuntimeTransport, RuntimeDelivery,
    RuntimeSyncOptions, SqliteClientStore, SqliteClientStoreOptions, StoredAppEvent,
    StoredAppMessage, StoredAppProfile, StoredAppRoom, StoredAppRoomState, StoredAppState,
    accept_pending_invite_joins, create_room_invite, finalize_invited_room,
    generate_account_secret, run_room_server_sync_tick, run_runtime_sync_tick,
    submit_invite_join_request,
};
use finitechat_hermes::{HermesAttachmentKindV1, HermesAttachmentV1, HermesMessagePayloadV1};
use finitechat_http::{SyncHintEvent, SyncStreamRequest, SyncWaitInvite, SyncWaitRoom};
use finitechat_mls::{NOSTR_SECRET_KEY_BYTES, NostrSecretKey};
use finitechat_proto::{
    AttachmentBlobMetadataV1, AttachmentBlobReferenceV1, ChatReactionV1, ChatReceiptStateV1,
    ChatReceiptV1, CreateRoomRequest, DecryptedApplicationEventV1, DeviceRef, DurableAppEventKind,
    InviteCodeV1, ListAccountRoomsRequest, MAX_INVITE_DISPLAY_NAME_BYTES, RoomProtocol,
    invite_current_pin, npub_decode, npub_encode,
};
use reqwest::header::CONTENT_TYPE;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

const ACCOUNT_SECRET_FILE: &str = "account-secret.hex";
const CLIENT_STORE_FILE: &str = "client.sqlite3";
const ATTACHMENT_CACHE_DIR: &str = "attachments";
const LEGACY_APP_MESSAGES_FILE: &str = "app-messages.json";
const MAX_APP_MESSAGES: usize = 5_000;
const MAX_APP_MESSAGES_U32: u32 = 5_000;
const DEFAULT_TRANSCRIPT_WINDOW: usize = 50;
const MAX_TRANSCRIPT_PAGE_SIZE: u32 = 100;
const DEFAULT_KEY_PACKAGE_TARGET_AVAILABLE: u32 = 2;
const DEFAULT_MAX_SYNC_PAGES_PER_ROOM: u32 = 16;
const DEFAULT_CREDENTIAL_VALIDITY_SECONDS: u64 = 10 * 365 * 24 * 60 * 60;
const DEFAULT_INVITE_TTL_MS: u64 = 15 * 60 * 1000;
const DEFAULT_INVITE_MAX_JOINS: u32 = 32;
const DEFAULT_APP_UPDATE_WAIT_MILLIS: u64 = 30_000;
const MIN_APP_UPDATE_WAIT_MILLIS: u64 = 1_000;
const MAX_APP_UPDATE_WAIT_MILLIS: u64 = 60_000;

const _: () = {
    assert!(MAX_APP_MESSAGES > 0);
    assert!(MAX_APP_MESSAGES_U32 as usize == MAX_APP_MESSAGES);
    assert!(DEFAULT_TRANSCRIPT_WINDOW > 0);
    assert!(DEFAULT_TRANSCRIPT_WINDOW <= MAX_APP_MESSAGES);
    assert!(MAX_TRANSCRIPT_PAGE_SIZE > 0);
};

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
    pub can_load_older: bool,
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
    SendReply {
        room_id: String,
        text: String,
        reply_to_message_id: String,
    },
    SendAttachment {
        room_id: String,
        filename: String,
        mime_type: String,
        kind: ChatMediaKind,
        bytes: Vec<u8>,
        caption: String,
        reply_to_message_id: Option<String>,
    },
    DownloadAttachment {
        room_id: String,
        message_id: String,
        attachment_id: String,
    },
    LoadOlderMessages {
        room_id: String,
        before_message_id: String,
        limit: u32,
    },
    ReactToMessage {
        room_id: String,
        message_id: String,
        emoji: String,
    },
    MarkRoomRead {
        room_id: String,
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
    chat_projection: ChatProjectionState,
    pending_invites: BTreeMap<String, PendingInvite>,
    owned_invites: BTreeMap<String, String>,
    invite_watch_marks: BTreeMap<String, InviteWatchMark>,
    loaded_message_counts: BTreeMap<String, usize>,
    local_read_seq: BTreeMap<String, u64>,
    profile_cache: BTreeMap<String, AppProfileSummary>,
    revoked_devices: BTreeSet<String>,
}

struct SendAttachmentInput {
    room_id: String,
    filename: String,
    mime_type: String,
    kind: ChatMediaKind,
    bytes: Vec<u8>,
    caption: String,
    reply_to_message_id: Option<String>,
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

#[derive(Debug, Default)]
struct CoreSyncProjection {
    result: SyncResult,
    events: Vec<StoredAppEvent>,
}

#[derive(Clone, Debug, Default)]
struct ChatProjectionState {
    messages: BTreeMap<(String, String), ChatMessage>,
    reaction_senders: BTreeSet<(String, String, String, String)>,
    delivered_through: BTreeMap<(String, String), u64>,
    read_through: BTreeMap<(String, String), u64>,
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

    pub fn send_attachment(
        &self,
        room_id: String,
        filename: String,
        mime_type: String,
        kind: ChatMediaKind,
        bytes: Vec<u8>,
        caption: String,
    ) -> Result<SyncResult, FiniteChatCoreError> {
        let mut state = self.lock()?;
        state.send_attachment(SendAttachmentInput {
            room_id,
            filename,
            mime_type,
            kind,
            bytes,
            caption,
            reply_to_message_id: None,
        })
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
        let chat_projection =
            ChatProjectionState::from_stored(stored_messages, stored_events, &owner);
        let all_messages = chat_projection.messages();
        let stored_rooms = core.store.load_app_rooms(&owner).map_err(store_error)?;
        let known_room_ids = core.known_room_ids().into_iter().collect::<BTreeSet<_>>();
        let mut persisted_room_ids = BTreeSet::new();
        let mut pending_invites = BTreeMap::new();
        let mut owned_invites = BTreeMap::new();
        let mut local_read_seq = BTreeMap::new();
        let mut rooms = Vec::new();
        for stored_room in stored_rooms {
            let room_id = stored_room.room_id.clone();
            let has_mls_room = known_room_ids.contains(&room_id);
            persisted_room_ids.insert(room_id.clone());
            local_read_seq.insert(room_id.clone(), stored_room.local_read_seq);
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
                local_read_seq.entry(room_id.clone()).or_default();
                rooms.push(connected_app_room(&room_id, &room_id));
            }
        }
        sort_app_rooms(&mut rooms);
        apply_room_message_projection(&mut rooms, &all_messages, &local_read_seq);
        let stored_app_state = core.store.load_app_state(&owner).map_err(store_error)?;
        let selected_room_id = selected_room_id_from_stored(&rooms, stored_app_state);
        let mut loaded_message_counts = BTreeMap::new();
        if let Some(room_id) = selected_room_id.clone() {
            loaded_message_counts.insert(room_id, DEFAULT_TRANSCRIPT_WINDOW);
        }
        let mut state = Self {
            core,
            app: AppState {
                rev: 0,
                identity,
                selected_room_id,
                rooms,
                active_invite: None,
                active_profile_id: None,
                status: "ready".to_owned(),
                toast: None,
                messages: Vec::new(),
                profiles: Vec::new(),
                devices: Vec::new(),
            },
            chat_projection,
            pending_invites,
            owned_invites,
            invite_watch_marks: BTreeMap::new(),
            loaded_message_counts,
            local_read_seq,
            profile_cache: BTreeMap::new(),
            revoked_devices: BTreeSet::new(),
        };
        state.sync_selected_room_messages();
        state.load_profile_cache()?;
        Ok(state)
    }

    fn dispatch(&mut self, action: AppAction) -> Result<(), FiniteChatCoreError> {
        self.app.toast = None;
        match action {
            AppAction::StartRuntime => self.start_runtime()?,
            AppAction::StopRuntime => self.app.status = "stopped".to_owned(),
            AppAction::OpenRoom { room_id } => self.open_room(room_id)?,
            AppAction::CreateRoom { display_name } => self.create_room(display_name)?,
            AppAction::CreateInvite { room_id } => self.create_invite(room_id)?,
            AppAction::ScanTarget { value } => self.scan_target(value)?,
            AppAction::SubmitInvitePin {
                pending_room_id,
                pin,
            } => self.submit_invite_pin(pending_room_id, pin)?,
            AppAction::SendMessage { room_id, text } => self.send_message(room_id, text)?,
            AppAction::SendReply {
                room_id,
                text,
                reply_to_message_id,
            } => self.send_reply(room_id, text, reply_to_message_id)?,
            AppAction::SendAttachment {
                room_id,
                filename,
                mime_type,
                kind,
                bytes,
                caption,
                reply_to_message_id,
            } => self.send_attachment(SendAttachmentInput {
                room_id,
                filename,
                mime_type,
                kind,
                bytes,
                caption,
                reply_to_message_id,
            })?,
            AppAction::DownloadAttachment {
                room_id,
                message_id,
                attachment_id,
            } => self.download_attachment(room_id, message_id, attachment_id)?,
            AppAction::LoadOlderMessages {
                room_id,
                before_message_id,
                limit,
            } => self.load_older_messages(room_id, before_message_id, limit)?,
            AppAction::ReactToMessage {
                room_id,
                message_id,
                emoji,
            } => self.react_to_message(room_id, message_id, emoji)?,
            AppAction::MarkRoomRead { room_id } => self.mark_room_read(room_id)?,
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
        let synced = self.core.sync_with_projection()?;
        self.apply_projection_events(synced.events);
        self.append_messages(synced.result.messages);
        self.try_finalize_pending_rooms()?;
        self.app.status = "ready".to_owned();
        Ok(())
    }

    fn open_room(&mut self, room_id: String) -> Result<(), FiniteChatCoreError> {
        self.app.selected_room_id = Some(room_id.clone());
        self.loaded_message_counts
            .entry(room_id.clone())
            .or_insert(DEFAULT_TRANSCRIPT_WINDOW);
        if self.room_mut(&room_id).is_none() {
            self.upsert_room(
                &room_id,
                &room_id,
                AppRoomState::NeedsAttention,
                "room is not available on this device",
            );
        }
        self.persist_app_state()?;
        self.sync_selected_room_messages();
        Ok(())
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
        self.persist_app_state()?;
        self.sync_selected_room_messages();
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
                self.persist_profile(&profile)?;
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
        self.persist_app_state()?;
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
        self.persist_app_state()?;
        self.app.status = "join requested".to_owned();
        Ok(())
    }

    fn send_message(&mut self, room_id: String, text: String) -> Result<(), FiniteChatCoreError> {
        self.send_message_with_reply(room_id, text, None)
    }

    fn send_reply(
        &mut self,
        room_id: String,
        text: String,
        reply_to_message_id: String,
    ) -> Result<(), FiniteChatCoreError> {
        let target_id = reply_to_message_id.trim();
        self.validate_reply_target(&room_id, target_id)?;
        self.send_message_with_reply(room_id, text, Some(target_id.to_owned()))
    }

    fn send_message_with_reply(
        &mut self,
        room_id: String,
        text: String,
        reply_to_message_id: Option<String>,
    ) -> Result<(), FiniteChatCoreError> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Ok(());
        }
        if !self.room_is_connected(&room_id) {
            return Err(FiniteChatCoreError::Client {
                reason: format!("room '{room_id}' is not ready to send"),
            });
        }
        let result =
            self.core
                .send_text_with_reply(&room_id, trimmed, reply_to_message_id.as_deref())?;
        self.append_messages(result.messages);
        if let Some(room) = self.room_mut(&room_id) {
            room.last_message_preview = trimmed.to_owned();
        }
        self.app.status = "sent".to_owned();
        Ok(())
    }

    fn send_attachment(
        &mut self,
        mut input: SendAttachmentInput,
    ) -> Result<(), FiniteChatCoreError> {
        if !self.room_is_connected(&input.room_id) {
            return Err(FiniteChatCoreError::Client {
                reason: format!("room '{}' is not ready to send", input.room_id),
            });
        }
        input.reply_to_message_id =
            self.normalize_reply_target(&input.room_id, input.reply_to_message_id)?;
        let result = self.core.send_attachment(input)?;
        self.append_messages(result.messages);
        self.app.status = "sent".to_owned();
        Ok(())
    }

    fn download_attachment(
        &mut self,
        room_id: String,
        message_id: String,
        attachment_id: String,
    ) -> Result<(), FiniteChatCoreError> {
        if !self.room_is_connected(&room_id) {
            return Err(FiniteChatCoreError::Client {
                reason: format!("room '{room_id}' is not ready to download attachments"),
            });
        }
        let Some(message) = self
            .app
            .messages
            .iter()
            .find(|message| message.room_id == room_id && message.message_id == message_id)
        else {
            return Err(FiniteChatCoreError::Client {
                reason: format!("message '{message_id}' is not available in room '{room_id}'"),
            });
        };
        let reference = attachment_reference_for_id(message, &attachment_id).ok_or_else(|| {
            FiniteChatCoreError::Client {
                reason: format!(
                    "attachment '{attachment_id}' is not available on message '{message_id}'"
                ),
            }
        })?;
        let path = self.core.download_attachment_blob(&reference)?;
        self.sync_chat_projection();
        let filename = reference.metadata.filename.trim();
        let display_name = if filename.is_empty() {
            "attachment"
        } else {
            filename
        };
        self.app.status = format!("downloaded {display_name}");
        debug_assert!(path.is_file());
        Ok(())
    }

    fn load_older_messages(
        &mut self,
        room_id: String,
        before_message_id: String,
        limit: u32,
    ) -> Result<(), FiniteChatCoreError> {
        if self.room(&room_id).is_none() {
            return Err(FiniteChatCoreError::Client {
                reason: format!("room '{room_id}' is not available"),
            });
        }

        if self.app.selected_room_id.as_deref() != Some(room_id.as_str()) {
            self.app.selected_room_id = Some(room_id.clone());
            self.loaded_message_counts
                .entry(room_id.clone())
                .or_insert(DEFAULT_TRANSCRIPT_WINDOW);
            self.persist_app_state()?;
            self.sync_selected_room_messages();
            return Ok(());
        }

        if let Some(oldest) = self.app.messages.first()
            && oldest.message_id != before_message_id
        {
            self.sync_selected_room_messages();
            return Ok(());
        }

        let page_size = normalized_transcript_page_size(limit);
        let current_count = self.loaded_message_count(&room_id);
        let total_count = self.chat_projection.room_message_count(&room_id);
        let next_count = current_count
            .saturating_add(page_size)
            .min(total_count)
            .min(MAX_APP_MESSAGES);
        self.loaded_message_counts.insert(
            room_id.clone(),
            next_count.max(DEFAULT_TRANSCRIPT_WINDOW.min(total_count)),
        );
        self.sync_selected_room_messages();
        self.app.status = "loaded older messages".to_owned();
        Ok(())
    }

    fn react_to_message(
        &mut self,
        room_id: String,
        message_id: String,
        emoji: String,
    ) -> Result<(), FiniteChatCoreError> {
        let emoji = emoji.trim().to_owned();
        if emoji.is_empty() {
            return Ok(());
        }
        if !self.room_is_connected(&room_id) {
            return Err(FiniteChatCoreError::Client {
                reason: format!("room '{room_id}' is not ready to react"),
            });
        }
        let Some(message) = self
            .app
            .messages
            .iter()
            .find(|message| message.room_id == room_id && message.message_id == message_id)
        else {
            return Err(FiniteChatCoreError::Client {
                reason: format!("message '{message_id}' is not available in room '{room_id}'"),
            });
        };
        if message
            .reactions
            .iter()
            .any(|reaction| reaction.emoji == emoji && reaction.reacted_by_me)
        {
            return Ok(());
        }

        let event = self.core.send_reaction(&room_id, &message_id, &emoji)?;
        self.apply_projection_events(vec![event]);
        self.app.status = "reacted".to_owned();
        Ok(())
    }

    fn mark_room_read(&mut self, room_id: String) -> Result<(), FiniteChatCoreError> {
        if self.room(&room_id).is_none() {
            return Ok(());
        }
        if let Some((_, seq)) = self.chat_projection.latest_peer_message(&room_id) {
            let current = self
                .local_read_seq
                .get(&room_id)
                .copied()
                .unwrap_or_default();
            if seq > current {
                self.local_read_seq.insert(room_id.clone(), seq);
                self.persist_room_projection(&room_id)?;
                self.sync_chat_projection();
            }
        }
        if !self.room_is_connected(&room_id) {
            return Ok(());
        }
        let owner = self.core.device.device_ref().clone();
        let Some((message_id, seq)) = self
            .chat_projection
            .latest_peer_message_needing_read_receipt(&room_id, &owner)
        else {
            return Ok(());
        };

        match self
            .core
            .send_read_receipt(&room_id, &message_id, seq, ChatReceiptStateV1::Read)
        {
            Ok(event) => self.apply_projection_events(vec![event]),
            Err(FiniteChatCoreError::Delivery { .. }) => {}
            Err(error) => return Err(error),
        }
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
        let response = match delivery.get_nostr_profiles(account_ids.clone(), now_ms) {
            Ok(response) => response,
            Err(error) => {
                let found = account_ids
                    .iter()
                    .any(|account_id| self.profile_cache.contains_key(account_id));
                self.sync_profile_state();
                if found {
                    return Ok(true);
                }
                return Err(runtime_error(error));
            }
        };
        let mut found = false;
        let mut stored = Vec::new();
        for entry in response.profiles {
            found = true;
            stored.push(StoredAppProfile {
                profile: entry.profile.clone(),
                stale: entry.stale,
            });
            self.profile_cache.insert(
                entry.profile.account_id.clone(),
                profile_from_record(entry.profile, entry.stale),
            );
        }
        if !stored.is_empty() {
            let owner = self.core.device.device_ref().clone();
            self.core
                .store
                .save_app_profiles(&owner, &stored)
                .map_err(store_error)?;
        }
        self.sync_profile_state();
        Ok(found)
    }

    fn load_profile_cache(&mut self) -> Result<(), FiniteChatCoreError> {
        let owner = self.core.device.device_ref().clone();
        let now_ms = self.core.now_millis()?;
        let stored = self
            .core
            .store
            .load_app_profiles(&owner)
            .map_err(store_error)?;
        self.profile_cache.clear();
        for profile in stored {
            let stale = profile.stale || profile.profile.expires_at_ms <= now_ms;
            self.profile_cache.insert(
                profile.profile.account_id.clone(),
                profile_from_record(profile.profile, stale),
            );
        }
        self.sync_profile_state();
        Ok(())
    }

    fn persist_profile(&mut self, profile: &AppProfileSummary) -> Result<(), FiniteChatCoreError> {
        let owner = self.core.device.device_ref().clone();
        let stored = stored_profile_from_app(profile);
        self.core
            .store
            .save_app_profiles(&owner, std::slice::from_ref(&stored))
            .map_err(store_error)
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

    fn append_messages(&mut self, messages: Vec<ChatMessage>) {
        if messages.is_empty() {
            return;
        }
        let selected_room_id = self.app.selected_room_id.clone();
        let selected_message_count = selected_room_id.as_ref().map_or(0, |room_id| {
            messages
                .iter()
                .filter(|message| message.room_id == *room_id)
                .count()
        });
        self.chat_projection.append_messages(messages);
        if let Some(room_id) = selected_room_id
            && selected_message_count > 0
        {
            let current_count = self.loaded_message_count(&room_id);
            if current_count > DEFAULT_TRANSCRIPT_WINDOW {
                let next_count = current_count
                    .saturating_add(selected_message_count)
                    .min(MAX_APP_MESSAGES);
                self.loaded_message_counts.insert(room_id, next_count);
            }
        }
        self.sync_chat_projection();
    }

    fn apply_projection_events(&mut self, events: Vec<StoredAppEvent>) {
        if events.is_empty() {
            return;
        }
        let owner = self.core.device.device_ref().clone();
        for event in events {
            self.chat_projection.apply_event(event, &owner);
        }
        self.sync_chat_projection();
    }

    fn sync_chat_projection(&mut self) {
        let messages = self.chat_projection.messages();
        apply_room_message_projection(&mut self.app.rooms, &messages, &self.local_read_seq);
        self.sync_selected_room_messages();
    }

    fn sync_selected_room_messages(&mut self) {
        let Some(room_id) = self.app.selected_room_id.clone() else {
            self.app.messages.clear();
            self.sync_transcript_load_state();
            return;
        };
        let count = self.loaded_message_count(&room_id);
        let mut messages = self
            .chat_projection
            .messages_for_room_window(&room_id, count);
        self.core.apply_attachment_cache_paths(&mut messages);
        self.app.messages = messages;
        self.sync_transcript_load_state();
    }

    fn sync_transcript_load_state(&mut self) {
        let selected_room_id = self.app.selected_room_id.clone();
        let selected_can_load_older = selected_room_id.as_ref().is_some_and(|room_id| {
            self.chat_projection.room_message_count(room_id) > self.loaded_message_count(room_id)
        });
        for room in &mut self.app.rooms {
            room.can_load_older = selected_room_id.as_deref() == Some(room.room_id.as_str())
                && selected_can_load_older;
        }
    }

    fn loaded_message_count(&self, room_id: &str) -> usize {
        self.loaded_message_counts
            .get(room_id)
            .copied()
            .unwrap_or(DEFAULT_TRANSCRIPT_WINDOW)
            .min(MAX_APP_MESSAGES)
    }

    fn persist_room_projection(&mut self, room_id: &str) -> Result<(), FiniteChatCoreError> {
        let Some(room) = self.room(room_id).cloned() else {
            return Ok(());
        };
        let pending = self.pending_invites.get(room_id);
        let owned_invite_url = self.owned_invites.get(room_id);
        let local_read_seq = self
            .local_read_seq
            .get(room_id)
            .copied()
            .unwrap_or_default();
        let stored = stored_room_from_app(&room, local_read_seq, pending, owned_invite_url);
        let owner = self.core.device.device_ref().clone();
        self.core
            .store
            .save_app_rooms(&owner, std::slice::from_ref(&stored))
            .map_err(store_error)
    }

    fn persist_app_state(&mut self) -> Result<(), FiniteChatCoreError> {
        let owner = self.core.device.device_ref().clone();
        let stored = StoredAppState {
            selected_room_id: self.app.selected_room_id.clone(),
        };
        self.core
            .store
            .save_app_state(&owner, &stored)
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
            can_load_older: false,
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

    fn normalize_reply_target(
        &self,
        room_id: &str,
        reply_to_message_id: Option<String>,
    ) -> Result<Option<String>, FiniteChatCoreError> {
        let Some(reply_to_message_id) = reply_to_message_id else {
            return Ok(None);
        };
        let target_id = reply_to_message_id.trim();
        self.validate_reply_target(room_id, target_id)?;
        Ok(Some(target_id.to_owned()))
    }

    fn validate_reply_target(
        &self,
        room_id: &str,
        target_id: &str,
    ) -> Result<(), FiniteChatCoreError> {
        if target_id.is_empty() {
            return Err(FiniteChatCoreError::Client {
                reason: "reply target message id cannot be empty".to_owned(),
            });
        }
        if !self.chat_projection.message_exists(room_id, target_id) {
            return Err(FiniteChatCoreError::Client {
                reason: format!("reply target '{target_id}' is not available in room '{room_id}'"),
            });
        }
        Ok(())
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

fn stored_profile_from_app(profile: &AppProfileSummary) -> StoredAppProfile {
    StoredAppProfile {
        profile: finitechat_http::NostrProfileRecord {
            account_id: profile.account_id.clone(),
            name: None,
            display_name: Some(profile.display_name.clone()),
            about: profile.about.clone(),
            picture: profile.picture.clone(),
            fetched_at_ms: 0,
            expires_at_ms: 1,
        },
        stale: profile.stale,
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

fn normalized_transcript_page_size(limit: u32) -> usize {
    let limit = if limit == 0 {
        MAX_TRANSCRIPT_PAGE_SIZE
    } else {
        limit.min(MAX_TRANSCRIPT_PAGE_SIZE)
    };
    usize::try_from(limit).expect("u32 transcript page limit fits usize")
}

fn recover_or_create_device_state(
    data_dir: &Path,
    account_secret: &NostrSecretKey,
    requested_config: FiniteChatDeviceConfig,
    explicit_account_secret: bool,
) -> Result<(SqliteClientStore, FiniteChatDeviceConfig), FiniteChatCoreError> {
    let db_path = data_dir.join(CLIENT_STORE_FILE);
    let account_id = hex::encode(account_secret.public_key().as_bytes());
    let mut requested_store = SqliteClientStore::open(
        &db_path,
        SqliteClientStoreOptions::from_nostr_secret(account_secret, &requested_config.device_id)
            .map_err(store_error)?,
    )
    .map_err(store_error)?;
    let stored_device_ids = requested_store
        .load_device_ids_for_account(&account_id)
        .map_err(store_error)?;

    if stored_device_ids.is_empty() || explicit_account_secret {
        let device = FiniteChatDevice::new(requested_config.clone()).map_err(client_error)?;
        requested_store
            .save_device_state(&device)
            .map_err(store_error)?;
        return Ok((requested_store, requested_config));
    }

    if stored_device_ids.len() == 1 {
        let mut recovered_config = requested_config;
        recovered_config.device_id = stored_device_ids[0].clone();
        let recovered_store = SqliteClientStore::open(
            db_path,
            SqliteClientStoreOptions::from_nostr_secret(
                account_secret,
                &recovered_config.device_id,
            )
            .map_err(store_error)?,
        )
        .map_err(store_error)?;
        return Ok((recovered_store, recovered_config));
    }

    Err(FiniteChatCoreError::Client {
        reason: format!(
            "device state not found for requested device '{}'; stored devices for this account are: {}",
            requested_config.device_id,
            stored_device_ids.join(", ")
        ),
    })
}

impl CoreState {
    fn open(options: OpenOptions) -> Result<Self, FiniteChatCoreError> {
        let requested_device_id = options.device_id.trim().to_owned();
        if requested_device_id.is_empty() {
            return Err(FiniteChatCoreError::Client {
                reason: "device id cannot be empty".to_owned(),
            });
        }
        let explicit_account_secret = options.account_secret_hex.is_some();

        let data_dir = PathBuf::from(options.data_dir);
        fs::create_dir_all(&data_dir).map_err(|error| FiniteChatCoreError::Filesystem {
            reason: format!("failed to create {}: {error}", data_dir.display()),
        })?;

        let account_secret =
            load_or_create_account_secret(&data_dir, options.account_secret_hex.as_deref())?;
        let now = options
            .now_unix_seconds
            .unwrap_or_else(current_unix_seconds);
        let mut config = FiniteChatDeviceConfig {
            account_secret_key: account_secret.clone(),
            device_id: requested_device_id.clone(),
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
                let (next_store, recovered_config) = recover_or_create_device_state(
                    &data_dir,
                    &account_secret,
                    config,
                    explicit_account_secret,
                )?;
                store = next_store;
                config = recovered_config;
                store.load_device(config.clone()).map_err(store_error)?
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
        self.send_text_with_reply(room_id, text, None)
    }

    fn send_text_with_reply(
        &mut self,
        room_id: &str,
        text: &str,
        reply_to_message_id: Option<&str>,
    ) -> Result<SyncResult, FiniteChatCoreError> {
        let chat_payload = encode_text_message_payload(text, reply_to_message_id)?;
        self.send_chat_payload(room_id, chat_payload)
    }

    fn send_attachment(
        &mut self,
        input: SendAttachmentInput,
    ) -> Result<SyncResult, FiniteChatCoreError> {
        let room_id = input.room_id;
        let filename = input.filename.trim().to_owned();
        let mime_type = input.mime_type.trim().to_owned();
        let metadata = AttachmentBlobMetadataV1 {
            mime_type: mime_type.clone(),
            filename: filename.clone(),
            dimensions: None,
        };
        metadata.validate_limits().map_err(client_error)?;
        let room_server_url = self.room_server_url(&room_id);
        let reference = self.upload_attachment_blob(&room_server_url, &input.bytes, metadata)?;
        self.cache_attachment_plaintext(&reference, &input.bytes)?;
        let chat_payload = encode_attachment_message_payload(
            input.caption.trim(),
            &filename,
            &mime_type,
            input.kind,
            reference,
            input.reply_to_message_id.as_deref(),
        )?;
        let mut result = self.send_chat_payload(&room_id, chat_payload)?;
        self.apply_attachment_cache_paths(&mut result.messages);
        Ok(result)
    }

    fn send_chat_payload(
        &mut self,
        room_id: &str,
        chat_payload: Vec<u8>,
    ) -> Result<SyncResult, FiniteChatCoreError> {
        let idempotency_key = self
            .device
            .generate_object_id("msg")
            .map_err(client_error)?;
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

        let room_server_url = self.room_server_url(room_id);
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

    fn upload_attachment_blob(
        &self,
        server_url: &str,
        plaintext: &[u8],
        metadata: AttachmentBlobMetadataV1,
    ) -> Result<AttachmentBlobReferenceV1, FiniteChatCoreError> {
        let prepared = prepare_attachment_upload(plaintext, metadata).map_err(client_error)?;
        let request = prepare_blossom_upload_http_request(&prepared).map_err(client_error)?;
        let upload_url = format!("{}{}", server_url.trim_end_matches('/'), request.path);
        let response = reqwest::blocking::Client::new()
            .put(upload_url)
            .header(CONTENT_TYPE, request.content_type)
            .body(request.body.to_vec())
            .send()
            .map_err(delivery_error)?;
        let status = response.status();
        if !status.is_success() {
            return Err(delivery_error(format!(
                "blob upload failed with status {status}"
            )));
        }
        let descriptor = response.json::<BlobDescriptor>().map_err(delivery_error)?;
        finish_blossom_upload_http_response(
            &prepared,
            BlossomUploadHttpResponse {
                status: status.as_u16(),
                descriptor,
            },
        )
        .map_err(client_error)
    }

    fn download_attachment_blob(
        &self,
        reference: &AttachmentBlobReferenceV1,
    ) -> Result<PathBuf, FiniteChatCoreError> {
        if let Some(path) = self.cached_attachment_path(reference)? {
            return Ok(path);
        }

        let request = prepare_blossom_download_http_request(reference).map_err(client_error)?;
        let response = reqwest::blocking::Client::new()
            .get(request.url)
            .send()
            .map_err(delivery_error)?;
        let status = response.status();
        let body = response.bytes().map_err(delivery_error)?;
        let downloaded = finish_blossom_download_http_response(
            reference,
            BlossomDownloadHttpResponse {
                status: status.as_u16(),
                body: body.as_ref(),
            },
        )
        .map_err(client_error)?;
        self.cache_attachment_plaintext(reference, &downloaded.plaintext)?;
        self.cached_attachment_path(reference)?
            .ok_or_else(|| FiniteChatCoreError::Filesystem {
                reason: "attachment cache write did not produce a readable file".to_owned(),
            })
    }

    fn apply_attachment_cache_paths(&self, messages: &mut [ChatMessage]) {
        for message in messages {
            let references = attachment_references_by_id(message);
            for attachment in &mut message.media {
                let Some(reference) = references.get(&attachment.attachment_id) else {
                    continue;
                };
                attachment.local_path = self
                    .cached_attachment_path(reference)
                    .ok()
                    .flatten()
                    .map(|path| path.to_string_lossy().into_owned());
            }
        }
    }

    fn cached_attachment_path(
        &self,
        reference: &AttachmentBlobReferenceV1,
    ) -> Result<Option<PathBuf>, FiniteChatCoreError> {
        let path = self.attachment_cache_path(reference);
        if !path.is_file() {
            return Ok(None);
        }
        let plaintext = fs::read(&path).map_err(|error| FiniteChatCoreError::Filesystem {
            reason: format!("failed to read {}: {error}", path.display()),
        })?;
        if attachment_plaintext_matches(reference, &plaintext) {
            return Ok(Some(path));
        }
        fs::remove_file(&path).map_err(|error| FiniteChatCoreError::Filesystem {
            reason: format!(
                "failed to remove corrupt attachment cache {}: {error}",
                path.display()
            ),
        })?;
        Ok(None)
    }

    fn cache_attachment_plaintext(
        &self,
        reference: &AttachmentBlobReferenceV1,
        plaintext: &[u8],
    ) -> Result<PathBuf, FiniteChatCoreError> {
        if !attachment_plaintext_matches(reference, plaintext) {
            return Err(FiniteChatCoreError::Client {
                reason: "attachment plaintext does not match encrypted reference".to_owned(),
            });
        }
        let path = self.attachment_cache_path(reference);
        let Some(parent) = path.parent() else {
            return Err(FiniteChatCoreError::Filesystem {
                reason: format!("attachment cache path has no parent: {}", path.display()),
            });
        };
        fs::create_dir_all(parent).map_err(|error| FiniteChatCoreError::Filesystem {
            reason: format!("failed to create {}: {error}", parent.display()),
        })?;
        let tmp_path = path.with_file_name(format!(
            ".{}.tmp",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("attachment")
        ));
        fs::write(&tmp_path, plaintext).map_err(|error| FiniteChatCoreError::Filesystem {
            reason: format!("failed to write {}: {error}", tmp_path.display()),
        })?;
        fs::rename(&tmp_path, &path).map_err(|error| FiniteChatCoreError::Filesystem {
            reason: format!(
                "failed to move {} to {}: {error}",
                tmp_path.display(),
                path.display()
            ),
        })?;
        Ok(path)
    }

    fn attachment_cache_path(&self, reference: &AttachmentBlobReferenceV1) -> PathBuf {
        self.data_dir
            .join(ATTACHMENT_CACHE_DIR)
            .join(&reference.plaintext_sha256)
            .join(sanitized_attachment_filename(&reference.metadata.filename))
    }

    fn room_server_url(&self, room_id: &str) -> String {
        self.device
            .room_server_url(room_id)
            .map(str::to_owned)
            .unwrap_or_else(|| self.server_url.clone())
    }

    fn send_reaction(
        &mut self,
        room_id: &str,
        target_message_id: &str,
        emoji: &str,
    ) -> Result<StoredAppEvent, FiniteChatCoreError> {
        let reaction = ChatReactionV1 {
            target_message_id: target_message_id.to_owned(),
            emoji: emoji.trim().to_owned(),
        };
        reaction.validate_limits().map_err(client_error)?;
        let reaction_payload = serde_json::to_vec(&reaction).map_err(client_error)?;
        let app_event_plaintext =
            encode_application_event(DurableAppEventKind::ChatReaction, None, &reaction_payload)?;
        let idempotency_key = self
            .device
            .generate_object_id("reaction")
            .map_err(client_error)?;
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
            .append_event(
                &request,
                DurableAppEventKind::ChatReaction.delivery_policy(),
            )
            .map_err(delivery_error)?;
        let event = StoredAppEvent {
            room_id: room_id.to_owned(),
            seq: accepted.seq,
            message_id: accepted.message_id,
            sender,
            plaintext: app_event_plaintext,
        };
        self.store
            .save_app_events(
                self.device.device_ref(),
                std::slice::from_ref(&event),
                MAX_APP_MESSAGES_U32,
            )
            .map_err(store_error)?;
        Ok(event)
    }

    fn send_read_receipt(
        &mut self,
        room_id: &str,
        target_message_id: &str,
        target_seq: u64,
        state: ChatReceiptStateV1,
    ) -> Result<StoredAppEvent, FiniteChatCoreError> {
        let receipt = ChatReceiptV1 {
            target_message_id: target_message_id.to_owned(),
            target_seq,
            state,
        };
        receipt.validate_limits().map_err(client_error)?;
        let receipt_payload = serde_json::to_vec(&receipt).map_err(client_error)?;
        let app_event_plaintext =
            encode_application_event(DurableAppEventKind::ChatReceipt, None, &receipt_payload)?;
        let idempotency_key = self
            .device
            .generate_object_id("receipt")
            .map_err(client_error)?;
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
            .append_event(&request, DurableAppEventKind::ChatReceipt.delivery_policy())
            .map_err(delivery_error)?;
        let event = StoredAppEvent {
            room_id: room_id.to_owned(),
            seq: accepted.seq,
            message_id: accepted.message_id,
            sender,
            plaintext: app_event_plaintext,
        };
        self.store
            .save_app_events(
                self.device.device_ref(),
                std::slice::from_ref(&event),
                MAX_APP_MESSAGES_U32,
            )
            .map_err(store_error)?;
        Ok(event)
    }

    fn sync(&mut self) -> Result<SyncResult, FiniteChatCoreError> {
        Ok(self.sync_with_projection()?.result)
    }

    fn sync_with_projection(&mut self) -> Result<CoreSyncProjection, FiniteChatCoreError> {
        let options = RuntimeSyncOptions {
            key_package_target_available: DEFAULT_KEY_PACKAGE_TARGET_AVAILABLE,
            max_sync_pages_per_room: DEFAULT_MAX_SYNC_PAGES_PER_ROOM,
        };
        let mut projection = CoreSyncProjection::default();

        let mut home_delivery = self.home_delivery();
        let home_report = run_runtime_sync_tick(
            &mut self.store,
            &mut self.device,
            &mut home_delivery,
            &options,
        )
        .map_err(runtime_error)?;
        let owner = self.device.device_ref().clone();
        projection.merge_report(home_report, &owner);

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
            projection.merge_report(report, &owner);
        }

        Ok(projection)
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

impl CoreSyncProjection {
    fn merge_report(&mut self, report: finitechat_client::RuntimeSyncReport, owner: &DeviceRef) {
        self.result.uploaded_key_packages = self
            .result
            .uploaded_key_packages
            .saturating_add(report.uploaded_key_packages);
        self.result.claimed_welcomes = self
            .result
            .claimed_welcomes
            .saturating_add(report.claimed_welcomes);
        self.result.activated_welcome_acks_sent = self
            .result
            .activated_welcome_acks_sent
            .saturating_add(report.activated_welcome_acks_sent);
        self.result.sync_pages = self.result.sync_pages.saturating_add(report.sync_pages);
        for entry in report.applied_entries {
            match entry.entry {
                AppliedLogEntry::Application { plaintext, sender } => {
                    if let Some(message) = project_chat_message(
                        entry.room_id.clone(),
                        entry.seq,
                        entry.message_id.clone(),
                        sender.clone(),
                        plaintext.clone(),
                        owner,
                    ) {
                        self.result.messages.push(message);
                    }
                    self.events.push(StoredAppEvent {
                        room_id: entry.room_id,
                        seq: entry.seq,
                        message_id: entry.message_id,
                        sender,
                        plaintext,
                    });
                }
                AppliedLogEntry::Commit { .. } => {}
            }
        }
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
    ChatReceipt(ChatReceiptV1),
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
        DecodedAppEvent::ChatReaction(_)
        | DecodedAppEvent::ChatReceipt(_)
        | DecodedAppEvent::Ignored => None,
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
        DurableAppEventKind::ChatReceipt => serde_json::from_slice::<ChatReceiptV1>(&event.payload)
            .ok()
            .filter(|receipt| receipt.validate_limits().is_ok())
            .map(DecodedAppEvent::ChatReceipt)
            .unwrap_or(DecodedAppEvent::Ignored),
        DurableAppEventKind::ConversationCreate
        | DurableAppEventKind::ConversationUpdate
        | DurableAppEventKind::ConversationArchive
        | DurableAppEventKind::ConversationSegmentStart
        | DurableAppEventKind::ChatEdit
        | DurableAppEventKind::RuntimeStateSnapshot
        | DurableAppEventKind::RuntimeCommandRequest
        | DurableAppEventKind::RuntimeCommandResult
        | DurableAppEventKind::RuntimeCommandCancel
        | DurableAppEventKind::StreamStart
        | DurableAppEventKind::StreamFinish
        | DurableAppEventKind::Namespaced { .. } => DecodedAppEvent::Ignored,
    }
}

fn encode_text_message_payload(
    text: &str,
    reply_to_message_id: Option<&str>,
) -> Result<Vec<u8>, FiniteChatCoreError> {
    HermesMessagePayloadV1 {
        payload_type: finitechat_hermes::HERMES_MESSAGE_PAYLOAD_TYPE_V1.to_owned(),
        conversation_id: None,
        text: text.to_owned(),
        kind: finitechat_hermes::HermesSendKindV1::Message,
        status: finitechat_hermes::HermesMessageStatusV1::Complete,
        edit_of: None,
        attachments: Vec::new(),
        reply_to_message_id: reply_to_message_id.map(ToOwned::to_owned),
        sender_name: None,
        metadata: BTreeMap::new(),
    }
    .encode()
    .map_err(client_error)
}

fn encode_attachment_message_payload(
    caption: &str,
    filename: &str,
    mime_type: &str,
    kind: ChatMediaKind,
    reference: AttachmentBlobReferenceV1,
    reply_to_message_id: Option<&str>,
) -> Result<Vec<u8>, FiniteChatCoreError> {
    HermesMessagePayloadV1 {
        payload_type: finitechat_hermes::HERMES_MESSAGE_PAYLOAD_TYPE_V1.to_owned(),
        conversation_id: None,
        text: caption.to_owned(),
        kind: finitechat_hermes::HermesSendKindV1::Media,
        status: finitechat_hermes::HermesMessageStatusV1::Complete,
        edit_of: None,
        attachments: vec![HermesAttachmentV1 {
            kind: hermes_attachment_kind(&kind),
            name: filename.to_owned(),
            mime_type: mime_type.to_owned(),
            path: None,
            url: Some(reference.url.clone()),
            blob: Some(reference),
        }],
        reply_to_message_id: reply_to_message_id.map(ToOwned::to_owned),
        sender_name: None,
        metadata: BTreeMap::new(),
    }
    .encode()
    .map_err(client_error)
}

fn hermes_attachment_kind(kind: &ChatMediaKind) -> HermesAttachmentKindV1 {
    match kind {
        ChatMediaKind::Image => HermesAttachmentKindV1::Image,
        ChatMediaKind::VoiceNote => HermesAttachmentKindV1::Audio,
        ChatMediaKind::Video => HermesAttachmentKindV1::Video,
        ChatMediaKind::File => HermesAttachmentKindV1::File,
    }
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

fn attachment_references_by_id(
    message: &ChatMessage,
) -> BTreeMap<String, AttachmentBlobReferenceV1> {
    let DecodedAppEvent::ChatMessage { payload, .. } = decode_application_event(&message.payload)
    else {
        return BTreeMap::new();
    };
    let Ok(Some(payload)) = HermesMessagePayloadV1::decode(&payload) else {
        return BTreeMap::new();
    };

    let mut references = BTreeMap::new();
    for (index, attachment) in payload.attachments.into_iter().enumerate() {
        let projected = chat_media_attachment(index, attachment.clone());
        if let Some(reference) = attachment.blob {
            references.insert(projected.attachment_id, reference);
        }
    }
    references
}

fn attachment_reference_for_id(
    message: &ChatMessage,
    attachment_id: &str,
) -> Option<AttachmentBlobReferenceV1> {
    attachment_references_by_id(message).remove(attachment_id)
}

fn attachment_plaintext_matches(reference: &AttachmentBlobReferenceV1, plaintext: &[u8]) -> bool {
    plaintext.len() as u64 == reference.plaintext_size
        && sha256_hex(plaintext) == reference.plaintext_sha256
}

fn sanitized_attachment_filename(filename: &str) -> String {
    let trimmed = filename.trim();
    let mut out = String::with_capacity(trimmed.len().min(128));
    for ch in trimmed.chars() {
        if out.len() >= 128 {
            break;
        }
        if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    let out = out.trim_matches('.');
    if out.is_empty() || out == "_" {
        return "attachment".to_owned();
    }
    if out == ".." {
        return "attachment".to_owned();
    }
    out.to_owned()
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
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

#[cfg(test)]
fn chat_messages_from_stored(
    stored_messages: Vec<StoredAppMessage>,
    stored_events: Vec<StoredAppEvent>,
    owner: &DeviceRef,
) -> Vec<ChatMessage> {
    ChatProjectionState::from_stored(stored_messages, stored_events, owner).messages()
}

impl ChatProjectionState {
    fn from_stored(
        stored_messages: Vec<StoredAppMessage>,
        stored_events: Vec<StoredAppEvent>,
        owner: &DeviceRef,
    ) -> Self {
        let mut projection = Self::default();
        for message in stored_messages {
            if let Some(projected) = chat_message_from_stored(message, owner) {
                projection.insert_message(projected);
            }
        }
        for event in stored_events {
            projection.apply_event(event, owner);
        }
        projection.trim_to_limit();
        projection
    }

    fn append_messages(&mut self, messages: Vec<ChatMessage>) {
        for message in messages {
            self.insert_message(message);
        }
        self.trim_to_limit();
    }

    fn apply_event(&mut self, event: StoredAppEvent, owner: &DeviceRef) {
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
                    self.insert_message(message);
                }
            }
            DecodedAppEvent::ChatReaction(reaction) => {
                self.apply_reaction(&event.room_id, &event.sender, owner, reaction);
            }
            DecodedAppEvent::ChatReceipt(receipt) => {
                self.apply_receipt(&event.room_id, &event.sender, receipt);
            }
            DecodedAppEvent::Ignored => {}
        }
        self.trim_to_limit();
    }

    fn messages(&self) -> Vec<ChatMessage> {
        let mut messages = self.messages.values().cloned().collect::<Vec<_>>();
        messages.sort_by(message_sort);
        messages
    }

    fn messages_for_room_window(&self, room_id: &str, limit: usize) -> Vec<ChatMessage> {
        let mut messages = self
            .messages
            .values()
            .filter(|message| message.room_id == room_id)
            .cloned()
            .collect::<Vec<_>>();
        messages.sort_by(message_sort);
        if messages.len() > limit {
            messages.drain(0..messages.len() - limit);
        }
        messages
    }

    fn room_message_count(&self, room_id: &str) -> usize {
        self.messages
            .values()
            .filter(|message| message.room_id == room_id)
            .count()
    }

    fn message_exists(&self, room_id: &str, message_id: &str) -> bool {
        self.messages
            .contains_key(&(room_id.to_owned(), message_id.to_owned()))
    }

    fn insert_message(&mut self, mut message: ChatMessage) {
        message.read_receipt =
            receipt_summary_for_message(&message, &self.delivered_through, &self.read_through);
        self.messages.insert(message_key(&message), message);
    }

    fn latest_peer_message_needing_read_receipt(
        &self,
        room_id: &str,
        owner: &DeviceRef,
    ) -> Option<(String, u64)> {
        let owner_key = device_label(owner);
        let read_through = self
            .read_through
            .get(&(room_id.to_owned(), owner_key))
            .copied()
            .unwrap_or_default();
        self.messages
            .values()
            .filter(|message| message.room_id == room_id)
            .filter(|message| !message.is_mine)
            .filter(|message| message.seq > read_through)
            .max_by(|left, right| {
                left.seq
                    .cmp(&right.seq)
                    .then_with(|| left.message_id.cmp(&right.message_id))
            })
            .map(|message| (message.message_id.clone(), message.seq))
    }

    fn latest_peer_message(&self, room_id: &str) -> Option<(String, u64)> {
        self.messages
            .values()
            .filter(|message| message.room_id == room_id)
            .filter(|message| !message.is_mine)
            .max_by(|left, right| {
                left.seq
                    .cmp(&right.seq)
                    .then_with(|| left.message_id.cmp(&right.message_id))
            })
            .map(|message| (message.message_id.clone(), message.seq))
    }

    fn apply_reaction(
        &mut self,
        room_id: &str,
        sender: &DeviceRef,
        owner: &DeviceRef,
        reaction: ChatReactionV1,
    ) {
        let key = (room_id.to_owned(), reaction.target_message_id);
        let emoji = reaction.emoji.trim().to_owned();
        let sender_key = device_label(sender);
        if !self
            .reaction_senders
            .insert((key.0.clone(), key.1.clone(), emoji.clone(), sender_key))
        {
            return;
        }

        let Some(message) = self.messages.get_mut(&key) else {
            return;
        };
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

    fn apply_receipt(&mut self, room_id: &str, sender: &DeviceRef, receipt: ChatReceiptV1) {
        let target_key = (room_id.to_owned(), receipt.target_message_id);
        let Some(target) = self.messages.get(&target_key) else {
            return;
        };
        if target.seq != receipt.target_seq {
            return;
        }

        let receipt_key = (room_id.to_owned(), device_label(sender));
        match receipt.state {
            ChatReceiptStateV1::Delivered => {
                upsert_receipt_marker(&mut self.delivered_through, receipt_key, receipt.target_seq);
            }
            ChatReceiptStateV1::Read | ChatReceiptStateV1::Seen => {
                upsert_receipt_marker(&mut self.read_through, receipt_key, receipt.target_seq);
            }
        }
        self.refresh_receipts_for_room(room_id);
    }

    fn refresh_receipts_for_room(&mut self, room_id: &str) {
        let keys = self
            .messages
            .keys()
            .filter(|(message_room_id, _)| message_room_id == room_id)
            .cloned()
            .collect::<Vec<_>>();
        for key in keys {
            let summary = self.messages.get(&key).and_then(|message| {
                receipt_summary_for_message(message, &self.delivered_through, &self.read_through)
            });
            if let Some(message) = self.messages.get_mut(&key) {
                message.read_receipt = summary;
            }
        }
    }

    fn trim_to_limit(&mut self) {
        if self.messages.len() <= MAX_APP_MESSAGES {
            return;
        }
        let mut keyed_messages = self
            .messages
            .values()
            .map(|message| {
                (
                    message.seq,
                    message.room_id.clone(),
                    message.message_id.clone(),
                )
            })
            .collect::<Vec<_>>();
        keyed_messages.sort();
        let drop_count = keyed_messages.len() - MAX_APP_MESSAGES;
        for (_, room_id, message_id) in keyed_messages.into_iter().take(drop_count) {
            self.messages.remove(&(room_id, message_id));
        }
        let message_keys = self.messages.keys().cloned().collect::<BTreeSet<_>>();
        self.reaction_senders.retain(|(room_id, message_id, _, _)| {
            message_keys.contains(&(room_id.clone(), message_id.clone()))
        });
        let message_rooms = message_keys
            .into_iter()
            .map(|(room_id, _)| room_id)
            .collect::<BTreeSet<_>>();
        self.delivered_through
            .retain(|(room_id, _), _| message_rooms.contains(room_id));
        self.read_through
            .retain(|(room_id, _), _| message_rooms.contains(room_id));
    }
}

fn upsert_receipt_marker(
    markers: &mut BTreeMap<(String, String), u64>,
    key: (String, String),
    target_seq: u64,
) {
    let entry = markers.entry(key).or_default();
    if target_seq > *entry {
        *entry = target_seq;
    }
}

fn receipt_summary_for_message(
    message: &ChatMessage,
    delivered_through: &BTreeMap<(String, String), u64>,
    read_through: &BTreeMap<(String, String), u64>,
) -> Option<ChatReadReceiptSummary> {
    let sender_key = format!("{}/{}", message.sender_account_id, message.sender_device_id);
    let mut delivered = BTreeSet::new();
    let mut read = BTreeSet::new();
    for ((room_id, device), through_seq) in delivered_through {
        if room_id == &message.room_id && *through_seq >= message.seq && device != &sender_key {
            delivered.insert(device.clone());
        }
    }
    for ((room_id, device), through_seq) in read_through {
        if room_id == &message.room_id && *through_seq >= message.seq && device != &sender_key {
            read.insert(device.clone());
            delivered.insert(device.clone());
        }
    }
    let read_count = bounded_u32_count(read.len());
    let delivered_count = bounded_u32_count(delivered.len());
    if read_count == 0 && delivered_count == 0 {
        return None;
    }
    let display_text = if read_count > 0 {
        format!("Read by {read_count}")
    } else {
        format!("Delivered to {delivered_count}")
    };
    Some(ChatReadReceiptSummary {
        delivered_count,
        read_count,
        display_text,
    })
}

fn bounded_u32_count(count: usize) -> u32 {
    count.min(u32::MAX as usize) as u32
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
        can_load_older: false,
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
        can_load_older: false,
    }
}

fn selected_room_id_from_stored(
    rooms: &[AppRoomSummary],
    stored: StoredAppState,
) -> Option<String> {
    let selected = stored.selected_room_id?;
    rooms
        .iter()
        .any(|room| room.room_id == selected)
        .then_some(selected)
}

fn stored_room_from_app(
    room: &AppRoomSummary,
    local_read_seq: u64,
    pending: Option<&PendingInvite>,
    owned_invite_url: Option<&String>,
) -> StoredAppRoom {
    StoredAppRoom {
        room_id: room.room_id.clone(),
        display_name: room.display_name.clone(),
        state: stored_app_room_state(&room.state),
        status: room.status.clone(),
        local_read_seq,
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
        local_read_seq: 0,
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

fn apply_room_message_projection(
    rooms: &mut [AppRoomSummary],
    messages: &[ChatMessage],
    local_read_seq: &BTreeMap<String, u64>,
) {
    let mut previews = BTreeMap::new();
    let mut unread_counts = BTreeMap::<String, u32>::new();
    for message in messages {
        previews.insert(message.room_id.clone(), message_preview(message));
        let read_seq = local_read_seq
            .get(&message.room_id)
            .copied()
            .unwrap_or_default();
        if !message.is_mine && message.seq > read_seq {
            unread_counts
                .entry(message.room_id.clone())
                .and_modify(|count| *count = count.saturating_add(1))
                .or_insert(1);
        }
    }

    for room in rooms {
        room.last_message_preview = previews.remove(&room.room_id).unwrap_or_default();
        room.unread_count = unread_counts.remove(&room.room_id).unwrap_or_default();
    }
}

fn message_preview(message: &ChatMessage) -> String {
    let text = message.text.trim();
    if !text.is_empty() {
        return text.to_owned();
    }
    let Some(attachment) = message.media.first() else {
        return String::new();
    };
    if !attachment.filename.trim().is_empty() {
        return attachment.filename.clone();
    }
    match &attachment.kind {
        ChatMediaKind::Image => "Image".to_owned(),
        ChatMediaKind::VoiceNote => "Voice note".to_owned(),
        ChatMediaKind::Video => "Video".to_owned(),
        ChatMediaKind::File => "File".to_owned(),
    }
}

fn sort_app_rooms(rooms: &mut [AppRoomSummary]) {
    rooms.sort_by(|left, right| {
        left.display_name
            .cmp(&right.display_name)
            .then_with(|| left.room_id.cmp(&right.room_id))
    });
}

fn message_sort(left: &ChatMessage, right: &ChatMessage) -> std::cmp::Ordering {
    left.seq
        .cmp(&right.seq)
        .then_with(|| left.room_id.cmp(&right.room_id))
        .then_with(|| left.message_id.cmp(&right.message_id))
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
        let chat_payload = encode_text_message_payload("event sourced hello", None).unwrap();
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
        let receipt = ChatReceiptV1 {
            target_message_id: "message-1".to_owned(),
            target_seq: 1,
            state: ChatReceiptStateV1::Read,
        };
        let receipt_payload = serde_json::to_vec(&receipt).unwrap();
        let receipt_event =
            encode_application_event(DurableAppEventKind::ChatReceipt, None, &receipt_payload)
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
                    sender: peer.clone(),
                    plaintext: reaction_event,
                },
                StoredAppEvent {
                    room_id: "room-main".to_owned(),
                    seq: 3,
                    message_id: "reaction-duplicate".to_owned(),
                    sender: peer.clone(),
                    plaintext: encode_application_event(
                        DurableAppEventKind::ChatReaction,
                        None,
                        &reaction_payload,
                    )
                    .unwrap(),
                },
                StoredAppEvent {
                    room_id: "room-main".to_owned(),
                    seq: 4,
                    message_id: "reaction-owner".to_owned(),
                    sender: owner.clone(),
                    plaintext: encode_application_event(
                        DurableAppEventKind::ChatReaction,
                        None,
                        &reaction_payload,
                    )
                    .unwrap(),
                },
                StoredAppEvent {
                    room_id: "room-main".to_owned(),
                    seq: 5,
                    message_id: "receipt-1".to_owned(),
                    sender: peer.clone(),
                    plaintext: receipt_event,
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
                count: 2,
                reacted_by_me: true,
            }]
        );
        assert_eq!(
            messages[0].read_receipt,
            Some(ChatReadReceiptSummary {
                delivered_count: 1,
                read_count: 1,
                display_text: "Read by 1".to_owned(),
            })
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
    fn app_runtime_windows_selected_room_transcript_and_loads_older() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().join("alice");
        let server_url = spawn_live_http_server(dir.path().join("server.sqlite3"));
        let app = FiniteChatRuntime::open(OpenOptions {
            data_dir: data_dir.to_string_lossy().into_owned(),
            server_url: server_url.clone(),
            device_id: "alice-ios".to_owned(),
            account_secret_hex: None,
            now_unix_seconds: Some(NOW),
        })
        .unwrap();

        let created = app
            .dispatch(AppAction::CreateRoom {
                display_name: "Windowed Chat".to_owned(),
            })
            .unwrap();
        let room_id = created.rooms.first().unwrap().room_id.clone();
        let total_messages = DEFAULT_TRANSCRIPT_WINDOW + 25;
        let mut state = created;
        for index in 0..total_messages {
            state = app
                .dispatch(AppAction::SendMessage {
                    room_id: room_id.clone(),
                    text: format!("message-{index:03}"),
                })
                .unwrap();
        }

        assert_eq!(state.messages.len(), DEFAULT_TRANSCRIPT_WINDOW);
        assert_eq!(state.messages.first().unwrap().text, "message-025");
        assert_eq!(state.messages.last().unwrap().text, "message-074");
        assert!(app_room(&state, &room_id).can_load_older);

        let stale = app
            .dispatch(AppAction::LoadOlderMessages {
                room_id: room_id.clone(),
                before_message_id: "not-the-current-oldest".to_owned(),
                limit: 25,
            })
            .unwrap();
        assert_eq!(stale.messages.len(), DEFAULT_TRANSCRIPT_WINDOW);
        assert_eq!(stale.messages.first().unwrap().text, "message-025");
        assert!(app_room(&stale, &room_id).can_load_older);

        let before_message_id = stale.messages.first().unwrap().message_id.clone();
        let loaded = app
            .dispatch(AppAction::LoadOlderMessages {
                room_id: room_id.clone(),
                before_message_id,
                limit: 25,
            })
            .unwrap();
        assert_eq!(loaded.messages.len(), total_messages);
        assert_eq!(loaded.messages.first().unwrap().text, "message-000");
        assert_eq!(loaded.messages.last().unwrap().text, "message-074");
        assert!(!app_room(&loaded, &room_id).can_load_older);

        drop(app);
        let reopened = FiniteChatRuntime::open(OpenOptions {
            data_dir: data_dir.to_string_lossy().into_owned(),
            server_url: unavailable_http_server_url(),
            device_id: "alice-ios".to_owned(),
            account_secret_hex: None,
            now_unix_seconds: Some(NOW),
        })
        .unwrap();
        let reopened_state = reopened.state().unwrap();
        assert_eq!(reopened_state.messages.len(), DEFAULT_TRANSCRIPT_WINDOW);
        assert_eq!(reopened_state.messages.first().unwrap().text, "message-025");
        assert_eq!(reopened_state.messages.last().unwrap().text, "message-074");
        assert!(app_room(&reopened_state, &room_id).can_load_older);
    }

    #[test]
    fn app_scan_npub_loads_server_backed_profile_cache() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().join("alice");
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
            data_dir: data_dir.to_string_lossy().into_owned(),
            server_url: server_url.clone(),
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
        drop(app);

        let reopened = FiniteChatRuntime::open(OpenOptions {
            data_dir: data_dir.to_string_lossy().into_owned(),
            server_url: unavailable_http_server_url(),
            device_id: "alice-ios".to_owned(),
            account_secret_hex: None,
            now_unix_seconds: Some(NOW),
        })
        .unwrap();
        let cached = reopened.state().unwrap();
        assert_eq!(cached.profiles.len(), 1);
        assert_eq!(cached.profiles[0].display_name, "Alice Finite");
        assert!(!cached.profiles[0].stale);

        let scanned_offline = reopened
            .dispatch(AppAction::ScanTarget {
                value: npub.clone(),
            })
            .unwrap();
        assert_eq!(scanned_offline.status, "profile loaded");
        assert_eq!(
            scanned_offline.active_profile_id.as_deref(),
            Some(account_id.as_str())
        );
        assert_eq!(scanned_offline.profiles[0].display_name, "Alice Finite");
    }

    #[test]
    fn app_scan_missing_npub_surfaces_stale_profile_placeholder() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().join("alice");
        let server_url = spawn_live_http_server(dir.path().join("server.sqlite3"));
        let account_id =
            "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789".to_owned();
        let npub = npub_encode(&account_id).unwrap();
        let app = FiniteChatRuntime::open(OpenOptions {
            data_dir: data_dir.to_string_lossy().into_owned(),
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
        drop(app);

        let reopened = FiniteChatRuntime::open(OpenOptions {
            data_dir: data_dir.to_string_lossy().into_owned(),
            server_url: unavailable_http_server_url(),
            device_id: "alice-ios".to_owned(),
            account_secret_hex: None,
            now_unix_seconds: Some(NOW),
        })
        .unwrap();
        let cached = reopened.state().unwrap();
        assert_eq!(cached.profiles.len(), 1);
        assert_eq!(cached.profiles[0].account_id, account_id);
        assert!(cached.profiles[0].stale);
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
    fn app_reopens_last_selected_room_before_network_sync() {
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

        let alpha = alice
            .dispatch(AppAction::CreateRoom {
                display_name: "Alpha".to_owned(),
            })
            .unwrap()
            .selected_room_id
            .unwrap();
        let zulu = alice
            .dispatch(AppAction::CreateRoom {
                display_name: "Zulu".to_owned(),
            })
            .unwrap()
            .selected_room_id
            .unwrap();
        assert_ne!(alpha, zulu);
        alice
            .dispatch(AppAction::SendMessage {
                room_id: zulu.clone(),
                text: "selected room survives force close".to_owned(),
            })
            .unwrap();
        alice
            .dispatch(AppAction::OpenRoom {
                room_id: zulu.clone(),
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
            local_snapshot.selected_room_id.as_deref(),
            Some(zulu.as_str())
        );
        assert!(
            local_snapshot
                .messages
                .iter()
                .any(|message| message.room_id == zulu
                    && message.text == "selected room survives force close"),
            "force-close reopen must restore the last selected room transcript before sync"
        );
        assert_eq!(app_room(&local_snapshot, &alpha).display_name, "Alpha");
    }

    #[test]
    fn app_reopens_unique_local_device_when_requested_device_id_is_stale() {
        let dir = tempfile::tempdir().unwrap();
        let server_url = spawn_live_http_server(dir.path().join("server.sqlite3"));
        let data_dir = dir.path().join("stable-app-store");
        let app = FiniteChatRuntime::open(OpenOptions {
            data_dir: data_dir.to_string_lossy().into_owned(),
            server_url,
            device_id: "qt433".to_owned(),
            account_secret_hex: None,
            now_unix_seconds: Some(NOW),
        })
        .unwrap();

        let state = app
            .dispatch(AppAction::CreateRoom {
                display_name: "Recovered".to_owned(),
            })
            .unwrap();
        let room_id = state.rooms.first().unwrap().room_id.clone();
        app.dispatch(AppAction::SendMessage {
            room_id: room_id.clone(),
            text: "still here after stale config".to_owned(),
        })
        .unwrap();
        drop(app);

        let reopened = FiniteChatRuntime::open(OpenOptions {
            data_dir: data_dir.to_string_lossy().into_owned(),
            server_url: unavailable_http_server_url(),
            device_id: "codex-persist-check".to_owned(),
            account_secret_hex: None,
            now_unix_seconds: Some(NOW),
        })
        .unwrap();
        let local_snapshot = reopened.state().unwrap();

        assert_eq!(local_snapshot.identity.device_id, "qt433");
        assert_eq!(
            app_room(&local_snapshot, &room_id).display_name,
            "Recovered"
        );
        assert!(
            local_snapshot
                .messages
                .iter()
                .any(|message| message.text == "still here after stale config"),
            "stale launch config must recover the durable local transcript before sync"
        );

        let started = reopened.dispatch(AppAction::StartRuntime).unwrap();
        assert_eq!(started.status, "offline");
        assert!(
            started
                .messages
                .iter()
                .any(|message| message.text == "still here after stale config"),
            "offline startup after stale config recovery must keep the transcript visible"
        );
    }

    #[test]
    fn app_reopens_synced_peer_chat_offline_after_force_close() {
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
        let bob = FiniteChatRuntime::open(OpenOptions {
            data_dir: bob_dir.to_string_lossy().into_owned(),
            server_url: server_url.clone(),
            device_id: "bob-ios".to_owned(),
            account_secret_hex: None,
            now_unix_seconds: Some(NOW),
        })
        .unwrap();

        let alice_state = alice
            .dispatch(AppAction::CreateRoom {
                display_name: "Force Close".to_owned(),
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
        alice.dispatch(AppAction::StartRuntime).unwrap();
        bob.dispatch(AppAction::RetryRoom {
            room_id: room_id.clone(),
        })
        .unwrap();

        bob.dispatch(AppAction::SendMessage {
            room_id: room_id.clone(),
            text: "remote message before force close".to_owned(),
        })
        .unwrap();
        let synced = alice.dispatch(AppAction::StartRuntime).unwrap();
        assert!(
            synced
                .messages
                .iter()
                .any(|message| message.text == "remote message before force close")
        );
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
            "Force Close"
        );
        assert_eq!(
            app_room(&local_snapshot, &room_id).last_message_preview,
            "remote message before force close"
        );
        assert!(
            local_snapshot
                .messages
                .iter()
                .any(|message| message.text == "remote message before force close"),
            "force-close reopen must render synced peer messages from local SQLite before sync"
        );

        let started = reopened.dispatch(AppAction::StartRuntime).unwrap();
        assert_eq!(started.status, "offline");
        assert!(
            started
                .messages
                .iter()
                .any(|message| message.text == "remote message before force close"),
            "offline startup must not clear a synced peer transcript"
        );
    }

    #[test]
    fn app_runtime_sends_reply_message_with_durable_target() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().join("alice");
        let server_url = spawn_live_http_server(dir.path().join("server.sqlite3"));
        let app = FiniteChatRuntime::open(OpenOptions {
            data_dir: data_dir.to_string_lossy().into_owned(),
            server_url: server_url.clone(),
            device_id: "alice-ios".to_owned(),
            account_secret_hex: None,
            now_unix_seconds: Some(NOW),
        })
        .unwrap();

        let created = app
            .dispatch(AppAction::CreateRoom {
                display_name: "Replies".to_owned(),
            })
            .unwrap();
        let room_id = created.rooms.first().unwrap().room_id.clone();
        let parent_state = app
            .dispatch(AppAction::SendMessage {
                room_id: room_id.clone(),
                text: "parent".to_owned(),
            })
            .unwrap();
        let parent_id = parent_state
            .messages
            .iter()
            .find(|message| message.text == "parent")
            .expect("parent message projects")
            .message_id
            .clone();

        let missing = app
            .dispatch(AppAction::SendReply {
                room_id: room_id.clone(),
                text: "nope".to_owned(),
                reply_to_message_id: "missing-message".to_owned(),
            })
            .expect_err("unknown reply targets are rejected by Rust policy");
        assert!(
            missing.to_string().contains("reply target"),
            "unexpected missing-target error: {missing}"
        );

        let replied = app
            .dispatch(AppAction::SendReply {
                room_id: room_id.clone(),
                text: "child".to_owned(),
                reply_to_message_id: parent_id.clone(),
            })
            .unwrap();
        let reply = replied
            .messages
            .iter()
            .find(|message| message.text == "child")
            .expect("reply message projects");
        assert_eq!(
            reply.reply_to_message_id.as_deref(),
            Some(parent_id.as_str())
        );

        let DecodedAppEvent::ChatMessage { payload, .. } = decode_application_event(&reply.payload)
        else {
            panic!("reply row must carry a chat message application event");
        };
        let hermes = HermesMessagePayloadV1::decode(&payload)
            .unwrap()
            .expect("reply row must carry Hermes message payload");
        assert_eq!(
            hermes.reply_to_message_id.as_deref(),
            Some(parent_id.as_str())
        );

        let media_replied = app
            .dispatch(AppAction::SendAttachment {
                room_id: room_id.clone(),
                filename: "reply-photo.jpg".to_owned(),
                mime_type: "image/jpeg".to_owned(),
                kind: ChatMediaKind::Image,
                bytes: b"reply image bytes".to_vec(),
                caption: "media child".to_owned(),
                reply_to_message_id: Some(parent_id.clone()),
            })
            .unwrap();
        let media_reply = media_replied
            .messages
            .iter()
            .find(|message| message.text == "media child" && !message.media.is_empty())
            .expect("media reply message projects");
        assert_eq!(
            media_reply.reply_to_message_id.as_deref(),
            Some(parent_id.as_str())
        );

        let DecodedAppEvent::ChatMessage { payload, .. } =
            decode_application_event(&media_reply.payload)
        else {
            panic!("media reply row must carry a chat message application event");
        };
        let hermes = HermesMessagePayloadV1::decode(&payload)
            .unwrap()
            .expect("media reply row must carry Hermes message payload");
        assert_eq!(
            hermes.reply_to_message_id.as_deref(),
            Some(parent_id.as_str())
        );

        drop(app);
        let reopened = FiniteChatRuntime::open(OpenOptions {
            data_dir: data_dir.to_string_lossy().into_owned(),
            server_url: unavailable_http_server_url(),
            device_id: "alice-ios".to_owned(),
            account_secret_hex: None,
            now_unix_seconds: Some(NOW),
        })
        .unwrap();
        let reopened_state = reopened.state().unwrap();
        let reopened_reply = reopened_state
            .messages
            .iter()
            .find(|message| message.text == "child")
            .expect("reply projection survives reopen");
        assert_eq!(
            reopened_reply.reply_to_message_id.as_deref(),
            Some(parent_id.as_str())
        );
        let reopened_media_reply = reopened_state
            .messages
            .iter()
            .find(|message| message.text == "media child" && !message.media.is_empty())
            .expect("media reply projection survives reopen");
        assert_eq!(
            reopened_media_reply.reply_to_message_id.as_deref(),
            Some(parent_id.as_str())
        );
    }

    #[test]
    fn app_runtime_sends_encrypted_attachment_blob_and_reopens_projection() {
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
        let alice_state = alice
            .dispatch(AppAction::CreateRoom {
                display_name: "Media Room".to_owned(),
            })
            .unwrap();
        let room_id = alice_state.rooms.first().unwrap().room_id.clone();
        let plaintext = b"fake jpeg plaintext bytes".to_vec();

        let sent = alice
            .dispatch(AppAction::SendAttachment {
                room_id: room_id.clone(),
                filename: "photo.jpg".to_owned(),
                mime_type: "image/jpeg".to_owned(),
                kind: ChatMediaKind::Image,
                bytes: plaintext.clone(),
                caption: String::new(),
                reply_to_message_id: None,
            })
            .unwrap();
        let message = sent
            .messages
            .iter()
            .find(|message| message.room_id == room_id && !message.media.is_empty())
            .expect("attachment message projects");
        assert_eq!(message.text, "");
        assert_eq!(message.media.len(), 1);
        let media = &message.media[0];
        assert_eq!(media.kind, ChatMediaKind::Image);
        assert_eq!(media.filename, "photo.jpg");
        assert_eq!(media.mime_type, "image/jpeg");
        let local_path = media
            .local_path
            .as_ref()
            .expect("sender caches uploaded attachment plaintext");
        assert_eq!(std::fs::read(local_path).unwrap(), plaintext);
        assert_eq!(app_room(&sent, &room_id).last_message_preview, "photo.jpg");

        let reference = attachment_reference_from_message(message);
        let url = media.url.as_ref().expect("projected blob URL");
        let ciphertext = reqwest::blocking::Client::new()
            .get(url)
            .send()
            .unwrap()
            .bytes()
            .unwrap();
        assert_ne!(ciphertext.as_ref(), plaintext.as_slice());
        let downloaded = finitechat_blob::finish_blossom_download_http_response(
            &reference,
            finitechat_blob::BlossomDownloadHttpResponse {
                status: 200,
                body: ciphertext.as_ref(),
            },
        )
        .unwrap();
        assert_eq!(downloaded.plaintext, plaintext);

        drop(alice);
        let reopened = FiniteChatRuntime::open(OpenOptions {
            data_dir: alice_dir.to_string_lossy().into_owned(),
            server_url: unavailable_http_server_url(),
            device_id: "alice-ios".to_owned(),
            account_secret_hex: None,
            now_unix_seconds: Some(NOW),
        })
        .unwrap();
        let reopened_state = reopened.state().unwrap();
        let reopened_message = reopened_state
            .messages
            .iter()
            .find(|message| message.room_id == room_id && !message.media.is_empty())
            .expect("attachment projection survives reopen");
        assert_eq!(reopened_message.media[0].filename, "photo.jpg");
        assert!(reopened_message.media[0].local_path.is_some());
        assert_eq!(
            app_room(&reopened_state, &room_id).last_message_preview,
            "photo.jpg"
        );
    }

    #[test]
    fn app_runtime_downloads_attachment_blob_to_verified_local_cache() {
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
        let bob = FiniteChatRuntime::open(OpenOptions {
            data_dir: bob_dir.to_string_lossy().into_owned(),
            server_url: server_url.clone(),
            device_id: "bob-ios".to_owned(),
            account_secret_hex: None,
            now_unix_seconds: Some(NOW),
        })
        .unwrap();

        let alice_state = alice
            .dispatch(AppAction::CreateRoom {
                display_name: "Media Download".to_owned(),
            })
            .unwrap();
        let room_id = alice_state.rooms.first().unwrap().room_id.clone();
        let alice_state = alice
            .dispatch(AppAction::CreateInvite {
                room_id: room_id.clone(),
            })
            .unwrap();
        let invite = alice_state.active_invite.unwrap();

        bob.dispatch(AppAction::ScanTarget {
            value: invite.invite_url.clone(),
        })
        .unwrap();
        bob.dispatch(AppAction::SubmitInvitePin {
            pending_room_id: room_id.clone(),
            pin: invite.pin,
        })
        .unwrap();
        alice.dispatch(AppAction::StartRuntime).unwrap();
        bob.dispatch(AppAction::RetryRoom {
            room_id: room_id.clone(),
        })
        .unwrap();

        let plaintext = b"download me after sync".to_vec();
        alice
            .dispatch(AppAction::SendAttachment {
                room_id: room_id.clone(),
                filename: "remote photo.jpg".to_owned(),
                mime_type: "image/jpeg".to_owned(),
                kind: ChatMediaKind::Image,
                bytes: plaintext.clone(),
                caption: "from alice".to_owned(),
                reply_to_message_id: None,
            })
            .unwrap();

        let bob_state = bob.dispatch(AppAction::StartRuntime).unwrap();
        let message = bob_state
            .messages
            .iter()
            .find(|message| message.room_id == room_id && !message.media.is_empty())
            .expect("receiver sees remote attachment");
        assert_eq!(message.text, "from alice");
        let attachment = message.media.first().unwrap();
        assert_eq!(attachment.filename, "remote photo.jpg");
        assert_eq!(attachment.local_path, None);

        let bob_state = bob
            .dispatch(AppAction::DownloadAttachment {
                room_id: room_id.clone(),
                message_id: message.message_id.clone(),
                attachment_id: attachment.attachment_id.clone(),
            })
            .unwrap();
        let downloaded = bob_state
            .messages
            .iter()
            .find(|message| message.room_id == room_id && !message.media.is_empty())
            .expect("downloaded message remains projected");
        let local_path = downloaded.media[0]
            .local_path
            .as_ref()
            .expect("downloaded attachment projects verified local path");
        assert!(local_path.ends_with("remote_photo.jpg"));
        assert_eq!(std::fs::read(local_path).unwrap(), plaintext);

        drop(bob);
        let reopened = FiniteChatRuntime::open(OpenOptions {
            data_dir: bob_dir.to_string_lossy().into_owned(),
            server_url: unavailable_http_server_url(),
            device_id: "bob-ios".to_owned(),
            account_secret_hex: None,
            now_unix_seconds: Some(NOW),
        })
        .unwrap();
        let reopened_state = reopened.state().unwrap();
        let reopened_message = reopened_state
            .messages
            .iter()
            .find(|message| message.room_id == room_id && !message.media.is_empty())
            .expect("cached attachment projection survives offline reopen");
        assert_eq!(
            std::fs::read(reopened_message.media[0].local_path.as_ref().unwrap()).unwrap(),
            plaintext
        );
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
    fn app_runtime_reactions_are_durable_and_live_projected() {
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
            server_url,
            device_id: "bob-ios".to_owned(),
            account_secret_hex: None,
            now_unix_seconds: Some(NOW),
        })
        .unwrap();

        let alice_state = alice
            .dispatch(AppAction::CreateRoom {
                display_name: "Reaction Room".to_owned(),
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
            value: invite.invite_url,
        })
        .unwrap();
        bob.dispatch(AppAction::SubmitInvitePin {
            pending_room_id: room_id.clone(),
            pin: invite.pin,
        })
        .unwrap();
        alice.dispatch(AppAction::StartRuntime).unwrap();
        bob.dispatch(AppAction::RetryRoom {
            room_id: room_id.clone(),
        })
        .unwrap();

        let bob_state = bob
            .dispatch(AppAction::SendMessage {
                room_id: room_id.clone(),
                text: "tap a reaction on this".to_owned(),
            })
            .unwrap();
        let target_message_id = bob_state
            .messages
            .iter()
            .find(|message| message.text == "tap a reaction on this")
            .expect("sent message projects")
            .message_id
            .clone();

        alice.dispatch(AppAction::StartRuntime).unwrap();
        let alice_state = alice
            .dispatch(AppAction::ReactToMessage {
                room_id: room_id.clone(),
                message_id: target_message_id.clone(),
                emoji: "👍".to_owned(),
            })
            .unwrap();
        assert_reaction(&alice_state, &target_message_id, "👍", 1, true);

        let alice_state = alice
            .dispatch(AppAction::ReactToMessage {
                room_id: room_id.clone(),
                message_id: target_message_id.clone(),
                emoji: "👍".to_owned(),
            })
            .unwrap();
        assert_reaction(&alice_state, &target_message_id, "👍", 1, true);

        let bob_state = bob.dispatch(AppAction::StartRuntime).unwrap();
        assert_reaction(&bob_state, &target_message_id, "👍", 1, false);
        drop(alice);

        let reopened = FiniteChatRuntime::open(OpenOptions {
            data_dir: alice_dir.to_string_lossy().into_owned(),
            server_url: unavailable_http_server_url(),
            device_id: "alice-ios".to_owned(),
            account_secret_hex: None,
            now_unix_seconds: Some(NOW),
        })
        .unwrap();
        assert_reaction(
            &reopened.state().unwrap(),
            &target_message_id,
            "👍",
            1,
            true,
        );
    }

    #[test]
    fn app_runtime_read_receipts_are_durable_and_live_projected() {
        let dir = tempfile::tempdir().unwrap();
        let server_url = spawn_live_http_server(dir.path().join("server.sqlite3"));
        let bob_dir = dir.path().join("bob");
        let alice = FiniteChatRuntime::open(OpenOptions {
            data_dir: dir.path().join("alice").to_string_lossy().into_owned(),
            server_url: server_url.clone(),
            device_id: "alice-ios".to_owned(),
            account_secret_hex: None,
            now_unix_seconds: Some(NOW),
        })
        .unwrap();
        let bob = FiniteChatRuntime::open(OpenOptions {
            data_dir: bob_dir.to_string_lossy().into_owned(),
            server_url,
            device_id: "bob-ios".to_owned(),
            account_secret_hex: None,
            now_unix_seconds: Some(NOW),
        })
        .unwrap();

        let alice_state = alice
            .dispatch(AppAction::CreateRoom {
                display_name: "Receipt Room".to_owned(),
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
            value: invite.invite_url,
        })
        .unwrap();
        bob.dispatch(AppAction::SubmitInvitePin {
            pending_room_id: room_id.clone(),
            pin: invite.pin,
        })
        .unwrap();
        alice.dispatch(AppAction::StartRuntime).unwrap();
        bob.dispatch(AppAction::RetryRoom {
            room_id: room_id.clone(),
        })
        .unwrap();

        let bob_state = bob
            .dispatch(AppAction::SendMessage {
                room_id: room_id.clone(),
                text: "read me".to_owned(),
            })
            .unwrap();
        let target_message_id = bob_state
            .messages
            .iter()
            .find(|message| message.text == "read me")
            .expect("sent message projects")
            .message_id
            .clone();

        alice.dispatch(AppAction::StartRuntime).unwrap();
        alice
            .dispatch(AppAction::MarkRoomRead {
                room_id: room_id.clone(),
            })
            .unwrap();
        alice
            .dispatch(AppAction::MarkRoomRead {
                room_id: room_id.clone(),
            })
            .unwrap();

        let bob_state = bob.dispatch(AppAction::StartRuntime).unwrap();
        assert_read_receipt(&bob_state, &target_message_id, 1, 1, "Read by 1");
        drop(bob);

        let reopened = FiniteChatRuntime::open(OpenOptions {
            data_dir: bob_dir.to_string_lossy().into_owned(),
            server_url: unavailable_http_server_url(),
            device_id: "bob-ios".to_owned(),
            account_secret_hex: None,
            now_unix_seconds: Some(NOW),
        })
        .unwrap();
        assert_read_receipt(
            &reopened.state().unwrap(),
            &target_message_id,
            1,
            1,
            "Read by 1",
        );
    }

    #[test]
    fn app_runtime_unread_counts_are_local_durable_and_offline_clearable() {
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
        let bob = FiniteChatRuntime::open(OpenOptions {
            data_dir: bob_dir.to_string_lossy().into_owned(),
            server_url: server_url.clone(),
            device_id: "bob-ios".to_owned(),
            account_secret_hex: None,
            now_unix_seconds: Some(NOW),
        })
        .unwrap();

        let alice_state = alice
            .dispatch(AppAction::CreateRoom {
                display_name: "Unread Room".to_owned(),
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
            value: invite.invite_url,
        })
        .unwrap();
        bob.dispatch(AppAction::SubmitInvitePin {
            pending_room_id: room_id.clone(),
            pin: invite.pin,
        })
        .unwrap();
        alice.dispatch(AppAction::StartRuntime).unwrap();
        bob.dispatch(AppAction::RetryRoom {
            room_id: room_id.clone(),
        })
        .unwrap();

        bob.dispatch(AppAction::SendMessage {
            room_id: room_id.clone(),
            text: "first unread".to_owned(),
        })
        .unwrap();
        let alice_state = alice.dispatch(AppAction::StartRuntime).unwrap();
        assert_eq!(app_room(&alice_state, &room_id).unread_count, 1);

        let alice_state = alice
            .dispatch(AppAction::MarkRoomRead {
                room_id: room_id.clone(),
            })
            .unwrap();
        assert_eq!(app_room(&alice_state, &room_id).unread_count, 0);

        bob.dispatch(AppAction::SendMessage {
            room_id: room_id.clone(),
            text: "second unread".to_owned(),
        })
        .unwrap();
        let alice_state = alice.dispatch(AppAction::StartRuntime).unwrap();
        assert_eq!(app_room(&alice_state, &room_id).unread_count, 1);
        assert_eq!(
            app_room(&alice_state, &room_id).last_message_preview,
            "second unread"
        );
        drop(alice);

        let reopened = FiniteChatRuntime::open(OpenOptions {
            data_dir: alice_dir.to_string_lossy().into_owned(),
            server_url: unavailable_http_server_url(),
            device_id: "alice-ios".to_owned(),
            account_secret_hex: None,
            now_unix_seconds: Some(NOW),
        })
        .unwrap();
        let offline_state = reopened.state().unwrap();
        assert_eq!(app_room(&offline_state, &room_id).unread_count, 1);

        let cleared = reopened
            .dispatch(AppAction::MarkRoomRead {
                room_id: room_id.clone(),
            })
            .unwrap();
        assert_eq!(app_room(&cleared, &room_id).unread_count, 0);
        drop(reopened);

        let reopened = FiniteChatRuntime::open(OpenOptions {
            data_dir: alice_dir.to_string_lossy().into_owned(),
            server_url: unavailable_http_server_url(),
            device_id: "alice-ios".to_owned(),
            account_secret_hex: None,
            now_unix_seconds: Some(NOW),
        })
        .unwrap();
        assert_eq!(
            app_room(&reopened.state().unwrap(), &room_id).unread_count,
            0
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

    fn assert_reaction(
        state: &AppState,
        message_id: &str,
        emoji: &str,
        count: u32,
        reacted_by_me: bool,
    ) {
        let message = state
            .messages
            .iter()
            .find(|message| message.message_id == message_id)
            .unwrap_or_else(|| panic!("missing message {message_id}"));
        let reaction = message
            .reactions
            .iter()
            .find(|reaction| reaction.emoji == emoji)
            .unwrap_or_else(|| panic!("missing reaction {emoji} on {message_id}"));
        assert_eq!(reaction.count, count);
        assert_eq!(reaction.reacted_by_me, reacted_by_me);
    }

    fn assert_read_receipt(
        state: &AppState,
        message_id: &str,
        delivered_count: u32,
        read_count: u32,
        display_text: &str,
    ) {
        let message = state
            .messages
            .iter()
            .find(|message| message.message_id == message_id)
            .unwrap_or_else(|| panic!("missing message {message_id}"));
        let receipt = message
            .read_receipt
            .as_ref()
            .unwrap_or_else(|| panic!("missing read receipt on {message_id}"));
        assert_eq!(receipt.delivered_count, delivered_count);
        assert_eq!(receipt.read_count, read_count);
        assert_eq!(receipt.display_text, display_text);
    }

    fn attachment_reference_from_message(message: &ChatMessage) -> AttachmentBlobReferenceV1 {
        let DecodedAppEvent::ChatMessage { payload, .. } =
            decode_application_event(&message.payload)
        else {
            panic!("expected chat message application event");
        };
        let hermes = HermesMessagePayloadV1::decode(&payload)
            .unwrap()
            .expect("Hermes payload");
        hermes
            .attachments
            .first()
            .and_then(|attachment| attachment.blob.clone())
            .expect("blob reference")
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
