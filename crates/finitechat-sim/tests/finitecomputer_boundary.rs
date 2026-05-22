use finitechat_proto::{
    CommandInboxPolicy, DeviceRef, DurableAppEventKind, RuntimeCommandJsonPayloadV1,
    RuntimeCommandPayloadKindV1, RuntimeCommandRequestV1, RuntimeCommandTargetV1,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FinitecomputerOperationPlane {
    PortableRuntimeCommand,
    RuntimeStateSnapshot,
    ChatPayload,
    HostedRunnerAdmin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DashboardTransportPlan {
    requires_inbound_agent_http: bool,
    status_plane: FinitecomputerOperationPlane,
    refresh_plane: FinitecomputerOperationPlane,
    message_plane: FinitecomputerOperationPlane,
}

#[test]
fn portable_agent_command_does_not_assume_hosted_runner() {
    let target = runtime_device();
    for (command, resource_key) in [
        (
            "finitecomputer.runtime.inference.apply",
            Some("hermes.config"),
        ),
        (
            "finitecomputer.runtime.gateway.restart",
            Some("hermes.config"),
        ),
        (
            "finitecomputer.runtime.connection.matrix.reconnect",
            Some("matrix.connection"),
        ),
        ("finitecomputer.runtime.status.refresh", None),
    ] {
        assert_eq!(
            classify_finitecomputer_operation(command),
            Some(FinitecomputerOperationPlane::PortableRuntimeCommand)
        );

        let request = portable_command_request(command, resource_key, &target).unwrap();

        request.validate_structure().unwrap();
        assert_eq!(
            request.target,
            RuntimeCommandTargetV1 {
                account_id: target.account_id.clone(),
                device_id: Some(target.device_id.clone()),
            }
        );
        assert!(!request.command.contains("pod"));
        assert!(!request.command.contains("kubernetes"));
        assert!(!request.command.contains("runner_image"));
    }
}

#[test]
fn hosted_runner_admin_operation_stays_out_of_generic_chat_command() {
    for operation in [
        "finitecomputer.hosted.hostname.reserve",
        "finitecomputer.hosted.auth_policy.apply",
        "finitecomputer.hosted.runner_image.update",
        "finitecomputer.hosted.emergency_pod.restart",
    ] {
        assert_eq!(
            classify_finitecomputer_operation(operation),
            Some(FinitecomputerOperationPlane::HostedRunnerAdmin)
        );
        assert!(portable_command_request(operation, None, &runtime_device()).is_none());
    }
}

#[test]
fn dashboard_does_not_require_inbound_agent_http() {
    let plan = dashboard_transport_plan();

    assert!(!plan.requires_inbound_agent_http);
    assert_eq!(
        plan.status_plane,
        FinitecomputerOperationPlane::RuntimeStateSnapshot
    );
    assert_eq!(
        plan.refresh_plane,
        FinitecomputerOperationPlane::PortableRuntimeCommand
    );
    assert_eq!(
        plan.message_plane,
        FinitecomputerOperationPlane::ChatPayload
    );
}

#[test]
fn chat_payloads_do_not_travel_over_generic_management_queue() {
    assert_eq!(
        DurableAppEventKind::ChatMessage
            .delivery_policy()
            .command_inbox,
        CommandInboxPolicy::Never
    );
    assert_eq!(
        DurableAppEventKind::ConversationCreate
            .delivery_policy()
            .command_inbox,
        CommandInboxPolicy::Never
    );
    assert_eq!(
        DurableAppEventKind::RuntimeStateSnapshot
            .delivery_policy()
            .command_inbox,
        CommandInboxPolicy::Never
    );
    assert_eq!(
        DurableAppEventKind::RuntimeCommandRequest
            .delivery_policy()
            .command_inbox,
        CommandInboxPolicy::Create
    );
}

fn dashboard_transport_plan() -> DashboardTransportPlan {
    DashboardTransportPlan {
        requires_inbound_agent_http: false,
        status_plane: FinitecomputerOperationPlane::RuntimeStateSnapshot,
        refresh_plane: FinitecomputerOperationPlane::PortableRuntimeCommand,
        message_plane: FinitecomputerOperationPlane::ChatPayload,
    }
}

fn portable_command_request(
    command: &str,
    resource_key: Option<&str>,
    target: &DeviceRef,
) -> Option<RuntimeCommandRequestV1> {
    if classify_finitecomputer_operation(command)?
        != FinitecomputerOperationPlane::PortableRuntimeCommand
    {
        return None;
    }
    Some(RuntimeCommandRequestV1 {
        payload_kind: RuntimeCommandPayloadKindV1::Request,
        request_id: format!("{command}:request_1"),
        command: command.to_string(),
        target: RuntimeCommandTargetV1 {
            account_id: target.account_id.clone(),
            device_id: Some(target.device_id.clone()),
        },
        resource_key: resource_key.map(str::to_string),
        body: RuntimeCommandJsonPayloadV1 {
            schema: format!("{command}.v1"),
            json_payload: br#"{}"#.to_vec(),
        },
    })
}

fn classify_finitecomputer_operation(operation: &str) -> Option<FinitecomputerOperationPlane> {
    match operation {
        "finitecomputer.runtime.inference.apply"
        | "finitecomputer.runtime.gateway.restart"
        | "finitecomputer.runtime.connection.matrix.reconnect"
        | "finitecomputer.runtime.status.refresh" => {
            Some(FinitecomputerOperationPlane::PortableRuntimeCommand)
        }
        "finitecomputer.runtime.gateway.status"
        | "finitecomputer.runtime.config.status"
        | "finitecomputer.runtime.capabilities" => {
            Some(FinitecomputerOperationPlane::RuntimeStateSnapshot)
        }
        "finitecomputer.chat.message" | "finitecomputer.chat.topic.create" => {
            Some(FinitecomputerOperationPlane::ChatPayload)
        }
        "finitecomputer.hosted.hostname.reserve"
        | "finitecomputer.hosted.auth_policy.apply"
        | "finitecomputer.hosted.runner_image.update"
        | "finitecomputer.hosted.emergency_pod.restart" => {
            Some(FinitecomputerOperationPlane::HostedRunnerAdmin)
        }
        _ => None,
    }
}

fn runtime_device() -> DeviceRef {
    DeviceRef {
        account_id: "agent_npub".to_string(),
        device_id: "finitec_box".to_string(),
    }
}
