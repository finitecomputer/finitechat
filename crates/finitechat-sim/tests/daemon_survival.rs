use std::collections::BTreeMap;

use finitechat_engine::{
    AppendApplicationEventRequest, AppendEventRequest, DeliveryService, EventAccepted, envelope,
};
use finitechat_proto::{ApplicationDeliveryPolicy, DeviceRef, DurableAppEventKind, LogEntryKind};
use finitechat_sim::{SimWorld, alice, bob};
use serde_json::json;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GatewayState {
    Live,
    Down,
    Hung,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommandStatus {
    Pending,
    Succeeded,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CommandRecord {
    request_id: String,
    command: String,
    status: CommandStatus,
}

#[derive(Debug, Default)]
struct CommandLedger {
    records: BTreeMap<String, CommandRecord>,
}

impl CommandLedger {
    fn record_request(&mut self, request_id: &str, command: &str) {
        self.records
            .entry(request_id.to_string())
            .or_insert_with(|| CommandRecord {
                request_id: request_id.to_string(),
                command: command.to_string(),
                status: CommandStatus::Pending,
            });
    }

    fn pending_requests(&self) -> Vec<CommandRecord> {
        self.records
            .values()
            .filter(|record| record.status == CommandStatus::Pending)
            .cloned()
            .collect()
    }

    fn mark_succeeded(&mut self, request_id: &str) {
        self.records
            .get_mut(request_id)
            .expect("request was recorded before success")
            .status = CommandStatus::Succeeded;
    }
}

#[derive(Debug)]
struct FakeDaemon {
    device: DeviceRef,
    room_id: String,
    group_id: String,
    after_seq: u64,
    gateway: GatewayState,
    ledger: CommandLedger,
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
            ledger: CommandLedger::default(),
            last_snapshot_message_id: None,
            crash_after_recording_first_request: false,
        }
    }

    fn with_persisted_ledger(mut self, ledger: CommandLedger, after_seq: u64) -> Self {
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
            if let Some((request_id, command)) =
                parse_gateway_restart_request(&entry.envelope.payload)
            {
                self.ledger.record_request(&request_id, &command);
                if self.crash_after_recording_first_request {
                    self.after_seq = page.next_after_seq;
                    self.crash_after_recording_first_request = false;
                    return;
                }
            }
        }
        self.after_seq = page.next_after_seq;
        self.execute_pending(server);
        self.publish_gateway_snapshot(server);
    }

    fn execute_pending(&mut self, server: &mut DeliveryService) {
        for record in self.ledger.pending_requests() {
            if record.command == "finitecomputer.runtime.gateway.restart" {
                self.gateway = GatewayState::Live;
                self.ledger.mark_succeeded(&record.request_id);
                append_application(
                    server,
                    ApplicationAppend {
                        room_id: &self.room_id,
                        group_id: &self.group_id,
                        sender: self.device.clone(),
                        epoch: current_epoch(server, &self.room_id),
                        payload: json!({
                            "type": "runtime.command.result",
                            "request_id": record.request_id,
                            "status": "succeeded"
                        })
                        .to_string()
                        .into_bytes(),
                        idempotency_key: format!("result_{}", record.request_id),
                        delivery_policy: DurableAppEventKind::RuntimeCommandResult
                            .delivery_policy(),
                    },
                );
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
            payload: json!({
                "type": "runtime.command.request",
                "request_id": "restart_1",
                "command": "finitecomputer.runtime.gateway.restart"
            })
            .to_string()
            .into_bytes(),
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
        daemon.ledger.records["restart_1"].status,
        CommandStatus::Succeeded
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
            payload: json!({
                "type": "runtime.command.request",
                "request_id": "restart_after_crash",
                "command": "finitecomputer.runtime.gateway.restart"
            })
            .to_string()
            .into_bytes(),
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
        daemon.ledger.records["restart_after_crash"].status,
        CommandStatus::Pending
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
        restarted.ledger.records["restart_after_crash"].status,
        CommandStatus::Succeeded
    );
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

fn parse_gateway_restart_request(payload: &[u8]) -> Option<(String, String)> {
    let value: serde_json::Value = serde_json::from_slice(payload).ok()?;
    if value.get("type")?.as_str()? != "runtime.command.request" {
        return None;
    }
    let command = value.get("command")?.as_str()?.to_string();
    if command != "finitecomputer.runtime.gateway.restart" {
        return None;
    }
    let request_id = value.get("request_id")?.as_str()?.to_string();
    Some((request_id, command))
}
