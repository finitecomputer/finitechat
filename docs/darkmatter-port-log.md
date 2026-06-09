# Darkmatter Port Log

This repo starts from the existing `finitechat` source tree so the current API,
docs, and tests remain the acceptance surface while the implementation moves to
Marmot/Darkmatter.

## Source State

- New repo: `/Users/futurepaul/dev/finite/finite-chat-darkmatter`
- Baseline source: `/Users/futurepaul/dev/finite/finitechat`
- Darkmatter source: `/Users/futurepaul/dev/finite/darkmatter`
- Darkmatter branch used for initial compatibility work:
  `codex/http-delivery-service-spike`

## Test Inventory To Port

Current copied acceptance surface:

- Rust tests: `287`
- Python Hermes adapter tests: `7`

Rust test distribution:

| File | Count |
| --- | ---: |
| `crates/finitechat-blob/src/lib.rs` | 17 |
| `crates/finitechat-client/tests/client_state.rs` | 31 |
| `crates/finitechat-engine/src/lib.rs` | 7 |
| `crates/finitechat-hermes/src/lib.rs` | 9 |
| `crates/finitechat-mls/src/lib.rs` | 14 |
| `crates/finitechat-proto/src/lib.rs` | 62 |
| `crates/finitechat-sim/tests/daemon_survival.rs` | 21 |
| `crates/finitechat-sim/tests/finitecomputer_boundary.rs` | 4 |
| `crates/finitechat-sim/tests/scenarios.rs` | 78 |
| `crates/finitechat-store/src/lib.rs` | 1 |
| `crates/finitechat-store/tests/sqlite_scenarios.rs` | 43 |

Python test distribution:

| File | Count |
| --- | ---: |
| `tests/hermes/test_finite_platform_adapter.py` | 7 |

## What Works Out Of The Box

- Darkmatter's HTTP delivery service core can sequence opaque group
  `TransportMessage` bytes, reject a second commit for the same source epoch,
  sync bounded pages, and claim owner-scoped KeyPackages once.
- Darkmatter's existing Marmot engine and Nostr peeler can produce real Welcome,
  invite Commit, and application messages that pass through the HTTP delivery
  service core and are ingested by recipients.
- The copied Finite Chat application-policy tests can remain above the encrypted
  application payload boundary. Push, unread, command-inbox, runtime-state, and
  activity projection logic does not need the server to decrypt payloads.

## Easy Logic For This Repo To Own

- Product-level DTOs for conversations, topics, segments, activity, runtime
  state, and Hermes bridge JSON.
- CLI/daemon command surfaces that call into the Darkmatter-backed client.
- Push/unread/command-inbox projections from decrypted application events.
- Thin server route DTOs and idempotency wrappers around the HTTP delivery
  service core, as long as the underlying state transition already exists.

## Thick Or Wonky Logic

- Durable Welcome claim/ack recovery. Finite tests require claimed Welcome bytes
  and ratchet-tree material to survive restart before activation and ack.
- Later-device fanout into existing rooms. Finite tests require distinct
  KeyPackages per room, persistent fanout plans, response-loss retry, and
  reprepare after same-epoch loss.
- Mapping Finite's server cursor, idempotency records, and repair states onto
  Darkmatter's engine/storage model without duplicating protocol state.
- Replacing the current fake-MLS reducer tests while preserving their
  transaction and replay assertions.

## Requires A Darkmatter Fork Until Upstreamed

- `DeliveryProfile::DangerouslyTrustServerOrdering`, or a safer upstream name
  for the same ordered-delivery profile. The current branch-local hook bypasses
  distributed convergence only for the next expected server-admitted Commit.
- Any HTTP delivery profile text that lets transport/server ordering influence
  canonical branch choice. Marmot's current distributed profile deliberately
  treats transport delivery as evidence, not consensus.

## First Port Checkpoint

Added `finitechat-darkmatter`, `finitechat-cli`, and `finitechat-server`
workspace members:

- `finitechat-darkmatter` compiles against the local Darkmatter branch and
  proves the HTTP delivery core orders one admitted Commit followed by one app
  message.
- `finitechat-cli` exposes `compat-report` and `http-smoke` commands.
- `finitechat-server` is only an executable shell over the in-memory delivery
  core. Socket-level HTTP routes, auth, persistence, and production server
  behavior remain unported.

Verified after adding the Darkmatter dependency graph:

- `cargo test --workspace`: pass
- `python3 -m unittest discover -s tests -p '*test*.py'`: pass
- `cargo clippy --workspace --all-targets -- -D warnings`: pass
- `cargo run -p finitechat-cli -- compat-report`: pass
- `cargo run -p finitechat-server`: pass

Dependency note:

- The copied Finite Chat workspace used `rusqlite 0.37`. Adding Darkmatter
  pulls the workspace toward `rusqlite 0.32` through OpenMLS/Darkmatter's
  SQLite dependency graph, so this port repo aligns its workspace `rusqlite`
  version to `0.32` to avoid two `libsqlite3-sys` packages linking `sqlite3`.

Next meaningful gate: make the server crate expose actual HTTP routes backed by
the Darkmatter service core, then move the copied server reducer tests onto that
route/store boundary.
