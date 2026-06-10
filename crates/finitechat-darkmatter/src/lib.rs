//! Darkmatter compatibility adapter for this Marmot/Darkmatter port.
//!
//! This crate is intentionally thin at the start of the port. Its job is to
//! keep Darkmatter dependencies executable from this workspace while the
//! existing application tests are moved from bespoke protocol internals onto
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
    EasyAdapterOwnedLogic,
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
            area: "marmot_engine_over_http_routes",
            status: PortStatus::WorksOutOfBox,
            evidence: "finitechat-server route tests carry real Marmot Welcome, invite Commit, and app messages through Axum HTTP handlers",
        },
        PortFinding {
            area: "application_policy_projection",
            status: PortStatus::EasyAdapterOwnedLogic,
            evidence: "application event push/unread/command policy can stay above opaque Marmot application payloads",
        },
        PortFinding {
            area: "http_application_delivery_effect_projection",
            status: PortStatus::EasyAdapterOwnedLogic,
            evidence: "finitechat-server /application-events records caller-supplied ApplicationDeliveryPolicy as delivery-effect projection state in the same SQLite transaction as the ordered event append, preserves push/unread/command-inbox counts, non-notifying event policies, custom no-push command policies, and opaque runtime command request ids after restart, rejects same-event replays with conflicting policy, and rolls back trigger-injected write failures without decrypting payloads",
        },
        PortFinding {
            area: "http_cli_route_client",
            status: PortStatus::EasyAdapterOwnedLogic,
            evidence: "finitechat-cli can construct and send Darkmatter HTTP route DTOs for publish, typed submit-commit, typed events, application delivery effects, ephemeral activity, requester-aware sync, invalid-commit reporting, device revoke, KeyPackage claim/lease expiry, fanout checkpoint, link-session pairing, direct-room create-or-get, account-room directory, and Welcome operations; CLI tests drive publish/sync, idempotency conflict, typed submit-commit, batch KeyPackage claim, fanout checkpoint, and Welcome claim/ack flows against a live Axum server; process smoke starts the server binary with SQLite and drives the CLI binary through persisted publish/sync/replay/conflict",
        },
        PortFinding {
            area: "http_sqlite_operation_log",
            status: PortStatus::EasyAdapterOwnedLogic,
            evidence: "finitechat-server can replay accepted Darkmatter HTTP delivery operations from SQLite after restart",
        },
        PortFinding {
            area: "http_publish_idempotency_replay",
            status: PortStatus::EasyAdapterOwnedLogic,
            evidence: "finitechat-server replays stored /messages receipts for matching idempotency keys and rejects conflicting retries",
        },
        PortFinding {
            area: "http_welcome_claim_ack_recovery",
            status: PortStatus::EasyAdapterOwnedLogic,
            evidence: "finitechat-server persists claimed Welcome inbox messages and terminal ack/failure state across restart",
        },
        PortFinding {
            area: "http_welcome_runtime_sync",
            status: PortStatus::EasyAdapterOwnedLogic,
            evidence: "finitechat-client run_runtime_sync_tick claims serialized WelcomeRecord payloads through Darkmatter HTTP inbox routes, activates locally, and acks without duplicate replay after restart",
        },
        PortFinding {
            area: "http_room_runtime_sync",
            status: PortStatus::EasyAdapterOwnedLogic,
            evidence: "finitechat-client run_runtime_sync_tick decodes serialized RoomLogEntry payloads from Darkmatter HTTP group pages and applies encrypted application entries with replay-safe cursors",
        },
        PortFinding {
            area: "http_group_sync_pagination",
            status: PortStatus::EasyAdapterOwnedLogic,
            evidence: "finitechat-server typed /events plus /sync/group return bounded requester-aware pages, preserve has_more and cursors, and continue paging correctly after SQLite restart",
        },
        PortFinding {
            area: "http_key_package_batch_claim_replay",
            status: PortStatus::EasyAdapterOwnedLogic,
            evidence: "finitechat-server can claim one KeyPackage per explicit device owner and replay the exact batch response by idempotency key after restart",
        },
        PortFinding {
            area: "http_key_package_payload_opacity",
            status: PortStatus::EasyAdapterOwnedLogic,
            evidence: "finitechat-server claims KeyPackages by route owner metadata rather than untrusted opaque package bytes; payload identity claims do not authorize the wrong owner, and the original package bytes survive SQLite restart unchanged",
        },
        PortFinding {
            area: "http_key_package_inventory_capacity",
            status: PortStatus::EasyAdapterOwnedLogic,
            evidence: "finitechat-server enforces MAX_KEY_PACKAGES_PER_DEVICE over wrapper-owned available/claimed/consumed KeyPackage inventory; claimed packages still count against capacity, accepted commits consume claimed packages, and consumed packages free replacement upload space after SQLite restart",
        },
        PortFinding {
            area: "http_key_package_publish_retry",
            status: PortStatus::EasyAdapterOwnedLogic,
            evidence: "finitechat-server replays exact KeyPackage publication retries after restart and rejects conflicting same-id packages without creating extra claimable inventory",
        },
        PortFinding {
            area: "http_key_package_inventory_runtime_sync",
            status: PortStatus::EasyAdapterOwnedLogic,
            evidence: "finitechat-client run_runtime_sync_tick replenishes KeyPackages through Darkmatter HTTP inventory/upload routes and replays zero duplicate uploads after server restart",
        },
        PortFinding {
            area: "http_key_package_claim_runtime_sync",
            status: PortStatus::EasyAdapterOwnedLogic,
            evidence: "finitechat-client RuntimeDelivery claims Darkmatter HTTP KeyPackages with metadata preserved in opaque package bytes, deterministic lease tokens reconstructed, and claimed inventory mapped back to leased counts",
        },
        PortFinding {
            area: "http_key_package_lease_reclaim",
            status: PortStatus::EasyAdapterOwnedLogic,
            evidence: "finitechat-server owns HTTP KeyPackage lease state above Marmot's opaque package bytes, can expire a claimed lease back to available, reclaim the same package after restart, and refuses to expire consumed packages",
        },
        PortFinding {
            area: "http_key_package_commit_lifecycle",
            status: PortStatus::EasyAdapterOwnedLogic,
            evidence: "finitechat-server typed /commits reject unclaimed KeyPackages and stale KeyPackage metadata before side effects, consume claimed packages atomically with accepted commits, preserve consumed state after restart, and reject consumed-package reuse",
        },
        PortFinding {
            area: "http_commit_welcome_release_coupling",
            status: PortStatus::EasyAdapterOwnedLogic,
            evidence: "finitechat-server typed /commits reject bad commit metadata without releasing Welcomes, preserve that absence after restart, and release exactly one Welcome only after the corrected commit is durably accepted",
        },
        PortFinding {
            area: "http_revoked_device_projection",
            status: PortStatus::EasyAdapterOwnedLogic,
            evidence: "finitechat-server persists revoked DeviceRefs over HTTP, rebuilds revoked state after restart, rejects revoked KeyPackage publish/claim, Welcome claim/activation, typed event senders, typed commit senders, and typed commits that add a revoked device; batch KeyPackage claim skips revoked owners without consuming inventory",
        },
        PortFinding {
            area: "http_runtime_delivery_adapter",
            status: PortStatus::EasyAdapterOwnedLogic,
            evidence: "finitechat-client exposes generic HttpRuntimeDelivery over HttpRuntimeTransport; tests now reuse that production adapter and keep only in-process routing plus failure injection in the harness",
        },
        PortFinding {
            area: "http_runtime_delivery_network_transport",
            status: PortStatus::EasyAdapterOwnedLogic,
            evidence: "finitechat-client ReqwestHttpRuntimeTransport drives HttpRuntimeDelivery against a live localhost Axum server for KeyPackage claim, ordered room sync, later-device fanout, and visible non-success HTTP status errors",
        },
        PortFinding {
            area: "http_route_dto_boundary",
            status: PortStatus::EasyAdapterOwnedLogic,
            evidence: "finitechat-http owns shared route DTOs; production finitechat-client and finitechat-cli no longer depend on finitechat-server for wire types",
        },
        PortFinding {
            area: "http_fanout_plan_checkpoint",
            status: PortStatus::EasyAdapterOwnedLogic,
            evidence: "finitechat-server persists opaque later-device fanout room plans, prepared message ids, reprepare checkpoints, and accepted seqs across restart",
        },
        PortFinding {
            area: "http_link_session_pairing",
            status: PortStatus::EasyAdapterOwnedLogic,
            evidence: "finitechat-server exposes link-session pairing over HTTP with opaque encrypted payload storage, duplicate/conflict/size-limit/closed-state rules, deterministic claim tokens across release/reclaim, ack validation, expiry, and SQLite restart recovery; oversized payloads reject without being stored, and finitechat-cli builds DTOs for every link-session route",
        },
        PortFinding {
            area: "http_account_room_directory_runtime_discovery",
            status: PortStatus::EasyAdapterOwnedLogic,
            evidence: "finitechat-client run_link_fanout_tick can discover persisted account-scoped account-room records through Darkmatter HTTP routes after server restart when the target device is already current",
        },
        PortFinding {
            area: "http_account_room_membership_filtering",
            status: PortStatus::EasyAdapterOwnedLogic,
            evidence: "finitechat-server normalizes typed account-room records to the requested account's devices, rejects records with no devices for that account, and pages the normalized projection after restart",
        },
        PortFinding {
            area: "http_account_room_bootstrap_projection",
            status: PortStatus::EasyAdapterOwnedLogic,
            evidence: "finitechat-server can bootstrap the creator's initial active account-room record from typed room metadata and reload it after restart",
        },
        PortFinding {
            area: "http_direct_room_constraints",
            status: PortStatus::EasyAdapterOwnedLogic,
            evidence: "finitechat-server persists direct-room account pairs in the room-membership projection, create-or-get returns the same room for reversed account order after restart, and typed /commits reject third-account adds before delivery side effects",
        },
        PortFinding {
            area: "http_account_room_commit_projection",
            status: PortStatus::EasyAdapterOwnedLogic,
            evidence: "finitechat-server can project typed /commits and raw /messages projection wrappers into persisted account-room records, then reload the updated discovery state after restart",
        },
        PortFinding {
            area: "http_welcome_ack_membership_activation",
            status: PortStatus::EasyAdapterOwnedLogic,
            evidence: "finitechat-server can decode a claimed WelcomeRecord on activated ack, mark the pending account-room device active, and reload that active projection after restart",
        },
        PortFinding {
            area: "http_delayed_welcome_forward_sync",
            status: PortStatus::EasyAdapterOwnedLogic,
            evidence: "finitechat-server persists a typed event appended after an add commit but before the recipient claims and acks the Welcome, then lets the activated device sync forward from the add-commit seq after SQLite restart",
        },
        PortFinding {
            area: "http_room_membership_projection",
            status: PortStatus::EasyAdapterOwnedLogic,
            evidence: "finitechat-server derives persisted room-membership intervals from typed bootstrap, typed /commits, raw commit projection wrappers, and Welcome ack activation; requester-aware sync filters hidden entries while advancing cursors, removed devices can sync through their removal but not later entries after restart, stale removed devices cannot send events or commits, and typed rooms reject raw plain commits without membership deltas",
        },
        PortFinding {
            area: "http_account_device_cap",
            status: PortStatus::EasyAdapterOwnedLogic,
            evidence: "finitechat-server enforces MAX_ACCOUNT_DEVICES_PER_ROOM during typed /commits, rejects overflow before durable append, and preserves claimed KeyPackage/no-Welcome/no-account-room side effects across SQLite restart",
        },
        PortFinding {
            area: "http_duplicate_pending_device_add",
            status: PortStatus::EasyAdapterOwnedLogic,
            evidence: "finitechat-server rejects typed /commits that re-add a current or pending device before durable append, preserving the original Welcome, account-room projection, and claimed retry KeyPackage across SQLite restart",
        },
        PortFinding {
            area: "http_membership_delta_structural_validation",
            status: PortStatus::EasyAdapterOwnedLogic,
            evidence: "finitechat-server rejects malformed typed /commits membership deltas before durable append, Welcome release, account-room projection, or claimed KeyPackage consumption across the base epoch, post-commit epoch, commit id, duplicate add/remove, add/remove overlap, and incomplete add matrix",
        },
        PortFinding {
            area: "http_invalid_commit_repair_state",
            status: PortStatus::EasyAdapterOwnedLogic,
            evidence: "finitechat-server /rooms/report-invalid-commit authorizes reporters against persisted membership intervals, including removed reporters at their removal seq, marks room-membership and account-room projections needs_repair in one SQLite transaction, reloads that state after restart, and blocks later typed /events and /commits",
        },
        PortFinding {
            area: "http_submit_commit_route",
            status: PortStatus::EasyAdapterOwnedLogic,
            evidence: "finitechat-server /commits accepts a typed SubmitCommitRequest, rejects malformed staged Welcomes before side effects, atomically persists accepted commit delivery, idempotency, derived Welcome releases, consumed KeyPackage projection, and adapter projections in SQLite, replays accepted commits and same-epoch rejected commits after restart, rolls back injected SQLite failures before retry convergence, and repairs legacy partial projection rows",
        },
        PortFinding {
            area: "http_typed_event_route",
            status: PortStatus::EasyAdapterOwnedLogic,
            evidence: "finitechat-server /events accepts typed AppendEventRequest payloads, rejects pending tracked senders and oversized payloads before durable append, preserves exact idempotent replay across restart, rejects duplicate typed message ids with new idempotency keys, publishes plain RoomLogEntry payloads, and persists the room head across restart",
        },
        PortFinding {
            area: "http_scoped_idempotency_capacity",
            status: PortStatus::EasyAdapterOwnedLogic,
            evidence: "finitechat-server derives typed publish idempotency scope from RoomLogEntry payloads, rejects fresh per-room/per-sender overflow, and still replays existing typed /events responses across SQLite restart",
        },
        PortFinding {
            area: "http_ephemeral_activity_cache",
            status: PortStatus::EasyAdapterOwnedLogic,
            evidence: "finitechat-server accepts opaque ephemeral activity over HTTP for active members, rejects pending, revoked, wrong-epoch, and expired activity, caps per-route volatile cache entries, keeps conversation-scoped topic and room-wide route keys distinct while treating repeated activity ids as opaque additive events, and does not append durable group messages or persist scoped cache state across SQLite restart",
        },
        PortFinding {
            area: "http_later_device_fanout_runtime_happy_path",
            status: PortStatus::EasyAdapterOwnedLogic,
            evidence: "finitechat-client run_link_fanout_tick can discover one room, claim a target KeyPackage, submit a serialized commit through the typed HTTP /commits route, sync completion, claim a server-released Welcome, and promote the later device from pending to active after ack through both in-process and live reqwest transports",
        },
        PortFinding {
            area: "http_later_device_fanout_submit_retry",
            status: PortStatus::EasyAdapterOwnedLogic,
            evidence: "finitechat-client run_link_fanout_tick retries a lost HTTP /commits response from typed bootstrap discovery and prepared local state, replays the idempotent server-side commit and Welcome publishes, and does not duplicate the group log entry",
        },
        PortFinding {
            area: "http_later_device_fanout_multi_room",
            status: PortStatus::EasyAdapterOwnedLogic,
            evidence: "finitechat-client run_link_fanout_tick can page typed bootstrap account-room discovery across two rooms, claim distinct target KeyPackages, submit both commits through Darkmatter HTTP, and activate both later-device Welcomes",
        },
        PortFinding {
            area: "http_later_device_fanout_partial_retry",
            status: PortStatus::EasyAdapterOwnedLogic,
            evidence: "finitechat-client run_link_fanout_tick keeps a completed room Done while a later room fails before HTTP accept, then retries only the prepared failed room over the persisted Darkmatter HTTP server and activates both Welcomes",
        },
        PortFinding {
            area: "http_later_device_fanout_same_epoch_reprepare",
            status: PortStatus::EasyAdapterOwnedLogic,
            evidence: "finitechat-client run_link_fanout_tick can recover from typed bootstrap discovery after an HTTP fanout submit fails before accept, a competing same-epoch commit wins, sync clears the local pending commit, and the worker reprepares at the next epoch",
        },
        PortFinding {
            area: "multi_device_later_device_fanout",
            status: PortStatus::EasyAdapterOwnedLogic,
            evidence: "Typed bootstrap/commit/event flows now have server-owned room-membership projection, filtered sync, pending-send rejection, Welcome ack activation, and rejection of raw plain commits that would otherwise weaken typed room membership",
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
    fn port_findings_name_current_status_buckets() {
        let findings = current_port_findings();
        assert!(
            findings
                .iter()
                .any(|finding| finding.status == PortStatus::WorksOutOfBox)
        );
        assert!(
            findings
                .iter()
                .any(|finding| finding.status == PortStatus::EasyAdapterOwnedLogic)
        );
        assert!(
            findings
                .iter()
                .all(|finding| finding.status != PortStatus::ThickOrWonkyLogic)
        );
        assert!(
            findings
                .iter()
                .any(|finding| finding.status == PortStatus::RequiresDarkmatterFork)
        );
    }
}
