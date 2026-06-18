use std::io::Write;

use finitechat_core::{AppAction, FiniteChatRuntime, OpenOptions};

use crate::{
    CliError, DEFAULT_SERVER_URL, parse_u64, reject_extra_args, take_option, take_positional,
    write_pretty_json,
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

    let runtime = FiniteChatRuntime::open(OpenOptions {
        data_dir,
        server_url: server,
        device_id,
        account_secret_hex,
        now_unix_seconds,
    })
    .map_err(map_core_error)?;

    match command.as_str() {
        "state" => {
            let start_runtime = take_flag(&mut args, "--start-runtime");
            let room_id = take_option(&mut args, "--room-id")?;
            reject_extra_args(&args)?;
            let mut state = if start_runtime {
                runtime
                    .dispatch(AppAction::StartRuntime)
                    .map_err(map_core_error)?
            } else {
                runtime.state().map_err(map_core_error)?
            };
            if let Some(room_id) = room_id {
                state = runtime
                    .dispatch(AppAction::OpenRoom { room_id })
                    .map_err(map_core_error)?;
            }
            write_pretty_json(output, &state)
        }
        _ => Err(CliError::Usage(usage())),
    }
}

pub(crate) fn usage() -> String {
    "app commands:\n  finitechat app [--data-dir DIR] [--server URL] [--device-id ID] [--account-secret-hex HEX] [--now SECONDS] state [--start-runtime] [--room-id ID]".to_owned()
}

fn map_core_error(error: finitechat_core::FiniteChatCoreError) -> CliError {
    CliError::Core(error.to_string())
}

fn take_flag(args: &mut Vec<String>, name: &str) -> bool {
    if let Some(index) = args.iter().position(|arg| arg == name) {
        args.remove(index);
        true
    } else {
        false
    }
}
