//! The `finitechat auth` subcommand family: the CLI surface over the shared
//! Finite identity (Finite Identity Contract v1).
//!
//! The account key lives at `$FINITE_HOME/identity/identity.json` (or
//! `~/.finite/identity/identity.json`) and is shared by every Finite tool.
//! `auth status` reports the identity; `auth import` writes an existing nsec
//! into the shared location (the only storage location — finitechat keeps no
//! copy of the secret in its own stores).

use std::io::{Read, Write};
use std::path::Path;

use finite_identity::{FiniteIdentity, IdentityPaths};
use finitechat_mls::{NOSTR_SECRET_KEY_BYTES, NostrSecretKey};
use finitechat_proto::nsec_decode;
use serde::Serialize;

use crate::{CliError, reject_extra_args, take_option, take_positional, write_pretty_json};

#[derive(Debug, Serialize, PartialEq, Eq)]
struct AuthStatusSummary {
    identity_file: String,
    account_id: String,
    npub: String,
    created_at: String,
    created_by: String,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
struct AuthImportSummary {
    identity_file: String,
    account_id: String,
    npub: String,
    imported: bool,
}

pub(crate) fn run<W: Write>(mut args: Vec<String>, output: &mut W) -> Result<(), CliError> {
    let Some(command) = take_positional(&mut args) else {
        return Err(CliError::Usage(usage()));
    };
    match command.as_str() {
        "status" => {
            reject_extra_args(&args)?;
            cmd_status(output)
        }
        "import" => {
            let file = take_option(&mut args, "--file")?;
            reject_extra_args(&args)?;
            cmd_import(file.as_deref(), &mut std::io::stdin(), output)
        }
        _ => Err(CliError::Usage(usage())),
    }
}

fn cmd_status<W: Write>(output: &mut W) -> Result<(), CliError> {
    let paths = identity_paths()?;
    let identity = FiniteIdentity::load(&paths).map_err(|error| {
        CliError::Identity(format!(
            "no usable Finite identity at {}: {error} (run any Finite tool to mint one, or `finitechat auth import`)",
            paths.identity_file().display()
        ))
    })?;
    write_pretty_json(
        output,
        &AuthStatusSummary {
            identity_file: paths.identity_file().display().to_string(),
            account_id: identity.public_key_hex().to_owned(),
            npub: identity.npub(),
            created_at: identity.created_at().to_owned(),
            created_by: identity.created_by().to_owned(),
        },
    )
}

fn cmd_import<W: Write>(
    file: Option<&str>,
    stdin: &mut impl Read,
    output: &mut W,
) -> Result<(), CliError> {
    // The secret is read from stdin or a file, never from an argv value
    // (argv leaks through process listings and shell history).
    let raw = match file {
        Some(path) => std::fs::read_to_string(path)
            .map_err(|error| CliError::Identity(format!("failed to read {path}: {error}")))?,
        None => {
            let mut buffer = String::new();
            stdin
                .read_to_string(&mut buffer)
                .map_err(|error| CliError::Identity(format!("failed to read stdin: {error}")))?;
            buffer
        }
    };
    let secret = parse_secret_input(&raw)?;

    let paths = identity_paths()?;
    write_imported_identity(&paths, &secret)?;

    // Verify by loading through the crate (fails closed on any mismatch).
    let identity = FiniteIdentity::load(&paths).map_err(|error| {
        CliError::Identity(format!("imported identity failed to load: {error}"))
    })?;
    write_pretty_json(
        output,
        &AuthImportSummary {
            identity_file: paths.identity_file().display().to_string(),
            account_id: identity.public_key_hex().to_owned(),
            npub: identity.npub(),
            imported: true,
        },
    )
}

pub(crate) fn identity_paths() -> Result<IdentityPaths, CliError> {
    IdentityPaths::resolve().map_err(|error| CliError::Identity(error.to_string()))
}

fn parse_secret_input(raw: &str) -> Result<NostrSecretKey, CliError> {
    let trimmed = raw.trim();
    let trimmed = trimmed.strip_prefix("nostr:").unwrap_or(trimmed);
    let secret_hex = if trimmed.starts_with("nsec1") {
        nsec_decode(trimmed)
            .map_err(|reason| CliError::Identity(format!("invalid nsec: {reason}")))?
    } else {
        trimmed.to_owned()
    };
    let bytes = crate::parse_hex("account secret", &secret_hex)?;
    let bytes: [u8; NOSTR_SECRET_KEY_BYTES] = bytes.try_into().map_err(|_| {
        CliError::Identity("account secret must be an nsec or 32 bytes of hex".to_owned())
    })?;
    NostrSecretKey::from_bytes(bytes)
        .map_err(|error| CliError::Identity(format!("invalid account secret: {error}")))
}

/// Write contract-v1 `identity.json` with exclusive create (refuses to
/// overwrite an existing identity) and 0600/0700 permissions.
///
/// TODO(finite-identity#2): replace this hand-written writer with
/// `FiniteIdentity::import` once the crate grows an import primitive, then
/// bump the pinned rev.
fn write_imported_identity(paths: &IdentityPaths, secret: &NostrSecretKey) -> Result<(), CliError> {
    create_identity_root(paths.root())?;
    let identity_file = paths.identity_file();
    if identity_file.exists() {
        return Err(CliError::Identity(format!(
            "refusing to overwrite existing identity at {} (move it aside by hand first)",
            identity_file.display()
        )));
    }

    let secret_hex = hex_lower(secret.as_bytes());
    let public_key_hex = hex_lower(secret.public_key().as_bytes());
    let created_at = rfc3339_now()?;
    let wire = serde_json::json!({
        "version": 1,
        "kind": "nostr-secp256k1",
        "secret_hex": secret_hex,
        "public_key_hex": public_key_hex,
        "created_at": created_at,
        "created_by": concat!("finitechat ", env!("CARGO_PKG_VERSION"), " (auth import)"),
    });
    let mut json =
        serde_json::to_string_pretty(&wire).expect("identity wire format always serializes");
    json.push('\n');

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&identity_file).map_err(|error| {
        CliError::Identity(format!(
            "failed to create {}: {error}",
            identity_file.display()
        ))
    })?;
    file.write_all(json.as_bytes())
        .and_then(|()| file.sync_all())
        .map_err(|error| {
            let _ = std::fs::remove_file(&identity_file);
            CliError::Identity(format!(
                "failed to write {}: {error}",
                identity_file.display()
            ))
        })?;
    Ok(())
}

fn create_identity_root(root: &Path) -> Result<(), CliError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(root)
            .map_err(|error| {
                CliError::Identity(format!("failed to create {}: {error}", root.display()))
            })
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(root).map_err(|error| {
            CliError::Identity(format!("failed to create {}: {error}", root.display()))
        })
    }
}

fn rfc3339_now() -> Result<String, CliError> {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|error| CliError::Identity(format!("timestamp formatting failed: {error}")))
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

pub(crate) fn usage() -> String {
    "auth commands:\n  finitechat auth status\n  finitechat auth import [--file PATH]   (reads an nsec or 64-hex secret from PATH or stdin)\n  (identity location: $FINITE_HOME/identity/identity.json, else ~/.finite/identity/identity.json)".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use finitechat_proto::{npub_encode, nsec_encode};

    fn secret() -> NostrSecretKey {
        NostrSecretKey::from_bytes([0x17; NOSTR_SECRET_KEY_BYTES]).unwrap()
    }

    fn secret_hex() -> String {
        hex_lower(secret().as_bytes())
    }

    #[test]
    fn import_writes_contract_v1_identity_and_verifies() {
        let dir = tempfile::tempdir().unwrap();
        let paths = IdentityPaths::with_finite_home(dir.path());

        write_imported_identity(&paths, &secret()).expect("import writes");
        let identity = FiniteIdentity::load(&paths).expect("crate loads imported identity");
        assert_eq!(
            identity.public_key_hex(),
            hex_lower(secret().public_key().as_bytes())
        );
        assert_eq!(
            identity.npub(),
            npub_encode(identity.public_key_hex()).unwrap()
        );
        assert!(identity.created_by().contains("auth import"));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(paths.identity_file())
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn import_refuses_to_overwrite_existing_identity() {
        let dir = tempfile::tempdir().unwrap();
        let paths = IdentityPaths::with_finite_home(dir.path());
        let existing = FiniteIdentity::load_or_generate(&paths, "test/0.0.0").unwrap();

        let error = write_imported_identity(&paths, &secret()).expect_err("refuses overwrite");
        assert!(error.to_string().contains("refusing to overwrite"));

        let reloaded = FiniteIdentity::load(&paths).unwrap();
        assert_eq!(reloaded.public_key_hex(), existing.public_key_hex());
    }

    #[test]
    fn secret_input_accepts_nsec_and_hex_forms() {
        let hex_form = secret_hex();
        let nsec_form = nsec_encode(&hex_form).unwrap();

        let from_hex = parse_secret_input(&format!("{hex_form}\n")).unwrap();
        let from_nsec = parse_secret_input(&format!("  {nsec_form}  ")).unwrap();
        let from_prefixed = parse_secret_input(&format!("nostr:{nsec_form}")).unwrap();
        assert_eq!(from_hex, secret());
        assert_eq!(from_nsec, secret());
        assert_eq!(from_prefixed, secret());

        assert!(parse_secret_input("nsec1notvalid").is_err());
        assert!(parse_secret_input("abc123").is_err());
    }
}
