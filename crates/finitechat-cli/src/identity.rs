use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use finitechat_client::generate_account_secret;
use finitechat_mls::{NOSTR_SECRET_KEY_BYTES, NostrSecretKey};
use finitechat_proto::{npub_encode, nsec_encode};
use serde::Serialize;

use crate::{CliError, reject_extra_args, take_option, take_positional, write_pretty_json};

pub(crate) const IDENTITY_ENV_FILE: &str = "identity.env";
pub(crate) const LEGACY_NSEC_FILE: &str = "agent.nsec";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentHomeResolution {
    pub path: PathBuf,
    pub source: &'static str,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct IdentitySummary {
    home: String,
    identity_env: String,
    secret_file: String,
    account_id: String,
    npub: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    secret_hex: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    nsec: Option<String>,
}

pub(crate) fn run<W: Write>(mut args: Vec<String>, output: &mut W) -> Result<(), CliError> {
    let home = resolve_agent_home(&mut args)?;
    let show_secret = take_flag(&mut args, "--show-secret");
    let Some(command) = take_positional(&mut args) else {
        return Err(CliError::Usage(usage()));
    };

    match command.as_str() {
        "init" => {
            reject_extra_args(&args)?;
            cmd_init(&home.path, show_secret, output)
        }
        "show" => {
            reject_extra_args(&args)?;
            cmd_show(&home.path, show_secret, output)
        }
        _ => Err(CliError::Usage(usage())),
    }
}

pub(crate) fn resolve_agent_home(args: &mut Vec<String>) -> Result<AgentHomeResolution, CliError> {
    resolve_agent_home_with(
        args,
        |name| std::env::var_os(name).map(PathBuf::from),
        || std::env::var_os("HOME").map(PathBuf::from),
    )
}

fn resolve_agent_home_with(
    args: &mut Vec<String>,
    env_path: impl Fn(&str) -> Option<PathBuf>,
    home_dir: impl Fn() -> Option<PathBuf>,
) -> Result<AgentHomeResolution, CliError> {
    if let Some(path) = take_option(args, "--agent-home")? {
        return Ok(AgentHomeResolution {
            path: PathBuf::from(path),
            source: "--agent-home",
        });
    }
    if let Some(path) = take_option(args, "--home")? {
        return Ok(AgentHomeResolution {
            path: PathBuf::from(path),
            source: "--home",
        });
    }
    for (name, source) in [
        ("FINITE_AGENT_HOME", "FINITE_AGENT_HOME"),
        ("FINITECHAT_HOME", "FINITECHAT_HOME"),
        ("FINITE_HOME", "FINITE_HOME"),
    ] {
        if let Some(path) = env_path(name) {
            return Ok(AgentHomeResolution { path, source });
        }
    }
    let Some(home) = home_dir() else {
        return Err(CliError::Usage(
            "pass --agent-home DIR, set FINITE_AGENT_HOME, or set HOME".to_owned(),
        ));
    };
    Ok(AgentHomeResolution {
        path: home.join(".finite").join("agent"),
        source: "HOME",
    })
}

fn cmd_init<W: Write>(home: &Path, show_secret: bool, output: &mut W) -> Result<(), CliError> {
    let identity_env = home.join(IDENTITY_ENV_FILE);
    let secret_file = home.join(LEGACY_NSEC_FILE);
    if identity_env.exists() || secret_file.exists() {
        return Err(CliError::Identity(format!(
            "agent home {} already has an identity",
            home.display()
        )));
    }
    fs::create_dir_all(home).map_err(identity_io)?;

    let secret =
        generate_account_secret().map_err(|error| CliError::Identity(error.to_string()))?;
    let secret_hex = hex_lower(secret.as_bytes());
    let account_id = account_id_for_secret(&secret);
    let npub = npub_encode(&account_id)
        .map_err(|error| CliError::Identity(format!("npub encoding failed: {error}")))?;
    write_private(
        &identity_env,
        &identity_env_contents(&secret_hex, &account_id, &npub),
    )?;
    // Compatibility with the current Hermes bridge while the runtime moves to
    // the shared identity store.
    write_private(&secret_file, &secret_hex)?;

    write_pretty_json(
        output,
        &summary(home, &account_id, &secret_hex, show_secret)?,
    )
}

fn cmd_show<W: Write>(home: &Path, show_secret: bool, output: &mut W) -> Result<(), CliError> {
    let secret = load_agent_secret(home)?;
    let secret_hex = hex_lower(secret.as_bytes());
    let account_id = account_id_for_secret(&secret);
    write_pretty_json(
        output,
        &summary(home, &account_id, &secret_hex, show_secret)?,
    )
}

pub(crate) fn agent_identity_exists(home: &Path) -> bool {
    home.join(IDENTITY_ENV_FILE).exists() || home.join(LEGACY_NSEC_FILE).exists()
}

pub(crate) fn load_agent_secret(home: &Path) -> Result<NostrSecretKey, CliError> {
    let secret_hex = load_secret_hex(home)?;
    secret_from_hex(&secret_hex)
}

pub(crate) fn persist_agent_identity(home: &Path, secret: &NostrSecretKey) -> Result<(), CliError> {
    fs::create_dir_all(home).map_err(identity_io)?;
    let secret_hex = hex_lower(secret.as_bytes());
    let account_id = account_id_for_secret(secret);
    let npub = npub_encode(&account_id)
        .map_err(|error| CliError::Identity(format!("npub encoding failed: {error}")))?;

    let identity_env = home.join(IDENTITY_ENV_FILE);
    if identity_env.exists() {
        let existing = load_secret_hex_from_identity_env(&identity_env)?;
        if existing != secret_hex {
            return Err(CliError::Identity(format!(
                "{} belongs to a different Agent Principal Key",
                identity_env.display()
            )));
        }
    } else {
        write_private(
            &identity_env,
            &identity_env_contents(&secret_hex, &account_id, &npub),
        )?;
    }

    let secret_file = home.join(LEGACY_NSEC_FILE);
    if secret_file.exists() {
        let existing = read_legacy_secret_hex(&secret_file)?;
        if existing != secret_hex {
            return Err(CliError::Identity(format!(
                "{} belongs to a different Agent Principal Key",
                secret_file.display()
            )));
        }
    } else {
        write_private(&secret_file, &secret_hex)?;
    }
    Ok(())
}

fn load_secret_hex(home: &Path) -> Result<String, CliError> {
    let identity_env = home.join(IDENTITY_ENV_FILE);
    if identity_env.exists() {
        return load_secret_hex_from_identity_env(&identity_env);
    }

    let secret_file = home.join(LEGACY_NSEC_FILE);
    read_legacy_secret_hex(&secret_file).map_err(|_| {
        CliError::Identity(format!(
            "agent home {} is missing identity.env (run identity init)",
            home.display()
        ))
    })
}

fn load_secret_hex_from_identity_env(path: &Path) -> Result<String, CliError> {
    let raw = fs::read_to_string(path).map_err(identity_io)?;
    for line in raw.lines() {
        let trimmed = line.trim();
        if let Some(value) = trimmed.strip_prefix("FINITE_AGENT_SECRET_HEX=") {
            return Ok(value.trim().to_owned());
        }
    }
    Err(CliError::Identity(format!(
        "{} is missing FINITE_AGENT_SECRET_HEX",
        path.display()
    )))
}

fn read_legacy_secret_hex(path: &Path) -> Result<String, CliError> {
    fs::read_to_string(path)
        .map(|raw| raw.trim().to_owned())
        .map_err(identity_io)
}

fn summary(
    home: &Path,
    account_id: &str,
    secret_hex: &str,
    show_secret: bool,
) -> Result<IdentitySummary, CliError> {
    let npub = npub_encode(account_id)
        .map_err(|error| CliError::Identity(format!("npub encoding failed: {error}")))?;
    let nsec = if show_secret {
        Some(
            nsec_encode(secret_hex)
                .map_err(|error| CliError::Identity(format!("nsec encoding failed: {error}")))?,
        )
    } else {
        None
    };
    Ok(IdentitySummary {
        home: home.display().to_string(),
        identity_env: home.join(IDENTITY_ENV_FILE).display().to_string(),
        secret_file: home.join(LEGACY_NSEC_FILE).display().to_string(),
        account_id: account_id.to_owned(),
        npub,
        secret_hex: show_secret.then(|| secret_hex.to_owned()),
        nsec,
    })
}

fn secret_from_hex(secret_hex: &str) -> Result<NostrSecretKey, CliError> {
    let bytes = crate::parse_hex("FINITE_AGENT_SECRET_HEX", secret_hex)?;
    let bytes: [u8; NOSTR_SECRET_KEY_BYTES] = bytes.try_into().map_err(|_| {
        CliError::Identity("FINITE_AGENT_SECRET_HEX must be 32 bytes of hex".to_owned())
    })?;
    NostrSecretKey::from_bytes(bytes).map_err(|error| CliError::Identity(error.to_string()))
}

fn account_id_for_secret(secret: &NostrSecretKey) -> String {
    hex_lower(secret.public_key().as_bytes())
}

fn identity_env_contents(secret_hex: &str, account_id: &str, npub: &str) -> String {
    format!(
        "FINITE_AGENT_SECRET_HEX={secret_hex}\nFINITE_AGENT_ACCOUNT_ID={account_id}\nFINITE_AGENT_NPUB={npub}\n"
    )
}

fn write_private(path: &Path, contents: &str) -> Result<(), CliError> {
    fs::write(path, contents).map_err(identity_io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(identity_io)?;
    }
    Ok(())
}

fn identity_io(error: std::io::Error) -> CliError {
    CliError::Identity(error.to_string())
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn take_flag(args: &mut Vec<String>, name: &str) -> bool {
    if let Some(index) = args.iter().position(|arg| arg == name) {
        args.remove(index);
        true
    } else {
        false
    }
}

pub(crate) fn usage() -> String {
    "identity commands:\n  finitechat identity [--agent-home DIR] init [--show-secret]\n  finitechat identity [--agent-home DIR] show [--show-secret]\n  (--home is accepted as a compatibility alias; FINITE_AGENT_HOME, FINITECHAT_HOME, FINITE_HOME, or ~/.finite/agent may replace --agent-home)".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_agent_home_prefers_explicit_agent_home() {
        let mut args = vec![
            "--agent-home".to_owned(),
            "/tmp/agent-a".to_owned(),
            "show".into(),
        ];
        let resolved =
            resolve_agent_home_with(&mut args, |_| None, || Some(PathBuf::from("/home/alice")))
                .expect("resolved");
        assert_eq!(resolved.path, PathBuf::from("/tmp/agent-a"));
        assert_eq!(resolved.source, "--agent-home");
        assert_eq!(args, vec!["show"]);
    }

    #[test]
    fn resolve_agent_home_prefers_finite_agent_home_env() {
        let mut args = vec!["show".to_owned()];
        let resolved = resolve_agent_home_with(
            &mut args,
            |name| (name == "FINITE_AGENT_HOME").then(|| PathBuf::from("/env/agent")),
            || Some(PathBuf::from("/home/alice")),
        )
        .expect("resolved");
        assert_eq!(resolved.path, PathBuf::from("/env/agent"));
        assert_eq!(resolved.source, "FINITE_AGENT_HOME");
        assert_eq!(args, vec!["show"]);
    }

    #[test]
    fn resolve_agent_home_falls_back_to_home_dot_finite_agent() {
        let mut args = vec!["show".to_owned()];
        let resolved =
            resolve_agent_home_with(&mut args, |_| None, || Some(PathBuf::from("/home/alice")))
                .expect("resolved");
        assert_eq!(resolved.path, PathBuf::from("/home/alice/.finite/agent"));
        assert_eq!(resolved.source, "HOME");
    }

    #[test]
    fn identity_env_round_trips_secret() {
        let secret_hex = "11".repeat(NOSTR_SECRET_KEY_BYTES);
        let secret = secret_from_hex(&secret_hex).expect("secret");
        let account_id = account_id_for_secret(&secret);
        let npub = npub_encode(&account_id).expect("npub");
        let raw = identity_env_contents(&secret_hex, &account_id, &npub);
        assert!(raw.contains("FINITE_AGENT_SECRET_HEX="));
        assert!(raw.contains("FINITE_AGENT_ACCOUNT_ID="));
        assert!(raw.contains("FINITE_AGENT_NPUB=npub1"));
    }

    #[test]
    fn init_then_show_uses_same_principal() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("agent");

        let mut init_output = Vec::new();
        cmd_init(&home, false, &mut init_output).expect("init");
        let init_json: serde_json::Value = serde_json::from_slice(&init_output).unwrap();
        assert!(home.join(IDENTITY_ENV_FILE).exists());
        assert!(home.join(LEGACY_NSEC_FILE).exists());
        assert!(init_json.get("secret_hex").is_none());
        assert!(init_json.get("nsec").is_none());

        let mut show_output = Vec::new();
        cmd_show(&home, true, &mut show_output).expect("show");
        let show_json: serde_json::Value = serde_json::from_slice(&show_output).unwrap();
        assert_eq!(show_json["account_id"], init_json["account_id"]);
        assert_eq!(show_json["npub"], init_json["npub"]);
        assert!(show_json["secret_hex"].as_str().is_some());
        assert!(
            show_json["nsec"]
                .as_str()
                .expect("nsec")
                .starts_with("nsec1")
        );
    }
}
