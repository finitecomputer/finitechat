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
            area: "finite_application_policy_projection",
            status: PortStatus::EasyFiniteOwnedLogic,
            evidence: "Finite app event push/unread/command policy can stay above opaque Marmot application payloads",
        },
        PortFinding {
            area: "http_cli_route_client",
            status: PortStatus::EasyFiniteOwnedLogic,
            evidence: "finitechat-cli can construct and send Darkmatter HTTP route DTOs for publish, sync, and KeyPackage operations",
        },
        PortFinding {
            area: "multi_device_later_device_fanout",
            status: PortStatus::ThickOrWonkyLogic,
            evidence: "Finite tests require later devices to join existing rooms with distinct per-room KeyPackages and durable fanout progress",
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
