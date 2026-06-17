# Finite Chat

Finite Chat is an end-to-end encrypted chat and command transport for the
finite computer product. It is now its own standalone Rust workspace: clients
own MLS cryptography and local state, while the server owns ordering,
durability, idempotency, and delivery projections over opaque bytes.

The current product target is a server-ordered MLS delivery service that works
for humans, multiple devices, and agent runtimes. The server can sequence and
filter messages, release Welcomes, lease KeyPackages, and drive push/unread
policy, but it never reads message contents or acts as an identity authority.

## Decision

- Keep Nostr account keys as portable user identity.
- Use OpenMLS directly for room encryption, device membership, forward
  secrecy, and post-compromise recovery.
- Use one ordered HTTP delivery service per room in v1.
- Treat the server as trusted for ordering, durability, and availability, not
  message confidentiality or cryptographic membership truth.
- Integrate with `finitecomputer` behind the existing relay/gateway product
  shape before changing the UI surface.

## Workspace

- `crates/finitechat-transport`: shared transport IDs, envelopes, messages,
  timestamps, and opaque KeyPackage wrappers.
- `crates/finitechat-delivery`: in-memory ordered HTTP delivery service and
  executable conformance suite.
- `crates/finitechat-proto`: DTOs, message ids, membership deltas, and wire
  validation helpers.
- `crates/finitechat-blob`: encrypted attachment references and
  Blossom-compatible content-addressed blob-store proof.
- `crates/finitechat-mls`: OpenMLS helpers and finite device credentials.
- `crates/finitechat-client`: device state machine, runtime delivery adapter,
  sync/fanout workers, and encrypted SQLite snapshot store.
- `crates/finitechat-core`: persisted app/runtime facade shared by CLI,
  future daemon entrypoints, and UniFFI/iOS.
- `crates/finitechat-http`: shared HTTP route DTOs used by the server, CLI, and
  runtime delivery client.
- `crates/finitechat-server`: Axum HTTP route layer with optional SQLite
  operation-log replay and snapshots for local durability.
- `crates/finitechat-hermes`: typed JSON bridge contract for the Hermes
  platform plugin.
- `crates/finitechat-cli`: `finitechat` CLI for HTTP route calls, local smoke
  checks, Hermes bridge commands, and `finitechat core ...` app/runtime flows.
- `crates/finitechat-rmp`: Rust Multiplatform helper for UniFFI Swift binding,
  XCFramework, Xcode project, and simulator runs.
- `uniffi-bindgen`: local UniFFI bindgen binary used by the RMP helper.
- `ios`: minimal SwiftUI app shell built on the shared `finitechat-core`
  UniFFI surface.
- `integrations/hermes/finite-platform`: thin Hermes plugin over the Finite
  Chat bridge.

RMP/iOS work should use the
[RMP Architecture Bible](https://github.com/rust-multiplatform/rmp/blob/master/rmp-architecture-bible.md)
as the best-practices baseline: Rust owns app state, protocol logic,
persistence, networking, and policy; native layers stay thin and focused on
rendering or bounded platform capabilities.

## Local App Loop

Start the standalone HTTP service:

```sh
cargo run -p finitechat-server -- serve 127.0.0.1:8787 --sqlite .state/finitechat.sqlite3
```

Build and launch the iOS simulator app against that server:

```sh
FINITECHAT_SERVER_URL=http://127.0.0.1:8787 cargo run -p finitechat-rmp -- run ios
```

The RMP runner and Xcode project intentionally use the same configured bundle
identifier (`computer.finite.finitechat`). Do not add a debug-only suffix for
ordinary app testing: on iOS that creates a different app container, which makes
the local SQLite transcript look missing even though it is under the other
bundle id.

The normal app flow is intentionally chat-shaped:

1. Tap **New Room**.
2. Enter the room and tap **Invite**.
3. The inviter shows a QR/code URL and PIN.
4. Another device opens **Scan**, scans or pastes the invite URL or an `npub`,
   enters the PIN, and lands in the room once admitted.
5. Messages appear through the Rust-owned SSE hint loop. There is no user-facing
   sync, accept, or finalize step.

The `finitechat core ...` commands expose the underlying protocol pieces for
tests and low-level debugging. They are not the product flow. A developer smoke
sequence can still drive two local devices explicitly:

```sh
finitechat core --data-dir .state/alice --device-id alice bootstrap-room --room-id room-main
finitechat core --data-dir .state/alice --device-id alice invite --room-id room-main
finitechat core --data-dir .state/bob --device-id bob join --invite-url "$INVITE_URL" --pin "$PIN"
finitechat core --data-dir .state/alice --device-id alice accept --invite-url "$INVITE_URL"
finitechat core --data-dir .state/bob --device-id bob finalize --invite-url "$INVITE_URL"
finitechat core --data-dir .state/alice --device-id alice send --room-id room-main --text "hello"
finitechat core --data-dir .state/bob --device-id bob sync
```

## Checks

```sh
cargo test --workspace
cargo run -p finitechat-rmp -- doctor
cargo run -p finitechat-rmp -- bindings swift
uvx --no-config ruff format --check .
uvx --no-config ruff check .
uvx --no-config --with hermes-agent basedpyright
python3 -m unittest discover -s tests -p '*test*.py'
cargo test -p finitechat-server --test http_routes
cargo test -p finitechat-server --test http_persistence
cargo test -p finitechat-server --test http_conformance
cargo test -p finitechat-cli
cargo run -p finitechat-server -- smoke
cargo run -p finitechat-cli -- http-smoke
```

The high-risk coverage proves the server-side ordering and persistence
contract:

- one accepted Commit per room epoch;
- idempotent mutation retries;
- no Welcome before durable Commit;
- KeyPackage leases consumed only by accepted Commits;
- removed devices can sync through their removal Commit;
- invalid accepted Commits fail closed into `NeedsRepair`;
- durable SQLite replay matches the delivery conformance contract after
  restart.

## Ship Target

The first `finitecomputer` merge should be a library and local-loop
integration, not a dashboard rewrite:

1. vend or import these crates under `finitecomputer/crates/finitechat-*`;
2. add a `finitec encrypted-chat` or feature-flagged `chat.v2` path;
3. keep dashboard API routes stable while encrypted rooms shadow plaintext
   threads;
4. turn on encrypted payloads per project after the local loop passes.

See [docs/implementation-plan.md](docs/implementation-plan.md) for the full
phase plan.
