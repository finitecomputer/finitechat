//! Darkmatter compatibility adapter for the Finite Chat port.
//!
//! This crate is intentionally thin at the start of the port. Its job is to
//! keep Darkmatter dependencies executable from this workspace while the
//! existing Finite Chat tests are moved from bespoke protocol internals onto
//! Marmot/Darkmatter primitives.

use cgka_traits::transport::{Timestamp, TransportEnvelope, TransportMessage, TransportSource};
use cgka_traits::{EpochId, GroupId, MessageId};
use serde::Serialize;
use transport_http_server::{
    HTTP_SERVER_SOURCE, HttpCommitAdmission, HttpDeliveryService, HttpPublishTarget,
    HttpServerError,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum PortStatus {
    WorksOutOfBox,
    EasyFiniteOwnedLogic,
    ThickOrWonkyLogic,
    RequiresDarkmatterFork,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PortFinding {
    pub area: &'static str,
    pub status: PortStatus,
    pub evidence: &'static str,
}

pub fn current_port_findings() -> Vec<PortFinding> {
    vec![
        PortFinding {
            area: "ordered_http_delivery_core",
            status: PortStatus::WorksOutOfBox,
            evidence: "transport-http-server can sequence group messages and expose bounded sync pages",
        },
        PortFinding {
            area: "marmot_engine_over_http_delivery",
            status: PortStatus::WorksOutOfBox,
            evidence: "Darkmatter http_delivery_compatibility test carries real Marmot invite and app messages through the service core",
        },
        PortFinding {
            area: "marmot_engine_over_finite_http_routes",
            status: PortStatus::WorksOutOfBox,
            evidence: "finitechat-server route tests carry real Marmot Welcome, invite Commit, and app messages through Axum HTTP handlers",
        },
        PortFinding {
            area: "finite_application_policy_projection",
            status: PortStatus::EasyFiniteOwnedLogic,
            evidence: "Finite app event push/unread/command policy can stay above opaque Marmot application payloads",
        },
        PortFinding {
            area: "http_cli_route_client",
            status: PortStatus::EasyFiniteOwnedLogic,
            evidence: "finitechat-cli can construct and send Darkmatter HTTP route DTOs for publish, typed submit-commit, sync, KeyPackage, fanout checkpoint, account-room directory, and Welcome operations",
        },
        PortFinding {
            area: "http_sqlite_operation_log",
            status: PortStatus::EasyFiniteOwnedLogic,
            evidence: "finitechat-server can replay accepted Darkmatter HTTP delivery operations from SQLite after restart",
        },
        PortFinding {
            area: "http_publish_idempotency_replay",
            status: PortStatus::EasyFiniteOwnedLogic,
            evidence: "finitechat-server replays stored /messages receipts for matching idempotency keys and rejects conflicting retries",
        },
        PortFinding {
            area: "http_welcome_claim_ack_recovery",
            status: PortStatus::EasyFiniteOwnedLogic,
            evidence: "finitechat-server persists claimed Welcome inbox messages and terminal ack/failure state across restart",
        },
        PortFinding {
            area: "http_welcome_runtime_sync",
            status: PortStatus::EasyFiniteOwnedLogic,
            evidence: "finitechat-client run_runtime_sync_tick claims serialized WelcomeRecord payloads through Darkmatter HTTP inbox routes, activates locally, and acks without duplicate replay after restart",
        },
        PortFinding {
            area: "http_room_runtime_sync",
            status: PortStatus::EasyFiniteOwnedLogic,
            evidence: "finitechat-client run_runtime_sync_tick decodes serialized RoomLogEntry payloads from Darkmatter HTTP group pages and applies encrypted application entries with replay-safe cursors",
        },
        PortFinding {
            area: "http_key_package_batch_claim_replay",
            status: PortStatus::EasyFiniteOwnedLogic,
            evidence: "finitechat-server can claim one KeyPackage per explicit device owner and replay the exact batch response by idempotency key after restart",
        },
        PortFinding {
            area: "http_key_package_inventory_runtime_sync",
            status: PortStatus::EasyFiniteOwnedLogic,
            evidence: "finitechat-client run_runtime_sync_tick replenishes KeyPackages through Darkmatter HTTP inventory/upload routes and replays zero duplicate uploads after server restart",
        },
        PortFinding {
            area: "http_key_package_claim_runtime_sync",
            status: PortStatus::EasyFiniteOwnedLogic,
            evidence: "finitechat-client RuntimeDelivery claims Darkmatter HTTP KeyPackages with Finite metadata preserved in opaque package bytes and deterministic lease tokens reconstructed",
        },
        PortFinding {
            area: "http_runtime_delivery_adapter",
            status: PortStatus::EasyFiniteOwnedLogic,
            evidence: "finitechat-client exposes generic HttpRuntimeDelivery over HttpRuntimeTransport; tests now reuse that production adapter and keep only in-process routing plus failure injection in the harness",
        },
        PortFinding {
            area: "http_fanout_plan_checkpoint",
            status: PortStatus::EasyFiniteOwnedLogic,
            evidence: "finitechat-server persists opaque later-device fanout room plans, prepared message ids, reprepare checkpoints, and accepted seqs across restart",
        },
        PortFinding {
            area: "http_account_room_directory_runtime_discovery",
            status: PortStatus::EasyFiniteOwnedLogic,
            evidence: "finitechat-client run_link_fanout_tick can discover persisted account-scoped account-room records through Darkmatter HTTP routes after server restart when the target device is already current",
        },
        PortFinding {
            area: "http_account_room_membership_filtering",
            status: PortStatus::EasyFiniteOwnedLogic,
            evidence: "finitechat-server normalizes typed account-room records to the requested account's devices, rejects records with no devices for that account, and pages the normalized projection after restart",
        },
        PortFinding {
            area: "http_account_room_bootstrap_projection",
            status: PortStatus::EasyFiniteOwnedLogic,
            evidence: "finitechat-server can bootstrap the creator's initial active account-room record from typed Finite room metadata and reload it after restart",
        },
        PortFinding {
            area: "http_account_room_commit_projection",
            status: PortStatus::EasyFiniteOwnedLogic,
            evidence: "finitechat-server can project typed /commits add/remove requests into persisted account-room records, keep raw /messages projection compatibility, and reload the updated discovery state after restart",
        },
        PortFinding {
            area: "http_welcome_ack_membership_activation",
            status: PortStatus::EasyFiniteOwnedLogic,
            evidence: "finitechat-server can decode a claimed Finite WelcomeRecord on activated ack, mark the pending account-room device active, and reload that active projection after restart",
        },
        PortFinding {
            area: "http_room_membership_projection",
            status: PortStatus::EasyFiniteOwnedLogic,
            evidence: "finitechat-server derives persisted room-membership intervals from typed bootstrap, typed /commits, and Welcome ack activation; requester-aware sync filters hidden entries while advancing cursors",
        },
        PortFinding {
            area: "http_submit_commit_route",
            status: PortStatus::EasyFiniteOwnedLogic,
            evidence: "finitechat-server /commits accepts a typed SubmitCommitRequest, rejects malformed staged Welcomes before side effects, publishes an ordered group RoomLogEntry, derives account-room and room-membership updates from the request, releases derived Welcome inbox messages, and replays idempotently after restart",
        },
        PortFinding {
            area: "http_typed_event_route",
            status: PortStatus::EasyFiniteOwnedLogic,
            evidence: "finitechat-server /events accepts typed AppendEventRequest payloads, rejects pending tracked senders, publishes plain RoomLogEntry payloads, and persists the room head across restart",
        },
        PortFinding {
            area: "http_later_device_fanout_runtime_happy_path",
            status: PortStatus::EasyFiniteOwnedLogic,
            evidence: "finitechat-client run_link_fanout_tick can discover one room, claim a target KeyPackage, submit a serialized commit through the typed HTTP /commits route, sync completion, claim a server-released Welcome, and promote the later device from pending to active after ack",
        },
        PortFinding {
            area: "http_later_device_fanout_submit_retry",
            status: PortStatus::EasyFiniteOwnedLogic,
            evidence: "finitechat-client run_link_fanout_tick retries a lost HTTP /commits response from typed bootstrap discovery and prepared local state, replays the idempotent server-side commit and Welcome publishes, and does not duplicate the group log entry",
        },
        PortFinding {
            area: "http_later_device_fanout_multi_room",
            status: PortStatus::EasyFiniteOwnedLogic,
            evidence: "finitechat-client run_link_fanout_tick can page typed bootstrap account-room discovery across two rooms, claim distinct target KeyPackages, submit both commits through Darkmatter HTTP, and activate both later-device Welcomes",
        },
        PortFinding {
            area: "http_later_device_fanout_same_epoch_reprepare",
            status: PortStatus::EasyFiniteOwnedLogic,
            evidence: "finitechat-client run_link_fanout_tick can recover from typed bootstrap discovery after an HTTP fanout submit fails before accept, a competing same-epoch commit wins, sync clears the local pending commit, and the worker reprepares at the next epoch",
        },
        PortFinding {
            area: "multi_device_later_device_fanout",
            status: PortStatus::ThickOrWonkyLogic,
            evidence: "Remaining Finite parity requires closing raw-history membership imports before strict member authorization can apply to every room; typed bootstrap/commit/event flows now have server-owned room-membership projection, filtered sync, pending-send rejection, and Welcome ack activation",
        },
        PortFinding {
            area: "ordered_delivery_profile",
            status: PortStatus::RequiresDarkmatterFork,
            evidence: "DangerouslyTrustServerOrdering is currently branch-local until Marmot accepts an ordered-delivery profile",
        },
    ]
}

pub fn prove_http_delivery_core_orders_commit_then_message()
-> Result<Vec<MessageId>, HttpServerError> {
    let mut service = HttpDeliveryService::default();
    let group_id = GroupId::new(b"port-room".to_vec());
    let transport_group_id = b"port-room-transport".to_vec();

    service.publish(
        HttpPublishTarget::Group {
            group_id: group_id.clone(),
            transport_group_id: transport_group_id.clone(),
            commit_admission: Some(HttpCommitAdmission {
                source_epoch: EpochId(1),
            }),
        },
        group_message("commit-epoch-1", transport_group_id.clone(), b"commit"),
    )?;
    service.publish(
        HttpPublishTarget::Group {
            group_id: group_id.clone(),
            transport_group_id,
            commit_admission: None,
        },
        group_message(
            "app-message-epoch-2",
            b"port-room-transport".to_vec(),
            b"app",
        ),
    )?;

    let page = service.sync_group(&group_id, 0, 10)?;
    Ok(page
        .entries
        .into_iter()
        .map(|entry| entry.message.id)
        .collect())
}

fn group_message(id: &str, transport_group_id: Vec<u8>, payload: &[u8]) -> TransportMessage {
    TransportMessage {
        id: MessageId::new(id.as_bytes().to_vec()),
        payload: payload.to_vec(),
        timestamp: Timestamp(1),
        causal_deps: Vec::new(),
        source: TransportSource(HTTP_SERVER_SOURCE.to_owned()),
        envelope: TransportEnvelope::GroupMessage { transport_group_id },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn darkmatter_http_delivery_core_orders_commit_then_message() {
        let ids = prove_http_delivery_core_orders_commit_then_message()
            .expect("Darkmatter HTTP delivery core orders messages");
        assert_eq!(
            ids,
            vec![
                MessageId::new(b"commit-epoch-1".to_vec()),
                MessageId::new(b"app-message-epoch-2".to_vec()),
            ]
        );
    }

    #[test]
    fn port_findings_name_all_status_buckets() {
        let findings = current_port_findings();
        assert!(
            findings
                .iter()
                .any(|finding| finding.status == PortStatus::WorksOutOfBox)
        );
        assert!(
            findings
                .iter()
                .any(|finding| finding.status == PortStatus::EasyFiniteOwnedLogic)
        );
        assert!(
            findings
                .iter()
                .any(|finding| finding.status == PortStatus::ThickOrWonkyLogic)
        );
        assert!(
            findings
                .iter()
                .any(|finding| finding.status == PortStatus::RequiresDarkmatterFork)
        );
    }
}
