# Finite Chat

Finite Chat is the encryption and ordering layer for Finite Computer chat.

The current `finitecomputer` chat spine is intentionally useful but plaintext:
dashboard routes create typed relay events, `finitec relay run` polls outbound
from the runtime, and Hermes replies through `finitec gateway`. This repo keeps
that machine-outbound product boundary, then replaces the chat payload model
with a server-ordered MLS Delivery Service.

## Decision

Build a new Rust workspace, not a fork of Pika or Marmot.

- Keep Nostr account keys as portable user identity.
- Use MLS for room encryption, device membership, forward secrecy, and
  post-compromise recovery.
- Use one authoritative room server per room in v1.
- Treat the server as trusted for ordering and availability, not message
  confidentiality or membership-policy truth.
- Start with fake MLS in deterministic tests, then wire OpenMLS once ordering
  and recovery invariants are stable.
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

## First Checks

```sh
cargo test
python3 -m unittest discover -s tests -p '*test*.py'
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
