# Finite Chat Darkmatter

Finite Chat Darkmatter is the port of Finite Chat's product and test surface
onto the Marmot/Darkmatter protocol stack.

This repository starts from the existing Finite Chat source tree so the current
tests remain the acceptance surface. The implementation goal is to replace the
bespoke protocol internals with Darkmatter-backed engine, HTTP delivery, CLI,
and daemon/server code while keeping a running compatibility log.

The current `finitecomputer` chat spine is intentionally useful but plaintext:
dashboard routes create typed relay events, `finitec relay run` polls outbound
from the runtime, and Hermes replies through `finitec gateway`. This repo keeps
that machine-outbound product boundary, then replaces the chat payload model
with a server-ordered MLS Delivery Service.

## Decision

Build a new Rust workspace that uses Darkmatter as the protocol substrate and
keeps Finite-owned product behavior above that boundary.

- Keep Nostr account keys as portable user identity.
- Use Marmot/Darkmatter's OpenMLS engine for room encryption, device
  membership, forward secrecy, and post-compromise recovery where it can satisfy
  the Finite Chat tests.
- Use one ordered HTTP delivery service per room in v1, backed by Darkmatter's
  HTTP delivery work where possible.
- Treat the server as trusted for ordering and availability only where an
  explicit ordered-delivery profile says so, never message confidentiality or
  membership-policy truth.
- Keep fake-MLS reducer tests as compatibility fixtures until each behavior is
  proven through Darkmatter.
- Integrate with `finitecomputer` by adding an encrypted chat mode behind the
  existing relay/gateway shape before changing the UI surface.

## Workspace

- `crates/finitechat-proto`: DTOs, message ids, membership deltas, and wire
  validation helpers.
- `crates/finitechat-blob`: encrypted attachment references and
  Blossom-compatible content-addressed blob-store proof.
- `crates/finitechat-engine`: deterministic in-memory Delivery Service model.
- `crates/finitechat-store`: SQLite-backed server parity store.
- `crates/finitechat-client`: OpenMLS/Nostr client state machine.
- `crates/finitechat-sim`: executable scenario tests for protocol invariants.
- `crates/finitechat-hermes`: typed JSON bridge contract for the Hermes
  platform plugin.
- `crates/finitechat-http`: shared HTTP route DTOs used by the server, CLI, and
  runtime delivery client.
- `crates/finitechat-darkmatter`: small adapter layer that records which
  Darkmatter primitives are already usable from this repo.
- `crates/finitechat-server`: Axum HTTP route layer over Darkmatter's delivery
  service core, with optional SQLite operation-log replay for local durability.
- `crates/finitechat-cli`: CLI entrypoint for compatibility reports, local
  smoke checks, and HTTP delivery route calls.
- `integrations/hermes/finite-platform`: thin Hermes plugin over the Finite
  Chat bridge.
- `docs/implementation-plan.md`: concrete ship plan.
- `docs/finitecomputer-integration.md`: how this lands in `../finitecomputer`.
- `docs/hermes-integration.md`: Hermes plugin ownership, bridge commands, and
  test contract.
- `docs/source-notes.md`: source-of-truth notes from Justin's planning repo,
  Pika/Marmot, and finitecomputer.
- `docs/scenario-coverage.md`: named simulator scenarios proven so far.
- `docs/storage-plan.md`: SQLite/Postgres/client-store decision record.
- `docs/engineering-style.md`: local rules for debt, asserts, and invariants.
- `docs/technical-debt-ledger.md`: observed finitecomputer integration debt,
  risks, proofs, and delete conditions.
- `docs/daemon-survival-testing.md`: strategy for proving chat/status/recovery
  still work when Hermes, inference, or bridge adapters are down.
- `docs/darkmatter-port-log.md`: running compatibility log for out-of-box,
  easy-owned, thick/wonky, and fork-required work.

## First Checks

```sh
cargo test
python3 -m unittest discover -s tests -p '*test*.py'
cargo test -p finitechat-server --test http_routes
cargo test -p finitechat-server --test http_persistence
cargo test -p finitechat-server --test http_engine_routes
cargo test -p finitechat-cli
cargo run -p finitechat-server -- smoke
cargo run -p finitechat-cli -- http-smoke
```

The fake simulator tests prove the server-side ordering and persistence
contract that Marmot-over-relays could not make reliable:

- one accepted Commit per room epoch;
- idempotent mutation retries;
- no Welcome before durable Commit;
- KeyPackage leases consumed only by accepted Commits;
- removed devices can sync through their removal Commit;
- invalid accepted Commits fail closed into `NeedsRepair`.

The SQLite suite replays the highest-risk reducer scenarios across reopen
boundaries, including accepted and rejected idempotency results. The OpenMLS
suite proves the first real credential, Welcome, Commit, and application-message
bytes through the same ordering path.

## Ship Target

The first finitecomputer merge should be a library and local-loop integration,
not a dashboard rewrite:

1. vend or import these crates under `finitecomputer/crates/finitechat-*`;
2. add a `finitec encrypted-chat` or feature-flagged `chat.v2` path;
3. keep dashboard API routes stable while encrypted rooms shadow plaintext
   threads;
4. turn on encrypted payloads per project after the local loop passes.

See [docs/implementation-plan.md](docs/implementation-plan.md) for the full
phase plan.
