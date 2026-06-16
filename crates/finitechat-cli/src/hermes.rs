//! The `finitechat hermes` subcommand family: the JSON bridge
//! the Hermes platform plugin shells to (ADR 0002), plus agent onboarding
//! (ADR 0006: init → invite URL/QR/PIN → chat).
//!
//! The agent's durable home lives under `--home` / `$FINITECHAT_HOME`:
//! `agent.nsec` (0600), `config.json`, `invites.json` (0600, each line is a
//! full invite URL — the URL carries the invite token), and the encrypted
//! client store `client.sqlite3`.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use finitechat_client::{
    AppliedLogEntry, CreateRoomInviteParams, FiniteChatDevice, FiniteChatDeviceConfig,
    HttpRuntimeDelivery, ReqwestHttpRuntimeTransport, RuntimeDelivery, RuntimeSyncOptions,
    SqliteClientStore, SqliteClientStoreOptions, accept_pending_invite_joins, create_room_invite,
    finalize_invited_room, generate_account_secret, run_room_server_sync_tick,
    run_runtime_sync_tick, submit_invite_join_request,
};
use finitechat_hermes::{
    HermesActivityRequestV1, HermesEditRequestV1, HermesMessagePayloadV1, HermesPollEventV1,
    HermesSendRequestV1, MAX_HERMES_POLL_TIMEOUT_MILLIS,
};
use finitechat_http::{HttpInviteJoinState, SyncWaitInvite, SyncWaitRequest, SyncWaitRoom};
use finitechat_mls::{NOSTR_SECRET_KEY_BYTES, NostrSecretKey};
use finitechat_proto::{
    AppendEphemeralActivityRequest, CreateRoomRequest, DurableAppEventKind,
    INVITE_PIN_WINDOW_SECONDS, InviteCodeV1, RoomProtocol, invite_current_pin, npub_encode,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::CliError;

const CONFIG_FILE: &str = "config.json";
const NSEC_FILE: &str = "agent.nsec";
const INVITES_FILE: &str = "invites.json";
const STORE_FILE: &str = "client.sqlite3";
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
        "send" => cmd_send(&home_dir, read_request(request_json)?, output),
        "edit" => cmd_edit(&home_dir, read_request(request_json)?, output),
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
    let mut events: Vec<HermesPollEventV1> = Vec::new();
    let mut joined: Vec<String> = Vec::new();
    let mut invite_counts: std::collections::BTreeMap<String, (u32, u32)> =
        std::collections::BTreeMap::new();

    loop {
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
            if events.len() >= limit {
                break;
            }
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
            let event = match HermesMessagePayloadV1::decode(plaintext)
                .map_err(|error| CliError::Hermes(error.to_string()))?
            {
                Some(payload) => payload.into_poll_event(
                    applied.room_id.clone(),
                    applied.seq,
                    applied.message_id.clone(),
                    sender.account_id.clone(),
                    sender.device_id.clone(),
                ),
                None => {
                    let Ok(text) = std::str::from_utf8(plaintext) else {
                        continue;
                    };
                    if text.trim().is_empty() {
                        continue;
                    }
                    HermesPollEventV1::finite_chat_text(
                        applied.room_id.clone(),
                        applied.seq,
                        applied.message_id.clone(),
                        sender.account_id.clone(),
                        sender.device_id.clone(),
                        text.to_owned(),
                    )
                    .map_err(|error| CliError::Hermes(error.to_string()))?
                }
            };
            events.push(event);
        }

        if !events.is_empty() || !joined.is_empty() || started.elapsed() >= timeout {
            break;
        }
        // Long-poll on the home server instead of sleeping: wake on any
        // watched room advancing or invite session changing. Rooms pinned
        // to other servers bound the wait so they still get re-synced.
        let cursors = device.room_sync_cursors();
        let has_pinned_rooms = cursors.iter().any(|cursor| cursor.server_url.is_some());
        let remaining = timeout.saturating_sub(started.elapsed()).as_millis() as u64;
        let wait_ms = if has_pinned_rooms {
            remaining.min(POLL_SLEEP_MS)
        } else {
            remaining
        };
        let wait = SyncWaitRequest {
            rooms: cursors
                .into_iter()
                .filter(|cursor| cursor.server_url.is_none())
                .map(|cursor| SyncWaitRoom {
                    room_id: cursor.room_id,
                    after_seq: cursor.after_seq,
                })
                .collect(),
            invites: invites
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
                .collect(),
            wait_ms,
        };
        if delivery.sync_wait(&wait).is_err() {
            // Older servers without /sync/wait fall back to a short sleep.
            std::thread::sleep(Duration::from_millis(POLL_SLEEP_MS));
        }
    }

    crate::write_pretty_json(output, &json!({ "events": events, "joined": joined }))
}

fn cmd_send<W: Write>(home_dir: &Path, request: Value, output: &mut W) -> Result<(), CliError> {
    let request: HermesSendRequestV1 = serde_json::from_value(request).map_err(CliError::Json)?;
    let payload = HermesMessagePayloadV1::from_send(&request)
        .encode()
        .map_err(|error| CliError::Hermes(error.to_string()))?;
    append_payload(home_dir, &request.room_id, payload, output)
}

fn cmd_edit<W: Write>(home_dir: &Path, request: Value, output: &mut W) -> Result<(), CliError> {
    let request: HermesEditRequestV1 = serde_json::from_value(request).map_err(CliError::Json)?;
    let payload = HermesMessagePayloadV1::from_edit(&request)
        .encode()
        .map_err(|error| CliError::Hermes(error.to_string()))?;
    append_payload(home_dir, &request.room_id, payload, output)
}

fn append_payload<W: Write>(
    home_dir: &Path,
    room_id: &str,
    payload: Vec<u8>,
    output: &mut W,
) -> Result<(), CliError> {
    let home = load_home(home_dir)?;
    let (mut store, mut device, mut delivery) = open_agent(&home)?;
    if device
        .has_pending_commit(room_id)
        .map_err(|error| CliError::Hermes(error.to_string()))?
    {
        // Merge our own pending add commit from the log before sending.
        run_runtime_sync_tick(&mut store, &mut device, &mut delivery, &sync_options())
            .map_err(|error| CliError::Hermes(format!("{error:?}")))?;
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
    crate::write_pretty_json(
        output,
        &json!({ "message_id": accepted.message_id, "seq": accepted.seq }),
    )
}

fn cmd_activity<W: Write>(home_dir: &Path, request: Value, output: &mut W) -> Result<(), CliError> {
    let request: HermesActivityRequestV1 =
        serde_json::from_value(request).map_err(CliError::Json)?;
    let home = load_home(home_dir)?;
    let (_store, device, mut delivery) = open_agent(&home)?;
    let plaintext = serde_json::to_vec(&json!({
        "activity_kind": request.activity_kind,
        "activity_id": request.activity_id,
        "action": request.action,
        "payload": request.payload,
    }))
    .map_err(CliError::Serialize)?;
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
    "hermes commands:\n  finitechat hermes [--home DIR] init --server URL [--device-id ID]\n  finitechat hermes [--home DIR] invite [--room-id ID] [--room-name NAME] [--max-joins N] [--ttl-ms N] [--json]\n  finitechat hermes [--home DIR] pin [--invite-id ID]\n  finitechat hermes [--home DIR] join --url INVITE_URL --pin PIN [--name NAME] [--timeout-ms N]\n  finitechat hermes [--home DIR] poll --json   (stdin: {room_id?, limit?, timeout_millis?})\n  finitechat hermes [--home DIR] send --json   (stdin: HermesSendRequestV1)\n  finitechat hermes [--home DIR] edit --json   (stdin: HermesEditRequestV1)\n  finitechat hermes [--home DIR] activity --json (stdin: HermesActivityRequestV1)\n  (FINITECHAT_HOME may replace --home; --request-json JSON may replace stdin)".to_owned()
}
