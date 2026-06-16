use std::io::Write;

use finitechat_core::{FiniteChatCore, OpenOptions};

use crate::{
    CliError, DEFAULT_SERVER_URL, parse_u64, reject_extra_args, required_option, take_option,
    take_positional, write_pretty_json,
};

const DEFAULT_DATA_DIR: &str = ".finitechat";
const DEFAULT_DEVICE_ID: &str = "cli";

pub(crate) fn run<W: Write>(mut args: Vec<String>, output: &mut W) -> Result<(), CliError> {
    let data_dir = take_option(&mut args, "--data-dir")?.unwrap_or_else(|| DEFAULT_DATA_DIR.into());
    let server =
        take_option(&mut args, "--server")?.unwrap_or_else(|| DEFAULT_SERVER_URL.to_owned());
    let device_id =
        take_option(&mut args, "--device-id")?.unwrap_or_else(|| DEFAULT_DEVICE_ID.into());
    let account_secret_hex = take_option(&mut args, "--account-secret-hex")?;
    let now_unix_seconds = take_option(&mut args, "--now")?
        .map(|value| parse_u64("--now", &value))
        .transpose()?;
    let Some(command) = take_positional(&mut args) else {
        return Err(CliError::Usage(usage()));
    };

    let core = FiniteChatCore::open(OpenOptions {
        data_dir,
        server_url: server,
        device_id,
        account_secret_hex,
        now_unix_seconds,
    })
    .map_err(map_core_error)?;

    match command.as_str() {
        "identity" => {
            reject_extra_args(&args)?;
            write_pretty_json(output, &core.identity().map_err(map_core_error)?)
        }
        "bootstrap-room" => {
            let mut args = args;
            let room_id = required_option(&mut args, "--room-id")?;
            let display_name = take_option(&mut args, "--display-name")?;
            reject_extra_args(&args)?;
            write_pretty_json(
                output,
                &core
                    .bootstrap_room(room_id, display_name)
                    .map_err(map_core_error)?,
            )
        }
        "invite" => {
            let mut args = args;
            let room_id = required_option(&mut args, "--room-id")?;
            let display_name = take_option(&mut args, "--display-name")?;
            reject_extra_args(&args)?;
            write_pretty_json(
                output,
                &core
                    .create_invite(room_id, display_name)
                    .map_err(map_core_error)?,
            )
        }
        "pin" => {
            let mut args = args;
            let invite_url = required_option(&mut args, "--invite-url")?;
            reject_extra_args(&args)?;
            writeln!(
                output,
                "{}",
                core.current_invite_pin(invite_url)
                    .map_err(map_core_error)?
            )
            .map_err(CliError::Output)
        }
        "join" => {
            let mut args = args;
            let invite_url = required_option(&mut args, "--invite-url")?;
            let pin = required_option(&mut args, "--pin")?;
            let display_name = take_option(&mut args, "--display-name")?;
            reject_extra_args(&args)?;
            write_pretty_json(
                output,
                &core
                    .join_invite(invite_url, pin, display_name)
                    .map_err(map_core_error)?,
            )
        }
        "accept" => {
            let mut args = args;
            let invite_url = required_option(&mut args, "--invite-url")?;
            reject_extra_args(&args)?;
            write_pretty_json(
                output,
                &core
                    .accept_invite_joins(invite_url)
                    .map_err(map_core_error)?,
            )
        }
        "finalize" => {
            let mut args = args;
            let invite_url = required_option(&mut args, "--invite-url")?;
            reject_extra_args(&args)?;
            core.finalize_invite(invite_url).map_err(map_core_error)?;
            writeln!(output, "{{\"ok\":true}}").map_err(CliError::Output)
        }
        "send" => {
            let mut args = args;
            let room_id = required_option(&mut args, "--room-id")?;
            let text = required_option(&mut args, "--text")?;
            reject_extra_args(&args)?;
            write_pretty_json(
                output,
                &core.send_text(room_id, text).map_err(map_core_error)?,
            )
        }
        "sync" => {
            reject_extra_args(&args)?;
            write_pretty_json(output, &core.sync().map_err(map_core_error)?)
        }
        _ => Err(CliError::Usage(usage())),
    }
}

pub(crate) fn usage() -> String {
    "core commands:\n  finitechat core [--data-dir DIR] [--server URL] [--device-id ID] [--account-secret-hex HEX] [--now SECONDS] identity\n  finitechat core [options] bootstrap-room --room-id ID [--display-name NAME]\n  finitechat core [options] invite --room-id ID [--display-name NAME]\n  finitechat core [options] pin --invite-url URL\n  finitechat core [options] join --invite-url URL --pin PIN [--display-name NAME]\n  finitechat core [options] accept --invite-url URL\n  finitechat core [options] finalize --invite-url URL\n  finitechat core [options] send --room-id ID --text TEXT\n  finitechat core [options] sync".to_owned()
}

fn map_core_error(error: finitechat_core::FiniteChatCoreError) -> CliError {
    CliError::Core(error.to_string())
}
