use finitechat_engine::{
    AppendApplicationEventRequest, AppendEventRequest, DeliveryService, EventAccepted, envelope,
};
use finitechat_proto::{
    ApplicationDeliveryPolicy, DeviceRef, DurableAppEventKind, LogEntryKind,
    RuntimeCommandIngressContext, RuntimeCommandJsonPayloadV1, RuntimeCommandLedger,
    RuntimeCommandLedgerStatus, RuntimeCommandPayloadKindV1, RuntimeCommandRequestV1,
    RuntimeCommandResultV1, RuntimeCommandTargetV1, RuntimeCommandTerminalStatusV1,
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
            if let Some(request) = parse_gateway_restart_request(&entry.envelope.payload) {
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
        for record in pending {
            if record.command == "finitecomputer.runtime.gateway.restart" {
                self.gateway = GatewayState::Live;
                append_application(
                    server,
                    ApplicationAppend {
                        room_id: &self.room_id,
                        group_id: &self.group_id,
                        sender: self.device.clone(),
                        epoch: current_epoch(server, &self.room_id),
                        payload: runtime_command_result_payload(&record.request_id),
                        idempotency_key: format!("result_{}", record.request_id),
                        delivery_policy: DurableAppEventKind::RuntimeCommandResult
                            .delivery_policy(),
                    },
                );
                self.ledger
                    .mark_terminal(
                        &self.room_id,
                        record.conversation_id.as_deref(),
                        &record.sender,
                        &record.request_id,
                        RuntimeCommandLedgerStatus::Succeeded,
                    )
                    .unwrap();
            }
        }
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
                payload: json!({
                    "type": "runtime.state.snapshot",
                    "state_key": "runtime.gateway",
                    "revision": server.room(&self.room_id).unwrap().last_seq + 1,
                    "status": status
                })
                .to_string()
                .into_bytes(),
                idempotency_key: format!(
                    "gateway_state_{}",
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

fn parse_gateway_restart_request(payload: &[u8]) -> Option<RuntimeCommandRequestV1> {
    let request: RuntimeCommandRequestV1 = serde_json::from_slice(payload).ok()?;
    request.validate_structure().ok()?;
    if request.command != "finitecomputer.runtime.gateway.restart" {
        return None;
    }
    Some(request)
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

fn runtime_command_result_payload(request_id: &str) -> Vec<u8> {
    let result = RuntimeCommandResultV1 {
        payload_kind: RuntimeCommandPayloadKindV1::Result,
        request_id: request_id.to_string(),
        status: RuntimeCommandTerminalStatusV1::Succeeded,
        body: Some(RuntimeCommandJsonPayloadV1 {
            schema: "finitecomputer.runtime.gateway.restart.result.v1".to_string(),
            json_payload: br#"{"status":"live"}"#.to_vec(),
        }),
        error: None,
        clears_activity: Vec::new(),
    };
    result.validate_structure().unwrap();
    serde_json::to_vec(&result).unwrap()
}
