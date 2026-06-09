use std::io::Write;

use cgka_traits::engine::KeyPackage;
use cgka_traits::transport::{Timestamp, TransportEnvelope, TransportMessage, TransportSource};
use cgka_traits::{EpochId, GroupId, MemberId, MessageId};
use finitechat_http::{
    AckWelcomeRequest, BootstrapAccountRoomRequest, ClaimKeyPackageRequest,
    ClaimKeyPackagesRequest, ClaimWelcomesRequest, GetFanoutRequest, GroupSyncRequest,
    HttpFanoutRoomPlan, InboxSyncRequest, KeyPackageInventoryRequest,
    ListAccountRoomDirectoryRequest, MarkFanoutDoneRequest, MarkFanoutPreparedRequest,
    PublishMessageRequest, SaveAccountRoomRequest, SaveFanoutRoomRequest,
};
use finitechat_proto::DeviceRef;
use serde::Serialize;
use serde_json::Value;
use thiserror::Error;
use transport_http_server::{
    HTTP_SERVER_SOURCE, HttpCommitAdmission, HttpKeyPackageId, HttpKeyPackagePublication,
    HttpPublishTarget,
};

const DEFAULT_SERVER_URL: &str = "http://127.0.0.1:8787";
const DEFAULT_SYNC_LIMIT: usize = 50;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PreparedHttpRequest {
    pub method: HttpMethod,
    pub url: String,
    pub json: Option<Value>,
}

#[derive(Debug, Error)]
pub enum CliError {
    #[error("{0}")]
    Usage(String),
    #[error("failed to serialize request: {0}")]
    Serialize(serde_json::Error),
    #[error("failed to parse JSON: {0}")]
    Json(serde_json::Error),
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("server returned {status}: {body}")]
    Server {
        status: reqwest::StatusCode,
        body: String,
    },
    #[error("failed to write output: {0}")]
    Output(std::io::Error),
}

impl CliError {
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Usage(_) => 2,
            Self::Serialize(_)
            | Self::Json(_)
            | Self::Http(_)
            | Self::Server { .. }
            | Self::Output(_) => 1,
        }
    }
}

pub fn run<I, S, W>(args: I, output: &mut W) -> Result<(), CliError>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
    W: Write,
{
    let args = args.into_iter().map(Into::into).collect::<Vec<_>>();
    match args.first().map(String::as_str) {
        Some("compat-report") => {
            let findings = finitechat_darkmatter::current_port_findings();
            write_pretty_json(output, &findings)
        }
        Some("http-smoke") => {
            let ids = finitechat_darkmatter::prove_http_delivery_core_orders_commit_then_message()
                .expect("HTTP delivery core smoke passes");
            writeln!(
                output,
                "ordered {} messages through Darkmatter HTTP delivery core",
                ids.len()
            )
            .map_err(CliError::Output)
        }
        Some("http") => {
            let request = prepare_http_request(args.into_iter().skip(1))?;
            execute_http_request(&request, output)
        }
        _ => Err(CliError::Usage(usage())),
    }
}

pub fn prepare_http_request<I, S>(args: I) -> Result<PreparedHttpRequest, CliError>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut args = args.into_iter().map(Into::into).collect::<Vec<_>>();
    let server =
        take_option(&mut args, "--server")?.unwrap_or_else(|| DEFAULT_SERVER_URL.to_owned());
    let Some(command) = take_positional(&mut args) else {
        return Err(CliError::Usage(http_usage()));
    };

    match command.as_str() {
        "health" => {
            reject_extra_args(&args)?;
            Ok(PreparedHttpRequest {
                method: HttpMethod::Get,
                url: route_url(&server, "/health"),
                json: None,
            })
        }
        "publish-group" => publish_group_request(&server, args),
        "publish-inbox" => publish_inbox_request(&server, args),
        "submit-commit" => submit_commit_request(&server, args),
        "sync-group" => sync_group_request(&server, args),
        "sync-inbox" => sync_inbox_request(&server, args),
        "publish-key-package" => publish_key_package_request(&server, args),
        "key-package-inventory" => key_package_inventory_request(&server, args),
        "claim-key-package" => claim_key_package_request(&server, args),
        "claim-key-packages" => claim_key_packages_request(&server, args),
        "fanout-get" => fanout_get_request(&server, args),
        "fanout-save-room" => fanout_save_room_request(&server, args),
        "fanout-mark-prepared" => fanout_mark_prepared_request(&server, args),
        "fanout-mark-done" => fanout_mark_done_request(&server, args),
        "account-room-bootstrap" => account_room_bootstrap_request(&server, args),
        "account-room-save" => account_room_save_request(&server, args),
        "account-rooms-list" => account_rooms_list_request(&server, args),
        "claim-welcomes" => claim_welcomes_request(&server, args),
        "ack-welcome" => ack_welcome_request(&server, args),
        _ => Err(CliError::Usage(http_usage())),
    }
}

fn publish_group_request(
    server: &str,
    mut args: Vec<String>,
) -> Result<PreparedHttpRequest, CliError> {
    let group_id = required_option(&mut args, "--group-id")?;
    let transport_group_id = required_option(&mut args, "--transport-group-id")?;
    let message_id = required_option(&mut args, "--message-id")?;
    let payload = required_option(&mut args, "--payload")?;
    let idempotency_key = take_option(&mut args, "--idempotency-key")?;
    let commit_epoch = take_option(&mut args, "--commit-epoch")?
        .map(|epoch| parse_u64("--commit-epoch", &epoch))
        .transpose()?;
    reject_extra_args(&args)?;

    let transport_group_id = transport_group_id.into_bytes();
    let request = PublishMessageRequest {
        target: HttpPublishTarget::Group {
            group_id: GroupId::new(group_id.into_bytes()),
            transport_group_id: transport_group_id.clone(),
            commit_admission: commit_epoch.map(|source_epoch| HttpCommitAdmission {
                source_epoch: EpochId(source_epoch),
            }),
        },
        message: TransportMessage {
            id: MessageId::new(message_id.into_bytes()),
            payload: payload.into_bytes(),
            timestamp: Timestamp(0),
            causal_deps: Vec::new(),
            source: TransportSource(HTTP_SERVER_SOURCE.to_owned()),
            envelope: TransportEnvelope::GroupMessage { transport_group_id },
        },
        idempotency_key,
    };
    post_json_request(server, "/messages", &request)
}

fn publish_inbox_request(
    server: &str,
    mut args: Vec<String>,
) -> Result<PreparedHttpRequest, CliError> {
    let recipient = required_option(&mut args, "--recipient")?;
    let message_id = required_option(&mut args, "--message-id")?;
    let payload = required_option(&mut args, "--payload")?;
    let idempotency_key = take_option(&mut args, "--idempotency-key")?;
    reject_extra_args(&args)?;

    let recipient = MemberId::new(recipient.into_bytes());
    let request = PublishMessageRequest {
        target: HttpPublishTarget::Inbox {
            recipient: recipient.clone(),
        },
        message: TransportMessage {
            id: MessageId::new(message_id.into_bytes()),
            payload: payload.into_bytes(),
            timestamp: Timestamp(0),
            causal_deps: Vec::new(),
            source: TransportSource(HTTP_SERVER_SOURCE.to_owned()),
            envelope: TransportEnvelope::Welcome { recipient },
        },
        idempotency_key,
    };
    post_json_request(server, "/messages", &request)
}

fn submit_commit_request(
    server: &str,
    mut args: Vec<String>,
) -> Result<PreparedHttpRequest, CliError> {
    let request_json = required_option(&mut args, "--request-json")?;
    reject_extra_args(&args)?;

    let request: Value = serde_json::from_str(&request_json).map_err(CliError::Json)?;
    post_json_request(server, "/commits", &request)
}

fn sync_group_request(
    server: &str,
    mut args: Vec<String>,
) -> Result<PreparedHttpRequest, CliError> {
    let group_id = required_option(&mut args, "--group-id")?;
    let after_seq = optional_u64(&mut args, "--after-seq", 0)?;
    let limit = optional_usize(&mut args, "--limit", DEFAULT_SYNC_LIMIT)?;
    reject_extra_args(&args)?;

    let request = GroupSyncRequest {
        group_id: GroupId::new(group_id.into_bytes()),
        after_seq,
        limit,
        requester: None,
    };
    post_json_request(server, "/sync/group", &request)
}

fn sync_inbox_request(
    server: &str,
    mut args: Vec<String>,
) -> Result<PreparedHttpRequest, CliError> {
    let recipient = required_option(&mut args, "--recipient")?;
    let after_seq = optional_u64(&mut args, "--after-seq", 0)?;
    let limit = optional_usize(&mut args, "--limit", DEFAULT_SYNC_LIMIT)?;
    reject_extra_args(&args)?;

    let request = InboxSyncRequest {
        recipient: MemberId::new(recipient.into_bytes()),
        after_seq,
        limit,
    };
    post_json_request(server, "/sync/inbox", &request)
}

fn publish_key_package_request(
    server: &str,
    mut args: Vec<String>,
) -> Result<PreparedHttpRequest, CliError> {
    let owner = required_option(&mut args, "--owner")?;
    let key_package_id = required_option(&mut args, "--key-package-id")?;
    let bytes = required_option(&mut args, "--bytes")?;
    reject_extra_args(&args)?;

    let request = HttpKeyPackagePublication {
        key_package_id: HttpKeyPackageId::new(key_package_id.into_bytes()),
        owner: MemberId::new(owner.into_bytes()),
        key_package: KeyPackage::new(bytes.into_bytes()),
    };
    post_json_request(server, "/key-packages", &request)
}

fn claim_key_package_request(
    server: &str,
    mut args: Vec<String>,
) -> Result<PreparedHttpRequest, CliError> {
    let owner = required_option(&mut args, "--owner")?;
    reject_extra_args(&args)?;

    let request = ClaimKeyPackageRequest {
        owner: MemberId::new(owner.into_bytes()),
    };
    post_json_request(server, "/key-packages/claim", &request)
}

fn key_package_inventory_request(
    server: &str,
    mut args: Vec<String>,
) -> Result<PreparedHttpRequest, CliError> {
    let owner = required_option(&mut args, "--owner")?;
    reject_extra_args(&args)?;

    let request = KeyPackageInventoryRequest {
        owner: MemberId::new(owner.into_bytes()),
    };
    post_json_request(server, "/key-packages/inventory", &request)
}

fn claim_key_packages_request(
    server: &str,
    mut args: Vec<String>,
) -> Result<PreparedHttpRequest, CliError> {
    let owners = take_repeated_option(&mut args, "--owner")?;
    let idempotency_key = take_option(&mut args, "--idempotency-key")?;
    reject_extra_args(&args)?;

    if owners.is_empty() {
        return Err(CliError::Usage(
            "claim-key-packages requires at least one --owner".to_owned(),
        ));
    }

    let request = ClaimKeyPackagesRequest {
        owners: owners
            .into_iter()
            .map(|owner| MemberId::new(owner.into_bytes()))
            .collect(),
        idempotency_key,
    };
    post_json_request(server, "/key-packages/claims", &request)
}

fn fanout_get_request(
    server: &str,
    mut args: Vec<String>,
) -> Result<PreparedHttpRequest, CliError> {
    let fanout_id = required_option(&mut args, "--fanout-id")?;
    reject_extra_args(&args)?;

    let request = GetFanoutRequest { fanout_id };
    post_json_request(server, "/fanouts/get", &request)
}

fn fanout_save_room_request(
    server: &str,
    mut args: Vec<String>,
) -> Result<PreparedHttpRequest, CliError> {
    let fanout_id = required_option(&mut args, "--fanout-id")?;
    let target_owner = required_option(&mut args, "--target-owner")?;
    let room_id = required_option(&mut args, "--room-id")?;
    let key_package_id = required_option(&mut args, "--key-package-id")?;
    let welcome_id = required_option(&mut args, "--welcome-id")?;
    let commit_idempotency_key = required_option(&mut args, "--commit-idempotency-key")?;
    let claimed_key_package_id = take_option(&mut args, "--claimed-key-package-id")?;
    reject_extra_args(&args)?;

    let request = SaveFanoutRoomRequest {
        fanout_id,
        target_owner: MemberId::new(target_owner.into_bytes()),
        room: HttpFanoutRoomPlan {
            room_id: GroupId::new(room_id.into_bytes()),
            key_package_id: HttpKeyPackageId::new(key_package_id.into_bytes()),
            welcome_id: MessageId::new(welcome_id.into_bytes()),
            commit_idempotency_key,
            claimed_key_package_id: claimed_key_package_id
                .map(|key_package_id| HttpKeyPackageId::new(key_package_id.into_bytes())),
        },
    };
    post_json_request(server, "/fanouts/rooms", &request)
}

fn fanout_mark_prepared_request(
    server: &str,
    mut args: Vec<String>,
) -> Result<PreparedHttpRequest, CliError> {
    let fanout_id = required_option(&mut args, "--fanout-id")?;
    let room_id = required_option(&mut args, "--room-id")?;
    let message_id = required_option(&mut args, "--message-id")?;
    reject_extra_args(&args)?;

    let request = MarkFanoutPreparedRequest {
        fanout_id,
        room_id: GroupId::new(room_id.into_bytes()),
        prepared_message_id: MessageId::new(message_id.into_bytes()),
    };
    post_json_request(server, "/fanouts/rooms/prepared", &request)
}

fn fanout_mark_done_request(
    server: &str,
    mut args: Vec<String>,
) -> Result<PreparedHttpRequest, CliError> {
    let fanout_id = required_option(&mut args, "--fanout-id")?;
    let room_id = required_option(&mut args, "--room-id")?;
    let message_id = required_option(&mut args, "--message-id")?;
    let accepted_seq = required_option(&mut args, "--accepted-seq")?;
    reject_extra_args(&args)?;

    let request = MarkFanoutDoneRequest {
        fanout_id,
        room_id: GroupId::new(room_id.into_bytes()),
        prepared_message_id: MessageId::new(message_id.into_bytes()),
        accepted_seq: parse_u64("--accepted-seq", &accepted_seq)?,
    };
    post_json_request(server, "/fanouts/rooms/done", &request)
}

fn account_room_save_request(
    server: &str,
    mut args: Vec<String>,
) -> Result<PreparedHttpRequest, CliError> {
    let account_id = required_option(&mut args, "--account-id")?;
    let room_id = required_option(&mut args, "--room-id")?;
    let record_json = required_option(&mut args, "--record-json")?;
    reject_extra_args(&args)?;

    let request = SaveAccountRoomRequest {
        account_id,
        room_id,
        record: serde_json::from_str(&record_json).map_err(CliError::Json)?,
    };
    post_json_request(server, "/account-rooms", &request)
}

fn account_room_bootstrap_request(
    server: &str,
    mut args: Vec<String>,
) -> Result<PreparedHttpRequest, CliError> {
    let room_id = required_option(&mut args, "--room-id")?;
    let mls_group_id = required_option(&mut args, "--mls-group-id")?;
    let account_id = required_option(&mut args, "--account-id")?;
    let device_id = required_option(&mut args, "--device-id")?;
    reject_extra_args(&args)?;

    let request = BootstrapAccountRoomRequest {
        room_id,
        mls_group_id,
        creator: DeviceRef {
            account_id,
            device_id,
        },
    };
    post_json_request(server, "/account-rooms/bootstrap", &request)
}

fn account_rooms_list_request(
    server: &str,
    mut args: Vec<String>,
) -> Result<PreparedHttpRequest, CliError> {
    let account_id = required_option(&mut args, "--account-id")?;
    let after_room_id = take_option(&mut args, "--after-room-id")?;
    let limit = optional_usize(&mut args, "--limit", DEFAULT_SYNC_LIMIT)?;
    reject_extra_args(&args)?;

    let request = ListAccountRoomDirectoryRequest {
        account_id,
        after_room_id,
        limit,
    };
    post_json_request(server, "/account-rooms/list", &request)
}

fn claim_welcomes_request(
    server: &str,
    mut args: Vec<String>,
) -> Result<PreparedHttpRequest, CliError> {
    let recipient = required_option(&mut args, "--recipient")?;
    let limit = optional_usize(&mut args, "--limit", DEFAULT_SYNC_LIMIT)?;
    reject_extra_args(&args)?;

    let request = ClaimWelcomesRequest {
        recipient: MemberId::new(recipient.into_bytes()),
        limit,
    };
    post_json_request(server, "/welcomes/claim", &request)
}

fn ack_welcome_request(
    server: &str,
    mut args: Vec<String>,
) -> Result<PreparedHttpRequest, CliError> {
    let message_id = required_option(&mut args, "--message-id")?;
    let activated = required_option(&mut args, "--activated")?;
    reject_extra_args(&args)?;

    let request = AckWelcomeRequest {
        message_id: MessageId::new(message_id.into_bytes()),
        activated: parse_bool("--activated", &activated)?,
    };
    post_json_request(server, "/welcomes/ack", &request)
}

fn post_json_request<T: Serialize>(
    server: &str,
    path: &str,
    body: &T,
) -> Result<PreparedHttpRequest, CliError> {
    Ok(PreparedHttpRequest {
        method: HttpMethod::Post,
        url: route_url(server, path),
        json: Some(serde_json::to_value(body).map_err(CliError::Serialize)?),
    })
}

fn execute_http_request<W: Write>(
    request: &PreparedHttpRequest,
    output: &mut W,
) -> Result<(), CliError> {
    let client = reqwest::blocking::Client::new();
    let builder = match request.method {
        HttpMethod::Get => client.get(&request.url),
        HttpMethod::Post => client
            .post(&request.url)
            .json(request.json.as_ref().expect("POST request has JSON body")),
    };
    let response = builder.send()?;
    let status = response.status();
    let body = response.text()?;
    if !status.is_success() {
        return Err(CliError::Server { status, body });
    }
    writeln!(output, "{body}").map_err(CliError::Output)
}

fn write_pretty_json<T: Serialize, W: Write>(output: &mut W, value: &T) -> Result<(), CliError> {
    serde_json::to_writer_pretty(&mut *output, value).map_err(CliError::Serialize)?;
    writeln!(output).map_err(CliError::Output)
}

fn route_url(server: &str, path: &str) -> String {
    format!(
        "{}/{}",
        server.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

fn take_positional(args: &mut Vec<String>) -> Option<String> {
    if args.is_empty() {
        None
    } else {
        Some(args.remove(0))
    }
}

fn required_option(args: &mut Vec<String>, name: &'static str) -> Result<String, CliError> {
    take_option(args, name)?.ok_or_else(|| CliError::Usage(format!("missing required {name}")))
}

fn take_option(args: &mut Vec<String>, name: &'static str) -> Result<Option<String>, CliError> {
    let Some(index) = args.iter().position(|arg| arg == name) else {
        return Ok(None);
    };
    if index + 1 >= args.len() {
        return Err(CliError::Usage(format!("missing value for {name}")));
    }
    let value = args.remove(index + 1);
    args.remove(index);
    Ok(Some(value))
}

fn take_repeated_option(
    args: &mut Vec<String>,
    name: &'static str,
) -> Result<Vec<String>, CliError> {
    let mut values = Vec::new();
    while let Some(index) = args.iter().position(|arg| arg == name) {
        if index + 1 >= args.len() {
            return Err(CliError::Usage(format!("missing value for {name}")));
        }
        let value = args.remove(index + 1);
        args.remove(index);
        values.push(value);
    }
    Ok(values)
}

fn optional_u64(args: &mut Vec<String>, name: &'static str, default: u64) -> Result<u64, CliError> {
    take_option(args, name)?
        .map(|value| parse_u64(name, &value))
        .unwrap_or(Ok(default))
}

fn optional_usize(
    args: &mut Vec<String>,
    name: &'static str,
    default: usize,
) -> Result<usize, CliError> {
    take_option(args, name)?
        .map(|value| {
            value
                .parse::<usize>()
                .map_err(|_| CliError::Usage(format!("{name} must be an unsigned integer")))
        })
        .unwrap_or(Ok(default))
}

fn parse_u64(name: &'static str, value: &str) -> Result<u64, CliError> {
    value
        .parse::<u64>()
        .map_err(|_| CliError::Usage(format!("{name} must be an unsigned integer")))
}

fn parse_bool(name: &'static str, value: &str) -> Result<bool, CliError> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(CliError::Usage(format!("{name} must be true or false"))),
    }
}

fn reject_extra_args(args: &[String]) -> Result<(), CliError> {
    if args.is_empty() {
        Ok(())
    } else {
        Err(CliError::Usage(format!(
            "unexpected argument '{}'",
            args[0]
        )))
    }
}

fn usage() -> String {
    format!(
        "usage: finitechat-darkmatter <compat-report|http-smoke|http>\n\n{}",
        http_usage()
    )
}

fn http_usage() -> String {
    "http commands:\n  finitechat-darkmatter http [--server URL] health\n  finitechat-darkmatter http [--server URL] publish-group --group-id ID --transport-group-id ID --message-id ID --payload BYTES [--commit-epoch N] [--idempotency-key KEY]\n  finitechat-darkmatter http [--server URL] publish-inbox --recipient ID --message-id ID --payload BYTES [--idempotency-key KEY]\n  finitechat-darkmatter http [--server URL] submit-commit --request-json JSON\n  finitechat-darkmatter http [--server URL] sync-group --group-id ID [--after-seq N] [--limit N]\n  finitechat-darkmatter http [--server URL] sync-inbox --recipient ID [--after-seq N] [--limit N]\n  finitechat-darkmatter http [--server URL] publish-key-package --owner ID --key-package-id ID --bytes BYTES\n  finitechat-darkmatter http [--server URL] key-package-inventory --owner ID\n  finitechat-darkmatter http [--server URL] claim-key-package --owner ID\n  finitechat-darkmatter http [--server URL] claim-key-packages --owner ID [--owner ID ...] [--idempotency-key KEY]\n  finitechat-darkmatter http [--server URL] fanout-get --fanout-id ID\n  finitechat-darkmatter http [--server URL] fanout-save-room --fanout-id ID --target-owner ID --room-id ID --key-package-id ID --welcome-id ID --commit-idempotency-key KEY [--claimed-key-package-id ID]\n  finitechat-darkmatter http [--server URL] fanout-mark-prepared --fanout-id ID --room-id ID --message-id ID\n  finitechat-darkmatter http [--server URL] fanout-mark-done --fanout-id ID --room-id ID --message-id ID --accepted-seq N\n  finitechat-darkmatter http [--server URL] account-room-bootstrap --room-id ID --mls-group-id ID --account-id ID --device-id ID\n  finitechat-darkmatter http [--server URL] account-room-save --account-id ID --room-id ID --record-json JSON\n  finitechat-darkmatter http [--server URL] account-rooms-list --account-id ID [--after-room-id ID] [--limit N]\n  finitechat-darkmatter http [--server URL] claim-welcomes --recipient ID [--limit N]\n  finitechat-darkmatter http [--server URL] ack-welcome --message-id ID --activated true|false".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use finitechat_engine::{CommitAccepted, SubmitCommitRequest, WelcomeRecord};
    use finitechat_http::{
        AckWelcomeRequest, BootstrapAccountRoomRequest, ClaimKeyPackagesRequest,
        ClaimWelcomesRequest, GroupSyncRequest, HttpClaimedWelcome, HttpFanoutPlan,
        HttpFanoutRoomStatus, HttpKeyPackageClaim, KeyPackageInventoryRequest,
        ListAccountRoomDirectoryRequest, MarkFanoutDoneRequest, MarkFanoutPreparedRequest,
        PublishKeyPackageResponse, PublishMessageRequest, SaveAccountRoomRequest,
        SaveFanoutRoomRequest,
    };
    use finitechat_proto::{
        FiniteEnvelope, LogEntryKind, MembershipAddV1, MembershipDeltaV1, StagedWelcomeV1,
        WelcomeState,
    };
    use transport_http_server::{HttpDeliveryPlane, HttpPublishReceipt, HttpSyncPage};

    #[test]
    fn publish_group_command_builds_route_dto() {
        let request = prepare_http_request([
            "--server",
            "http://localhost:9000/",
            "publish-group",
            "--group-id",
            "room-a",
            "--transport-group-id",
            "transport-a",
            "--message-id",
            "commit-a",
            "--payload",
            "commit-bytes",
            "--commit-epoch",
            "4",
            "--idempotency-key",
            "idem-commit-a",
        ])
        .expect("prepared request");

        assert_eq!(request.method, HttpMethod::Post);
        assert_eq!(request.url, "http://localhost:9000/messages");
        let body: PublishMessageRequest =
            serde_json::from_value(request.json.expect("json")).expect("message request");
        match body.target {
            HttpPublishTarget::Group {
                group_id,
                transport_group_id,
                commit_admission,
            } => {
                assert_eq!(group_id.as_slice(), b"room-a");
                assert_eq!(transport_group_id, b"transport-a");
                assert_eq!(
                    commit_admission.map(|admission| admission.source_epoch),
                    Some(EpochId(4))
                );
            }
            HttpPublishTarget::Inbox { .. } => panic!("expected group target"),
        }
        assert_eq!(body.message.id.as_slice(), b"commit-a");
        assert_eq!(body.message.payload, b"commit-bytes");
        assert_eq!(body.message.source.0, HTTP_SERVER_SOURCE);
        assert_eq!(body.idempotency_key.as_deref(), Some("idem-commit-a"));
    }

    #[test]
    fn sync_group_command_defaults_cursor_and_limit() {
        let request =
            prepare_http_request(["sync-group", "--group-id", "room-a"]).expect("prepared request");

        assert_eq!(request.method, HttpMethod::Post);
        assert_eq!(request.url, "http://127.0.0.1:8787/sync/group");
        let body: GroupSyncRequest =
            serde_json::from_value(request.json.expect("json")).expect("sync request");
        assert_eq!(body.group_id.as_slice(), b"room-a");
        assert_eq!(body.after_seq, 0);
        assert_eq!(body.limit, DEFAULT_SYNC_LIMIT);
    }

    #[test]
    fn publish_inbox_command_builds_welcome_envelope() {
        let request = prepare_http_request([
            "publish-inbox",
            "--recipient",
            "bob-device",
            "--message-id",
            "welcome-bob",
            "--payload",
            "welcome-bytes",
        ])
        .expect("prepared request");

        let body: PublishMessageRequest =
            serde_json::from_value(request.json.expect("json")).expect("message request");
        assert!(matches!(body.target, HttpPublishTarget::Inbox { .. }));
        match body.message.envelope {
            TransportEnvelope::Welcome { recipient } => {
                assert_eq!(recipient.as_slice(), b"bob-device");
            }
            TransportEnvelope::GroupMessage { .. } => panic!("expected Welcome envelope"),
        }
    }

    #[test]
    fn submit_commit_command_posts_request_json() {
        let request = prepare_http_request([
            "--server",
            "http://localhost:9000",
            "submit-commit",
            "--request-json",
            r#"{"room_id":"room-a","idempotency_key":"idem-a"}"#,
        ])
        .expect("prepared request");

        assert_eq!(request.method, HttpMethod::Post);
        assert_eq!(request.url, "http://localhost:9000/commits");
        let body = request.json.expect("json");
        assert_eq!(body["room_id"], "room-a");
        assert_eq!(body["idempotency_key"], "idem-a");
    }

    #[test]
    fn claim_key_package_command_builds_claim_request() {
        let request =
            prepare_http_request(["claim-key-package", "--owner", "alice"]).expect("request");

        assert_eq!(request.method, HttpMethod::Post);
        assert_eq!(request.url, "http://127.0.0.1:8787/key-packages/claim");
        let body: ClaimKeyPackageRequest =
            serde_json::from_value(request.json.expect("json")).expect("claim request");
        assert_eq!(body.owner.as_slice(), b"alice");
    }

    #[test]
    fn key_package_inventory_command_builds_inventory_request() {
        let request =
            prepare_http_request(["key-package-inventory", "--owner", "alice"]).expect("request");

        assert_eq!(request.method, HttpMethod::Post);
        assert_eq!(request.url, "http://127.0.0.1:8787/key-packages/inventory");
        let body: KeyPackageInventoryRequest =
            serde_json::from_value(request.json.expect("json")).expect("inventory request");
        assert_eq!(body.owner.as_slice(), b"alice");
    }

    #[test]
    fn claim_key_packages_command_builds_batch_claim_request() {
        let request = prepare_http_request([
            "claim-key-packages",
            "--owner",
            "alice-phone",
            "--owner",
            "alice-laptop",
            "--idempotency-key",
            "fanout-claim-1",
        ])
        .expect("request");

        assert_eq!(request.method, HttpMethod::Post);
        assert_eq!(request.url, "http://127.0.0.1:8787/key-packages/claims");
        let body: ClaimKeyPackagesRequest =
            serde_json::from_value(request.json.expect("json")).expect("batch claim request");
        assert_eq!(body.owners.len(), 2);
        assert_eq!(body.owners[0].as_slice(), b"alice-phone");
        assert_eq!(body.owners[1].as_slice(), b"alice-laptop");
        assert_eq!(body.idempotency_key.as_deref(), Some("fanout-claim-1"));
    }

    #[test]
    fn fanout_save_room_command_builds_route_dto() {
        let request = prepare_http_request([
            "fanout-save-room",
            "--fanout-id",
            "fanout-a",
            "--target-owner",
            "alice-phone",
            "--room-id",
            "room-a",
            "--key-package-id",
            "kp-a",
            "--welcome-id",
            "welcome-a",
            "--commit-idempotency-key",
            "link-a",
            "--claimed-key-package-id",
            "kp-a",
        ])
        .expect("request");

        assert_eq!(request.method, HttpMethod::Post);
        assert_eq!(request.url, "http://127.0.0.1:8787/fanouts/rooms");
        let body: SaveFanoutRoomRequest =
            serde_json::from_value(request.json.expect("json")).expect("fanout room request");
        assert_eq!(body.fanout_id, "fanout-a");
        assert_eq!(body.target_owner.as_slice(), b"alice-phone");
        assert_eq!(body.room.room_id.as_slice(), b"room-a");
        assert_eq!(body.room.key_package_id.as_slice(), b"kp-a");
        assert_eq!(body.room.welcome_id.as_slice(), b"welcome-a");
        assert_eq!(body.room.commit_idempotency_key, "link-a");
        assert_eq!(
            body.room
                .claimed_key_package_id
                .expect("claimed package")
                .as_slice(),
            b"kp-a"
        );
    }

    #[test]
    fn fanout_status_commands_build_route_dtos() {
        let prepared = prepare_http_request([
            "fanout-mark-prepared",
            "--fanout-id",
            "fanout-a",
            "--room-id",
            "room-a",
            "--message-id",
            "commit-a",
        ])
        .expect("prepared request");

        assert_eq!(prepared.url, "http://127.0.0.1:8787/fanouts/rooms/prepared");
        let body: MarkFanoutPreparedRequest =
            serde_json::from_value(prepared.json.expect("json")).expect("prepared body");
        assert_eq!(body.fanout_id, "fanout-a");
        assert_eq!(body.room_id.as_slice(), b"room-a");
        assert_eq!(body.prepared_message_id.as_slice(), b"commit-a");

        let done = prepare_http_request([
            "fanout-mark-done",
            "--fanout-id",
            "fanout-a",
            "--room-id",
            "room-a",
            "--message-id",
            "commit-b",
            "--accepted-seq",
            "9",
        ])
        .expect("done request");

        assert_eq!(done.url, "http://127.0.0.1:8787/fanouts/rooms/done");
        let body: MarkFanoutDoneRequest =
            serde_json::from_value(done.json.expect("json")).expect("done body");
        assert_eq!(body.fanout_id, "fanout-a");
        assert_eq!(body.room_id.as_slice(), b"room-a");
        assert_eq!(body.prepared_message_id.as_slice(), b"commit-b");
        assert_eq!(body.accepted_seq, 9);
    }

    #[test]
    fn account_room_commands_build_route_dtos() {
        let bootstrap = prepare_http_request([
            "account-room-bootstrap",
            "--room-id",
            "room-a",
            "--mls-group-id",
            "mls-a",
            "--account-id",
            "alice",
            "--device-id",
            "alice-phone",
        ])
        .expect("bootstrap request");

        assert_eq!(bootstrap.method, HttpMethod::Post);
        assert_eq!(
            bootstrap.url,
            "http://127.0.0.1:8787/account-rooms/bootstrap"
        );
        let body: BootstrapAccountRoomRequest =
            serde_json::from_value(bootstrap.json.expect("json"))
                .expect("account-room bootstrap request");
        assert_eq!(body.room_id, "room-a");
        assert_eq!(body.mls_group_id, "mls-a");
        assert_eq!(body.creator.account_id, "alice");
        assert_eq!(body.creator.device_id, "alice-phone");

        let save = prepare_http_request([
            "account-room-save",
            "--account-id",
            "alice",
            "--room-id",
            "room-a",
            "--record-json",
            r#"{"room_id":"room-a","current_epoch":2}"#,
        ])
        .expect("save request");

        assert_eq!(save.method, HttpMethod::Post);
        assert_eq!(save.url, "http://127.0.0.1:8787/account-rooms");
        let body: SaveAccountRoomRequest =
            serde_json::from_value(save.json.expect("json")).expect("account-room save request");
        assert_eq!(body.account_id, "alice");
        assert_eq!(body.room_id, "room-a");
        assert_eq!(body.record["current_epoch"], 2);

        let list = prepare_http_request([
            "account-rooms-list",
            "--account-id",
            "alice",
            "--after-room-id",
            "room-a",
            "--limit",
            "3",
        ])
        .expect("list request");

        assert_eq!(list.method, HttpMethod::Post);
        assert_eq!(list.url, "http://127.0.0.1:8787/account-rooms/list");
        let body: ListAccountRoomDirectoryRequest =
            serde_json::from_value(list.json.expect("json")).expect("account-room list request");
        assert_eq!(body.account_id, "alice");
        assert_eq!(body.after_room_id.as_deref(), Some("room-a"));
        assert_eq!(body.limit, 3);
    }

    #[test]
    fn claim_welcomes_command_builds_claim_request() {
        let request = prepare_http_request([
            "claim-welcomes",
            "--recipient",
            "bob-device",
            "--limit",
            "3",
        ])
        .expect("request");

        assert_eq!(request.method, HttpMethod::Post);
        assert_eq!(request.url, "http://127.0.0.1:8787/welcomes/claim");
        let body: ClaimWelcomesRequest =
            serde_json::from_value(request.json.expect("json")).expect("claim welcomes request");
        assert_eq!(body.recipient.as_slice(), b"bob-device");
        assert_eq!(body.limit, 3);
    }

    #[test]
    fn ack_welcome_command_builds_ack_request() {
        let request = prepare_http_request([
            "ack-welcome",
            "--message-id",
            "welcome-bob",
            "--activated",
            "true",
        ])
        .expect("request");

        assert_eq!(request.method, HttpMethod::Post);
        assert_eq!(request.url, "http://127.0.0.1:8787/welcomes/ack");
        let body: AckWelcomeRequest =
            serde_json::from_value(request.json.expect("json")).expect("ack welcome request");
        assert_eq!(body.message_id.as_slice(), b"welcome-bob");
        assert!(body.activated);
    }

    #[test]
    fn live_cli_submit_commit_claim_and_ack_welcome_over_http_server() {
        let dir = tempfile::tempdir().expect("tempdir");
        let server_db = dir.path().join("cli-live-submit.sqlite3");
        let server_url = spawn_live_cli_server(&server_db);
        let creator = DeviceRef::new("alice", "alice-laptop");
        let phone = DeviceRef::new("alice", "alice-phone");
        let room_id = "room-cli-live-submit";
        let mls_group_id = "mls-cli-live-submit";
        let welcome_id = "welcome-cli-live-phone";
        let submit = submit_add_device_request(
            room_id,
            mls_group_id,
            &creator,
            &phone,
            welcome_id,
            "commit-cli-live-idempotency",
        );
        let submit_json = serde_json::to_string(&submit).expect("submit json");

        let bootstrap = run_cli_json([
            "http",
            "--server",
            &server_url,
            "account-room-bootstrap",
            "--room-id",
            room_id,
            "--mls-group-id",
            mls_group_id,
            "--account-id",
            &creator.account_id,
            "--device-id",
            &creator.device_id,
        ]);
        assert_eq!(bootstrap["bootstrapped"], true);

        let accepted: CommitAccepted = serde_json::from_value(run_cli_json([
            "http",
            "--server",
            &server_url,
            "submit-commit",
            "--request-json",
            &submit_json,
        ]))
        .expect("commit accepted");
        let expected_message_id = submit.envelope.message_id().expect("submit message id");
        assert_eq!(accepted.seq, 1);
        assert_eq!(accepted.message_id, expected_message_id);
        assert_eq!(accepted.released_welcomes, vec![welcome_id.to_owned()]);

        let replayed: CommitAccepted = serde_json::from_value(run_cli_json([
            "http",
            "--server",
            &server_url,
            "submit-commit",
            "--request-json",
            &submit_json,
        ]))
        .expect("commit replay");
        assert_eq!(replayed, accepted);

        let group_page: HttpSyncPage = serde_json::from_value(run_cli_json([
            "http",
            "--server",
            &server_url,
            "sync-group",
            "--group-id",
            room_id,
            "--limit",
            "10",
        ]))
        .expect("group sync");
        assert_eq!(group_page.entries.len(), 1);
        assert_eq!(group_page.entries[0].seq, accepted.seq);
        assert_eq!(
            group_page.entries[0].message.id.as_slice(),
            accepted.message_id.as_bytes()
        );

        let recipient = member_for_device(&phone);
        let claimed: Vec<HttpClaimedWelcome> = serde_json::from_value(run_cli_json([
            "http",
            "--server",
            &server_url,
            "claim-welcomes",
            "--recipient",
            std::str::from_utf8(recipient.as_slice()).expect("recipient json"),
            "--limit",
            "10",
        ]))
        .expect("claimed welcomes");
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].seq, 1);
        assert_eq!(claimed[0].message.id.as_slice(), welcome_id.as_bytes());
        let welcome: WelcomeRecord =
            serde_json::from_slice(&claimed[0].message.payload).expect("welcome record");
        assert_eq!(welcome.welcome_id, welcome_id);
        assert_eq!(welcome.commit_seq, accepted.seq);
        assert_eq!(welcome.recipient, phone);
        assert_eq!(welcome.state, WelcomeState::Released);

        let duplicate_claim: Vec<HttpClaimedWelcome> = serde_json::from_value(run_cli_json([
            "http",
            "--server",
            &server_url,
            "claim-welcomes",
            "--recipient",
            std::str::from_utf8(recipient.as_slice()).expect("recipient json"),
            "--limit",
            "10",
        ]))
        .expect("duplicate claim");
        assert!(duplicate_claim.is_empty());

        let acked = run_cli_json([
            "http",
            "--server",
            &server_url,
            "ack-welcome",
            "--message-id",
            welcome_id,
            "--activated",
            "true",
        ]);
        assert_eq!(acked["acked"], true);

        let acked_again = run_cli_json([
            "http",
            "--server",
            &server_url,
            "ack-welcome",
            "--message-id",
            welcome_id,
            "--activated",
            "true",
        ]);
        assert_eq!(acked_again["acked"], true);

        let conflict = run(
            [
                "http",
                "--server",
                &server_url,
                "ack-welcome",
                "--message-id",
                welcome_id,
                "--activated",
                "false",
            ],
            &mut Vec::new(),
        )
        .expect_err("conflicting ack fails");
        assert!(matches!(
            conflict,
            CliError::Server {
                status: reqwest::StatusCode::CONFLICT,
                ..
            }
        ));

        let listed = run_cli_json([
            "http",
            "--server",
            &server_url,
            "account-rooms-list",
            "--account-id",
            "alice",
            "--limit",
            "10",
        ]);
        assert_eq!(listed["rooms"][0]["devices"][1]["active"], true);
    }

    #[test]
    fn live_cli_publish_sync_and_idempotency_conflict_over_http_server() {
        let dir = tempfile::tempdir().expect("tempdir");
        let server_db = dir.path().join("cli-live-publish.sqlite3");
        let server_url = spawn_live_cli_server(&server_db);
        let group_id = "cli-live-room";
        let transport_group_id = "cli-live-transport";
        let message_id = "cli-live-commit-1";
        let idempotency_key = "cli-live-idempotency";

        let health = run_cli_json(["http", "--server", &server_url, "health"]);
        assert_eq!(health["status"], "ok");

        let published: HttpPublishReceipt = serde_json::from_value(run_cli_json([
            "http",
            "--server",
            &server_url,
            "publish-group",
            "--group-id",
            group_id,
            "--transport-group-id",
            transport_group_id,
            "--message-id",
            message_id,
            "--payload",
            "commit-bytes",
            "--commit-epoch",
            "1",
            "--idempotency-key",
            idempotency_key,
        ]))
        .expect("publish receipt");
        assert_eq!(published.message_id.as_slice(), message_id.as_bytes());
        assert_eq!(published.plane, HttpDeliveryPlane::Group);
        assert_eq!(published.seq, 1);
        assert!(!published.duplicate);

        let replayed: HttpPublishReceipt = serde_json::from_value(run_cli_json([
            "http",
            "--server",
            &server_url,
            "publish-group",
            "--group-id",
            group_id,
            "--transport-group-id",
            transport_group_id,
            "--message-id",
            message_id,
            "--payload",
            "commit-bytes",
            "--commit-epoch",
            "1",
            "--idempotency-key",
            idempotency_key,
        ]))
        .expect("replayed publish receipt");
        assert_eq!(replayed, published);

        let page: HttpSyncPage = serde_json::from_value(run_cli_json([
            "http",
            "--server",
            &server_url,
            "sync-group",
            "--group-id",
            group_id,
            "--limit",
            "10",
        ]))
        .expect("group page");
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].seq, published.seq);
        assert_eq!(page.entries[0].message.payload, b"commit-bytes");

        let conflict = run(
            [
                "http",
                "--server",
                &server_url,
                "publish-group",
                "--group-id",
                group_id,
                "--transport-group-id",
                transport_group_id,
                "--message-id",
                "cli-live-commit-conflict",
                "--payload",
                "different-commit",
                "--commit-epoch",
                "2",
                "--idempotency-key",
                idempotency_key,
            ],
            &mut Vec::new(),
        )
        .expect_err("conflicting idempotency key fails");
        assert!(matches!(
            conflict,
            CliError::Server {
                status: reqwest::StatusCode::CONFLICT,
                ..
            }
        ));
    }

    #[test]
    fn live_cli_batch_key_package_claim_replays_over_http_server() {
        let dir = tempfile::tempdir().expect("tempdir");
        let server_db = dir.path().join("cli-live-key-packages.sqlite3");
        let server_url = spawn_live_cli_server(&server_db);

        for (owner, key_package_id, bytes) in [
            ("live-laptop", "live-laptop-1", "laptop-package"),
            ("live-phone", "live-phone-1", "phone-package-1"),
            ("live-phone", "live-phone-2", "phone-package-2"),
        ] {
            let response: PublishKeyPackageResponse = serde_json::from_value(run_cli_json([
                "http",
                "--server",
                &server_url,
                "publish-key-package",
                "--owner",
                owner,
                "--key-package-id",
                key_package_id,
                "--bytes",
                bytes,
            ]))
            .expect("publish package response");
            assert!(response.published);
        }

        let claims: Vec<HttpKeyPackageClaim> = serde_json::from_value(run_cli_json([
            "http",
            "--server",
            &server_url,
            "claim-key-packages",
            "--owner",
            "live-laptop",
            "--owner",
            "live-phone",
            "--idempotency-key",
            "live-batch-claim",
        ]))
        .expect("batch claims");
        assert_eq!(claims.len(), 2);
        assert_claimed_package(&claims[0], "live-laptop", "live-laptop-1");
        assert_claimed_package(&claims[1], "live-phone", "live-phone-1");

        let replayed: Vec<HttpKeyPackageClaim> = serde_json::from_value(run_cli_json([
            "http",
            "--server",
            &server_url,
            "claim-key-packages",
            "--owner",
            "live-laptop",
            "--owner",
            "live-phone",
            "--idempotency-key",
            "live-batch-claim",
        ]))
        .expect("batch claim replay");
        assert_eq!(replayed, claims);

        let remaining: transport_http_server::HttpClaimedKeyPackage =
            serde_json::from_value(run_cli_json([
                "http",
                "--server",
                &server_url,
                "claim-key-package",
                "--owner",
                "live-phone",
            ]))
            .expect("remaining phone package");
        assert_eq!(remaining.key_package_id.as_slice(), b"live-phone-2");
        assert_eq!(remaining.owner.as_slice(), b"live-phone");
    }

    #[test]
    fn live_cli_fanout_checkpoint_flow_over_http_server() {
        let dir = tempfile::tempdir().expect("tempdir");
        let server_db = dir.path().join("cli-live-fanout.sqlite3");
        let server_url = spawn_live_cli_server(&server_db);

        let saved: HttpFanoutPlan = serde_json::from_value(run_cli_json([
            "http",
            "--server",
            &server_url,
            "fanout-save-room",
            "--fanout-id",
            "live-fanout",
            "--target-owner",
            "live-phone",
            "--room-id",
            "live-room",
            "--key-package-id",
            "live-kp-1",
            "--welcome-id",
            "live-welcome-1",
            "--commit-idempotency-key",
            "live-commit-key",
            "--claimed-key-package-id",
            "live-kp-1",
        ]))
        .expect("saved fanout");
        assert_eq!(saved.fanout_id, "live-fanout");
        assert_eq!(saved.rooms.len(), 1);
        assert!(matches!(
            saved.rooms[0].status,
            HttpFanoutRoomStatus::Pending
        ));

        let prepared: HttpFanoutPlan = serde_json::from_value(run_cli_json([
            "http",
            "--server",
            &server_url,
            "fanout-mark-prepared",
            "--fanout-id",
            "live-fanout",
            "--room-id",
            "live-room",
            "--message-id",
            "live-commit-loser",
        ]))
        .expect("prepared fanout");
        assert!(matches!(
            prepared.rooms[0].status,
            HttpFanoutRoomStatus::Prepared {
                ref prepared_message_id
            } if prepared_message_id.as_slice() == b"live-commit-loser"
        ));

        let reprepared: HttpFanoutPlan = serde_json::from_value(run_cli_json([
            "http",
            "--server",
            &server_url,
            "fanout-mark-prepared",
            "--fanout-id",
            "live-fanout",
            "--room-id",
            "live-room",
            "--message-id",
            "live-commit-retry",
        ]))
        .expect("reprepared fanout");
        assert!(matches!(
            reprepared.rooms[0].status,
            HttpFanoutRoomStatus::Prepared {
                ref prepared_message_id
            } if prepared_message_id.as_slice() == b"live-commit-retry"
        ));

        let done: HttpFanoutPlan = serde_json::from_value(run_cli_json([
            "http",
            "--server",
            &server_url,
            "fanout-mark-done",
            "--fanout-id",
            "live-fanout",
            "--room-id",
            "live-room",
            "--message-id",
            "live-commit-retry",
            "--accepted-seq",
            "12",
        ]))
        .expect("done fanout");
        assert!(matches!(
            done.rooms[0].status,
            HttpFanoutRoomStatus::Done {
                ref prepared_message_id,
                accepted_seq: 12,
            } if prepared_message_id.as_slice() == b"live-commit-retry"
        ));

        let loaded: HttpFanoutPlan = serde_json::from_value(run_cli_json([
            "http",
            "--server",
            &server_url,
            "fanout-get",
            "--fanout-id",
            "live-fanout",
        ]))
        .expect("loaded fanout");
        assert_eq!(loaded, done);
    }

    #[test]
    fn unknown_option_is_usage_error() {
        let error = prepare_http_request(["health", "--wat"]).expect_err("usage error");
        assert!(matches!(error, CliError::Usage(_)));
    }

    fn run_cli_json<const N: usize>(args: [&str; N]) -> Value {
        let mut output = Vec::new();
        run(args, &mut output).expect("cli run");
        serde_json::from_slice(&output).expect("cli json output")
    }

    fn spawn_live_cli_server(path: &std::path::Path) -> String {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let app = finitechat_server::http_router(
            finitechat_server::HttpServerState::from_sqlite_path(path).unwrap(),
        );
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Runtime::new().unwrap();
            runtime.block_on(async move {
                let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                axum::serve(listener, app).await.unwrap();
            });
        });
        let server_url = format!("http://{addr}");
        wait_for_live_cli_server(&server_url);
        server_url
    }

    fn wait_for_live_cli_server(server_url: &str) {
        let health_url = format!("{}/health", server_url.trim_end_matches('/'));
        let client = reqwest::blocking::Client::new();
        for _ in 0..100 {
            if client
                .get(&health_url)
                .send()
                .map(|response| response.status().is_success())
                .unwrap_or(false)
            {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("live CLI test server did not become healthy at {health_url}");
    }

    fn member_for_device(device: &DeviceRef) -> MemberId {
        MemberId::new(serde_json::to_vec(device).expect("device member id json"))
    }

    fn assert_claimed_package(claim: &HttpKeyPackageClaim, owner: &str, key_package_id: &str) {
        assert_eq!(claim.owner.as_slice(), owner.as_bytes());
        let claimed = claim.claimed.as_ref().expect("claimed package");
        assert_eq!(claimed.owner.as_slice(), owner.as_bytes());
        assert_eq!(claimed.key_package_id.as_slice(), key_package_id.as_bytes());
    }

    fn submit_add_device_request(
        room_id: &str,
        mls_group_id: &str,
        sender: &DeviceRef,
        added: &DeviceRef,
        welcome_id: &str,
        idempotency_key: &str,
    ) -> SubmitCommitRequest {
        let envelope = FiniteEnvelope {
            room_id: room_id.to_owned(),
            mls_group_id: mls_group_id.to_owned(),
            epoch: 0,
            sender: sender.clone(),
            kind: LogEntryKind::Commit,
            payload: b"commit-add-device".to_vec(),
        };
        let commit_message_id = envelope.message_id().expect("commit message id");
        SubmitCommitRequest {
            room_id: room_id.to_owned(),
            sender: sender.clone(),
            expected_epoch: 0,
            envelope,
            membership_delta: MembershipDeltaV1 {
                base_epoch: 0,
                post_commit_epoch: 1,
                commit_message_id,
                adds: vec![MembershipAddV1 {
                    device: added.clone(),
                    key_package_id: "key-package-add-device".to_owned(),
                    key_package_ref: "key-package-ref-add-device".to_owned(),
                    key_package_hash: "key-package-hash-add-device".to_owned(),
                    welcome_id: welcome_id.to_owned(),
                }],
                removes: Vec::new(),
            },
            staged_welcomes: vec![StagedWelcomeV1 {
                welcome_id: welcome_id.to_owned(),
                welcome_payload: b"welcome-add-device".to_vec(),
                ratchet_tree_payload: b"ratchet-tree-add-device".to_vec(),
            }],
            idempotency_key: idempotency_key.to_owned(),
        }
    }
}
