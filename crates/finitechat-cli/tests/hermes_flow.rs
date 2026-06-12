//! End-to-end exercise of the `hermes` subcommand family against a live
//! server: init → invite (URL/QR/PIN) → a user device joins via the invite
//! → poll admits the join → send/edit/activity round trips. This is the
//! same surface the Python platform plugin shells to.

use finitechat_client::{
    AppliedLogEntry, FiniteChatDevice, FiniteChatDeviceConfig, HttpRuntimeDelivery,
    ReqwestHttpRuntimeTransport, RuntimeSyncOptions, SqliteClientStore, SqliteClientStoreOptions,
    finalize_invited_room, run_runtime_sync_tick, submit_invite_join_request,
};
use finitechat_hermes::{HermesMessagePayloadV1, HermesMessageStatusV1, HermesSendKindV1};
use finitechat_mls::{NOSTR_SECRET_KEY_BYTES, NostrSecretKey};
use finitechat_proto::{DurableAppEventKind, InviteCodeV1, invite_current_pin};
use finitechat_server::{HttpServerState, http_router};
use serde_json::{Value, json};
use std::time::{SystemTime, UNIX_EPOCH};

const USER_SECRET: [u8; NOSTR_SECRET_KEY_BYTES] = [41; NOSTR_SECRET_KEY_BYTES];

fn spawn_live_http_server(path: &std::path::Path) -> String {
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
    let health_url = format!("{server_url}/health");
    let client = reqwest::blocking::Client::new();
    for _ in 0..100 {
        if client
            .get(&health_url)
            .send()
            .map(|response| response.status().is_success())
            .unwrap_or(false)
        {
            return server_url;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("live HTTP server did not become healthy");
}

fn hermes(args: &[&str]) -> Value {
    let mut output = Vec::new();
    finitechat_cli::run(args.iter().map(|arg| arg.to_string()), &mut output)
        .unwrap_or_else(|error| panic!("hermes {args:?} failed: {error}"));
    serde_json::from_slice(&output)
        .unwrap_or_else(|error| panic!("hermes {args:?} produced invalid JSON: {error}"))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

#[test]
fn hermes_cli_inits_invites_admits_and_round_trips_messages() {
    let dir = tempfile::tempdir().unwrap();
    let server_db = dir.path().join("server.sqlite3");
    let server_url = spawn_live_http_server(&server_db);
    let home = dir.path().join("agent-home");
    let home_arg = home.display().to_string();

    // init: fresh nostr identity, encrypted store, 0600 secrets.
    let init = hermes(&["hermes", "--home", &home_arg, "init", "--server", &server_url]);
    let agent_account = init["account_id"].as_str().unwrap().to_owned();
    assert!(init["npub"].as_str().unwrap().starts_with("npub1"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(home.join("agent.nsec"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    // invite: creates the room, prints URL + QR + PIN.
    let invite = hermes(&[
        "hermes",
        "--home",
        &home_arg,
        "invite",
        "--room-name",
        "Hermes Agent",
        "--json",
    ]);
    let url = invite["url"].as_str().unwrap();
    let room_id = invite["room_id"].as_str().unwrap().to_owned();
    assert!(!invite["qr"].as_str().unwrap().is_empty());
    assert_eq!(invite["pin"].as_str().unwrap().len(), 6);
    let code = InviteCodeV1::parse(url).expect("printed URL is a valid invite code");
    assert_eq!(code.inviter_account_id, agent_account);
    assert_eq!(code.room_id, room_id);

    // pin: re-displays the current PIN for headless agents.
    let pin_out = hermes(&["hermes", "--home", &home_arg, "pin"]);
    assert_eq!(pin_out["invite_id"], invite["invite_id"]);

    // The user scans the QR and types the PIN.
    let user_config = FiniteChatDeviceConfig {
        account_secret_key: NostrSecretKey::from_bytes(USER_SECRET).unwrap(),
        device_id: "user_phone".to_owned(),
        now_unix_seconds: now_ms() / 1000,
        credential_not_before_unix_seconds: now_ms() / 1000 - 3600,
        credential_not_after_unix_seconds: now_ms() / 1000 + 86400,
    };
    let mut user_store = SqliteClientStore::open(
        dir.path().join("user.sqlite3"),
        SqliteClientStoreOptions::from_nostr_secret(
            &user_config.account_secret_key,
            &user_config.device_id,
        )
        .unwrap(),
    )
    .unwrap();
    let mut user = FiniteChatDevice::new(user_config.clone()).unwrap();
    user_store.save_device_state(&user).unwrap();
    let mut user_delivery =
        HttpRuntimeDelivery::new(ReqwestHttpRuntimeTransport::new(server_url.clone()));
    let pin = invite_current_pin(&code.invite_token, now_ms() / 1000);
    submit_invite_join_request(
        &mut user_store,
        &mut user,
        &mut user_delivery,
        &code,
        &pin,
        Some("Paul".to_owned()),
        now_ms(),
    )
    .unwrap();

    // poll admits the verified join (and merges the agent's own commit).
    let poll = hermes(&[
        "hermes",
        "--home",
        &home_arg,
        "poll",
        "--request-json",
        r#"{"timeout_millis":0}"#,
    ]);
    assert_eq!(
        poll["joined"].as_array().unwrap(),
        &vec![Value::String(user.device_ref().account_id.clone())]
    );

    // The user activates the Welcome, verifies the agent, pins the server.
    let options = RuntimeSyncOptions {
        key_package_target_available: 0,
        max_sync_pages_per_room: 4,
    };
    let report =
        run_runtime_sync_tick(&mut user_store, &mut user, &mut user_delivery, &options).unwrap();
    assert_eq!(report.claimed_welcomes, 1);
    finalize_invited_room(&mut user_store, &mut user, &code).unwrap();

    // Agent sends through the bridge; the user reads it decrypted.
    let send_request = json!({
        "room_id": room_id,
        "conversation_id": null,
        "text": "hello from your agent",
        "kind": "message",
        "status": "complete",
        "reply_to_message_id": null,
    });
    let sent = hermes(&[
        "hermes",
        "--home",
        &home_arg,
        "send",
        "--request-json",
        &send_request.to_string(),
    ]);
    let agent_message_id = sent["message_id"].as_str().unwrap().to_owned();

    let report = run_runtime_sync_tick(
        &mut user_store,
        &mut user,
        &mut user_delivery,
        &options,
    )
    .unwrap();
    // The user's room is pinned to its room server; in this test home and
    // room server are the same process, so use the room-server tick.
    let report = if report.applied_entries.is_empty() {
        finitechat_client::run_room_server_sync_tick(
            &mut user_store,
            &mut user,
            &mut user_delivery,
            &options,
            &server_url,
        )
        .unwrap()
    } else {
        report
    };
    let AppliedLogEntry::Application { plaintext, sender } = &report.applied_entries[0].entry
    else {
        panic!("expected application entry");
    };
    assert_eq!(sender.account_id, agent_account);
    let payload = HermesMessagePayloadV1::decode(plaintext).unwrap().unwrap();
    assert_eq!(payload.text, "hello from your agent");

    // The user replies; the bridge polls it out with the authenticated
    // sender identity.
    let reply = HermesMessagePayloadV1 {
        payload_type: finitechat_hermes::HERMES_MESSAGE_PAYLOAD_TYPE_V1.to_owned(),
        conversation_id: None,
        text: "hi agent".to_owned(),
        kind: HermesSendKindV1::Message,
        status: HermesMessageStatusV1::Complete,
        edit_of: None,
        attachments: Vec::new(),
        reply_to_message_id: None,
        sender_name: Some("Paul".to_owned()),
        metadata: Default::default(),
    };
    let request = user
        .create_application_request(&code.room_id, &reply.encode().unwrap(), "user-reply-1")
        .unwrap();
    user_store.save_device_state(&user).unwrap();
    user_delivery
        .append_event(
            &request,
            DurableAppEventKind::ChatMessage.delivery_policy(),
        )
        .unwrap();

    let poll = hermes(&[
        "hermes",
        "--home",
        &home_arg,
        "poll",
        "--request-json",
        r#"{"timeout_millis":5000}"#,
    ]);
    let events = poll["events"].as_array().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["text"], "hi agent");
    assert_eq!(
        events[0]["source"]["user_id"].as_str().unwrap(),
        user.device_ref().account_id
    );
    assert_eq!(events[0]["source"]["user_name"], "Paul");

    // Streaming edit finalization lands as a new payload superseding the
    // original message id.
    let edit_request = json!({
        "room_id": code.room_id,
        "conversation_id": null,
        "message_id": agent_message_id,
        "text": "hello from your agent (edited)",
        "status": "complete",
        "finalize": true,
    });
    hermes(&[
        "hermes",
        "--home",
        &home_arg,
        "edit",
        "--request-json",
        &edit_request.to_string(),
    ]);
    let report = finitechat_client::run_room_server_sync_tick(
        &mut user_store,
        &mut user,
        &mut user_delivery,
        &options,
        &server_url,
    )
    .unwrap();
    let edited = report
        .applied_entries
        .iter()
        .find_map(|entry| match &entry.entry {
            AppliedLogEntry::Application { plaintext, .. } => {
                HermesMessagePayloadV1::decode(plaintext).unwrap()
            }
            _ => None,
        })
        .expect("edit payload");
    assert_eq!(edited.edit_of.as_deref(), Some(agent_message_id.as_str()));
    assert_eq!(edited.text, "hello from your agent (edited)");

    // Typing indicator goes out encrypted under the room's exporter key.
    let activity_request = json!({
        "room_id": code.room_id,
        "conversation_id": null,
        "activity_kind": "working",
        "activity_id": null,
        "action": "set",
        "payload": {},
        "expires_in_millis": 30000,
    });
    let activity = hermes(&[
        "hermes",
        "--home",
        &home_arg,
        "activity",
        "--request-json",
        &activity_request.to_string(),
    ]);
    assert_eq!(activity["accepted"], true);
}
