# Finite Chat

Finite Chat is the native encrypted chat stack for Finite Computer. Rust owns
protocol state, persistence, networking, and product policy; SwiftUI renders the
Rust-owned app state and dispatches typed actions.

The v1 product shape is a phone chat app for people and agents:

- Nostr keys provide portable account identity and profile discovery.
- OpenMLS protects room contents and membership truth.
- The HTTP server orders opaque encrypted payloads, persists delivery state, and
  never reads message contents.
- Offline text sends are durable, explicit retry is required after failure, and
  attachment upload stays online-only.
- Hermes integration uses the same chat surface as human conversations.

## Repository Map

- `crates/finitechat-core` - Rust app/runtime facade used by CLI and iOS.
- `crates/finitechat-client` - device state machine and encrypted local store.
- `crates/finitechat-server` - Axum HTTP delivery server with SQLite durability.
- `crates/finitechat-proto` / `finitechat-http` - wire DTOs and route contracts.
- `crates/finitechat-mls` - OpenMLS helpers and finite device credentials.
- `crates/finitechat-cli` - local smoke, server calls, and Hermes bridge tools.
- `crates/finitechat-rmp` - UniFFI, XCFramework, Xcode, and simulator helper.
- `ios` - SwiftUI app shell for `computer.finite.finitechat`.
- `integrations/hermes/finite-platform` - Hermes platform plugin adapter.
- `docs/adr` and `docs/protocol-v1.md` - current product/protocol decisions.

## Local Loop

The production/default app server is `https://chat.finite.computer`. Local
server URLs are explicit development and test overrides only.

For a friend self-building the native app on their own Mac and phone, start
with `docs/friends-alpha-self-build.md`. That runbook covers branch checkout,
generated iOS bindings/project files, Apple signing, clean physical-device
install, and confirming the app is using the deployed server instead of a local
development override.

For server iteration or local automated testing, start a local delivery server:

```sh
cargo run -p finitechat-server -- serve 127.0.0.1:8787 --sqlite .state/finitechat.sqlite3
```

Run the iOS simulator app against that server with an explicit override:

```sh
FINITECHAT_SERVER_URL=http://127.0.0.1:8787 cargo run -p finitechat-rmp -- run ios
```

To test the iOS app surface with a real local Hermes gateway, use the bundled
runner instead of the plain server command. This is a low-level local runner,
not the physical-phone canary gate:

```sh
scripts/hermes-real-gateway-demo.sh
```

In another terminal, point the simulator app at the runner's local server:

```sh
FINITECHAT_SERVER_URL=http://127.0.0.1:18788 cargo run -p finitechat-rmp -- run ios
```

The Hermes runner needs a prepared Hermes checkout with a `.venv`; set
`FINITECHAT_HERMES_REPO=/path/to/hermes-agent` if it is not in the default
finitecomputer checkout location. It also needs the model provider key used by
the Hermes profile. The runner loads `.env` when present, or set
`FINITECHAT_HERMES_ENV_FILE=/path/to/provider.env`.

For the hardened "fresh Hermes instance to Paul's phone" quality loop, use
`docs/hermes-phone-canary-loop.md`. That runbook defines the local phone,
remote Docker, and Tinfoil promotion gates and the evidence required before a
human invite is handed out.

For team testing, the normal Hermes phone canary is:

```sh
cp .env.example .env
# Fill in one model provider key in .env, usually OPENROUTER_API_KEY.
xcrun devicectl list devices
scripts/hermes-phone-canary.py \
  --install-phone-app \
  --ios-device <device identifier or hardware UDID> \
  --ios-development-team <Apple team id> \
  --keep-running
```

The script uses `https://chat.finite.computer`, builds the current
`finitechat` binary, installs the current iOS app on the paired phone, starts
real Hermes 0.17 with the `finite-platform` plugin, proves invite admission
with a throwaway client, requires a real model reply, then prints the human
invite URL, report path, and `stop.sh`. Do not hand an invite to a
human from lower-level scripts that have not produced a passed report.

Remote Docker is the next promotion layer for teammates with access to the
builder host:

```sh
scripts/hermes-remote-docker-canary.py --keep-running
```

That wrapper requires a passed local phone report by default, builds the real
runtime image on `ssh://finite-lat-2`, proves real Hermes chat before and after
entrypoint backup/restore, and only then prints the invite URL for the restored
container.

The normal app flow is:

1. Sign in with an `nsec` or create a local Nostr identity.
2. Use **People** to open an existing profile or **Scan** to scan/paste an
   invite URL or `npub`.
3. Chat from the room surface. Rust owns send state, retry state, delivery
   projection, and attachment download decisions.

## Checks

Fast Rust/server checks:

```sh
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test --workspace
cargo test -p finitechat-server --test http_routes
cargo test -p finitechat-server --test http_persistence
cargo test -p finitechat-server --test http_conformance
```

iOS checks:

```sh
cargo run -p finitechat-rmp -- doctor
cargo run -p finitechat-rmp -- bindings swift
cargo run -p finitechat-rmp -- test ios-simulator
```

`finitechat-rmp test ios-simulator` owns the simulator test lifecycle: it
creates or reuses a dedicated RMP simulator, shuts it down, erases it, runs the
full `FiniteChat` test scheme with isolated derived data and `.xcresult` output
under `.state`, then terminates and shuts the simulator down. Use `--json` when
automation needs the resolved UDID and result bundle path.

Hermes/Python checks:

```sh
uvx --no-config ruff format --check .
uvx --no-config ruff check .
uvx --no-config --with hermes-agent basedpyright
python3 -m unittest discover -s tests -p '*test*.py'
```

## Publish Safety

The repo is intended to publish as `finitecomputer/finitechat`.

Tracked source excludes local and generated state:

- `.env`, key files, SQLite stores, and `.state/` are ignored.
- `target/`, generated Xcode projects, Swift bindings, and XCFrameworks are
  ignored.
- iOS signing uses `ios/project.yml`; the generated `.xcodeproj` is local.

Before pushing, verify the GitHub target is the new repo. If
`finitecomputer/finitechat` resolves to `finitecomputer/finitechat-old`, do not
push or force-push; create or restore the new `finitecomputer/finitechat` repo
first.

## Deployment

This repo owns the Finite Chat server source, HTTP contract, and release gate
for `https://chat.finite.computer`. Production rollout mechanics belong in
`../finitecomputer`, which owns host sync, backups, Nix/k3s deployment,
`finited`, and runtime health checks. Do not ship a native app/TestFlight build
that depends on server behavior until the deployed chat server has been
verified against the finite-chat commit being shipped.

The production health endpoint must identify the deployed server build:

```sh
cargo run -q -p finitechat-cli -- http --server https://chat.finite.computer health
```

Expected production output includes `status: "ok"`, `server_version`,
`source_commit`, and `source_dirty: false`. If `source_commit` is missing,
the production server is an old build and the app release is blocked until
`../finitecomputer` deploys a compatible finite-chat commit. See
`docs/server-deployment-gate.md` for the required handoff and verification
steps.

For iOS beta distribution, see `docs/testflight-runbook.md`. Finite Chat uses
bundle ID `computer.finite.finitechat`.
