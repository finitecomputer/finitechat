//! The `finitechat hermes` subcommand family: the JSON bridge
//! the Hermes platform plugin shells to (ADR 0002), plus agent onboarding
//! (ADR 0006: init → invite URL/QR/PIN → chat).
//!
//! The agent's durable home lives under `--home` / `$FINITECHAT_HOME`:
//! `agent.nsec` (0600), `config.json`, `invites.json` (0600, each line is a
//! full invite URL — the URL carries the invite token), and the encrypted
//! client store `client.sqlite3`.

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use finitechat_blob::{
    BlossomDownloadHttpResponse, finish_blossom_download_http_response,
    prepare_blossom_download_http_request, sha256_hex,
};
use finitechat_client::{
    AppliedLogEntry, CreateRoomInviteParams, FiniteChatDevice, FiniteChatDeviceConfig,
    HttpRuntimeDelivery, ReqwestHttpRuntimeTransport, RuntimeDelivery, RuntimeSyncOptions,
    SqliteClientStore, SqliteClientStoreOptions, accept_pending_invite_joins, create_room_invite,
    finalize_invited_room, generate_account_secret, run_room_server_sync_tick,
    run_runtime_sync_tick, submit_invite_join_request,
};
use finitechat_hermes::{
    HermesAckRequestV1, HermesActivityRequestV1, HermesEditRequestV1, HermesMessagePayloadV1,
    HermesMessageStatusV1, HermesPollEventV1, HermesSendRequestV1, MAX_HERMES_POLL_TIMEOUT_MILLIS,
};
use finitechat_http::{HttpInviteJoinState, SyncWaitInvite, SyncWaitRequest, SyncWaitRoom};
use finitechat_mls::{NOSTR_SECRET_KEY_BYTES, NostrSecretKey};
use finitechat_proto::{
    AppendEphemeralActivityRequest, AttachmentBlobReferenceV1, CreateRoomRequest,
    DecryptedApplicationEventV1, DecryptedEphemeralActivityV1, DurableAppEventKind,
    EphemeralActivityActionV1, EventAccepted, INVITE_PIN_WINDOW_SECONDS, InviteCodeV1,
    RoomProtocol, invite_current_pin, npub_encode,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::CliError;

const CONFIG_FILE: &str = "config.json";
const NSEC_FILE: &str = "agent.nsec";
const INVITES_FILE: &str = "invites.json";
const HERMES_INBOX_FILE: &str = "hermes-inbox.json";
const HERMES_RUNNING_FILE: &str = "hermes-running.json";
const STORE_FILE: &str = "client.sqlite3";
const ATTACHMENT_CACHE_DIR: &str = "attachments";
const DEFAULT_DEVICE_ID: &str = "agent";
const DEFAULT_MAX_JOINS: u32 = 8;
const DEFAULT_INVITE_TTL_MS: u64 = 24 * 60 * 60 * 1000;
const CREDENTIAL_VALIDITY_SECONDS: u64 = 90 * 24 * 60 * 60;
const KEY_PACKAGE_TARGET_AVAILABLE: u32 = 4;
const POLL_SLEEP_MS: u64 = 300;
const ACTIVITY_DEFAULT_EXPIRY_MS: u64 = 30 * 1000;

#[derive(Debug, Serialize, Deserialize)]
struct AgentConfig {
    server_url: String,
    device_id: String,
    account_id: String,
}

struct AgentHome {
    dir: PathBuf,
    config: AgentConfig,
    secret: NostrSecretKey,
}

pub(crate) fn run<W: Write>(args: Vec<String>, output: &mut W) -> Result<(), CliError> {
    let mut args = args;
    let home_dir = resolve_home(&mut args)?;
    let json_mode = take_flag(&mut args, "--json");
    let request_json = crate::take_option(&mut args, "--request-json")?;
    let Some(command) = args.first().cloned() else {
        return Err(CliError::Usage(hermes_usage()));
    };
    let rest = args[1..].to_vec();

    match command.as_str() {
        "init" => cmd_init(&home_dir, rest, output),
        "invite" => cmd_invite(&home_dir, rest, json_mode, output),
        "pin" => cmd_pin(&home_dir, rest, output),
        "join" => cmd_join(&home_dir, rest, output),
        "poll" => cmd_poll(&home_dir, read_request(request_json)?, output),
        "ack" => cmd_ack(&home_dir, read_request(request_json)?, output),
        "send" => cmd_send(&home_dir, read_request(request_json)?, output),
        "edit" => cmd_edit(&home_dir, read_request(request_json)?, output),
        "recover" => cmd_recover(&home_dir, read_request(request_json)?, output),
        "activity" => cmd_activity(&home_dir, read_request(request_json)?, output),
        _ => Err(CliError::Usage(hermes_usage())),
    }
}

fn cmd_init<W: Write>(
    home_dir: &Path,
    mut args: Vec<String>,
    output: &mut W,
) -> Result<(), CliError> {
    let server_url = crate::required_option(&mut args, "--server")?;
    let device_id = crate::take_option(&mut args, "--device-id")?
        .unwrap_or_else(|| DEFAULT_DEVICE_ID.to_owned());
    crate::reject_extra_args(&args)?;
    if home_dir.join(CONFIG_FILE).exists() {
        return Err(CliError::Hermes(format!(
            "agent home {} is already initialized",
            home_dir.display()
        )));
    }
    fs::create_dir_all(home_dir).map_err(|error| CliError::Hermes(error.to_string()))?;

    let secret = generate_account_secret().map_err(|error| CliError::Hermes(error.to_string()))?;
    let device = FiniteChatDevice::new(device_config(&secret, &device_id, now_secs()))
        .map_err(|error| CliError::Hermes(error.to_string()))?;
    let mut store = open_store(home_dir, &secret, &device_id)?;
    store
        .save_device_state(&device)
        .map_err(|error| CliError::Hermes(error.to_string()))?;

    let config = AgentConfig {
        server_url,
        device_id,
        account_id: device.device_ref().account_id.clone(),
    };
    write_private(home_dir.join(NSEC_FILE), &hex_lower(secret.as_bytes()))?;
    write_private(
        home_dir.join(CONFIG_FILE),
        &serde_json::to_string_pretty(&config).map_err(CliError::Serialize)?,
    )?;
    write_private(home_dir.join(INVITES_FILE), "[]")?;

    let npub = npub_encode(&config.account_id)
        .map_err(|error| CliError::Hermes(format!("npub encoding failed: {error}")))?;
    crate::write_pretty_json(
        output,
        &json!({
            "home": home_dir.display().to_string(),
            "server_url": config.server_url,
            "device_id": config.device_id,
            "account_id": config.account_id,
            "npub": npub,
        }),
    )
}

fn cmd_invite<W: Write>(
    home_dir: &Path,
    mut args: Vec<String>,
    json_mode: bool,
    output: &mut W,
) -> Result<(), CliError> {
    let room_id = crate::take_option(&mut args, "--room-id")?;
    let room_name = crate::take_option(&mut args, "--room-name")?;
    let max_joins = crate::take_option(&mut args, "--max-joins")?
        .map(|value| crate::parse_u64("--max-joins", &value))
        .transpose()?
        .unwrap_or(DEFAULT_MAX_JOINS as u64) as u32;
    let ttl_ms = crate::take_option(&mut args, "--ttl-ms")?
        .map(|value| crate::parse_u64("--ttl-ms", &value))
        .transpose()?
        .unwrap_or(DEFAULT_INVITE_TTL_MS);
    crate::reject_extra_args(&args)?;

    let home = load_home(home_dir)?;
    let (mut store, mut device, mut delivery) = open_agent(&home)?;
    let now_ms = now_ms();

    // Resolve or create the room this invite admits people to.
    let room_id = match room_id {
        Some(room_id) => room_id,
        None => {
            let room_id = device
                .generate_object_id("room")
                .map_err(|error| CliError::Hermes(error.to_string()))?;
            let mls_group_id = format!("mls-{room_id}");
            device
                .create_group_state(&room_id, &mls_group_id)
                .map_err(|error| CliError::Hermes(error.to_string()))?;
            store
                .save_device_state(&device)
                .map_err(|error| CliError::Hermes(error.to_string()))?;
            delivery
                .bootstrap_account_room(&CreateRoomRequest {
                    room_id: room_id.clone(),
                    mls_group_id,
                    creator: device.device_ref().clone(),
                    protocol: RoomProtocol::default(),
                })
                .map_err(|error| CliError::Hermes(format!("{error:?}")))?;
            room_id
        }
    };

    let code = create_room_invite(
        &device,
        &mut delivery,
        CreateRoomInviteParams {
            room_id: &room_id,
            server_url: &home.config.server_url,
            display_name: room_name,
            max_joins,
            ttl_ms,
            now_ms,
        },
    )
    .map_err(|error| CliError::Hermes(format!("{error:?}")))?;
    let url = code
        .encode()
        .map_err(|error| CliError::Hermes(error.to_string()))?;
    append_invite(home_dir, &url)?;

    let pin = invite_current_pin(&code.invite_token, now_ms / 1000);
    let npub = npub_encode(&code.inviter_account_id)
        .map_err(|error| CliError::Hermes(format!("npub encoding failed: {error}")))?;
    let qr = render_qr(&url)?;

    if json_mode {
        crate::write_pretty_json(
            output,
            &json!({
                "url": url,
                "qr": qr,
                "pin": pin,
                "pin_window_seconds": INVITE_PIN_WINDOW_SECONDS,
                "invite_id": code.invite_id,
                "room_id": room_id,
                "npub": npub,
            }),
        )
    } else {
        writeln!(output, "{qr}").map_err(CliError::Output)?;
        writeln!(output, "Scan or open in Finite Chat:\n  {url}").map_err(CliError::Output)?;
        writeln!(output, "Agent identity: {npub}").map_err(CliError::Output)?;
        writeln!(
            output,
            "Challenge PIN (rotates every {INVITE_PIN_WINDOW_SECONDS}s): {pin}"
        )
        .map_err(CliError::Output)?;
        writeln!(
            output,
            "Re-display with: finitechat hermes pin --invite-id {}",
            code.invite_id
        )
        .map_err(CliError::Output)
    }
}

fn cmd_pin<W: Write>(
    home_dir: &Path,
    mut args: Vec<String>,
    output: &mut W,
) -> Result<(), CliError> {
    let invite_id = crate::take_option(&mut args, "--invite-id")?;
    crate::reject_extra_args(&args)?;
    let invites = load_invites(home_dir)?;
    let code = match invite_id {
        Some(invite_id) => invites
            .into_iter()
            .find(|code| code.invite_id == invite_id)
            .ok_or_else(|| CliError::Hermes(format!("no stored invite {invite_id}")))?,
        None => invites
            .into_iter()
            .next_back()
            .ok_or_else(|| CliError::Hermes("no stored invites".to_owned()))?,
    };
    let now = now_secs();
    let pin = invite_current_pin(&code.invite_token, now);
    let url = code
        .encode()
        .map_err(|error| CliError::Hermes(error.to_string()))?;
    crate::write_pretty_json(
        output,
        &json!({
            "invite_id": code.invite_id,
            "room_id": code.room_id,
            "url": url,
            "qr": render_qr(&url)?,
            "pin": pin,
            "pin_window_seconds": INVITE_PIN_WINDOW_SECONDS,
            "seconds_remaining": INVITE_PIN_WINDOW_SECONDS - (now % INVITE_PIN_WINDOW_SECONDS),
        }),
    )
}

/// User-side join: scan/paste the invite URL, type the PIN, land in the
/// chat (ADR 0006). Submits the proof-bound join request, waits for the
/// inviter's verdict, activates the Welcome from the room's server,
/// verifies the inviter credential, and pins the room to its server.
fn cmd_join<W: Write>(
    home_dir: &Path,
    mut args: Vec<String>,
    output: &mut W,
) -> Result<(), CliError> {
    let url = crate::required_option(&mut args, "--url")?;
    let pin = crate::required_option(&mut args, "--pin")?;
    let display_name = crate::take_option(&mut args, "--name")?;
    let timeout_ms = crate::take_option(&mut args, "--timeout-ms")?
        .map(|value| crate::parse_u64("--timeout-ms", &value))
        .transpose()?
        .unwrap_or(60_000);
    crate::reject_extra_args(&args)?;

    let code = InviteCodeV1::parse(&url).map_err(|error| CliError::Hermes(error.to_string()))?;
    let home = load_home(home_dir)?;
    let (mut store, mut device, _home_delivery) = open_agent(&home)?;
    // The invite names the room's server; every leg of the join talks to it.
    let mut delivery =
        HttpRuntimeDelivery::new(ReqwestHttpRuntimeTransport::new(code.server_url.clone()));
    let handle = submit_invite_join_request(
        &mut store,
        &mut device,
        &mut delivery,
        &code,
        &pin,
        display_name,
        now_ms(),
    )
    .map_err(|error| CliError::Hermes(format!("{error:?}")))?;

    let deadline = Instant::now() + Duration::from_millis(timeout_ms);
    let verdict = loop {
        let status = delivery
            .invite_join_status(&code.invite_id, &handle.request_id)
            .map_err(|error| CliError::Hermes(format!("{error:?}")))?;
        match status.state {
            HttpInviteJoinState::Accepted => break "accepted",
            HttpInviteJoinState::Rejected => break "rejected",
            HttpInviteJoinState::Pending => {
                if Instant::now() >= deadline {
                    break "pending";
                }
                let remaining = deadline
                    .saturating_duration_since(Instant::now())
                    .as_millis() as u64;
                let wait = SyncWaitRequest {
                    rooms: Vec::new(),
                    invites: vec![SyncWaitInvite {
                        invite_id: code.invite_id.clone(),
                        // Wake only when a further request gets resolved
                        // (ours is among the pending ones).
                        seen_requests: u32::MAX,
                        seen_resolved: status.resolved_requests,
                    }],
                    wait_ms: remaining,
                };
                if delivery.sync_wait(&wait).is_err() {
                    std::thread::sleep(Duration::from_millis(POLL_SLEEP_MS));
                }
            }
        }
    };
    if verdict != "accepted" {
        crate::write_pretty_json(
            output,
            &json!({ "state": verdict, "room_id": code.room_id }),
        )?;
        return if verdict == "rejected" {
            Err(CliError::Hermes(
                "join was rejected (wrong or expired PIN?)".to_owned(),
            ))
        } else {
            Err(CliError::Hermes(
                "join timed out awaiting the agent".to_owned(),
            ))
        };
    }

    // Claim and activate the Welcome from the room's server.
    let options = sync_options();
    while Instant::now() < deadline {
        run_runtime_sync_tick(&mut store, &mut device, &mut delivery, &options)
            .map_err(|error| CliError::Hermes(format!("{error:?}")))?;
        if device.room_server_url(&code.room_id).is_some()
            || device.group_epoch(&code.room_id).is_ok()
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(POLL_SLEEP_MS));
    }
    finalize_invited_room(&mut store, &mut device, &code)
        .map_err(|error| CliError::Hermes(error.to_string()))?;
    crate::write_pretty_json(
        output,
        &json!({
            "state": "joined",
            "room_id": code.room_id,
            "server_url": code.server_url,
            "inviter_account_id": code.inviter_account_id,
        }),
    )
}

#[derive(Debug, Deserialize)]
struct PollRequest {
    #[serde(default)]
    room_id: Option<String>,
    #[serde(default)]
    limit: Option<u32>,
    #[serde(default)]
    timeout_millis: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct HermesInboxState {
    #[serde(default)]
    events: Vec<HermesInboxEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HermesInboxEvent {
    key: String,
    room_id: String,
    seq: u64,
    message_id: String,
    created_at_ms: u64,
    event: HermesPollEventV1,
}

fn cmd_poll<W: Write>(home_dir: &Path, request: Value, output: &mut W) -> Result<(), CliError> {
    let request: PollRequest = serde_json::from_value(request).map_err(CliError::Json)?;
    let limit = request.limit.unwrap_or(10).clamp(1, 32) as usize;
    let timeout = Duration::from_millis(
        request
            .timeout_millis
            .unwrap_or(0)
            .min(MAX_HERMES_POLL_TIMEOUT_MILLIS),
    );
    let home = load_home(home_dir)?;
    let (mut store, mut device, mut delivery) = open_agent(&home)?;
    let invites = load_invites(home_dir)?;
    let options = sync_options();
    let started = Instant::now();
    let own_account = device.device_ref().account_id.clone();
    let mut inbox = load_hermes_inbox(home_dir)?;
    let mut events = pending_hermes_inbox_events(&inbox, request.room_id.as_deref(), limit);
    let mut joined: Vec<String> = Vec::new();
    let mut invite_counts: BTreeMap<String, (u32, u32)> = BTreeMap::new();

    while events.is_empty() {
        let mut report = run_runtime_sync_tick(&mut store, &mut device, &mut delivery, &options)
            .map_err(|error| CliError::Hermes(format!("{error:?}")))?;

        // Rooms pinned to other servers (joined via invite) sync against
        // their own server (ADR 0005 grouping).
        let mut room_servers: Vec<String> = device
            .room_sync_cursors()
            .into_iter()
            .filter_map(|cursor| cursor.server_url)
            .collect();
        room_servers.sort_unstable();
        room_servers.dedup();
        for server_url in room_servers {
            let mut room_delivery =
                HttpRuntimeDelivery::new(ReqwestHttpRuntimeTransport::new(server_url.clone()));
            let room_report = run_room_server_sync_tick(
                &mut store,
                &mut device,
                &mut room_delivery,
                &options,
                &server_url,
            )
            .map_err(|error| CliError::Hermes(format!("{error:?}")))?;
            report.applied_entries.extend(room_report.applied_entries);
        }

        // Process pending invite joins after the sync tick so any
        // previously submitted add commit has merged (pending-commit rule).
        let mut accepted_any = false;
        for code in &invites {
            match accept_pending_invite_joins(
                &mut store,
                &mut device,
                &mut delivery,
                code,
                now_ms(),
            ) {
                Ok(invite_report) => {
                    accepted_any |= !invite_report.accepted.is_empty();
                    invite_counts.insert(
                        code.invite_id.clone(),
                        (
                            invite_report.total_requests,
                            invite_report.resolved_requests,
                        ),
                    );
                    joined.extend(
                        invite_report
                            .accepted
                            .iter()
                            .map(|joiner| joiner.account_id.clone()),
                    );
                }
                // Expired/closed sessions are routine; the next `hermes
                // invite` replaces them.
                Err(_) => continue,
            }
        }
        if accepted_any {
            // Merge our own add commit promptly so sends are not blocked on
            // the next poll cycle.
            run_runtime_sync_tick(&mut store, &mut device, &mut delivery, &options)
                .map_err(|error| CliError::Hermes(format!("{error:?}")))?;
        }

        for applied in &report.applied_entries {
            if let Some(room_filter) = &request.room_id
                && room_filter != &applied.room_id
            {
                continue;
            }
            let AppliedLogEntry::Application { plaintext, sender } = &applied.entry else {
                continue;
            };
            if sender.account_id == own_account {
                continue;
            }
            let context = HermesPollEventContext {
                home_dir,
                room_id: &applied.room_id,
                seq: applied.seq,
                message_id: &applied.message_id,
                sender_account_id: &sender.account_id,
                sender_device_id: &sender.device_id,
            };
            if let Some(event) = hermes_poll_event_from_application_plaintext(context, plaintext)? {
                enqueue_hermes_inbox_event(home_dir, &mut inbox, event)?;
            }
        }
        events = pending_hermes_inbox_events(&inbox, request.room_id.as_deref(), limit);

        if !events.is_empty() || !joined.is_empty() || started.elapsed() >= timeout {
            break;
        }
        let remaining = timeout.saturating_sub(started.elapsed()).as_millis() as u64;
        wait_for_hermes_sync_hint(
            &home,
            &mut delivery,
            &device,
            &invites,
            &invite_counts,
            remaining,
        );
    }

    crate::write_pretty_json(output, &json!({ "events": events, "joined": joined }))
}

fn cmd_ack<W: Write>(home_dir: &Path, request: Value, output: &mut W) -> Result<(), CliError> {
    let request: HermesAckRequestV1 = serde_json::from_value(request).map_err(CliError::Json)?;
    request
        .validate_limits()
        .map_err(|error| CliError::Hermes(error.to_string()))?;
    let mut inbox = load_hermes_inbox(home_dir)?;
    let key = hermes_inbox_key(&request.room_id, request.seq, &request.message_id);
    let before = inbox.events.len();
    inbox.events.retain(|event| event.key != key);
    if inbox.events.len() != before {
        save_hermes_inbox(home_dir, &inbox)?;
    }
    crate::write_pretty_json(
        output,
        &json!({ "acked": inbox.events.len() != before, "room_id": request.room_id, "seq": request.seq, "message_id": request.message_id }),
    )
}

fn load_hermes_inbox(home_dir: &Path) -> Result<HermesInboxState, CliError> {
    let path = home_dir.join(HERMES_INBOX_FILE);
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Default::default()),
        Err(error) => return Err(CliError::Hermes(error.to_string())),
    };
    serde_json::from_str(&raw).map_err(CliError::Json)
}

fn save_hermes_inbox(home_dir: &Path, inbox: &HermesInboxState) -> Result<(), CliError> {
    write_private(
        home_dir.join(HERMES_INBOX_FILE),
        &serde_json::to_string_pretty(inbox).map_err(CliError::Serialize)?,
    )
}

fn enqueue_hermes_inbox_event(
    home_dir: &Path,
    inbox: &mut HermesInboxState,
    event: HermesPollEventV1,
) -> Result<(), CliError> {
    let key = hermes_inbox_key(&event.room_id, event.seq, &event.message_id);
    if inbox.events.iter().any(|existing| existing.key == key) {
        return Ok(());
    }
    inbox.events.push(HermesInboxEvent {
        key,
        room_id: event.room_id.clone(),
        seq: event.seq,
        message_id: event.message_id.clone(),
        created_at_ms: now_ms(),
        event,
    });
    save_hermes_inbox(home_dir, inbox)
}

fn pending_hermes_inbox_events(
    inbox: &HermesInboxState,
    room_filter: Option<&str>,
    limit: usize,
) -> Vec<HermesPollEventV1> {
    inbox
        .events
        .iter()
        .filter(|entry| match room_filter {
            Some(room_id) => room_id == entry.room_id,
            None => true,
        })
        .take(limit)
        .map(|entry| entry.event.clone())
        .collect()
}

fn hermes_inbox_key(room_id: &str, seq: u64, message_id: &str) -> String {
    format!("{room_id}\x1f{seq}\x1f{message_id}")
}

fn wait_for_hermes_sync_hint(
    home: &AgentHome,
    delivery: &mut AgentDelivery,
    device: &FiniteChatDevice,
    invites: &[InviteCodeV1],
    invite_counts: &BTreeMap<String, (u32, u32)>,
    wait_ms: u64,
) {
    if wait_ms == 0 {
        return;
    }
    let cursors = device.room_sync_cursors();
    let (home_rooms, remote_rooms) = group_sync_wait_rooms(
        &home.config.server_url,
        cursors
            .into_iter()
            .map(|cursor| (cursor.room_id, cursor.after_seq, cursor.server_url)),
    );
    let invite_waits: Vec<SyncWaitInvite> = invites
        .iter()
        .map(|code| {
            let (seen_requests, seen_resolved) = invite_counts
                .get(&code.invite_id)
                .copied()
                .unwrap_or((u32::MAX, u32::MAX));
            SyncWaitInvite {
                invite_id: code.invite_id.clone(),
                seen_requests,
                seen_resolved,
            }
        })
        .collect();
    let wait_target_count =
        usize::from(!home_rooms.is_empty() || !invite_waits.is_empty()) + remote_rooms.len();
    if wait_target_count == 0 {
        std::thread::sleep(Duration::from_millis(wait_ms.min(POLL_SLEEP_MS)));
        return;
    }
    let per_target_wait_ms = if wait_target_count == 1 {
        wait_ms
    } else {
        wait_ms.min(1_000)
    };
    let started = Instant::now();

    if !home_rooms.is_empty() || !invite_waits.is_empty() {
        let target_wait_ms = bounded_remaining_wait_ms(wait_ms, per_target_wait_ms, started);
        let wait = SyncWaitRequest {
            rooms: home_rooms,
            invites: invite_waits,
            wait_ms: target_wait_ms,
        };
        sync_wait_or_sleep(delivery, &wait, target_wait_ms);
    }
    for (server_url, rooms) in remote_rooms {
        let target_wait_ms = bounded_remaining_wait_ms(wait_ms, per_target_wait_ms, started);
        if target_wait_ms == 0 {
            break;
        }
        let mut room_delivery =
            HttpRuntimeDelivery::new(ReqwestHttpRuntimeTransport::new(server_url));
        let wait = SyncWaitRequest {
            rooms,
            invites: Vec::new(),
            wait_ms: target_wait_ms,
        };
        sync_wait_or_sleep(&mut room_delivery, &wait, target_wait_ms);
    }
}

fn bounded_remaining_wait_ms(wait_ms: u64, per_target_wait_ms: u64, started: Instant) -> u64 {
    let elapsed = started.elapsed().as_millis() as u64;
    wait_ms.saturating_sub(elapsed).min(per_target_wait_ms)
}

fn sync_wait_or_sleep(delivery: &mut AgentDelivery, wait: &SyncWaitRequest, fallback_wait_ms: u64) {
    if delivery.sync_wait(wait).is_err() {
        std::thread::sleep(Duration::from_millis(fallback_wait_ms.min(POLL_SLEEP_MS)));
    }
}

fn group_sync_wait_rooms<I>(
    home_server_url: &str,
    cursors: I,
) -> (Vec<SyncWaitRoom>, BTreeMap<String, Vec<SyncWaitRoom>>)
where
    I: IntoIterator<Item = (String, u64, Option<String>)>,
{
    let mut home_rooms = Vec::new();
    let mut remote_rooms: BTreeMap<String, Vec<SyncWaitRoom>> = BTreeMap::new();
    for (room_id, after_seq, server_url) in cursors {
        let room = SyncWaitRoom { room_id, after_seq };
        match server_url {
            Some(server_url) if server_url != home_server_url => {
                remote_rooms.entry(server_url).or_default().push(room);
            }
            Some(_) | None => home_rooms.push(room),
        }
    }
    (home_rooms, remote_rooms)
}

#[derive(Clone, Copy)]
struct HermesPollEventContext<'a> {
    home_dir: &'a Path,
    room_id: &'a str,
    seq: u64,
    message_id: &'a str,
    sender_account_id: &'a str,
    sender_device_id: &'a str,
}

fn hermes_poll_event_from_application_plaintext(
    context: HermesPollEventContext<'_>,
    plaintext: &[u8],
) -> Result<Option<HermesPollEventV1>, CliError> {
    if let Ok(event) = serde_json::from_slice::<DecryptedApplicationEventV1>(plaintext) {
        if event.validate_limits().is_err() {
            return Ok(None);
        }
        return match event.kind {
            DurableAppEventKind::ChatMessage => {
                hermes_poll_event_from_chat_payload(context, &event.payload, true)
            }
            DurableAppEventKind::ConversationCreate
            | DurableAppEventKind::ConversationUpdate
            | DurableAppEventKind::ConversationArchive
            | DurableAppEventKind::ConversationSegmentStart
            | DurableAppEventKind::ChatEdit
            | DurableAppEventKind::ChatReaction
            | DurableAppEventKind::ChatReceipt
            | DurableAppEventKind::RuntimeStateSnapshot
            | DurableAppEventKind::RuntimeCommandRequest
            | DurableAppEventKind::RuntimeCommandResult
            | DurableAppEventKind::RuntimeCommandCancel
            | DurableAppEventKind::StreamStart
            | DurableAppEventKind::StreamFinish
            | DurableAppEventKind::Namespaced { .. } => Ok(None),
        };
    }

    hermes_poll_event_from_chat_payload(context, plaintext, false)
}

fn hermes_poll_event_from_chat_payload(
    context: HermesPollEventContext<'_>,
    payload: &[u8],
    typed_chat_message: bool,
) -> Result<Option<HermesPollEventV1>, CliError> {
    if let Some(payload) = HermesMessagePayloadV1::decode(payload)
        .map_err(|error| CliError::Hermes(error.to_string()))?
    {
        let mut event = payload.into_poll_event(
            context.room_id.to_owned(),
            context.seq,
            context.message_id.to_owned(),
            context.sender_account_id.to_owned(),
            context.sender_device_id.to_owned(),
        );
        materialize_poll_event_attachments(context.home_dir, &mut event)?;
        return Ok(Some(event));
    }

    if typed_chat_message && payload_is_typed_json(payload) {
        return Ok(None);
    }

    let Ok(text) = std::str::from_utf8(payload) else {
        return Ok(None);
    };
    if text.trim().is_empty() {
        return Ok(None);
    }
    HermesPollEventV1::finite_chat_text(
        context.room_id.to_owned(),
        context.seq,
        context.message_id.to_owned(),
        context.sender_account_id.to_owned(),
        context.sender_device_id.to_owned(),
        text.to_owned(),
    )
    .map(Some)
    .map_err(|error| CliError::Hermes(error.to_string()))
}

fn materialize_poll_event_attachments(
    home_dir: &Path,
    event: &mut HermesPollEventV1,
) -> Result<(), CliError> {
    for attachment in &mut event.attachments {
        if attachment.path.is_some() {
            continue;
        }
        let Some(reference) = attachment.blob.clone() else {
            continue;
        };
        let path = materialize_blob_attachment(home_dir, &reference)?;
        attachment.path = Some(path.to_string_lossy().into_owned());
    }
    Ok(())
}

fn materialize_blob_attachment(
    home_dir: &Path,
    reference: &AttachmentBlobReferenceV1,
) -> Result<PathBuf, CliError> {
    let path = hermes_attachment_cache_path(home_dir, reference);
    if let Ok(existing) = fs::read(&path)
        && existing.len() as u64 == reference.plaintext_size
        && sha256_hex(&existing) == reference.plaintext_sha256
    {
        return Ok(path);
    }

    let request = prepare_blossom_download_http_request(reference)
        .map_err(|error| CliError::Hermes(error.to_string()))?;
    let response = reqwest::blocking::Client::new()
        .get(request.url)
        .send()
        .map_err(|error| CliError::Hermes(format!("attachment download failed: {error}")))?;
    let status = response.status().as_u16();
    let body = response
        .bytes()
        .map_err(|error| CliError::Hermes(format!("attachment download failed: {error}")))?
        .to_vec();
    let downloaded = finish_blossom_download_http_response(
        reference,
        BlossomDownloadHttpResponse {
            status,
            body: &body,
        },
    )
    .map_err(|error| CliError::Hermes(format!("attachment verification failed: {error}")))?;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| CliError::Hermes(error.to_string()))?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, &downloaded.plaintext).map_err(|error| CliError::Hermes(error.to_string()))?;
    fs::rename(&tmp, &path).map_err(|error| CliError::Hermes(error.to_string()))?;
    Ok(path)
}

fn hermes_attachment_cache_path(home_dir: &Path, reference: &AttachmentBlobReferenceV1) -> PathBuf {
    home_dir
        .join(ATTACHMENT_CACHE_DIR)
        .join(&reference.plaintext_sha256)
        .join(sanitized_attachment_filename(&reference.metadata.filename))
}

fn sanitized_attachment_filename(filename: &str) -> String {
    let leaf = Path::new(filename)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("attachment")
        .trim();
    let sanitized = leaf
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() || sanitized == "." || sanitized == ".." {
        "attachment".to_owned()
    } else {
        sanitized
    }
}

fn payload_is_typed_json(payload: &[u8]) -> bool {
    serde_json::from_slice::<Value>(payload)
        .ok()
        .and_then(|value| value.get("type").and_then(Value::as_str).map(str::to_owned))
        .is_some()
}

fn cmd_send<W: Write>(home_dir: &Path, request: Value, output: &mut W) -> Result<(), CliError> {
    let request: HermesSendRequestV1 = serde_json::from_value(request).map_err(CliError::Json)?;
    let payload = HermesMessagePayloadV1::from_send(&request)
        .encode()
        .map_err(|error| CliError::Hermes(error.to_string()))?;
    let accepted = append_payload_to_room(home_dir, &request.room_id, payload)?;
    update_running_after_send(home_dir, &request, &accepted)?;
    write_event_accepted(output, &accepted)
}

fn cmd_edit<W: Write>(home_dir: &Path, request: Value, output: &mut W) -> Result<(), CliError> {
    let request: HermesEditRequestV1 = serde_json::from_value(request).map_err(CliError::Json)?;
    let payload = HermesMessagePayloadV1::from_edit(&request)
        .encode()
        .map_err(|error| CliError::Hermes(error.to_string()))?;
    let accepted = append_payload_to_room(home_dir, &request.room_id, payload)?;
    update_running_after_edit(home_dir, &request)?;
    write_event_accepted(output, &accepted)
}

fn cmd_recover<W: Write>(home_dir: &Path, _request: Value, output: &mut W) -> Result<(), CliError> {
    let running = load_hermes_running(home_dir)?;
    let mut recovered = 0usize;
    for message in &running.messages {
        let recovery = HermesEditRequestV1 {
            room_id: message.room_id.clone(),
            conversation_id: message.conversation_id.clone(),
            message_id: message.message_id.clone(),
            text: "Hermes gateway restarted before this turn completed.".to_owned(),
            status: HermesMessageStatusV1::Complete,
            finalize: true,
            metadata: BTreeMap::new(),
        };
        let payload = HermesMessagePayloadV1::from_edit(&recovery)
            .encode()
            .map_err(|error| CliError::Hermes(error.to_string()))?;
        append_payload_to_room(home_dir, &recovery.room_id, payload)?;
        recovered += 1;
    }
    if recovered > 0 {
        save_hermes_running(home_dir, &HermesRunningState::default())?;
    }
    crate::write_pretty_json(output, &json!({ "recovered": recovered }))
}

fn append_payload_to_room(
    home_dir: &Path,
    room_id: &str,
    payload: Vec<u8>,
) -> Result<EventAccepted, CliError> {
    let home = load_home(home_dir)?;
    let (mut store, mut device, mut delivery) = open_agent(&home)?;
    sync_agent_room(&mut store, &mut device, &mut delivery, room_id)?;
    if device
        .has_pending_commit(room_id)
        .map_err(|error| CliError::Hermes(error.to_string()))?
    {
        // Merge our own pending add commit from the log before sending.
        sync_agent_room(&mut store, &mut device, &mut delivery, room_id)?;
    }
    let idempotency_key = device
        .generate_object_id("hermes-send")
        .map_err(|error| CliError::Hermes(error.to_string()))?;
    let request = device
        .create_application_request(room_id, &payload, idempotency_key)
        .map_err(|error| CliError::Hermes(error.to_string()))?;
    store
        .save_device_state(&device)
        .map_err(|error| CliError::Hermes(error.to_string()))?;
    let accepted = delivery
        .append_event(&request, DurableAppEventKind::ChatMessage.delivery_policy())
        .map_err(|error| CliError::Hermes(format!("{error:?}")))?;
    if let Err(error) = sync_agent_room(&mut store, &mut device, &mut delivery, room_id) {
        eprintln!(
            "warning: post-accept sync failed after appending {}: {error}",
            accepted.message_id
        );
    }
    Ok(accepted)
}

fn write_event_accepted<W: Write>(
    output: &mut W,
    accepted: &EventAccepted,
) -> Result<(), CliError> {
    crate::write_pretty_json(
        output,
        &json!({ "message_id": &accepted.message_id, "seq": accepted.seq }),
    )
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct HermesRunningState {
    #[serde(default)]
    messages: Vec<HermesRunningMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HermesRunningMessage {
    room_id: String,
    conversation_id: Option<String>,
    message_id: String,
}

fn load_hermes_running(home_dir: &Path) -> Result<HermesRunningState, CliError> {
    let path = home_dir.join(HERMES_RUNNING_FILE);
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Default::default()),
        Err(error) => return Err(CliError::Hermes(error.to_string())),
    };
    serde_json::from_str(&raw).map_err(CliError::Json)
}

fn save_hermes_running(home_dir: &Path, running: &HermesRunningState) -> Result<(), CliError> {
    write_private(
        home_dir.join(HERMES_RUNNING_FILE),
        &serde_json::to_string_pretty(running).map_err(CliError::Serialize)?,
    )
}

fn update_running_after_send(
    home_dir: &Path,
    request: &HermesSendRequestV1,
    accepted: &EventAccepted,
) -> Result<(), CliError> {
    if request.status != HermesMessageStatusV1::Running {
        return Ok(());
    }
    upsert_hermes_running_message(
        home_dir,
        HermesRunningMessage {
            room_id: request.room_id.clone(),
            conversation_id: request.conversation_id.clone(),
            message_id: accepted.message_id.clone(),
        },
    )
}

fn update_running_after_edit(
    home_dir: &Path,
    request: &HermesEditRequestV1,
) -> Result<(), CliError> {
    if request.finalize || request.status == HermesMessageStatusV1::Complete {
        return remove_hermes_running_message(home_dir, &request.room_id, &request.message_id);
    }
    upsert_hermes_running_message(
        home_dir,
        HermesRunningMessage {
            room_id: request.room_id.clone(),
            conversation_id: request.conversation_id.clone(),
            message_id: request.message_id.clone(),
        },
    )
}

fn upsert_hermes_running_message(
    home_dir: &Path,
    message: HermesRunningMessage,
) -> Result<(), CliError> {
    let mut running = load_hermes_running(home_dir)?;
    running.messages.retain(|existing| {
        existing.room_id != message.room_id || existing.message_id != message.message_id
    });
    running.messages.push(message);
    save_hermes_running(home_dir, &running)
}

fn remove_hermes_running_message(
    home_dir: &Path,
    room_id: &str,
    message_id: &str,
) -> Result<(), CliError> {
    let mut running = load_hermes_running(home_dir)?;
    let before = running.messages.len();
    running
        .messages
        .retain(|message| message.room_id != room_id || message.message_id != message_id);
    if running.messages.len() != before {
        save_hermes_running(home_dir, &running)?;
    }
    Ok(())
}

fn sync_agent_room(
    store: &mut SqliteClientStore,
    device: &mut FiniteChatDevice,
    home_delivery: &mut AgentDelivery,
    room_id: &str,
) -> Result<(), CliError> {
    let options = sync_options();
    if let Some(room_server_url) = device.room_server_url(room_id).map(str::to_owned) {
        let mut room_delivery =
            HttpRuntimeDelivery::new(ReqwestHttpRuntimeTransport::new(room_server_url.clone()));
        run_room_server_sync_tick(
            store,
            device,
            &mut room_delivery,
            &options,
            &room_server_url,
        )
    } else {
        run_runtime_sync_tick(store, device, home_delivery, &options)
    }
    .map(|_| ())
    .map_err(|error| CliError::Hermes(format!("{error:?}")))
}

fn cmd_activity<W: Write>(home_dir: &Path, request: Value, output: &mut W) -> Result<(), CliError> {
    let request: HermesActivityRequestV1 =
        serde_json::from_value(request).map_err(CliError::Json)?;
    let home = load_home(home_dir)?;
    let (_store, device, mut delivery) = open_agent(&home)?;
    let payload = if matches!(request.action, EphemeralActivityActionV1::Set) {
        serde_json::to_vec(&request.payload).map_err(CliError::Serialize)?
    } else {
        Vec::new()
    };
    let activity = DecryptedEphemeralActivityV1 {
        activity_kind: request.activity_kind,
        activity_id: request.activity_id,
        action: request.action,
        payload,
    };
    activity
        .validate_limits()
        .map_err(|error| CliError::Hermes(error.to_string()))?;
    let plaintext = serde_json::to_vec(&activity).map_err(CliError::Serialize)?;
    let payload = device
        .encrypt_activity_payload(&request.room_id, &plaintext)
        .map_err(|error| CliError::Hermes(error.to_string()))?;
    let now = now_ms();
    let expires_in = if request.expires_in_millis == 0 {
        ACTIVITY_DEFAULT_EXPIRY_MS
    } else {
        request.expires_in_millis
    };
    let accepted = delivery
        .append_activity(&AppendEphemeralActivityRequest {
            room_id: request.room_id.clone(),
            mls_group_id: device
                .room_mls_group_id(&request.room_id)
                .map_err(|error| CliError::Hermes(error.to_string()))?,
            epoch: device
                .group_epoch(&request.room_id)
                .map_err(|error| CliError::Hermes(error.to_string()))?,
            sender: device.device_ref().clone(),
            conversation_id: request.conversation_id.clone(),
            payload,
            received_at_ms: now,
            expires_at_ms: now.saturating_add(expires_in),
        })
        .map_err(|error| CliError::Hermes(format!("{error:?}")))?;
    crate::write_pretty_json(output, &json!({ "accepted": true, "result": accepted }))
}

// --- agent home plumbing ---

fn resolve_home(args: &mut Vec<String>) -> Result<PathBuf, CliError> {
    if let Some(home) = crate::take_option(args, "--home")? {
        return Ok(PathBuf::from(home));
    }
    std::env::var("FINITECHAT_HOME")
        .map(PathBuf::from)
        .map_err(|_| CliError::Usage("pass --home DIR or set FINITECHAT_HOME".to_owned()))
}

fn load_home(dir: &Path) -> Result<AgentHome, CliError> {
    let config: AgentConfig =
        serde_json::from_str(&fs::read_to_string(dir.join(CONFIG_FILE)).map_err(|_| {
            CliError::Hermes(format!(
                "agent home {} is not initialized (run hermes init)",
                dir.display()
            ))
        })?)
        .map_err(CliError::Json)?;
    let nsec_hex = fs::read_to_string(dir.join(NSEC_FILE))
        .map_err(|error| CliError::Hermes(error.to_string()))?;
    let bytes = crate::parse_hex("agent.nsec", nsec_hex.trim())?;
    let bytes: [u8; NOSTR_SECRET_KEY_BYTES] = bytes
        .try_into()
        .map_err(|_| CliError::Hermes("agent.nsec must be 32 bytes of hex".to_owned()))?;
    let secret =
        NostrSecretKey::from_bytes(bytes).map_err(|error| CliError::Hermes(error.to_string()))?;
    Ok(AgentHome {
        dir: dir.to_path_buf(),
        config,
        secret,
    })
}

type AgentDelivery = HttpRuntimeDelivery<ReqwestHttpRuntimeTransport>;

fn open_agent(
    home: &AgentHome,
) -> Result<(SqliteClientStore, FiniteChatDevice, AgentDelivery), CliError> {
    let store = open_store(&home.dir, &home.secret, &home.config.device_id)?;
    let config = device_config(&home.secret, &home.config.device_id, now_secs());
    let device = store
        .load_device(config)
        .map_err(|error| CliError::Hermes(error.to_string()))?;
    let delivery = HttpRuntimeDelivery::new(ReqwestHttpRuntimeTransport::new(
        home.config.server_url.clone(),
    ));
    // The store mutably borrows during ticks; return all three.
    Ok((store, device, delivery))
}

fn open_store(
    dir: &Path,
    secret: &NostrSecretKey,
    device_id: &str,
) -> Result<SqliteClientStore, CliError> {
    let options = SqliteClientStoreOptions::from_nostr_secret(secret, device_id)
        .map_err(|error| CliError::Hermes(error.to_string()))?;
    SqliteClientStore::open(dir.join(STORE_FILE), options)
        .map_err(|error| CliError::Hermes(error.to_string()))
}

fn device_config(
    secret: &NostrSecretKey,
    device_id: &str,
    now_secs: u64,
) -> FiniteChatDeviceConfig {
    FiniteChatDeviceConfig {
        account_secret_key: secret.clone(),
        device_id: device_id.to_owned(),
        now_unix_seconds: now_secs,
        credential_not_before_unix_seconds: now_secs.saturating_sub(3600),
        credential_not_after_unix_seconds: now_secs + CREDENTIAL_VALIDITY_SECONDS,
    }
}

fn sync_options() -> RuntimeSyncOptions {
    RuntimeSyncOptions {
        key_package_target_available: KEY_PACKAGE_TARGET_AVAILABLE,
        max_sync_pages_per_room: 8,
    }
}

fn load_invites(dir: &Path) -> Result<Vec<InviteCodeV1>, CliError> {
    let raw = fs::read_to_string(dir.join(INVITES_FILE)).unwrap_or_else(|_| "[]".to_owned());
    let urls: Vec<String> = serde_json::from_str(&raw).map_err(CliError::Json)?;
    let mut codes = Vec::with_capacity(urls.len());
    for url in urls {
        codes.push(InviteCodeV1::parse(&url).map_err(|error| CliError::Hermes(error.to_string()))?);
    }
    Ok(codes)
}

fn append_invite(dir: &Path, url: &str) -> Result<(), CliError> {
    let raw = fs::read_to_string(dir.join(INVITES_FILE)).unwrap_or_else(|_| "[]".to_owned());
    let mut urls: Vec<String> = serde_json::from_str(&raw).map_err(CliError::Json)?;
    urls.push(url.to_owned());
    write_private(
        dir.join(INVITES_FILE),
        &serde_json::to_string_pretty(&urls).map_err(CliError::Serialize)?,
    )
}

fn write_private(path: PathBuf, contents: &str) -> Result<(), CliError> {
    fs::write(&path, contents).map_err(|error| CliError::Hermes(error.to_string()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
            .map_err(|error| CliError::Hermes(error.to_string()))?;
    }
    Ok(())
}

fn read_request(request_json: Option<String>) -> Result<Value, CliError> {
    match request_json {
        Some(raw) => serde_json::from_str(&raw).map_err(CliError::Json),
        None => {
            let mut raw = String::new();
            std::io::stdin()
                .read_to_string(&mut raw)
                .map_err(|error| CliError::Hermes(error.to_string()))?;
            if raw.trim().is_empty() {
                return Ok(Value::Object(serde_json::Map::new()));
            }
            serde_json::from_str(&raw).map_err(CliError::Json)
        }
    }
}

fn render_qr(url: &str) -> Result<String, CliError> {
    let code = qrcode::QrCode::new(url.as_bytes())
        .map_err(|error| CliError::Hermes(format!("QR encoding failed: {error}")))?;
    Ok(code
        .render::<qrcode::render::unicode::Dense1x2>()
        .quiet_zone(true)
        .build())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn now_secs() -> u64 {
    now_ms() / 1000
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn take_flag(args: &mut Vec<String>, name: &str) -> bool {
    if let Some(index) = args.iter().position(|arg| arg == name) {
        args.remove(index);
        return true;
    }
    false
}

pub(crate) fn hermes_usage() -> String {
    "hermes commands:\n  finitechat hermes [--home DIR] init --server URL [--device-id ID]\n  finitechat hermes [--home DIR] invite [--room-id ID] [--room-name NAME] [--max-joins N] [--ttl-ms N] [--json]\n  finitechat hermes [--home DIR] pin [--invite-id ID]\n  finitechat hermes [--home DIR] join --url INVITE_URL --pin PIN [--name NAME] [--timeout-ms N]\n  finitechat hermes [--home DIR] poll --json   (stdin: {room_id?, limit?, timeout_millis?})\n  finitechat hermes [--home DIR] ack --json    (stdin: HermesAckRequestV1)\n  finitechat hermes [--home DIR] send --json   (stdin: HermesSendRequestV1)\n  finitechat hermes [--home DIR] edit --json   (stdin: HermesEditRequestV1)\n  finitechat hermes [--home DIR] recover --json\n  finitechat hermes [--home DIR] activity --json (stdin: HermesActivityRequestV1)\n  (FINITECHAT_HOME may replace --home; --request-json JSON may replace stdin)".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use finitechat_hermes::HermesMessageTypeV1;

    #[test]
    fn poll_decoder_unwraps_typed_chat_message_but_ignores_non_hermes_typed_payloads() {
        let home = tempfile::tempdir().unwrap();
        let wrapped_poll = DecryptedApplicationEventV1 {
            kind: DurableAppEventKind::ChatMessage,
            conversation_id: None,
            payload: br#"{"type":"finitechat.chat.poll.v1","question":"Lunch?","options":[]}"#
                .to_vec(),
        };
        let plaintext = serde_json::to_vec(&wrapped_poll).unwrap();
        let event = hermes_poll_event_from_application_plaintext(
            HermesPollEventContext {
                home_dir: home.path(),
                room_id: "room-main",
                seq: 1,
                message_id: "message-1",
                sender_account_id: "alice",
                sender_device_id: "ios",
            },
            &plaintext,
        )
        .unwrap();
        assert!(
            event.is_none(),
            "typed non-Hermes payloads must not leak to agents as JSON text"
        );

        let wrapped_text = DecryptedApplicationEventV1 {
            kind: DurableAppEventKind::ChatMessage,
            conversation_id: None,
            payload: b"plain hello".to_vec(),
        };
        let plaintext = serde_json::to_vec(&wrapped_text).unwrap();
        let event = hermes_poll_event_from_application_plaintext(
            HermesPollEventContext {
                home_dir: home.path(),
                room_id: "room-main",
                seq: 2,
                message_id: "message-2",
                sender_account_id: "alice",
                sender_device_id: "ios",
            },
            &plaintext,
        )
        .unwrap()
        .expect("typed plain-text chat is still bridge-visible");
        assert_eq!(event.text, "plain hello");
        assert_eq!(event.message_type, HermesMessageTypeV1::Text);
    }

    #[test]
    fn sync_wait_grouping_keeps_same_server_pinned_rooms_on_home_wait() {
        let (home_rooms, remote_rooms) = group_sync_wait_rooms(
            "https://chat.finite.computer",
            vec![
                ("room-home".to_owned(), 4, None),
                (
                    "room-pinned-same".to_owned(),
                    9,
                    Some("https://chat.finite.computer".to_owned()),
                ),
            ],
        );

        assert_eq!(home_rooms.len(), 2);
        assert_eq!(home_rooms[0].room_id, "room-home");
        assert_eq!(home_rooms[1].room_id, "room-pinned-same");
        assert!(remote_rooms.is_empty());
    }

    #[test]
    fn sync_wait_grouping_sends_other_server_rooms_to_that_server() {
        let (home_rooms, remote_rooms) = group_sync_wait_rooms(
            "https://chat.finite.computer",
            vec![(
                "room-remote".to_owned(),
                7,
                Some("https://other.example".to_owned()),
            )],
        );

        assert!(home_rooms.is_empty());
        let rooms = remote_rooms.get("https://other.example").unwrap();
        assert_eq!(rooms.len(), 1);
        assert_eq!(rooms[0].room_id, "room-remote");
        assert_eq!(rooms[0].after_seq, 7);
    }
}
