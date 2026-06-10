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
  `301`
- Current Rust tests overall, including HTTP route/CLI adapter tests: `376`
- Python tests overall: `8`
- Python Hermes adapter tests: `7`
- Python process binary smoke tests: `1`

Copied/application Rust test distribution:

| File | Count |
| --- | ---: |
| `crates/finitechat-blob/src/lib.rs` | 17 |
| `crates/finitechat-client/tests/client_state.rs` | 46 |
| `crates/finitechat-engine/src/lib.rs` | 7 |
| `crates/finitechat-hermes/src/lib.rs` | 9 |
| `crates/finitechat-mls/src/lib.rs` | 14 |
| `crates/finitechat-proto/src/lib.rs` | 62 |
| `crates/finitechat-sim/tests/daemon_survival.rs` | 21 |
| `crates/finitechat-sim/tests/finitecomputer_boundary.rs` | 4 |
| `crates/finitechat-sim/tests/scenarios.rs` | 78 |
| `crates/finitechat-store/src/lib.rs` | 1 |
| `crates/finitechat-store/tests/sqlite_scenarios.rs` | 43 |

Additional HTTP/CLI/Darkmatter Rust test distribution:

| File | Count |
| --- | ---: |
| `crates/finitechat-cli/src/lib.rs` | 23 |
| `crates/finitechat-darkmatter/src/lib.rs` | 2 |
| `crates/finitechat-server/tests/http_engine_routes.rs` | 1 |
| `crates/finitechat-server/tests/http_persistence.rs` | 43 |
| `crates/finitechat-server/tests/http_routes.rs` | 5 |

Python test distribution:

| File | Count |
| --- | ---: |
| `tests/hermes/test_finite_platform_adapter.py` | 7 |
| `tests/test_process_binary_smoke.py` | 1 |

## Test Suite Parity Audit

Audit state:

- Port code state audited: `codex/darkmatter-port`, including HTTP
  KeyPackage publish retry/conflict, KeyPackage lease-expiry/reclaim,
  Welcome-release coupling, revoked-device projection, ephemeral activity
  route/cache coverage, and application delivery-effect projection
- Baseline checkout: `/Users/futurepaul/dev/finite/finitechat`,
  `marmot-investigation` at `7e8048d`
- Baseline untracked docs were ignored because they are outside `crates/` and
  `tests/`.
- Method: compare parsed test keys by relative path, language, and test name.
  Rust tests were parsed from `#[test]` / `#[tokio::test]` attributes and the
  following `fn`; Python tests were parsed from `def test_*`.

Parity result:

- Baseline test-bearing files: `12`
- Port test-bearing files: `18`
- Baseline parsed tests: `294` (`287` Rust, `7` Python)
- Port parsed tests: `384` (`376` Rust, `8` Python)
- Missing baseline test names in the port: `0`
- Port-only test names: `90`
- Intentionally reshaped baseline test names: `0` at the parsed test-key layer.
  The baseline relative paths and test names are preserved; port-only tests
  add Darkmatter HTTP/CLI/runtime/process coverage around them.

Port-only test buckets:

| Bucket | Count | Files |
| --- | ---: | --- |
| HTTP CLI request/live-server coverage | 23 | `crates/finitechat-cli/src/lib.rs` |
| Runtime client over Darkmatter HTTP routes/live reqwest | 15 | `crates/finitechat-client/tests/client_state.rs` |
| Darkmatter compatibility report/core smoke | 2 | `crates/finitechat-darkmatter/src/lib.rs` |
| Server HTTP route, persistence, and real-engine route coverage | 49 | `crates/finitechat-server/tests/http_routes.rs`, `crates/finitechat-server/tests/http_persistence.rs`, `crates/finitechat-server/tests/http_engine_routes.rs` |
| Process-level server/CLI binary smoke | 1 | `tests/test_process_binary_smoke.py` |

Conclusion: the port currently preserves the full baseline test-name surface and
adds Darkmatter-specific coverage. This audit does not prove every baseline test
body is byte-identical or that every product workflow is now driven only through
process binaries; those are tracked separately below.

## Process-Level Coverage Decision

Decision: no additional process-level runtime/client binary coverage is needed
for the current repo shape.

Evidence:

- The only production binaries currently exposed by this workspace are
  `finitechat-darkmatter-server` and `finitechat-darkmatter`.
- `finitechat-darkmatter-server` exposes `smoke` and `serve [addr] [--sqlite
  PATH]`.
- `finitechat-darkmatter` exposes `compat-report`, `http-smoke`, and
  route-oriented `http` commands.
- The process-level smoke builds both binaries, starts the server binary with
  SQLite, drives the CLI binary through health, publish, sync, exact idempotent
  replay, conflict rejection, server restart, and persisted sync.
- Runtime sync and later-device fanout are library/client workflows, not binary
  entrypoints in this repo. They are covered by client-state tests over
  `HttpRuntimeDelivery`, deterministic in-process HTTP/failure-injection
  transports, and `ReqwestHttpRuntimeTransport` against live Axum servers.

Process coverage should grow when the product exposes a daemon/runtime binary
that owns `run_runtime_sync_tick` or `run_link_fanout_tick`. Until then, adding
a process wrapper solely for tests would create a new product surface rather
than prove an existing one.

## Preserved Baseline Backend Audit

Audit method:

- Start from the `294` baseline test names proven present in the parity audit.
- Classify preserved tests by file-level backend ownership, then separately
  account for the `90` port-only Darkmatter/HTTP/CLI/process tests.
- This audit intentionally treats preserved baseline tests as still requiring
  migration unless their file is already product-only or OpenMLS-helper-only.

Preserved baseline classification:

| Class | Count | Meaning |
| --- | ---: | --- |
| Product logic, no delivery service | 95 | `finitechat-blob`, `finitechat-proto`, `finitechat-hermes`, and Hermes adapter tests validate product DTO/policy/bridge behavior above the encrypted payload boundary. |
| Real OpenMLS helper coverage | 14 | `finitechat-mls` proves credentials, KeyPackages, Welcome, and basic OpenMLS operations without exercising the delivery service. |
| Real OpenMLS client over old delivery service | 31 | Preserved `finitechat-client` tests use real client/MLS state but still use the original `DeliveryService` as the ordered server. |
| Old in-memory delivery service | 110 | `finitechat-engine` plus `finitechat-sim` scenario/survival/boundary tests still prove reducer behavior through the original fake/in-memory service. |
| Old SQLite delivery store | 44 | `finitechat-store` tests still prove durable reducer behavior through the original SQLite store. |

Port-only Darkmatter coverage added so far:

| Class | Count | Meaning |
| --- | ---: | --- |
| CLI HTTP route coverage | 23 | Request building and live-server route calls through `finitechat_cli::run`. |
| Runtime HTTP coverage | 15 | `HttpRuntimeDelivery`, in-process HTTP fault injection, and live `ReqwestHttpRuntimeTransport` tests. |
| Server HTTP coverage | 49 | Axum route, SQLite HTTP-operation replay, and real Marmot engine route tests. |
| Darkmatter core smoke/report | 2 | HTTP delivery core ordering and compatibility bucket tests. |
| Process binary smoke | 1 | Server binary plus CLI binary over SQLite-backed HTTP. |

Highest-risk preserved fake/store proof checkpoints ported to HTTP so far:

| Priority | Preserved tests | Target proof shape |
| --- | --- | --- |
| P0 | `accepted_commit_response_lost_then_server_restart_replays_same_result`, `sqlite_commit_crash_matrix_rolls_back_and_retry_converges` | Darkmatter HTTP `/commits` plus SQLite operation-log crash/replay matrix. |
| P0 | `commit_effects_are_atomic_at_reducer_boundary`, `sqlite_rejected_commit_is_replayable_after_reopen` | Typed HTTP commit transaction tests that inject failures before and after delivery append. |
| P0 | `invalid_commit_report_fails_closed`, `membership_delta_disagreement_enters_needs_repair`, `sqlite_invalid_commit_report_blocks_room_after_reopen` | Darkmatter-backed invalid-commit/repair-state route tests. |
| P1 | `welcome_is_not_released_before_accepted_commit`, `sqlite_welcome_not_released_before_accepted_commit` | Typed submit-commit tests that assert Welcome release remains coupled to durable accepted Commit append. |

Current P0 progress: typed HTTP `/commits` now proves successful
lost-response replay after restart, replayed rejection after a same-epoch loser
is retried after restart, repair after legacy commit publish/idempotency rows
survive without adapter projection rows, and SQLite crash-matrix rollback/retry
convergence across commit delivery, commit idempotency, Welcome delivery,
Welcome idempotency, account-room projection, room-membership projection, and
KeyPackage consumed-projection write points. The HTTP server also has an
invalid-commit repair route that persists `NeedsRepair` room state, reloads it
after restart, and blocks later typed events and commits. The remaining
old-store delta is not the atomic `/commits` bundle; it is the broader
fake/in-memory reducer and SQLite store surface that still needs to be audited
for route/runtime equivalents.

Current KeyPackage progress: typed HTTP `/commits` now rejects unclaimed
KeyPackages and stale KeyPackage metadata before side effects, consumes claimed
packages atomically with accepted commits, rebuilds consumed state after
restart, rejects consumed-package reuse, and maps HTTP claimed inventory back to
leased counts in the runtime adapter. The HTTP wrapper now owns KeyPackage
lease state above Marmot's opaque package bytes: it can expire a claimed lease
back to available, persist that across restart, reclaim the same package, and
reject attempts to expire consumed packages. Exact KeyPackage publication
retries now replay safely after restart, while conflicting same-id packages are
rejected without creating extra claimable inventory.

Current Welcome-release progress: typed HTTP `/commits` now has a focused
restart proof that bad commit metadata does not release a Welcome, the absence
of that Welcome survives server restart, and the corrected commit releases
exactly one Welcome only after it is accepted.

Current revocation progress: the HTTP server now persists revoked `DeviceRef`s,
rebuilds that state after restart, rejects revoked KeyPackage
publish and single-owner claim, skips revoked owners in batch KeyPackage claim
without consuming inventory, rejects revoked Welcome claim and activated ack,
rejects revoked typed application-event and commit senders, and rejects typed
commits that try to add a revoked device.

Current room-membership removal progress: the HTTP server now persists removal
intervals across restart. A removed device can sync the commit that removes it,
cannot send later typed events or commits, and later requester-filtered sync
advances over hidden post-removal messages without exposing them.

Current account-device cap progress: typed HTTP `/commits` now enforce the
per-account device cap for a room before durable append. Filling a room to
`MAX_ACCOUNT_DEVICES_PER_ROOM` succeeds, the next same-account add is rejected,
and SQLite restart proves there is no overflow commit, no released Welcome, no
account-room projection leak, and the overflow KeyPackage remains claimed.

Current duplicate-device add progress: typed HTTP `/commits` now reject a
current or pending device being added again before durable append. The retry
KeyPackage remains claimed, only the original Welcome is visible, and the
account-room projection still contains one pending device after SQLite restart.

Current direct-room progress: the HTTP server now exposes direct-room
create-or-get as wrapper-owned room-membership projection state. The sorted
account pair survives SQLite restart, reversed account order returns the
existing room, and typed `/commits` reject third-account adds before delivery
append, Welcome release, or account-room projection side effects.

Current membership-delta validation progress: typed HTTP `/commits` now prove
the structural matrix at the route boundary. Wrong base epoch, wrong
post-commit epoch, wrong commit message id, duplicate adds, duplicate removes,
add/remove overlap, and incomplete add metadata all fail before durable append,
Welcome release, account-room projection, or claimed KeyPackage consumption.

Current group-sync pagination progress: typed HTTP `/events` plus `/sync/group`
now prove bounded requester-aware pages over persisted Darkmatter group logs.
The first full page returns `MAX_HTTP_SYNC_PAGE_ENTRIES`, sets `has_more`, and
the next page after SQLite restart returns the remaining entry with the correct
cursor.

Current typed-event progress: the HTTP `/events` route now preserves stricter
typed-event semantics above the looser Darkmatter transport duplicate rule. It
rejects oversized application payloads before durable append, replays the exact
original response for the same idempotency key after restart, rejects a
duplicate typed event message id when retried with a new idempotency key, and
leaves the durable group log with a single entry.

Current idempotency-capacity progress: typed HTTP publishes now derive
per-room/per-sender idempotency scope from serialized `RoomLogEntry` payloads.
The HTTP wrapper rejects fresh typed `/events` once the sender reaches
`MAX_IDEMPOTENCY_RECORDS_PER_ROOM_DEVICE`, while exact replay of an existing
typed event still succeeds before and after SQLite restart.

Current application delivery-effect progress: the HTTP server now exposes
`/application-events` for typed application publishes that also record a
server-side delivery-effect projection. The projection stores the
caller-supplied `ApplicationDeliveryPolicy` by message id, reloads it from
SQLite, reports aggregate push/unread/command-inbox counts, replays exact
idempotent publishes, and rejects a conflicting replay that tries to attach a
different delivery policy to the same durable event.

Remaining application delivery-effect hardening: this projection is currently a
post-append wrapper write, not part of the stronger `/commits` candidate
snapshot transaction. A later pass should either persist the event append and
delivery-effect projection in one SQLite mutation or repair the missing
projection from a replayed `/application-events` request after an interrupted
server window.

Current ephemeral activity progress: the HTTP server now accepts opaque
ephemeral activity through `/activities` only for active, non-revoked senders at
the current typed room epoch. It rejects pending, revoked, wrong-epoch, and
expired activity, caps per-route volatile cache entries, does not append durable
group messages, and does not persist activity across SQLite restart.

Current link-session progress: the HTTP server now exposes link-session
pairing as wrapper-owned rendezvous state. Create/upload/claim/release/ack and
expire survive SQLite restart, same-payload upload is idempotent, conflicting
payloads and late uploads reject, and CLI route DTO coverage exists for every
link-session route.

Conclusion: the port now preserves all baseline names and adds Darkmatter HTTP
coverage, but the migration is not complete. The remaining implementation work
is concentrated in the old fake/in-memory reducer and SQLite store tests that
do not yet run through Darkmatter HTTP.

## Darkmatter-Facing Delta Buckets

Darkmatter source state:

- Branch: `/Users/futurepaul/dev/finite/darkmatter` HTTP delivery branch at
  `5b17774`
- Base: `origin/master` at `89ece10`
- Delta: `7` commits, `12` files, about `1,557` insertions over upstream
  master.

Refresh result:

- `origin/master` advanced from `add4fc7` to `89ece10`.
- The HTTP delivery branch rebased cleanly from `8449fee` to `5b17774`.
- Focused Darkmatter verification passed for `transport-http-server`,
  `cgka-engine`, and `cgka-conformance-simulator --test
  http_delivery_compatibility`.
- The port compatibility report still has exactly one
  `RequiresDarkmatterFork` item: `ordered_delivery_profile`.
- The port lockfile picked up upstream Darkmatter's new `hex` dependency edge
  for `storage-sqlite`.

Upstreamable HTTP transport work:

- `crates/transport-http-server`: the prototype single-server HTTP delivery
  state for opaque Marmot `TransportMessage`s, commit admission by source
  epoch, bounded group/inbox sync pages, and owner-scoped KeyPackage
  publication/claim.
- Workspace registration and conformance-simulator integration for
  HTTP-delivery compatibility tests.
- This is upstreamable as a transport-adapter/service crate because it stays
  below CGKA, does not decrypt payloads, and does not encode application
  policy.

Upstreamable ordered-delivery profile work:

- `crates/cgka-engine/src/delivery_profile.rs`
- `EngineBuilder::delivery_profile`
- The `message_processor` convergence gate that lets a server-admitted next
  Commit fall through to OpenMLS processing under the explicit
  `DangerouslyTrustServerOrdering` profile.
- This needs upstream design review and probably a less alarming public API
  name. Until Marmot accepts an ordered-delivery profile, this remains the only
  Darkmatter fork-required item.

Adapter/application logic:

- `finitechat-http` shared route DTOs.
- `finitechat-server` Axum routes, SQLite operation log, idempotency wrappers,
  Welcome claim/ack state, KeyPackage inventory/batch-claim wrappers, fanout
  checkpoints, link-session pairing state, account-room projection,
  room-membership projection, typed submit-commit, typed application-event
  routes, and revoked-device projection.
- `finitechat-client` runtime HTTP adapter, reqwest transport, local state,
  link-fanout retry/reprepare, and product-level payload decoding.
- `finitechat-cli` route client and compatibility report.
- Product DTOs and application policy for conversations, topics, Hermes bridge,
  push/unread/command inbox, runtime state, activity, and attachments.

Fork-only requirements beyond the current HTTP branch:

- None identified beyond the ordered-delivery profile if Marmot accepts an
  equivalent upstream design.
- If Marmot rejects any transport/server-ordering influence on canonical branch
  processing, downstream adopters would need to keep a fork or carry a
  compatibility layer with weaker/more expensive convergence behavior.

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
  Darkmatter HTTP boundary without teaching the server application-specific
  device structure.
- The HTTP KeyPackage publication path replays exact same-id package retries
  after restart and rejects conflicting same-id bytes without creating another
  claimable package.
- The HTTP revoked-device wrapper can persist terminal device status,
  rebuild it after restart, block revoked devices from server-mediated
  KeyPackage, Welcome, event, and Commit paths, and skip revoked owners during
  batch KeyPackage claim without consuming inventory.
- The HTTP fanout wrapper can persist opaque later-device fanout room plans,
  prepared message ids, reprepare checkpoints, and accepted sequence markers
  across restart without teaching the server MLS semantics.
- The HTTP link-session wrapper can persist opaque encrypted pairing payloads,
  claim/release/ack/expire state, and deterministic claim tokens across restart
  without interpreting the payload or joining it to room membership state.
- The HTTP account-room directory wrapper can persist typed account-room
  records, normalize them to the requested account's devices, page them by room
  id, and reload them from SQLite. This gives the runtime link-fanout discovery
  loop a Darkmatter HTTP boundary while preventing arbitrary room-membership
  JSON from becoming discovery output.
- The HTTP account-room bootstrap wrapper can derive the creator's initial
  active account-room record from typed room metadata, persist it, replay
  it idempotently after restart, and reject conflicting bootstrap attempts.
- The HTTP route layer can also project accepted typed add/remove commits into
  the account-room directory. Typed `/commits` derives this projection from the
  submitted request, while raw `/messages` keeps compatibility with an explicit
  projection wrapper. The later-device HTTP fanout test proves an accepted add
  commit persists the new device as pending after restart, and a remove-commit
  HTTP runtime test proves the removed account no longer lists the room after
  restart without a second manual `/account-rooms` write.
- The typed HTTP `/commits` route keeps Welcome release coupled to accepted
  commit append: bad commit metadata leaves the group log and recipient Welcome
  inbox empty across restart, and the corrected commit releases one Welcome only
  after acceptance.
- The HTTP Welcome ack wrapper can decode a claimed `WelcomeRecord` on
  activated ack and promote the account-room device from pending to active
  across SQLite restart.
- The HTTP room-membership projection can derive server-owned membership
  intervals from typed bootstrap, typed `/commits`, and activated Welcome acks,
  persist them, filter group sync pages by requester, and reject typed
  application events or tracked typed commits from pending/unacked devices.
  It now also persists removal intervals across restart so removed devices can
  sync through their removal commit, cannot send later events or commits, and
  advance cursors over hidden post-removal entries without seeing them.
  Typed `/commits` publish a `FiniteAccountRoomCommitProjection` payload, and
  raw `/messages` commit imports for typed rooms must carry the same projection
  wrapper; plain raw commits are rejected before append instead of weakening
  strict membership filtering.
- The typed HTTP `/commits` route now preserves the per-account room device
  cap: it rejects an add commit that would exceed
  `MAX_ACCOUNT_DEVICES_PER_ROOM` before durable append, Welcome release,
  KeyPackage consumption, or account-room projection update.
- The typed HTTP `/commits` route also preserves the duplicate current or
  pending device rule: a fresh add for a device already pending in the room is
  rejected before durable append, duplicate Welcome release, KeyPackage
  consumption, or account-room projection duplication.
- The HTTP direct-room wrapper preserves the two-account direct-room admission
  rule: create-or-get stores a sorted account pair in the
  room-membership projection, reversed account order returns the existing room
  after restart, and typed `/commits` reject third-account adds before durable
  append.
- The typed HTTP `/commits` route rejects malformed typed membership deltas
  before touching ordered delivery state: base epoch mismatch, post-commit
  epoch mismatch, commit id mismatch, duplicate add/remove entries, add/remove
  overlap, and incomplete add metadata all leave the group log, Welcome inbox,
  account-room projection, and claimed KeyPackage state unchanged.
- The runtime link-fanout worker can complete a one-room later-device happy
  path over the HTTP adapter when the initial room log is published and
  account-room discovery starts from typed bootstrap projection: discover the
  room, claim the target device's KeyPackage, submit the add-device Commit
  through the typed HTTP `/commits` route, sync the Commit back, and let the
  later device claim and activate the server-released Welcome.
- The same one-room HTTP fanout path can retry a lost submit response from the
  persisted prepared state. The typed submit route replays the idempotent
  commit and Welcome publishes, so retry completes without appending a
  duplicate group entry or delivering duplicate Welcomes.
- The HTTP fanout path also handles the worker's multi-room shape from typed
  bootstrap discovery: paged account-room discovery across two rooms, two
  distinct target KeyPackage claims, two submitted commits, two completion
  syncs, and two later-device Welcome activations.
- The multi-room HTTP fanout path also preserves partial-progress isolation:
  after one room is already `Done`, a before-accept failure in a later room
  leaves the completed room terminal, reloads the failed room as prepared, and
  retries only that room against the persisted Darkmatter HTTP server.
- The HTTP fanout path can reprepare from typed bootstrap discovery after a
  same-epoch race: a fanout submit fails before accept, a competing member
  commit wins the epoch, the client syncs that winner and clears its pending
  commit, then the worker reprepares and submits the fanout commit at the next
  epoch.
- A focused HTTP runtime delivery test now proves requester-filtered group
  sync and typed `/events`: pre-invite history is hidden from a future member,
  the pending member can still pull the add Commit, pending application sends
  are rejected, activated Welcome ack promotes the device, and a post-ack
  application event decrypts for the existing member.
- The HTTP `/sync/group` route now proves bounded pagination over typed
  application events: a full page contains `MAX_HTTP_SYNC_PAGE_ENTRIES`, keeps
  `has_more`, and continues from the returned cursor after SQLite restart.
- The typed HTTP `/events` route now also rejects oversized application
  payloads before durable append, replays the exact response for the same
  idempotency key across restart, and rejects duplicate typed event message ids
  submitted with a new idempotency key without appending a second group entry.
- The typed HTTP publish wrapper now enforces the scoped idempotency
  capacity rule above Darkmatter's opaque transport: fresh typed events for a
  full room/sender bucket are rejected, but exact replay remains available
  across SQLite restart.
- The HTTP `/application-events` route can attach a caller-supplied
  `ApplicationDeliveryPolicy` to an accepted typed application event and store
  the derived delivery-effect projection by message id. This keeps
  push/unread/command-inbox routing above opaque Marmot payloads while making
  the server-side counts durable and replay-safe.
- The HTTP `/activities` route accepts opaque ephemeral activity for active
  non-revoked members at the current typed room epoch, rejects pending,
  revoked, wrong-epoch, and expired requests, caps per-route volatile cache
  entries, and keeps the group log sequence unchanged across restart.
- `finitechat-client` now owns a generic `HttpRuntimeDelivery<T>` adapter over
  a small `HttpRuntimeTransport` trait. The adapter maps runtime KeyPackage,
  Welcome, account-room, typed commit, typed event, and ordered room-sync calls
  onto the HTTP DTOs; the client-state tests now provide only an in-process
  transport harness and failure injection.
- `finitechat-client` also provides a concrete reqwest-backed runtime transport
  for live HTTP servers. A client-state test starts the Axum server on an
  ephemeral localhost listener, exercises KeyPackage upload/claim, ordered room
  sync, and later-device fanout through `RuntimeDelivery`, and verifies
  non-success HTTP statuses remain visible to callers.
- `finitechat-http` now owns the shared HTTP route DTOs. The server imports
  those types for handlers and re-exports them for compatibility, while
  `finitechat-client` and `finitechat-cli` depend on the shared crate instead
  of the server crate for production DTO mapping.
- Darkmatter's existing Marmot engine and Nostr peeler can produce real Welcome,
  invite Commit, and application messages that pass through the HTTP delivery
  service core and are ingested by recipients.
- The `finitechat-server` Axum route layer can carry those real Marmot Welcome,
  invite Commit, and application messages end to end when driven by
  Darkmatter's conformance simulator clients.
- The copied application-policy tests can remain above the encrypted
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
- Group sync pagination for the HTTP delivery surface. Darkmatter owns the
  bounded ordered group pages; the route wrapper adds typed
  requester-aware filtering, typed payload decoding expectations, and SQLite
  restart coverage for cursor continuation.
- Runtime HTTP delivery adapter boundary. The production client can own the DTO
  mapping, envelope/body consistency checks, commit request validation, and
  ordered room-sync decoding independently from the test transport used to
  exercise the Axum routes.
- Runtime HTTP network transport. The production client can now reuse the same
  adapter against a real server URL through reqwest, while tests can still
  inject a different transport for deterministic response-loss scenarios.
- Runtime KeyPackage metadata mapping. The Darkmatter HTTP KeyPackage store
  carries opaque bytes; the client adapter can encode the original
  `UploadKeyPackageRequest` so a later claim reconstructs the package
  ref, hash, payload, owner, and deterministic lease token.
- Batch KeyPackage claim replay for the HTTP delivery surface. This wraps
  Darkmatter's owner-scoped `claim_key_package` primitive so fanout callers can
  ask for one package per device owner and safely retry after response loss.
- KeyPackage inventory projection for the HTTP delivery surface. This mirrors
  Darkmatter's available/consumed package state as available/claimed counts, so
  runtime clients can replenish toward a target without listing package bytes.
- KeyPackage lease expiry/reclaim for the HTTP delivery surface. This keeps
  claimed/available/consumed lease state in wrapper-owned durable
  inventory while reusing Marmot's opaque KeyPackage publication payloads.
- Revoked-device projection for the HTTP delivery surface. This is
  wrapper-owned server state keyed by `DeviceRef`; it gates
  server-mediated KeyPackage, Welcome, typed event, and typed commit operations
  without requiring Marmot's transport core to understand product device
  lifecycle policy.
- Opaque fanout plan checkpointing for the HTTP delivery surface. This stores
  the coordination fields a client worker needs to resume after restart or
  response loss, while leaving MLS validation and local pending Commit state on
  the client.
- Opaque link-session pairing for the HTTP delivery surface. This is
  wrapper-owned rendezvous state for encrypted pairing payloads; Darkmatter
  does not need to understand the payload, only the HTTP adapter's
  duplicate/conflict/claim/release/ack/expiry lifecycle.
- Account-room discovery projection for the HTTP delivery surface. This stores
  typed current-room membership snapshots keyed by account and room id,
  normalizes saved records to the requested account's devices, and rejects
  records with no current devices for that account, while leaving the actual
  source of membership truth outside Darkmatter's transport core.
- Account-room bootstrap projection for the HTTP delivery surface. This derives
  the creator's initial active device record from typed room metadata, so the
  later-device fanout path no longer needs an arbitrary opaque account-room
  write just to discover a newly created room.
- Commit-derived account-room projection for the HTTP delivery surface. The
  typed `/commits` route applies adds/removes from the submitted
  `SubmitCommitRequest` to persisted `AccountRoomRecord`s, while raw
  `/messages` can still decode an explicit compatibility projection wrapper.
  This keeps discovery state in step with accepted add/remove commits, but it
  is still a query-side account-room projection rather than Darkmatter becoming
  the MLS membership authority.
- Welcome-ack-derived account-room activation for the HTTP delivery surface. The
  server can decode a claimed `WelcomeRecord` on activated ack and flip the
  matching pending account-room device to active, matching the existing store
  activation rule.
- Typed submit-commit route for the HTTP delivery surface. The route accepts a
  typed `SubmitCommitRequest`, validates its structural commit metadata,
  publishes a `FiniteAccountRoomCommitProjection` into the ordered Darkmatter
  group queue, derives account-room and room-membership updates from the
  submitted request,
  publishes derived `WelcomeRecord`s to recipient inboxes, and returns
  `CommitAccepted` from the accepted HTTP sequence. Malformed staged Welcome
  inputs and bad commit metadata are rejected before the route appends delivery
  side effects or releases Welcomes, and exact idempotent commit retries replay
  even after the room head has advanced.
- Typed application-event route for the HTTP delivery surface. The route
  accepts a typed `AppendEventRequest`, checks the requester against the
  server-owned room-membership projection when the projection is complete or
  tracks that sender, rejects oversized payloads before durable append,
  preserves exact idempotent replay, maps duplicate typed event message ids
  with new idempotency keys back to the product-level duplicate-message rule,
  publishes a plain `RoomLogEntry`, and persists the room head across restart.
- Application delivery-effect projection for the HTTP delivery surface. The
  wrapper accepts a typed application-event request plus an
  `ApplicationDeliveryPolicy`, records the derived push/unread/command-inbox
  effect by message id after the ordered event append is accepted, reloads that
  projection from SQLite, and rejects exact event replays that try to change
  the caller-supplied policy.
- Scoped idempotency capacity for the HTTP delivery surface. The wrapper
  derives room/sender scope from typed `RoomLogEntry` payloads, applies
  `MAX_IDEMPOTENCY_RECORDS_PER_ROOM_DEVICE` to fresh typed
  publishes, and checks replay/conflict first so existing requests remain
  replayable after the cap is reached.
- Ephemeral activity cache for the HTTP delivery surface. The route accepts
  opaque activity bytes above Darkmatter transport, gates them with the
  server-owned typed room-membership projection, and keeps the cache bounded and
  volatile instead of making activity part of the durable ordered group log.
- Server-owned room-membership projection for typed HTTP rooms. This remains a
  wrapper-owned projection over Darkmatter's ordered transport: typed bootstrap
  creates the active creator interval, typed commits add pending intervals and
  close removed intervals, activated Welcome ack marks the pending interval
  active, and requester-aware group sync filters pages while still advancing
  cursors over hidden messages.
- Account device caps for the HTTP delivery surface. The wrapper counts current
  and pending devices per account in the room-membership projection and rejects
  typed add commits that would exceed `MAX_ACCOUNT_DEVICES_PER_ROOM`, leaving
  Darkmatter's transport core unaware of product-specific room cardinality.
- Duplicate current or pending device admission for the HTTP delivery surface.
  The wrapper rejects typed add commits for devices that already have an open
  membership interval in the room, keeping duplicate-add policy above
  Darkmatter's ordered transport.
- Direct-room account-pair constraints for the HTTP delivery surface. The
  wrapper stores direct-room account pairs in server-owned room-membership
  projection state and rejects typed add commits for devices outside that pair,
  while Darkmatter still only sequences the opaque commit payload.
- Membership-delta structural validation for the HTTP delivery surface. The
  wrapper rejects malformed typed commit metadata before appending the opaque
  Marmot commit, so Darkmatter remains responsible for ordered transport while
  the wrapper keeps its product-specific membership semantics at the adapter
  layer.
- Shared HTTP route DTOs. `finitechat-http` keeps request/response wire types
  reusable by the Axum server, CLI route builder, and runtime HTTP delivery
  client without making production clients depend on the server crate.
- Public byte encoding for opaque IDs and payloads. The current CLI maps string
  arguments directly to bytes for local testing.

## Thick Or Wonky Logic

- Later-device fanout into existing rooms. The preserved tests require distinct
  KeyPackages per room, persistent fanout plans, response-loss retry, and
  reprepare after same-epoch loss. The HTTP batch claim wrapper now covers the
  server-side package response-loss piece, the HTTP fanout wrapper now covers
  opaque room-plan checkpointing, and the account-room directory covers
  discovery over HTTP. The HTTP server now covers a typed submit-commit route
  that publishes a commit projection with membership deltas, derives
  account-room membership updates from the submitted request, and releases
  derived Welcomes, including response-loss retry after the typed submit is
  accepted; the runtime adapter covers that path across one-room and two-room
  fanout ticks, including partial retry where completed rooms stay terminal
  while only the failed prepared room is resubmitted.
  Commit-derived account-room updates are now proven for add-device and
  remove-device commits, typed bootstrap projection is proven for the creator's
  initial active device, account-room save/list now normalizes records to the
  requested account, Welcome ack activation now promotes pending devices to
  active, typed room-membership projection now filters sync and rejects pending
  sends, and typed rooms reject raw plain Commit imports that do not include
  membership deltas.
- Mapping the server cursor, repair states, and full crash-atomic
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
- `finitechat-cli` exposes `compat-report`, `http-smoke`, and route-specific
  HTTP commands.
- `finitechat-server` exposes in-process HTTP routes over the in-memory
  Darkmatter delivery core and keeps `serve` as an explicit binary mode. It can
  optionally rebuild state from a SQLite operation log with
  `serve [addr] --sqlite PATH`. Auth and production server behavior remain
  unported.
- `finitechat-cli` can now call the HTTP delivery routes for health, group
  publish/sync, requester-aware sync, inbox publish/sync, typed submit-commit,
  typed application events, ephemeral activity, invalid-commit reporting,
  KeyPackage publish and claim, fanout checkpoints, account-room projections,
  direct-room create-or-get, and Welcome claim/ack.

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
- Route/store/engine tests added so far: `49`
- Route coverage proven:
  - `GET /health`
  - `POST /messages`
  - `POST /commits`
  - `POST /events`
  - `POST /application-events`
  - `POST /application-effects/get`
  - `POST /application-effects/counts`
  - `POST /activities`
  - `POST /sync/group`
  - `POST /sync/inbox`
  - `POST /devices/revoke`
  - `POST /key-packages`
  - `POST /key-packages/inventory`
  - `POST /key-packages/claim`
  - `POST /key-packages/claims`
  - `POST /fanouts/get`
  - `POST /fanouts/rooms`
  - `POST /fanouts/rooms/prepared`
  - `POST /fanouts/rooms/done`
  - `POST /link-sessions`
  - `POST /link-sessions/get`
  - `POST /link-sessions/payload`
  - `POST /link-sessions/claim`
  - `POST /link-sessions/ack`
  - `POST /link-sessions/release`
  - `POST /link-sessions/expire`
  - `POST /direct-rooms`
  - `POST /account-rooms/bootstrap`
  - `POST /account-rooms`
  - `POST /account-rooms/list`
  - `POST /rooms/report-invalid-commit`
  - `POST /welcomes/claim`
  - `POST /welcomes/ack`
- Persistence coverage proven:
  - group queue and duplicate-message index rebuild after restart
  - same-epoch commit admission rebuilds after restart
  - consumed KeyPackage state rebuilds after restart
  - KeyPackage available/claimed inventory survives restart and idempotent
    publish replay does not resurrect claimed inventory
  - exact KeyPackage publication retry survives restart and conflicting
    same-id package bytes reject without creating extra claimable inventory
  - typed `/commits` rejects unclaimed KeyPackages and stale KeyPackage
    metadata before side effects
  - typed `/commits` consumes claimed KeyPackages atomically with accepted
    commits, rebuilds consumed state after restart, and rejects consumed reuse
  - typed `/commits` rejects bad commit metadata without releasing a Welcome,
    preserves the empty recipient inbox after restart, then releases exactly one
    Welcome after the corrected commit is accepted
  - revoked device status survives restart; revoked devices cannot publish or
    single-claim KeyPackages, batch claims skip revoked owners without consuming
    inventory, revoked devices cannot claim or activate Welcomes, revoked active
    devices cannot send typed events or commits, and typed commits cannot add a
    revoked device
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
  - link-session pairing state survives restart; duplicate session creation
    conflicts, same-payload upload is idempotent, different payload conflicts,
    encrypted payload bytes stay opaque, release/reclaim keeps the deterministic
    claim token stable, bad ack tokens reject, delivered/expired sessions reject
    late uploads, and terminal states survive SQLite reopen
  - typed account-room bootstrap survives restart, replays idempotently, and
    rejects a conflicting creator device
  - account-room directory normalizes typed records to the requested account's
    devices, rejects records with no devices for that account, pages by room id,
    and survives restart
  - activated Welcome ack promotes the pending account-room device to
    active and the projection survives restart
  - requester-aware group sync filters through a persisted room-membership
    projection, advances cursors over hidden messages, admits pending members
    after typed commits, rejects pending typed application events and tracked
    pending typed commits, and accepts typed application events after Welcome
    activation survives restart
  - group sync returns bounded pages for typed application events, sets
    `has_more` when another entry remains, and continues from the returned
    cursor after SQLite restart
  - removed-device membership intervals survive restart; removed devices can
    sync through their removal commit, cannot send later typed events or
    commits, and advance cursors over hidden post-removal messages
  - account device cap enforcement survives SQLite reload; filling a typed
    room to `MAX_ACCOUNT_DEVICES_PER_ROOM` succeeds, the next same-account add
    fails before durable append, no overflow Welcome is released, the
    account-room projection stays capped, and the overflow KeyPackage remains
    claimed
  - duplicate current or pending device adds fail before side effects; the
    group log cursor stays at the original add, the duplicate Welcome is not
    released, the account-room projection keeps one pending device, and the
    retry KeyPackage remains claimed after SQLite restart
  - direct-room create-or-get state survives restart; reversed account order
    returns the original room, direct account pairs stay attached to the
    room-membership projection, and typed `/commits` reject third-account adds
    before group append, Welcome release, or account-room projection side
    effects
  - malformed membership deltas fail at typed `/commits` validation before
    side effects; wrong epochs, wrong commit id, duplicate add/remove entries,
    add/remove overlap, and incomplete add metadata leave the group log,
    Welcome inbox, account-room projection, and claimed KeyPackage unchanged
  - typed `/events` rejects oversized application payloads before durable
    append, replays exact idempotent responses after restart, rejects duplicate
    typed event message ids submitted with new idempotency keys, and leaves the
    durable group log with one entry
  - scoped typed publish idempotency capacity survives SQLite reload; fresh
    typed events for a full room/sender bucket fail with
    `idempotency_capacity_exceeded`, exact replay still succeeds, and overflow
    does not append another group entry
  - application delivery effects survive SQLite reload; exact typed
    application-event replay returns the same accepted event, aggregate
    push/unread/command-inbox counts are rebuilt from stored policies, and
    reusing the same idempotency key with a conflicting delivery policy fails
    without changing the projection
  - ephemeral activity is accepted for active members, rejects pending,
    revoked, wrong-epoch, and expired requests, caps the per-route volatile
    cache, does not advance the durable group sequence, and does not persist
    across restart
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

- The copied Rust and Python suites still mostly exercise the original
  implementation. They are preserved here as the acceptance surface. The
  Darkmatter-backed behavior directly proven in this repo is currently the
  adapter smoke test plus the HTTP route, persistence, real-engine route, and
  KeyPackage/Welcome/room-pull runtime-delivery, application delivery-effect,
  and live CLI tests above.

Additional CLI checkpoint:

- `cargo test -p finitechat-cli`: pass
- New CLI tests added: `22`
- Request construction coverage proven:
  - group publish builds the `/messages` DTO with optional commit admission
    and optional idempotency key
  - inbox publish builds a Welcome envelope
  - typed submit-commit posts caller-provided JSON to `/commits`
  - device revoke builds the `/devices/revoke` DTO
  - group sync defaults to `after_seq = 0` and `limit = 50`, and can include a
    requester for membership-filtered sync
  - typed event and ephemeral activity commands post caller-provided JSON to
    `/events` and `/activities`
  - application delivery-effect commands post typed application-event JSON to
    `/application-events`, fetch a stored effect by message id, and request
    aggregate effect counts
  - KeyPackage inventory builds the route DTO
  - KeyPackage claim builds the route DTO
  - batch KeyPackage claim builds the route DTO with repeated owners and an
    idempotency key
  - fanout save-room, mark-prepared, and mark-done commands build the route
    DTOs
  - link-session create/get/upload/claim/release/ack/expire commands build the
    route DTOs
  - direct-room create-or-get builds the `/direct-rooms` DTO
  - account-room bootstrap, save, and list commands build the route DTOs
  - invalid-commit reporting builds the `/rooms/report-invalid-commit` DTO
  - Welcome claim and ack build the route DTOs
  - unknown CLI flags fail as usage errors
- Automated live-server CLI coverage now drives account-room bootstrap,
  typed `/commits`, idempotent submit replay, group sync, Welcome claim,
  duplicate claim hiding, idempotent activated ack, conflicting failed ack
  rejection, and account-room activation through `finitechat_cli::run`
  against a localhost Axum server.
- Additional automated live-server CLI coverage now drives health, persistent
  group publish/sync, matching idempotent publish replay, conflicting
  idempotency-key rejection, batch KeyPackage claim replay, direct claim of the
  remaining package, and fanout save/prepared/reprepare/done/get checkpoints
  through `finitechat_cli::run` against a localhost Axum server.
- Process-level binary smoke now builds `finitechat-darkmatter-server` and
  `finitechat-darkmatter`, starts the server binary with a temporary SQLite
  file, drives the CLI binary through health, publish, sync, exact idempotent
  replay, and conflict rejection, then restarts the server binary and verifies
  the persisted group log is still visible.
- Live localhost smoke verified with a temporary server on `127.0.0.1:18787`:
  - `finitechat-darkmatter http --server http://127.0.0.1:18787 health`
  - `finitechat-darkmatter http --server http://127.0.0.1:18787 publish-group --group-id cli-room --transport-group-id cli-transport --message-id cli-commit-1 --payload commit-bytes --commit-epoch 1`
  - `finitechat-darkmatter http --server http://127.0.0.1:18787 sync-group --group-id cli-room --limit 10`

Dependency note:

- The copied workspace used `rusqlite 0.37`. Adding Darkmatter
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
- `cargo test -p finitechat-client --test client_state runtime_submit_commit_removes_account_room_over_darkmatter_http_routes`: pass
- `cargo test -p finitechat-server --test http_persistence sqlite_submit_commit_route_publishes_room_entry_and_derives_membership_after_restart`: pass
- `cargo test -p finitechat-server --test http_persistence sqlite_submit_commit_replay_repairs_projection_after_partial_durable_publish`: pass
- `cargo test -p finitechat-server --test http_persistence sqlite_rejected_submit_commit_replays_rejection_after_restart`: pass
- `cargo test -p finitechat-server --test http_persistence sqlite_invalid_commit_report_blocks_typed_mutations_after_restart`: pass
- `cargo test -p finitechat-server --test http_persistence sqlite_raw_message_commit_projection_compatibility_survives_restart`: pass
- `cargo test -p finitechat-server --test http_persistence sqlite_account_room_bootstrap_rejects_raw_commit_history_without_membership_delta`: pass
- `cargo test -p finitechat-server --test http_persistence submit_commit_route_rejects_missing_staged_welcome_before_side_effects`: pass
- `cargo test -p finitechat-server --test http_persistence sqlite_submit_commit_crash_matrix_rolls_back_and_retry_converges`: pass
- `cargo test -p finitechat-server --test http_persistence sqlite_submit_commit_validates_and_consumes_claimed_key_package_after_restart`: pass
- `cargo test -p finitechat-server --test http_persistence sqlite_key_package_publish_retry_and_conflict_survive_restart`: pass
- `cargo test -p finitechat-server --test http_persistence sqlite_welcome_not_released_before_accepted_commit_over_http`: pass
- `cargo test -p finitechat-server --test http_persistence sqlite_key_package_lease_expiry_and_reclaim_survives_restart_over_http`: pass
- `cargo test -p finitechat-cli expire_key_package_lease_command_builds_expiry_request`: pass
- `cargo test -p finitechat-client --test client_state runtime_link_fanout_retries_http_submit_response_loss_without_duplicates`: pass
- `cargo test -p finitechat-client --test client_state runtime_link_fanout_tick_links_multiple_rooms_over_darkmatter_http_routes`: pass
- `cargo test -p finitechat-client --test client_state runtime_link_fanout_retries_only_failed_room_over_darkmatter_http_routes`: pass
- `cargo test -p finitechat-client --test client_state runtime_link_fanout_reprepares_after_http_same_epoch_loss`: pass
- `cargo test -p finitechat-cli link_session_commands_build_route_dtos`: pass
- `cargo test -p finitechat-server --test http_persistence sqlite_link_session_state_machine_survives_restart_over_http`: pass
- `cargo test -p finitechat-server --test http_persistence sqlite_direct_room_create_or_get_and_third_account_rejection_over_http`: pass
- `cargo test -p finitechat-server --test http_persistence sqlite_group_sync_filters_by_persisted_room_membership_projection`: pass
- `cargo test -p finitechat-server --test http_persistence sqlite_submit_commit_rejects_account_device_cap_before_side_effects`: pass
- `cargo test -p finitechat-server --test http_persistence sqlite_submit_commit_rejects_duplicate_pending_device_before_side_effects`: pass
- `cargo test -p finitechat-server --test http_persistence sqlite_submit_commit_rejects_membership_delta_structural_matrix_before_side_effects`: pass
- `cargo test -p finitechat-server --test http_persistence sqlite_typed_event_sync_returns_bounded_pages_after_restart`: pass
- `cargo test -p finitechat-server --test http_persistence sqlite_typed_event_idempotency_capacity_rejects_new_keys_but_replays_after_restart`: pass
- `cargo test -p finitechat-server --test http_persistence sqlite_removed_device_syncs_through_removal_and_cannot_send_over_http`: pass
- `cargo test -p finitechat-server --test http_persistence sqlite_typed_event_rejects_oversized_payload_without_persisting_log`: pass
- `cargo test -p finitechat-server --test http_persistence sqlite_typed_event_duplicate_message_id_with_new_idempotency_key_conflicts`: pass
- `cargo test -p finitechat-server --test http_persistence sqlite_application_delivery_effects_survive_restart_over_http`: pass
- `cargo test -p finitechat-server --test http_persistence sqlite_revoked_device_status_survives_restart_and_blocks_key_packages_over_http`: pass
- `cargo test -p finitechat-server --test http_persistence sqlite_revoked_device_blocks_welcome_activation_and_typed_routes_after_restart`: pass
- `cargo test -p finitechat-server --test http_persistence sqlite_ephemeral_activity_over_http_does_not_persist_or_advance_sequence`: pass
- `cargo test -p finitechat-server --test http_persistence sqlite_ephemeral_activity_over_http_authorizes_members_and_bounds_cache`: pass
- `cargo test -p finitechat-cli revoke_device_command_builds_revoke_request`: pass
- `cargo test -p finitechat-client --test client_state http_runtime_delivery_filters_membership_and_rejects_pending_sends`: pass
- `cargo test -p finitechat-client --test client_state reqwest`: pass
- The real `run_runtime_sync_tick` worker can replenish KeyPackages through the
  Darkmatter HTTP `/key-packages/inventory` and `/key-packages` routes.
- Reopening the HTTP server from SQLite proves the worker sees the persisted
  inventory and uploads zero duplicate KeyPackages on replay.
- HTTP KeyPackage publication retry is now proven directly at the route layer:
  an exact same-id publish can be retried after restart, the original package
  bytes remain claimable, and conflicting same-id bytes reject without adding a
  second claimable package.
- The runtime delivery adapter can claim a KeyPackage through
  `/key-packages/claim`, recover the original package metadata, compute
  the same deterministic lease token, and replay after server restart with no
  duplicate claim.
- The same worker can claim a valid serialized `WelcomeRecord` carried through
  the Darkmatter HTTP inbox, activate the Welcome locally, ack `/welcomes/ack`,
  and replay after server restart without duplicate claim or ack.
- The same worker can pull serialized `RoomLogEntry` payloads through
  Darkmatter HTTP `/sync/group`, decrypt an application entry, advance the
  client cursor, and replay without applying the entry twice.
- Group sync pagination is now proven over the HTTP wrapper and SQLite rebuild:
  typed application events fill one bounded page, the page advertises more
  results, and the next page after restart returns the remaining entry from the
  saved cursor.
- The link-fanout worker can read serialized account-room discovery records
  through the HTTP account-room directory after server restart and complete a
  discovery-only tick when the target device is already current in the room.
- The link-fanout worker can also complete a one-room later-device happy path
  over the HTTP adapter: it discovers a room from a typed bootstrap
  account-room projection, claims the later device's KeyPackage, prepares and
  submits the add-device Commit through `/commits`, syncs that Commit back
  through `/sync/group`, and the later device claims and activates the
  server-released Welcome through the HTTP inbox routes.
- The same HTTP happy path now proves the accepted add-device Commit updates
  the persisted account-room record. After reopening the HTTP server from the
  same SQLite file, discovery lists the new device in that room as pending
  without a second manual `/account-rooms` write.
- After the later device claims, activates, and acks the released Welcome, the
  HTTP server reopens from SQLite with that device marked active in the
  account-room projection.
- A remove-commit runtime test now proves the same projection path can remove
  a persisted account-room record. After reopening the HTTP server from the
  same SQLite file, discovery for the removed account no longer lists that
  room.
- When the HTTP submit response is lost after `/commits` has accepted the
  commit and Welcome publishes, the worker starts from typed bootstrap
  discovery, reloads the prepared commit from durable local state, retries the
  same HTTP idempotency keys through the typed route, completes the room, and
  leaves exactly one new group Commit and one claimed Welcome.
- A server persistence test now covers the narrower interrupted-server window
  where the commit publish operation and idempotency receipt are durable, but
  adapter account-room and room-membership projections were not written.
  Retrying typed `/commits` repairs the projections, replays the accepted
  commit response, and releases one Welcome.
- A server persistence test now covers rejected same-epoch submit replay. After
  one typed `/commits` request advances the room to epoch 1, a losing epoch-0
  add-device request is rejected before side effects, the same rejection is
  returned after server restart, no loser Welcome is published, and account-room
  discovery only includes the winning device.
- A server persistence test now covers invalid-commit repair state through a
  typed HTTP route. The reporter is checked against persisted membership
  intervals, the room-membership and account-room records are marked
  `needs_repair`, the state survives restart, and later typed `/events` and
  `/commits` fail closed with `room_not_open`.
- With two existing rooms, the same worker pages typed bootstrap account-room
  discovery one room at a time, claims two distinct target-device KeyPackages,
  submits and completes both room commits, and the later device activates both
  released Welcomes.
- If one room in a multi-room fanout is already complete and a later room's
  submit fails before HTTP accept, the worker reloads room A as `Done`, room B
  as prepared, retries only room B, and then the later device activates both
  Welcomes from the persisted Darkmatter HTTP server.
- If the fanout submit fails before HTTP accept and a competing same-epoch
  member commit wins, the worker starts from typed bootstrap discovery, syncing
  that winning commit clears the local pending commit, and the next worker tick
  reprepares/submits the fanout commit at the next epoch.
- This proves the current client runtime harness can be reused above a
  Darkmatter HTTP adapter for KeyPackage inventory/upload/claim, typed
  submit-commit, typed application events, Welcome claim/ack, requester-filtered
  ordered room pull, account-room discovery, commit-derived account-room
  updates for add/remove commits, and later-device fanout from typed bootstrap
  across the happy path, submit response-loss retry, multi-room fanout,
  partial failed-room retry, and same-epoch reprepare. Typed
  bootstrap/commit/event flows now have
  server-owned room-membership projection, and typed rooms reject raw plain
  Commit imports that lack membership deltas before they can weaken strict
  membership filtering.
- The test-local HTTP runtime adapter has been reduced to a transport harness:
  it serializes JSON into the in-process Axum router, exposes HTTP status
  errors to assertions, and injects before-accept or after-accept `/commits`
  failures. The runtime DTO mapping and validation it used to duplicate now
  live in `finitechat-client::HttpRuntimeDelivery`.
- Live-network runtime transport tests prove
  `finitechat-client::ReqwestHttpRuntimeTransport` can drive that production
  adapter against an actual localhost Axum server, including successful
  KeyPackage upload/claim, ordered room sync with cursor replay, later-device
  fanout through typed submit and Welcome activation, and a visible `404`
  server-status error.
- The route DTO boundary is now shared through `finitechat-http`: production
  `finitechat-client` and `finitechat-cli` no longer depend on
  `finitechat-server`; only test harnesses import the server crate for
  in-process routers and state.
- Link-session pairing is now proven over the HTTP wrapper and SQLite rebuild:
  encrypted pairing payloads remain opaque, duplicate/conflicting uploads and
  terminal closed states follow the old reducer rules, release/reclaim keeps
  the deterministic claim token stable, ack tokens are validated, and
  create/get/upload/claim/release/ack/expire all have CLI route DTO coverage.
- Direct-room constraints are now proven over the HTTP wrapper and SQLite
  rebuild: create-or-get persists the sorted account pair, reversed account
  order returns the same room after restart, and typed commits cannot add a
  third account to that direct room.
- Removed-device sync is now proven over the HTTP wrapper and SQLite rebuild:
  removed devices can pull their own removal commit, cannot send later typed
  events or commits, and requester-filtered sync advances over hidden
  post-removal messages without exposing them.
- Account device caps are now proven over the HTTP wrapper and SQLite rebuild:
  typed room membership counts current and pending devices by account, rejects
  fresh add commits that exceed `MAX_ACCOUNT_DEVICES_PER_ROOM`, and keeps the
  group log, Welcome inbox, account-room projection, and KeyPackage consumed
  state unchanged after rejection.
- Duplicate pending-device adds are now proven over the HTTP wrapper and SQLite
  rebuild: typed room membership rejects a second add for the same pending
  device, keeps only the original Welcome visible, avoids duplicating the
  account-room device record, and leaves the retry KeyPackage claimed.
- Malformed membership deltas are now proven over the HTTP wrapper and SQLite
  rebuild: typed `/commits` rejects wrong epochs, wrong commit id, duplicate
  add/remove entries, add/remove overlap, and incomplete add metadata before
  durable append, Welcome release, account-room projection, or claimed
  KeyPackage consumption.
- Revoked-device status is now proven over the HTTP wrapper and SQLite rebuild:
  it blocks KeyPackage publish/claim, Welcome claim/activation, typed event and
  commit senders, and typed commits that add a revoked device while preserving
  existing inventory.
- Typed application-event rejection/idempotency is now proven over the HTTP
  wrapper and SQLite rebuild: oversized payloads do not append, exact
  idempotent replay survives restart, and duplicate typed message ids with new
  idempotency keys conflict without a second durable group entry.
- Application delivery effects are now proven over the HTTP wrapper and SQLite
  rebuild: typed application-event publishes record caller-supplied delivery
  policy, push/unread/command-inbox counts survive restart, exact replay
  succeeds, and conflicting policy replay fails without changing the
  projection.
- Scoped idempotency capacity is now proven over the HTTP wrapper and SQLite
  rebuild: typed publish records count by room and sender, fresh overflow is
  rejected, exact replay remains available after the cap is reached, and the
  rejected overflow does not append to the group log.
- Ephemeral activity is now proven over the HTTP wrapper: it is authorized
  against typed room membership and revocation state, bounded per route,
  omitted from durable ordered group sync, and cleared by server restart.

## Remaining Gates

- [x] Add a process-level smoke that starts the
  `finitechat-darkmatter-server` binary with a temporary SQLite file and drives
  the `finitechat-darkmatter` CLI binary against it, not only the library-level
  CLI runner.
- [x] Run a test-suite parity audit against the baseline
  `/Users/futurepaul/dev/finite/finitechat` checkout: compare test files and
  test names, then document every intentionally reshaped, added, or still
  missing acceptance case.
- [x] Decide whether any runtime/client flow still needs process-level binary
  coverage beyond the current library-level runtime tests, live Axum tests, and
  CLI binary smoke.
- [x] Split the Darkmatter-facing delta into maintainable buckets: upstreamable
  HTTP transport work, upstreamable ordered-delivery profile work,
  adapter/application logic, and fork-only requirements.
- [x] Refresh the compatibility report after the next Darkmatter branch update
  and verify that `RequiresDarkmatterFork` still only names the ordered
  delivery profile.
- [x] Audit which preserved baseline tests still prove behavior only through
  the original fake/in-memory delivery service instead of a Darkmatter engine
  or HTTP route path, then classify them by risk and owner.
- [ ] Continue porting preserved fake/in-memory reducer proofs that still lack
  Darkmatter-backed engine, HTTP route, or runtime-delivery equivalents.
