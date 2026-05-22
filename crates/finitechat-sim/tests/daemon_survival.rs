use finitechat_engine::{
    AppendApplicationEventRequest, AppendEventRequest, DeliveryService, EventAccepted,
    RoomSyncProjection, envelope,
};
use finitechat_proto::{
    ApplicationDeliveryPolicy, DeviceRef, DurableAppEventKind, LogEntryKind,
    MAX_RUNTIME_COMMAND_LEDGER_RECORDS, RuntimeCommandIngressContext, RuntimeCommandJsonPayloadV1,
    RuntimeCommandLedger, RuntimeCommandLedgerStatus, RuntimeCommandPayloadKindV1,
    RuntimeCommandRequestV1, RuntimeCommandResultV1, RuntimeCommandTargetV1,
    RuntimeCommandTerminalContext, RuntimeCommandTerminalDecision, RuntimeCommandTerminalStatusV1,
    RuntimeStateSnapshotV1,
};
use finitechat_sim::{SimWorld, alice, bob};
use serde_json::json;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GatewayState {
    Live,
    Down,
    Hung,
}

#[derive(Debug)]
struct FakeDaemon {
    device: DeviceRef,
    room_id: String,
    group_id: String,
    after_seq: u64,
    gateway: GatewayState,
    config_generation: u64,
    ledger: RuntimeCommandLedger,
    last_snapshot_message_id: Option<String>,
    crash_after_recording_first_request: bool,
}

impl FakeDaemon {
    fn new(device: DeviceRef, room_id: String, group_id: String, gateway: GatewayState) -> Self {
        Self {
            device,
            room_id,
            group_id,
            after_seq: 0,
            gateway,
            config_generation: 0,
            ledger: RuntimeCommandLedger::default(),
            last_snapshot_message_id: None,
            crash_after_recording_first_request: false,
        }
    }

    fn with_persisted_ledger(mut self, ledger: RuntimeCommandLedger, after_seq: u64) -> Self {
        self.ledger = ledger;
        self.after_seq = after_seq;
        self
    }

    fn sync_tick(&mut self, server: &mut DeliveryService) {
        self.execute_pending(server);
        let page = server
            .sync_events(&self.room_id, &self.device, self.after_seq)
            .unwrap();
        for entry in &page.entries {
            if let Some(request) = parse_runtime_command_request(&entry.envelope.payload) {
                self.ledger
                    .record_request(
                        RuntimeCommandIngressContext {
                            room_id: &self.room_id,
                            conversation_id: None,
                            accepted_seq: entry.seq,
                            original_message_id: &entry.message_id,
                            sender: &entry.sender,
                            local_device: &self.device,
                        },
                        &request,
                    )
                    .unwrap();
                self.after_seq = entry.seq;
                if self.crash_after_recording_first_request {
                    self.crash_after_recording_first_request = false;
                    return;
                }
            }
            self.after_seq = entry.seq;
        }
        if page.entries.is_empty() {
            self.after_seq = page.next_after_seq;
        }
        self.execute_pending(server);
        self.publish_gateway_snapshot(server);
    }

    fn execute_pending(&mut self, server: &mut DeliveryService) {
        let pending = self
            .ledger
            .pending_requests()
            .into_iter()
            .cloned()
            .collect::<Vec<_>>();
        assert!(pending.len() <= MAX_RUNTIME_COMMAND_LEDGER_RECORDS as usize);
        for record in pending {
            match record.command.as_str() {
                "finitecomputer.runtime.gateway.restart" => {
                    self.gateway = GatewayState::Live;
                    let result = runtime_command_result(&record.request_id);
                    self.append_runtime_command_result(server, &record, &result);
                }
                "finitecomputer.runtime.config.update" => {
                    self.config_generation += 1;
                    let result =
                        runtime_config_command_result(&record.request_id, self.config_generation);
                    self.append_runtime_command_result(server, &record, &result);
                    self.publish_config_snapshot(server);
                }
                _ => {}
            }
        }
    }

    fn append_runtime_command_result(
        &mut self,
        server: &mut DeliveryService,
        record: &finitechat_proto::RuntimeCommandLedgerRecord,
        result: &RuntimeCommandResultV1,
    ) {
        let accepted = append_application(
            server,
            ApplicationAppend {
                room_id: &self.room_id,
                group_id: &self.group_id,
                sender: self.device.clone(),
                epoch: current_epoch(server, &self.room_id),
                payload: runtime_command_result_payload(result),
                idempotency_key: format!("result_{}", record.request_id),
                delivery_policy: DurableAppEventKind::RuntimeCommandResult.delivery_policy(),
            },
        );
        let decision = self
            .ledger
            .apply_result(
                RuntimeCommandTerminalContext {
                    room_id: &self.room_id,
                    conversation_id: record.conversation_id.as_deref(),
                    request_sender: &record.sender,
                    accepted_seq: accepted.seq,
                    terminal_message_id: &accepted.message_id,
                },
                result,
            )
            .unwrap();
        assert_eq!(decision, RuntimeCommandTerminalDecision::Recorded);
    }

    fn publish_gateway_snapshot(&mut self, server: &mut DeliveryService) {
        let status = match self.gateway {
            GatewayState::Live => "live",
            GatewayState::Down => "down",
            GatewayState::Hung => "hung",
        };
        let accepted = append_application(
            server,
            ApplicationAppend {
                room_id: &self.room_id,
                group_id: &self.group_id,
                sender: self.device.clone(),
                epoch: current_epoch(server, &self.room_id),
                payload: runtime_gateway_snapshot_payload(server, &self.room_id, status),
                idempotency_key: format!(
                    "gateway_state_{}",
                    server.room(&self.room_id).unwrap().last_seq + 1
                ),
                delivery_policy: DurableAppEventKind::RuntimeStateSnapshot.delivery_policy(),
            },
        );
        self.last_snapshot_message_id = Some(accepted.message_id);
    }

    fn publish_config_snapshot(&mut self, server: &mut DeliveryService) {
        let accepted = append_application(
            server,
            ApplicationAppend {
                room_id: &self.room_id,
                group_id: &self.group_id,
                sender: self.device.clone(),
                epoch: current_epoch(server, &self.room_id),
                payload: runtime_config_snapshot_payload(
                    server,
                    &self.room_id,
                    self.config_generation,
                ),
                idempotency_key: format!(
                    "runtime_config_state_{}",
                    server.room(&self.room_id).unwrap().last_seq + 1
                ),
                delivery_policy: DurableAppEventKind::RuntimeStateSnapshot.delivery_policy(),
            },
        );
        self.last_snapshot_message_id = Some(accepted.message_id);
    }
}

#[test]
fn daemon_starts_when_hermes_is_absent_and_restarts_gateway() {
    let mut world = world_with_runtime();
    append_application(
        &mut world.server,
        ApplicationAppend {
            room_id: &world.room_id,
            group_id: &world.group_id,
            sender: alice(),
            epoch: 1,
            payload: runtime_command_request_payload("restart_1"),
            idempotency_key: "restart_1".to_string(),
            delivery_policy: DurableAppEventKind::RuntimeCommandRequest.delivery_policy(),
        },
    );
    let mut daemon = FakeDaemon::new(
        bob(),
        world.room_id.clone(),
        world.group_id.clone(),
        GatewayState::Down,
    );

    daemon.sync_tick(&mut world.server);

    assert_eq!(daemon.gateway, GatewayState::Live);
    assert_eq!(
        daemon
            .ledger
            .get(&world.room_id, None, &alice(), "restart_1")
            .unwrap()
            .status,
        RuntimeCommandLedgerStatus::Succeeded
    );
    let result_seq = command_result_seq(&world.server, &world.room_id, "restart_1").unwrap();
    let snapshot_seq = runtime_state_snapshot_seq_after(
        &world.server,
        &world.room_id,
        "runtime.gateway",
        result_seq,
    )
    .unwrap();
    assert!(snapshot_seq > result_seq);
    assert_eq!(world.server.command_inbox_len(), 1);
    let snapshot_effect = world
        .server
        .application_effect(daemon.last_snapshot_message_id.as_ref().unwrap())
        .unwrap();
    assert!(!snapshot_effect.creates_push());
    assert!(!snapshot_effect.creates_unread());
    assert!(!snapshot_effect.creates_command_inbox_work());
}

#[test]
fn hermes_hang_does_not_block_room_sync_or_state_snapshot() {
    let mut world = world_with_runtime();
    let message = append_application(
        &mut world.server,
        ApplicationAppend {
            room_id: &world.room_id,
            group_id: &world.group_id,
            sender: alice(),
            epoch: 1,
            payload: br#"{"type":"chat.message","body":"are you alive?"}"#.to_vec(),
            idempotency_key: "user_message_while_hung".to_string(),
            delivery_policy: DurableAppEventKind::ChatMessage.delivery_policy(),
        },
    );
    let mut daemon = FakeDaemon::new(
        bob(),
        world.room_id.clone(),
        world.group_id.clone(),
        GatewayState::Hung,
    );

    daemon.sync_tick(&mut world.server);

    assert!(daemon.after_seq >= message.seq);
    let snapshot_effect = world
        .server
        .application_effect(daemon.last_snapshot_message_id.as_ref().unwrap())
        .unwrap();
    assert!(!snapshot_effect.creates_push());
    assert_eq!(daemon.gateway, GatewayState::Hung);
}

#[test]
fn runtime_state_command_result_publishes_post_mutation_snapshot() {
    let mut world = world_with_runtime();
    append_application(
        &mut world.server,
        ApplicationAppend {
            room_id: &world.room_id,
            group_id: &world.group_id,
            sender: alice(),
            epoch: 1,
            payload: runtime_command_request_payload("restart_snapshot"),
            idempotency_key: "restart_snapshot".to_string(),
            delivery_policy: DurableAppEventKind::RuntimeCommandRequest.delivery_policy(),
        },
    );
    let mut daemon = FakeDaemon::new(
        bob(),
        world.room_id.clone(),
        world.group_id.clone(),
        GatewayState::Down,
    );

    daemon.sync_tick(&mut world.server);

    let result_seq = command_result_seq(&world.server, &world.room_id, "restart_snapshot").unwrap();
    let snapshot =
        runtime_state_snapshot_after(&world.server, &world.room_id, "runtime.gateway", result_seq)
            .unwrap();
    assert!(snapshot.seq > result_seq);
    assert_eq!(
        snapshot.snapshot.schema,
        "finitecomputer.runtime.gateway.status.v1"
    );
    assert_eq!(snapshot.snapshot.status_payload, br#"{"status":"live"}"#);
    let snapshot_effect = world
        .server
        .application_effect(&snapshot.message_id)
        .unwrap();
    assert!(!snapshot_effect.creates_push());
    assert!(!snapshot_effect.creates_unread());
    assert!(!snapshot_effect.creates_command_inbox_work());
}

#[test]
fn runtime_config_command_result_includes_post_mutation_status() {
    let mut world = world_with_runtime();
    append_application(
        &mut world.server,
        ApplicationAppend {
            room_id: &world.room_id,
            group_id: &world.group_id,
            sender: alice(),
            epoch: 1,
            payload: runtime_config_update_request_payload("config_update_1"),
            idempotency_key: "config_update_1".to_string(),
            delivery_policy: DurableAppEventKind::RuntimeCommandRequest.delivery_policy(),
        },
    );
    let mut daemon = FakeDaemon::new(
        bob(),
        world.room_id.clone(),
        world.group_id.clone(),
        GatewayState::Live,
    );

    daemon.sync_tick(&mut world.server);

    let result =
        runtime_command_result_after(&world.server, &world.room_id, "config_update_1").unwrap();
    let body = result.result.body.as_ref().unwrap();
    let result_status: serde_json::Value = serde_json::from_slice(&body.json_payload).unwrap();
    let snapshot =
        runtime_state_snapshot_after(&world.server, &world.room_id, "runtime.config", result.seq)
            .unwrap();
    let snapshot_status: serde_json::Value =
        serde_json::from_slice(&snapshot.snapshot.status_payload).unwrap();

    assert_eq!(daemon.config_generation, 1);
    assert_eq!(
        body.schema,
        "finitecomputer.runtime.config.update.result.v1"
    );
    assert_eq!(result_status, snapshot_status);
    assert_eq!(result_status["status"], "applied");
    assert_eq!(result_status["config_generation"], 1);
    assert!(snapshot.seq > result.seq);
    assert_eq!(
        snapshot.snapshot.schema,
        "finitecomputer.runtime.config.status.v1"
    );
    let result_effect = world.server.application_effect(&result.message_id).unwrap();
    assert!(!result_effect.creates_push());
    assert!(!result_effect.creates_unread());
    assert!(!result_effect.creates_command_inbox_work());
}

#[test]
fn runtime_stream_callback_only_triggers_sync() {
    let mut world = world_with_runtime();
    let accepted = append_application(
        &mut world.server,
        ApplicationAppend {
            room_id: &world.room_id,
            group_id: &world.group_id,
            sender: alice(),
            epoch: 1,
            payload: runtime_command_request_payload("restart_stream_hint"),
            idempotency_key: "restart_stream_hint".to_string(),
            delivery_policy: DurableAppEventKind::RuntimeCommandRequest.delivery_policy(),
        },
    );
    let mut projection = RoomSyncProjection::default();
    let mut daemon = FakeDaemon::new(
        bob(),
        world.room_id.clone(),
        world.group_id.clone(),
        GatewayState::Down,
    );

    assert!(
        projection
            .observe_stream_hint(&world.room_id, accepted.seq)
            .unwrap()
    );
    assert_eq!(projection.server_cursor(), 0);
    assert!(projection.applied_message_ids().is_empty());
    assert!(daemon.ledger.is_empty());
    assert_eq!(daemon.gateway, GatewayState::Down);
    assert!(command_result_seq(&world.server, &world.room_id, "restart_stream_hint").is_none());

    daemon.sync_tick(&mut world.server);

    assert_eq!(daemon.gateway, GatewayState::Live);
    assert_eq!(
        daemon
            .ledger
            .get(&world.room_id, None, &alice(), "restart_stream_hint")
            .unwrap()
            .status,
        RuntimeCommandLedgerStatus::Succeeded
    );
    assert!(command_result_seq(&world.server, &world.room_id, "restart_stream_hint").is_some());
}

#[test]
fn command_ledger_survives_restart_after_request_before_execution() {
    let mut world = world_with_runtime();
    append_application(
        &mut world.server,
        ApplicationAppend {
            room_id: &world.room_id,
            group_id: &world.group_id,
            sender: alice(),
            epoch: 1,
            payload: runtime_command_request_payload("restart_after_crash"),
            idempotency_key: "restart_after_crash".to_string(),
            delivery_policy: DurableAppEventKind::RuntimeCommandRequest.delivery_policy(),
        },
    );
    let mut daemon = FakeDaemon::new(
        bob(),
        world.room_id.clone(),
        world.group_id.clone(),
        GatewayState::Down,
    );
    daemon.crash_after_recording_first_request = true;

    daemon.sync_tick(&mut world.server);
    assert_eq!(
        daemon
            .ledger
            .get(&world.room_id, None, &alice(), "restart_after_crash")
            .unwrap()
            .status,
        RuntimeCommandLedgerStatus::Pending
    );
    assert_eq!(daemon.gateway, GatewayState::Down);

    let mut restarted = FakeDaemon::new(
        bob(),
        world.room_id.clone(),
        world.group_id.clone(),
        GatewayState::Down,
    )
    .with_persisted_ledger(daemon.ledger, daemon.after_seq);
    restarted.sync_tick(&mut world.server);

    assert_eq!(restarted.gateway, GatewayState::Live);
    assert_eq!(
        restarted
            .ledger
            .get(&world.room_id, None, &alice(), "restart_after_crash")
            .unwrap()
            .status,
        RuntimeCommandLedgerStatus::Succeeded
    );
}

#[test]
fn survival_fuzzer_keeps_sync_status_and_command_ledger_bounded() {
    let mut world = world_with_runtime();
    let mut daemon = FakeDaemon::new(
        bob(),
        world.room_id.clone(),
        world.group_id.clone(),
        GatewayState::Down,
    );
    let mut appended_restart_commands = 0usize;
    let mut last_after_seq = 0u64;

    for step in 0..96u32 {
        match step % 8 {
            0 => {
                append_chat_message(&mut world.server, &world.room_id, &world.group_id, step);
            }
            1 => {
                append_restart_command(
                    &mut world.server,
                    &world.room_id,
                    &world.group_id,
                    &format!("restart_fuzz_{step}"),
                );
                appended_restart_commands += 1;
            }
            2 => daemon.gateway = GatewayState::Hung,
            3 => daemon.sync_tick(&mut world.server),
            4 => {
                append_restart_command(
                    &mut world.server,
                    &world.room_id,
                    &world.group_id,
                    &format!("restart_crash_{step}"),
                );
                appended_restart_commands += 1;
                daemon.crash_after_recording_first_request = true;
                daemon.sync_tick(&mut world.server);
            }
            5 => {
                daemon = FakeDaemon::new(
                    bob(),
                    world.room_id.clone(),
                    world.group_id.clone(),
                    daemon.gateway,
                )
                .with_persisted_ledger(daemon.ledger, daemon.after_seq);
            }
            6 => daemon.gateway = GatewayState::Down,
            7 => daemon.sync_tick(&mut world.server),
            _ => unreachable!("modulo keeps action bounded"),
        }

        assert!(daemon.after_seq >= last_after_seq);
        assert!(daemon.after_seq <= world.server.room(&world.room_id).unwrap().last_seq);
        assert!(daemon.ledger.len() <= appended_restart_commands);
        assert!(world.server.command_inbox_len() <= appended_restart_commands);
        last_after_seq = daemon.after_seq;

        if let Some(message_id) = daemon.last_snapshot_message_id.as_ref() {
            let effect = world.server.application_effect(message_id).unwrap();
            assert!(!effect.creates_push());
            assert!(!effect.creates_unread());
            assert!(!effect.creates_command_inbox_work());
        }
    }

    for _ in 0..8 {
        daemon.sync_tick(&mut world.server);
    }

    assert!(daemon.ledger.pending_requests().is_empty());
    assert_eq!(daemon.gateway, GatewayState::Live);
    assert!(world.server.command_inbox_len() <= appended_restart_commands);
}

fn world_with_runtime() -> SimWorld {
    let mut world = SimWorld::direct_room().unwrap();
    world
        .add_device_commit(
            alice(),
            bob(),
            "kp_bob_survival",
            "welcome_bob_survival",
            0,
            "add_bob_survival",
        )
        .unwrap();
    world
        .activate_device("welcome_bob_survival", bob())
        .unwrap();
    world
}

fn append_chat_message(server: &mut DeliveryService, room_id: &str, group_id: &str, step: u32) {
    append_application(
        server,
        ApplicationAppend {
            room_id,
            group_id,
            sender: alice(),
            epoch: current_epoch(server, room_id),
            payload: json!({
                "type": "chat.message",
                "body": format!("survival fuzz {step}")
            })
            .to_string()
            .into_bytes(),
            idempotency_key: format!("survival_chat_{step}"),
            delivery_policy: DurableAppEventKind::ChatMessage.delivery_policy(),
        },
    );
}

fn append_restart_command(
    server: &mut DeliveryService,
    room_id: &str,
    group_id: &str,
    request_id: &str,
) {
    append_application(
        server,
        ApplicationAppend {
            room_id,
            group_id,
            sender: alice(),
            epoch: current_epoch(server, room_id),
            payload: runtime_command_request_payload(request_id),
            idempotency_key: request_id.to_string(),
            delivery_policy: DurableAppEventKind::RuntimeCommandRequest.delivery_policy(),
        },
    );
}

struct ApplicationAppend<'a> {
    room_id: &'a str,
    group_id: &'a str,
    sender: DeviceRef,
    epoch: u64,
    payload: Vec<u8>,
    idempotency_key: String,
    delivery_policy: ApplicationDeliveryPolicy,
}

fn append_application(server: &mut DeliveryService, app: ApplicationAppend<'_>) -> EventAccepted {
    server
        .append_application_event(AppendApplicationEventRequest {
            event: AppendEventRequest {
                room_id: app.room_id.to_string(),
                sender: app.sender.clone(),
                envelope: envelope(
                    app.room_id.to_string(),
                    app.group_id.to_string(),
                    app.sender,
                    app.epoch,
                    LogEntryKind::Application,
                    app.payload,
                ),
                idempotency_key: app.idempotency_key,
            },
            delivery_policy: app.delivery_policy,
        })
        .unwrap()
}

fn current_epoch(server: &DeliveryService, room_id: &str) -> u64 {
    server.room(room_id).unwrap().current_epoch
}

fn parse_runtime_command_request(payload: &[u8]) -> Option<RuntimeCommandRequestV1> {
    let request: RuntimeCommandRequestV1 = serde_json::from_slice(payload).ok()?;
    request.validate_structure().ok()?;
    match request.command.as_str() {
        "finitecomputer.runtime.gateway.restart" | "finitecomputer.runtime.config.update" => {
            Some(request)
        }
        _ => None,
    }
}

fn runtime_command_request_payload(request_id: &str) -> Vec<u8> {
    let request = RuntimeCommandRequestV1 {
        payload_kind: RuntimeCommandPayloadKindV1::Request,
        request_id: request_id.to_string(),
        command: "finitecomputer.runtime.gateway.restart".to_string(),
        target: RuntimeCommandTargetV1 {
            account_id: bob().account_id,
            device_id: Some(bob().device_id),
        },
        resource_key: Some("hermes.config".to_string()),
        body: RuntimeCommandJsonPayloadV1 {
            schema: "finitecomputer.runtime.gateway.restart.v1".to_string(),
            json_payload: br#"{}"#.to_vec(),
        },
    };
    request.validate_structure().unwrap();
    serde_json::to_vec(&request).unwrap()
}

fn runtime_config_update_request_payload(request_id: &str) -> Vec<u8> {
    let request = RuntimeCommandRequestV1 {
        payload_kind: RuntimeCommandPayloadKindV1::Request,
        request_id: request_id.to_string(),
        command: "finitecomputer.runtime.config.update".to_string(),
        target: RuntimeCommandTargetV1 {
            account_id: bob().account_id,
            device_id: Some(bob().device_id),
        },
        resource_key: Some("hermes.config".to_string()),
        body: RuntimeCommandJsonPayloadV1 {
            schema: "finitecomputer.runtime.config.update.v1".to_string(),
            json_payload: br#"{"gateway_enabled":true}"#.to_vec(),
        },
    };
    request.validate_structure().unwrap();
    serde_json::to_vec(&request).unwrap()
}

fn runtime_command_result(request_id: &str) -> RuntimeCommandResultV1 {
    RuntimeCommandResultV1 {
        payload_kind: RuntimeCommandPayloadKindV1::Result,
        request_id: request_id.to_string(),
        status: RuntimeCommandTerminalStatusV1::Succeeded,
        body: Some(RuntimeCommandJsonPayloadV1 {
            schema: "finitecomputer.runtime.gateway.restart.result.v1".to_string(),
            json_payload: br#"{"status":"live"}"#.to_vec(),
        }),
        error: None,
        clears_activity: Vec::new(),
    }
}

fn runtime_config_command_result(
    request_id: &str,
    config_generation: u64,
) -> RuntimeCommandResultV1 {
    RuntimeCommandResultV1 {
        payload_kind: RuntimeCommandPayloadKindV1::Result,
        request_id: request_id.to_string(),
        status: RuntimeCommandTerminalStatusV1::Succeeded,
        body: Some(RuntimeCommandJsonPayloadV1 {
            schema: "finitecomputer.runtime.config.update.result.v1".to_string(),
            json_payload: runtime_config_status_payload(config_generation),
        }),
        error: None,
        clears_activity: Vec::new(),
    }
}

fn runtime_command_result_payload(result: &RuntimeCommandResultV1) -> Vec<u8> {
    result.validate_structure().unwrap();
    serde_json::to_vec(&result).unwrap()
}

fn runtime_gateway_snapshot_payload(
    server: &DeliveryService,
    room_id: &str,
    status: &str,
) -> Vec<u8> {
    let revision = server.room(room_id).unwrap().last_seq + 1;
    let observed_at_ms = revision * 1_000;
    let snapshot = RuntimeStateSnapshotV1 {
        state_key: "runtime.gateway".to_string(),
        schema: "finitecomputer.runtime.gateway.status.v1".to_string(),
        revision,
        observed_at_ms,
        expires_at_ms: observed_at_ms + 60_000,
        status_payload: json!({ "status": status }).to_string().into_bytes(),
    };
    snapshot.validate_limits().unwrap();
    serde_json::to_vec(&snapshot).unwrap()
}

fn runtime_config_snapshot_payload(
    server: &DeliveryService,
    room_id: &str,
    config_generation: u64,
) -> Vec<u8> {
    let revision = server.room(room_id).unwrap().last_seq + 1;
    let observed_at_ms = revision * 1_000;
    let snapshot = RuntimeStateSnapshotV1 {
        state_key: "runtime.config".to_string(),
        schema: "finitecomputer.runtime.config.status.v1".to_string(),
        revision,
        observed_at_ms,
        expires_at_ms: observed_at_ms + 60_000,
        status_payload: runtime_config_status_payload(config_generation),
    };
    snapshot.validate_limits().unwrap();
    serde_json::to_vec(&snapshot).unwrap()
}

fn runtime_config_status_payload(config_generation: u64) -> Vec<u8> {
    json!({
        "config_generation": config_generation,
        "gateway_enabled": true,
        "status": "applied"
    })
    .to_string()
    .into_bytes()
}

fn command_result_seq(server: &DeliveryService, room_id: &str, request_id: &str) -> Option<u64> {
    server
        .room(room_id)?
        .log
        .iter()
        .find(|entry| {
            serde_json::from_slice::<RuntimeCommandResultV1>(&entry.envelope.payload)
                .map(|result| result.request_id == request_id)
                .unwrap_or(false)
        })
        .map(|entry| entry.seq)
}

struct CommandResultLogEntry {
    seq: u64,
    message_id: String,
    result: RuntimeCommandResultV1,
}

fn runtime_command_result_after(
    server: &DeliveryService,
    room_id: &str,
    request_id: &str,
) -> Option<CommandResultLogEntry> {
    server
        .room(room_id)?
        .log
        .iter()
        .filter_map(|entry| {
            let result =
                serde_json::from_slice::<RuntimeCommandResultV1>(&entry.envelope.payload).ok()?;
            if result.request_id != request_id {
                return None;
            }
            Some(CommandResultLogEntry {
                seq: entry.seq,
                message_id: entry.message_id.clone(),
                result,
            })
        })
        .next()
}

fn runtime_state_snapshot_seq_after(
    server: &DeliveryService,
    room_id: &str,
    state_key: &str,
    after_seq: u64,
) -> Option<u64> {
    runtime_state_snapshot_after(server, room_id, state_key, after_seq).map(|entry| entry.seq)
}

struct SnapshotLogEntry {
    seq: u64,
    message_id: String,
    snapshot: RuntimeStateSnapshotV1,
}

fn runtime_state_snapshot_after(
    server: &DeliveryService,
    room_id: &str,
    state_key: &str,
    after_seq: u64,
) -> Option<SnapshotLogEntry> {
    server
        .room(room_id)?
        .log
        .iter()
        .filter_map(|entry| {
            if entry.seq <= after_seq {
                return None;
            }
            let snapshot =
                serde_json::from_slice::<RuntimeStateSnapshotV1>(&entry.envelope.payload).ok()?;
            if snapshot.state_key != state_key {
                return None;
            }
            Some(SnapshotLogEntry {
                seq: entry.seq,
                message_id: entry.message_id.clone(),
                snapshot,
            })
        })
        .next()
}
