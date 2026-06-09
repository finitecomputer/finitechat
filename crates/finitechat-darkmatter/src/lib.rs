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
            evidence: "finitechat-cli can construct and send Darkmatter HTTP route DTOs for publish, sync, KeyPackage, fanout checkpoint, account-room directory, and Welcome operations",
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
            area: "http_fanout_plan_checkpoint",
            status: PortStatus::EasyFiniteOwnedLogic,
            evidence: "finitechat-server persists opaque later-device fanout room plans, prepared message ids, reprepare checkpoints, and accepted seqs across restart",
        },
        PortFinding {
            area: "http_account_room_directory_runtime_discovery",
            status: PortStatus::EasyFiniteOwnedLogic,
            evidence: "finitechat-client run_link_fanout_tick can discover persisted opaque account-room records through Darkmatter HTTP routes after server restart when the target device is already current",
        },
        PortFinding {
            area: "http_later_device_fanout_runtime_happy_path",
            status: PortStatus::EasyFiniteOwnedLogic,
            evidence: "finitechat-client run_link_fanout_tick can discover one room, claim a target KeyPackage, submit a serialized commit through Darkmatter HTTP, sync completion, and release a Welcome that the later device activates",
        },
        PortFinding {
            area: "multi_device_later_device_fanout",
            status: PortStatus::ThickOrWonkyLogic,
            evidence: "Remaining Finite parity requires membership-derived account-room writes, server-side membership validation/filtering, response-loss retry coverage, and same-epoch reprepare over the HTTP adapter",
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
