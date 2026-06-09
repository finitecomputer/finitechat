use std::io::Write;

use cgka_traits::engine::KeyPackage;
use cgka_traits::transport::{Timestamp, TransportEnvelope, TransportMessage, TransportSource};
use cgka_traits::{EpochId, GroupId, MemberId, MessageId};
use finitechat_server::{
    ClaimKeyPackageRequest, GroupSyncRequest, InboxSyncRequest, PublishMessageRequest,
};
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
            Self::Serialize(_) | Self::Http(_) | Self::Server { .. } | Self::Output(_) => 1,
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
        "sync-group" => sync_group_request(&server, args),
        "sync-inbox" => sync_inbox_request(&server, args),
        "publish-key-package" => publish_key_package_request(&server, args),
        "claim-key-package" => claim_key_package_request(&server, args),
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
    "http commands:\n  finitechat-darkmatter http [--server URL] health\n  finitechat-darkmatter http [--server URL] publish-group --group-id ID --transport-group-id ID --message-id ID --payload BYTES [--commit-epoch N] [--idempotency-key KEY]\n  finitechat-darkmatter http [--server URL] publish-inbox --recipient ID --message-id ID --payload BYTES [--idempotency-key KEY]\n  finitechat-darkmatter http [--server URL] sync-group --group-id ID [--after-seq N] [--limit N]\n  finitechat-darkmatter http [--server URL] sync-inbox --recipient ID [--after-seq N] [--limit N]\n  finitechat-darkmatter http [--server URL] publish-key-package --owner ID --key-package-id ID --bytes BYTES\n  finitechat-darkmatter http [--server URL] claim-key-package --owner ID".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use finitechat_server::{GroupSyncRequest, PublishMessageRequest};

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
    fn unknown_option_is_usage_error() {
        let error = prepare_http_request(["health", "--wat"]).expect_err("usage error");
        assert!(matches!(error, CliError::Usage(_)));
    }
}
