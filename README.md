# FiniteChat

FiniteChat is the native encrypted chat stack for Finite Computer. Rust owns
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

Start a local delivery server:

```sh
cargo run -p finitechat-server -- serve 127.0.0.1:8787 --sqlite .state/finitechat.sqlite3
```

Run the iOS simulator app against that server:

```sh
FINITECHAT_SERVER_URL=http://127.0.0.1:8787 cargo run -p finitechat-rmp -- run ios
```

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
```

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

This repo owns FiniteChat source. Production rollout mechanics belong in
`../finitecomputer`, which owns host sync, backups, Nix/k3s deployment,
`finited`, and runtime health checks.
