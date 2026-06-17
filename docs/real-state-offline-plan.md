# Real State And Offline Plan

Date: 2026-06-17
Status: active plan

## Problem Statement

Finite Chat's protocol is designed to avoid user-visible impossible states:
once a device has joined a room and has durable local MLS/client state, the
chat should remain readable and locally usable whether the server is reachable
or not. A stopped server, wrong dev URL, failed invite creation, or launch-test
fixture must not turn a real room into "needs attention" or make the app look
empty.

The current code has most of the Rust storage pieces, including encrypted
client SQLite rooms, selected room, messages, profile cache, and local outbox
rows. The remaining risk is product proof: tests and simulator runs have used
several launch/config paths, and that makes it hard to distinguish a real
protocol/client bug from a malformed dev state. We need one canonical product
state path and an online/offline matrix that exercises it directly.

## State Model

There is one real app state:

- Rust owns the durable product state: MLS state, room membership/mapping,
  selected room, local transcript projection, local outbox, profile cache,
  device list, read state, media cache references, and retry policy.
- Swift renders the Rust `AppState` and dispatches typed actions. Swift may
  hold transient OS handles or draft UI state, but it must not persist chat
  routing, room lifecycle, send eligibility, retry decisions, or protocol
  phase state.
- A room is connected when local state proves this device is an active local
  member. Server reachability is runtime connectivity, not a room lifecycle.
- Offline launch reads local SQLite first. Network sync may then update the
  state, but network failure must not hide durable rooms or transcripts.
- Diagnostic/transient stores are explicit test tools. They must not be used
  by ordinary simulator, phone, RMP, Xcode, or Home Screen launches.

This follows ADR 0008 and the RMP rule: native UI is a pure view and bounded
capability layer over Rust-owned state.

## Acceptance Criteria

The product state work is done when all of these are true:

- Stable iOS launches, RMP launches, Xcode launches, Home Screen relaunches,
  and phone installs use the same bundle id, persisted runtime identity, and
  client SQLite store unless an explicit transient flag is supplied.
- Unit and integration tests that need fake server URLs or fake app support
  paths always pass isolated `applicationSupportURL` and `configStorageURL`
  values, or explicitly opt into `FiniteChatTransient/<device>`.
- Hidden Developer settings show the active server URL, device id, config
  file, store path, transient/stable flag, runtime status, and latest raw
  transport diagnostics.
- Normal chat list and room transcript surfaces never expose raw HTTP/runtime
  diagnostics as product copy.
- Connected rooms never become `NeedsAttention` or read-only only because the
  server is down or an invite/profile/device-list action failed.
- A room with local MLS state can be opened offline, display its cached
  transcript, accept new outbound messages, and keep those messages visible
  after force close.
- Restarting the server drains the local outbox exactly once per message and
  promotes local outbound bubbles to accepted server-backed transcript rows.

## Offline Send Semantics

Sending while offline is a first-class path, not an error edge.

Target user-visible states:

- `sending`: message exists locally and delivery is being attempted.
- `undelivered`: message is saved locally, visible in the transcript, and will
  be retried when the runtime can reach the server again.
- `sent`: message is accepted by the ordered server log and projected back from
  durable state.

Implementation rules:

- `SendMessage` persists the local outbox row before attempting transport.
- Text, reply, poll, and attachment messages all use the same Rust-owned
  outbox promotion rule.
- Outbox rows keep the local message id, room id, sender, encrypted/plaintext
  projection needed for local display, delivery state, bounded failure reason,
  and retry metadata.
- Attachment outbox rows store verified local cache paths, not plaintext bytes
  in SQLite.
- Retry uses the persisted row and deterministic idempotency material. Swift
  never reconstructs an outbound payload from view state.
- On accepted append, Rust deletes the matching outbox row, removes the local
  placeholder, inserts the accepted server-backed message/event projection,
  and preserves the visible transcript position.
- Automatic retry runs from Rust on startup, after a successful sync/hint
  reconnect, and when a user opens a room. It is bounded per tick and uses
  backoff so a dead server does not create an unbounded hot loop.
- Manual retry remains a convenience action on an undelivered bubble, not the
  only way delivery recovers.

## Online/Offline Test Matrix

All tests in this matrix use the same real product store for a given account
and device. The test toggles server availability; it does not change device
ids, data dirs, bundle ids, config files, or app containers mid-test.

| Scenario | Server | Required proof |
| --- | --- | --- |
| First launch empty | on | app opens stable store, no transient flag, no fake diagnostics |
| Create room | on | room appears in list, opens transcript, local MLS mapping exists |
| Send text | on | message is accepted, survives force close and relaunch |
| Relaunch cached room | off | chat list and transcript render before network sync, no `NeedsAttention` |
| Send text offline | off | outbound bubble appears as `undelivered`, survives force close |
| Restart server | on | outbox drains automatically or through retry, accepted row replaces local bubble exactly once |
| Peer sync | on | second real client receives exactly one copy |
| Send attachment offline | off | media bubble survives force close with verified local cache path |
| Restart server for media | on | upload/send promotes to accepted encrypted blob-reference message |
| Invite action offline | off | action failure is surfaced as an action diagnostic/toast, existing room remains readable and sendable |
| Profile/device-list offline | off | stale cache or dev diagnostics only; room lifecycle does not change |
| Kill app during retry | mixed | relaunch shows either durable outbox row or accepted row, never neither |

## Implementation Phases

### Phase 1: Canonical Product Harness

- Add a product-state test harness that launches the iOS app with the same
  bundle id, same persisted config, and same client SQLite path across server
  toggles.
- Make transient diagnostics opt-in in every launch helper.
- Add a startup assertion/test that stable product launches cannot write test
  config to the normal app support directory.
- Document the exact store path in hidden Developer settings and in test logs.

### Phase 2: Room Lifecycle Cleanup

- Audit every assignment of `AppRoomState::Offline` or `NeedsAttention`.
- Define those states as admission/repair states only, not transport states for
  an otherwise connected room.
- Keep runtime connectivity in `AppState.status`, `toast`, and hidden Developer
  diagnostics until a dedicated Rust-projected connectivity field exists.
- Keep the composer available when local MLS membership exists, even if the
  most recent server operation failed.

### Phase 3: Durable Outbox Product Semantics

- Rename scary product copy from "failed" to "undelivered" for saved local
  sends that are expected to retry.
- Verify current Rust outbox paths for text, replies, polls, voice notes,
  multi-attachment sends, and cached media retries.
- Move retry scheduling into the runtime tick instead of relying on the user to
  tap Retry.
- Keep a hidden Developer view of raw transport errors for debugging.

### Phase 4: End-To-End Product Proof

- Add an iOS simulator/phone E2E that creates or opens a real room, sends
  online, kills the app, turns the server off, relaunches, sends offline,
  restarts the server, and verifies delivery.
- Add a CLI or second-simulator peer proof that the promoted message is
  received exactly once.
- Add attachment parity to the same matrix after text is stable.
- Run the matrix against both simulator and physical phone when a phone is
  attached.

### Phase 5: Cleanup And Guardrails

- Delete stale dev stores that were only useful for malformed tests, or provide
  an explicit documented reset command for them.
- Add lint/test guards for test config leaking into product app support paths.
- Keep the technical debt ledger updated until the product-state matrix is
  reliable enough to delete the debt row.

## Done List

This work is done when a new session can run one documented local command set
and prove:

1. A saved room opens with the server on.
2. The same saved room opens with the server off.
3. A message can be sent while the server is off.
4. Force close and relaunch keep that undelivered message visible.
5. Restarting the server delivers that message exactly once.
6. A peer client sees the message.
7. No normal user surface says the room is offline, needs attention, or broken
   only because the server was temporarily unreachable.

## References

- `docs/engineering-style.md`
- `docs/storage-plan.md`
- `docs/adr/0007-hint-channel-abstraction.md`
- `docs/adr/0008-rust-owned-app-runtime.md`
- `docs/technical-debt-ledger.md`
- `docs/feature-audit-marmot-pika.md`
- [RMP Architecture Bible](https://github.com/rust-multiplatform/rmp/blob/master/rmp-architecture-bible.md)
