# Darkmatter Port Log

This repo starts from the existing `finitechat` source tree so the current API,
docs, and tests remain the acceptance surface while the implementation moves to
Marmot/Darkmatter.

## Source State

- New repo: `/Users/futurepaul/dev/finite/finite-chat-darkmatter`
- Baseline source: `/Users/futurepaul/dev/finite/finitechat`
- Darkmatter source: `/Users/futurepaul/dev/finite/darkmatter`
- Darkmatter HTTP delivery branch checked out locally in the source tree above.

## Test Inventory To Port

Current copied acceptance surface:

- Copied Rust tests at repo creation: `287`
- Current copied/application Rust tests after Darkmatter HTTP harness additions:
  `296`
- Current Rust tests overall, including HTTP route/CLI adapter tests: `329`
- Python Hermes adapter tests: `7`

Rust test distribution:

| File | Count |
| --- | ---: |
| `crates/finitechat-blob/src/lib.rs` | 17 |
| `crates/finitechat-client/tests/client_state.rs` | 40 |
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
- A thin Axum route layer can expose that service core without extra protocol
  logic. The current route tests cover group publish/sync, exact duplicate
  replay, same-epoch commit conflict, inbox publish/sync, and single-use
  KeyPackage claims.
- A SQLite operation log can replay accepted HTTP delivery operations into a
  fresh Darkmatter service core after restart. The current persistence tests
  prove group queue order, duplicate replay, same-epoch commit admission, and
  consumed KeyPackage state survive restart. KeyPackage inventory is rebuilt
  from that operation log and checkpointed as a query-side table.
- The HTTP `/messages` route now accepts an optional idempotency key. Matching
  retries replay the original receipt after restart, and same-key retries with
  a different target/message conflict without appending a second delivery.
- The HTTP Welcome wrapper can claim Welcome inbox messages, hide already
  claimed messages from duplicate claims, and persist activated or failed ack
  terminal state across restart.
- The HTTP KeyPackage wrapper can claim one package per explicit device owner
  in a batch and replay the exact batch response by idempotency key after
  restart.
- The HTTP KeyPackage wrapper can also expose available/claimed inventory for
  an owner. This lets the runtime KeyPackage replenishment worker run over the
  Darkmatter HTTP boundary without teaching the server Finite-specific device
  structure.
- The HTTP fanout wrapper can persist opaque later-device fanout room plans,
  prepared message ids, reprepare checkpoints, and accepted sequence markers
  across restart without teaching the server MLS semantics.
- The HTTP account-room directory wrapper can persist opaque account-room
  records, page them by room id, and reload them from SQLite. This gives the
  runtime link-fanout discovery loop a Darkmatter HTTP boundary, as long as a
  product-owned membership projection writes the directory.
- The runtime link-fanout worker can complete a one-room later-device happy
  path over the HTTP adapter when the initial room log and account-room
  directory are seeded: discover the room, claim the target device's KeyPackage,
  publish the add-device Commit through Darkmatter HTTP, sync the Commit back,
  and let the later device claim and activate the released Welcome.
- The same one-room HTTP fanout path can retry a lost submit response from the
  persisted prepared state. The commit publish and Welcome publish are
  idempotent, so retry completes without appending a duplicate group entry or
  delivering duplicate Welcomes.
- The HTTP fanout path also handles the worker's multi-room shape: paged
  account-room discovery across two rooms, two distinct target KeyPackage
  claims, two submitted commits, two completion syncs, and two later-device
  Welcome activations.
- The HTTP fanout path can reprepare after a same-epoch race: a fanout submit
  fails before accept, a competing member commit wins the epoch, the client
  syncs that winner and clears its pending commit, then the worker reprepares
  and submits the fanout commit at the next epoch.
- Darkmatter's existing Marmot engine and Nostr peeler can produce real Welcome,
  invite Commit, and application messages that pass through the HTTP delivery
  service core and are ingested by recipients.
- The `finitechat-server` Axum route layer can carry those real Marmot Welcome,
  invite Commit, and application messages end to end when driven by
  Darkmatter's conformance simulator clients.
- The copied Finite Chat application-policy tests can remain above the encrypted
  application payload boundary. Push, unread, command-inbox, runtime-state, and
  activity projection logic does not need the server to decrypt payloads.

## Easy Logic For This Repo To Own

- Product-level DTOs for conversations, topics, segments, activity, runtime
  state, and Hermes bridge JSON.
- CLI/daemon command surfaces that call into the Darkmatter-backed client.
- Push/unread/command-inbox projections from decrypted application events.
- Public server DTO polish, auth, rate limits, and additional idempotency
  wrappers around the HTTP delivery service core, as long as the underlying
  state transition already exists.
- Welcome claim/ack recovery for the HTTP delivery surface. This is route/store
  wrapper state over Darkmatter Welcome inbox messages, not a Darkmatter fork.
- Runtime Welcome payload mapping. The Darkmatter HTTP inbox carries opaque
  transport messages; the client adapter can decode a product-level
  `WelcomeRecord` from the payload, activate it locally, and ack the transport
  message id.
- Runtime room-log payload mapping. The Darkmatter HTTP group queue carries
  opaque transport messages; the client adapter can decode product-level
  `RoomLogEntry` payloads and reuse the existing encrypted application apply
  path.
- Runtime KeyPackage metadata mapping. The Darkmatter HTTP KeyPackage store
  carries opaque bytes; the client adapter can encode the original
  `UploadKeyPackageRequest` so a later claim reconstructs Finite's package
  ref, hash, payload, owner, and deterministic lease token.
- Batch KeyPackage claim replay for the HTTP delivery surface. This wraps
  Darkmatter's owner-scoped `claim_key_package` primitive so fanout callers can
  ask for one package per device owner and safely retry after response loss.
- KeyPackage inventory projection for the HTTP delivery surface. This mirrors
  Darkmatter's available/consumed package state as available/claimed counts, so
  runtime clients can replenish toward a target without listing package bytes.
- Opaque fanout plan checkpointing for the HTTP delivery surface. This stores
  the coordination fields a client worker needs to resume after restart or
  response loss, while leaving MLS validation and local pending Commit state on
  the client.
- Account-room discovery projection for the HTTP delivery surface. This stores
  opaque current-room membership snapshots keyed by account and room id, while
  leaving the actual source of membership truth outside Darkmatter's transport
  core.
- Runtime submit-commit mapping for the HTTP delivery surface. The adapter can
  serialize a Finite `RoomLogEntry` into a Darkmatter HTTP group message,
  publish derived `WelcomeRecord`s to recipient inboxes, and reconstruct
  `CommitAccepted` from the HTTP receipt.
- Moving route DTOs into a shared protocol crate. The current CLI imports the
  server crate's DTOs directly so the route client cannot drift, but that is
  not the right long-term crate boundary.
- Public byte encoding for opaque IDs and payloads. The current CLI maps string
  arguments directly to bytes for local testing.

## Thick Or Wonky Logic

- Later-device fanout into existing rooms. Finite tests require distinct
  KeyPackages per room, persistent fanout plans, response-loss retry, and
  reprepare after same-epoch loss. The HTTP batch claim wrapper now covers the
  server-side package response-loss piece, the HTTP fanout wrapper now covers
  opaque room-plan checkpointing, and the account-room directory covers
  discovery over HTTP. The HTTP runtime adapter now covers a one-room
  happy-path commit submission and Welcome release, including response-loss
  retry after the submit publish is accepted, plus a two-room fanout tick.
  Membership-derived directory writes, server-side membership
  validation/filtering, and replacing seeded wrapper state with server-derived
  membership projections remain unported.
- Mapping Finite's server cursor, repair states, and full crash-atomic
  transaction model onto Darkmatter's engine/storage model without duplicating
  protocol state. The SQLite operation log now proves basic restart replay for
  accepted Darkmatter HTTP operations and `/messages` idempotency, but not the
  full copied reducer matrix.
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
- `finitechat-server` exposes in-process HTTP routes over the in-memory
  Darkmatter delivery core and keeps `serve` as an explicit binary mode. It can
  optionally rebuild state from a SQLite operation log with
  `serve [addr] --sqlite PATH`. Auth and production server behavior remain
  unported.
- `finitechat-cli` can now call the HTTP delivery routes for health, group
  publish/sync, inbox publish/sync, KeyPackage publish, and KeyPackage claim.

Verified after adding the Darkmatter dependency graph:

- `cargo test --workspace`: pass
- `python3 -m unittest discover -s tests -p '*test*.py'`: pass
- `cargo clippy --workspace --all-targets -- -D warnings`: pass
- `cargo run -p finitechat-cli -- compat-report`: pass
- `cargo run -p finitechat-server -- smoke`: pass
- `cargo run -p finitechat-cli -- http-smoke`: pass

Additional HTTP route checkpoint:

- `cargo test -p finitechat-server --test http_routes`: pass
- `cargo test -p finitechat-server --test http_persistence`: pass
- Route/store/engine tests added so far: `19`
- Route coverage proven:
  - `GET /health`
  - `POST /messages`
  - `POST /sync/group`
  - `POST /sync/inbox`
  - `POST /key-packages`
  - `POST /key-packages/inventory`
  - `POST /key-packages/claim`
  - `POST /key-packages/claims`
  - `POST /fanouts/get`
  - `POST /fanouts/rooms`
  - `POST /fanouts/rooms/prepared`
  - `POST /fanouts/rooms/done`
  - `POST /account-rooms`
  - `POST /account-rooms/list`
- Persistence coverage proven:
  - group queue and duplicate-message index rebuild after restart
  - same-epoch commit admission rebuilds after restart
  - consumed KeyPackage state rebuilds after restart
  - KeyPackage available/claimed inventory survives restart and idempotent
    publish replay does not resurrect claimed inventory
  - idempotent `/messages` retry replays the original receipt after restart
  - same idempotency key with a different target/message conflicts without a
    second append
  - claimed Welcome inbox messages are not claimed twice before ack
  - activated Welcome ack is idempotent after restart
  - failed Welcome ack is terminal after restart
  - idempotent batch KeyPackage claim replays the exact original claims after
    restart
  - conflicting batch KeyPackage claim idempotency key has no package side
    effects
  - fanout room plan, prepared state, reprepare state, and done state survive
    restart
  - conflicting fanout room plan update does not overwrite the stored plan
  - account-room directory pages by room id and survives restart
- Real Marmot engine coverage proven:
  - `cargo test -p finitechat-server --test http_engine_routes`: pass
  - route layer carries a real create Welcome, invite Commit, invite Welcome,
    and application message between `HarnessClient`s
- Live persistent-mode smoke verified with a temporary SQLite file on
  `127.0.0.1:18788`:
  - `finitechat-darkmatter http --server http://127.0.0.1:18788 health`
  - `finitechat-darkmatter http --server http://127.0.0.1:18788 publish-group --group-id sqlite-live-room --transport-group-id sqlite-live-transport --message-id sqlite-live-commit --payload commit --commit-epoch 1`
- Live idempotency smoke verified with a temporary SQLite file on
  `127.0.0.1:18789`:
  - publishing the same `/messages` request twice with
    `--idempotency-key idem-live-key` returned the same `seq:1` receipt with
    `duplicate:false`
  - publishing a different target/message with the same key returned
    `409 idempotency_conflict`
- Live Welcome claim/ack smoke verified with a temporary SQLite file on
  `127.0.0.1:18790`:
  - `publish-inbox` stored a Welcome message for `live-welcome-recipient`
  - first `claim-welcomes` returned the Welcome, duplicate `claim-welcomes`
    returned `[]`
  - `ack-welcome --activated true` returned `{"acked":true}`
- Live batch KeyPackage claim smoke verified with a temporary SQLite file on
  `127.0.0.1:18791`:
  - published `live-laptop-1`, `live-phone-1`, and `live-phone-2`
  - `claim-key-packages --owner live-laptop --owner live-phone
    --idempotency-key live-batch-claim` returned `live-laptop-1` and
    `live-phone-1`
  - after server restart, replaying the same batch returned the same packages
  - a direct `claim-key-package --owner live-phone` then returned
    `live-phone-2`
- Live fanout checkpoint smoke verified with a temporary SQLite file on
  `127.0.0.1:18792`:
  - `fanout-save-room` stored `live-fanout` / `live-room` with claimed
    `live-kp-1`
  - `fanout-mark-prepared` stored `live-commit-loser`
  - after server restart, `fanout-get` returned the prepared loser state
  - a second `fanout-mark-prepared` replaced it with `live-commit-retry`
  - `fanout-mark-done --accepted-seq 12` recorded the terminal done state

Important test caveat:

- The copied Rust and Python suites still mostly exercise the original Finite
  Chat implementation. They are preserved here as the acceptance surface. The
  Darkmatter-backed behavior directly proven in this repo is currently the
  adapter smoke test plus the HTTP route, persistence, real-engine route, and
  KeyPackage/Welcome/room-pull runtime-delivery tests above.

Additional CLI checkpoint:

- `cargo test -p finitechat-cli`: pass
- New CLI tests added: `12`
- Request construction coverage proven:
  - group publish builds the `/messages` DTO with optional commit admission
    and optional idempotency key
  - inbox publish builds a Welcome envelope
  - group sync defaults to `after_seq = 0` and `limit = 50`
  - KeyPackage inventory builds the route DTO
  - KeyPackage claim builds the route DTO
  - batch KeyPackage claim builds the route DTO with repeated owners and an
    idempotency key
  - fanout save-room, mark-prepared, and mark-done commands build the route
    DTOs
  - account-room save and list commands build the route DTOs
  - Welcome claim and ack build the route DTOs
  - unknown CLI flags fail as usage errors
- Live localhost smoke verified with a temporary server on `127.0.0.1:18787`:
  - `finitechat-darkmatter http --server http://127.0.0.1:18787 health`
  - `finitechat-darkmatter http --server http://127.0.0.1:18787 publish-group --group-id cli-room --transport-group-id cli-transport --message-id cli-commit-1 --payload commit-bytes --commit-epoch 1`
  - `finitechat-darkmatter http --server http://127.0.0.1:18787 sync-group --group-id cli-room --limit 10`

Dependency note:

- The copied Finite Chat workspace used `rusqlite 0.37`. Adding Darkmatter
  pulls the workspace toward `rusqlite 0.32` through OpenMLS/Darkmatter's
  SQLite dependency graph, so this port repo aligns its workspace `rusqlite`
  version to `0.32` to avoid two `libsqlite3-sys` packages linking `sqlite3`.
- The real-engine route test uses Darkmatter's `cgka-conformance-simulator` as
  a dev dependency. That is useful proof for this port, but it pulls in the
  simulator's Nostr/storage/proptest dependency graph. A long-term upstream PR
  should probably expose a smaller reusable HTTP route harness or keep this test
  in Darkmatter proper.

Runtime delivery checkpoint:

- `cargo test -p finitechat-client --test client_state runtime_sync_tick_replenishes_key_packages_over_darkmatter_http_routes`: pass
- `cargo test -p finitechat-client --test client_state runtime_delivery_claims_key_package_metadata_over_darkmatter_http_routes`: pass
- `cargo test -p finitechat-client --test client_state runtime_sync_tick_claims_and_acks_welcomes_over_darkmatter_http_routes`: pass
- `cargo test -p finitechat-client --test client_state runtime_sync_tick_syncs_room_pages_over_darkmatter_http_routes`: pass
- `cargo test -p finitechat-client --test client_state runtime_link_fanout_discovers_account_rooms_over_darkmatter_http_routes`: pass
- `cargo test -p finitechat-client --test client_state runtime_link_fanout_tick_links_later_device_over_darkmatter_http_routes`: pass
- `cargo test -p finitechat-client --test client_state runtime_link_fanout_retries_http_submit_response_loss_without_duplicates`: pass
- `cargo test -p finitechat-client --test client_state runtime_link_fanout_tick_links_multiple_rooms_over_darkmatter_http_routes`: pass
- `cargo test -p finitechat-client --test client_state runtime_link_fanout_reprepares_after_http_same_epoch_loss`: pass
- The real `run_runtime_sync_tick` worker can replenish KeyPackages through the
  Darkmatter HTTP `/key-packages/inventory` and `/key-packages` routes.
- Reopening the HTTP server from SQLite proves the worker sees the persisted
  inventory and uploads zero duplicate KeyPackages on replay.
- The runtime delivery adapter can claim a KeyPackage through
  `/key-packages/claim`, recover the original Finite package metadata, compute
  the same deterministic lease token, and replay after server restart with no
  duplicate claim.
- The same worker can claim a valid serialized `WelcomeRecord` carried through
  the Darkmatter HTTP inbox, activate the Welcome locally, ack `/welcomes/ack`,
  and replay after server restart without duplicate claim or ack.
- The same worker can pull serialized `RoomLogEntry` payloads through
  Darkmatter HTTP `/sync/group`, decrypt an application entry, advance the
  client cursor, and replay without applying the entry twice.
- The link-fanout worker can read serialized account-room discovery records
  through the HTTP account-room directory after server restart and complete a
  discovery-only tick when the target device is already current in the room.
- The link-fanout worker can also complete a one-room later-device happy path
  over the HTTP adapter: it discovers a room from the account-room directory,
  claims the later device's KeyPackage, prepares and submits the add-device
  Commit through `/messages`, syncs that Commit back through `/sync/group`, and
  the later device claims and activates the released Welcome through the HTTP
  inbox routes.
- When the HTTP submit response is lost after the commit and Welcome publishes
  have been accepted, the worker reloads the prepared commit from durable local
  state, retries the same HTTP idempotency keys, completes the room, and leaves
  exactly one new group Commit and one claimed Welcome.
- With two existing rooms, the same worker pages account-room discovery one
  room at a time, claims two distinct target-device KeyPackages, submits and
  completes both room commits, and the later device activates both released
  Welcomes.
- If the fanout submit fails before HTTP accept and a competing same-epoch
  member commit wins, syncing that winning commit clears the local pending
  commit and the next worker tick reprepares/submits the fanout commit at the
  next epoch.
- This proves the current client runtime harness can be reused above a
  Darkmatter HTTP adapter for KeyPackage inventory/upload/claim, Welcome
  claim/ack, ordered room pull, account-room discovery, and a one-room
  later-device fanout happy path with submit response-loss retry and multi-room
  fanout, including same-epoch reprepare. It does not yet prove
  membership-derived directory writes or server membership filtering over the
  same adapter.

Next meaningful gate: extend the Darkmatter-backed runtime delivery boundary
from seeded account-room directory records into membership-derived directory
writes/server filtering, or start moving the test-only HTTP runtime adapter into
production client/server boundaries.
