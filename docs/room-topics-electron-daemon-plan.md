# Room/Topics Electron Daemon Plan

Status: active implementation plan.

## Goal

Build a desktop Finite Chat client that proves the room/topic/segment product
model end to end while keeping `finitechat-core::AppState` and `AppAction` as
the source of truth for CLI, iOS, Electron, hosted web bridge, and runtime
daemon behavior.

This is not a compatibility layer for legacy dashboard chat. The legacy web UI
is a source of proven frontend code and interaction detail, not an API or data
model to preserve. The implementation should deepen the Finite Chat model:
Rooms own membership and encryption, Topics own work lanes inside a Room, and
Segments mark context/reset boundaries inside a Topic.

## Product Shape

- A Room is the MLS-backed membership and delivery boundary.
- A Topic is the normal user-facing "new chat" lane inside a Room.
- A Segment is a context boundary inside a Topic, usually created by `/new` or a
  first-class reset action.
- Messages belong to a Room and may belong to a Topic.
- Runtime state, command results, and activity are projected through typed
  Finite Chat app events, not inferred from transcript strings.
- Electron and iOS render Rust-projected state; they do not own sync, room
  admission, retries, send eligibility, topic semantics, or runtime command
  policy.

## Musts

- Keep the daemon API as close as practical to serialized `AppState`,
  `AppUpdate`, and typed `AppAction`/intent shapes from `finitechat-core`.
- Add missing topic and segment concepts to `finitechat-core` first, then use
  them from Electron.
- Import real frontend code from the legacy chat UI where it carries product
  value: transcript rendering, composer ergonomics, attachments, run-state
  affordances, responsive layout, and existing shadcn/ui primitives.
- Keep frontend-only state limited to window/layout concerns, drafts, focus,
  local file handles, protocol-handler plumbing, and optimistic rendering that
  is reconciled by Rust state.
- Test against the current `finitecomputer-v2` runtime image shape:
  `deploy/finite-computer/images/runtime.Dockerfile`, the hosted `/invite`
  health surface, and the packaged Hermes/finitechat plugin layout.
- Register and handle `finite://join?...` in the desktop shell and route it to
  Rust-owned scan target handling.
- Preserve local-device E2EE language only for the Electron daemon mode where
  device secrets stay on the user's machine.

## Must Nots

- Do not recreate `chat.bootstrap`, `chat.list_threads`, `chat.send_message`,
  or the machine relay API as the desktop daemon contract.
- Do not map Topics through a legacy `Thread` compatibility model.
- Do not make "new chat" create a new Room unless membership changes.
- Do not hide topic/segment gaps in Electron-local state.
- Do not create a second desktop-only projection that iOS cannot reuse.
- Do not add hosted web E2EE product copy. Hosted web bridge mode is useful web
  chat, but it is a trusted server client if the server holds device secrets.

## Phase 1: Core Topic Projection

Add first-class topic/segment projection to `finitechat-core`.

Scope:

- Add `AppTopicSummary` and selected-topic fields to `AppState`.
- Add topic-aware room details where useful.
- Project messages for the selected `(room_id, topic_id)` pair.
- Preserve unscoped room messages for older or system events.
- Add `AppAction::CreateTopic`, `OpenTopic`, `RenameTopic`, `ArchiveTopic`,
  and `StartSegment`.
- Add topic-aware message send actions or extend send actions with
  `conversation_id`.
- Keep existing iOS-compatible behavior compiling while the iOS UI remains
  room-first.

Acceptance:

- Rust tests prove creating a topic appends/loads conversation metadata.
- Rust tests prove opening a topic changes selected messages without changing
  Room membership.
- Rust tests prove `/new`-equivalent segment creation appends a segment boundary
  in the selected topic, not a new Room or Topic.
- Existing invite, send, attachment, poll, receipt, and device tests still pass.

## Phase 2: Daemon Surface

Add a daemon crate or CLI subcommand that exposes the core app model over a
local authenticated HTTP/SSE boundary.

Preferred shape:

- `GET /v1/app/state` returns serialized `AppState`.
- `GET /v1/app/updates` streams `AppUpdate` as SSE.
- `POST /v1/app/actions` accepts typed intent JSON that maps directly to
  `AppAction` or narrow daemon-only lifecycle actions.
- `GET /v1/healthz` returns process, server URL, device id, and store path
  diagnostics without plaintext message bodies.

Local security:

- Bind to loopback by default.
- Use a random per-install bearer token or Unix-domain socket where practical.
- Store daemon state under the platform app-support directory.
- Keep account secrets in the shared Finite identity path or an explicit
  platform secret-store bridge, not in Electron renderer storage.

Acceptance:

- Daemon can open a stable store, emit initial `AppState`, stream updates, and
  dispatch core actions.
- Daemon restart preserves identity, device id, selected room/topic, pending
  outbox, invite state, and sync cursors.
- Daemon can scan a `finite://join?...` URL and converge through Rust-owned room
  admission.

## Phase 3: Electron Shell With Imported Legacy UI

Create an Electron app that vendors/adapts the proven legacy chat frontend code
instead of rewriting the interface from scratch.

Scope:

- Import the legacy `FiniteChat` component family and related CSS/patterns as a
  starting point.
- Replace data hooks with `AppState`/`AppAction` daemon bindings.
- Reuse existing shadcn/ui primitives already used by the legacy dashboard.
- When new chat-specific UI primitives are needed, evaluate current shadcn chat
  components and add only primitives that fit the domain.
- Keep the first viewport as the usable app, not a landing page.

Acceptance:

- Electron renders Rooms, Topics, selected Topic transcript, composer,
  attachments, activity, and runtime state from daemon `AppState`.
- Creating "New chat" creates a Topic.
- `/new` or reset creates a Segment boundary in the selected Topic.
- UI updates arrive from daemon SSE and reconcile with local draft/layout state.
- `finite://join?...` links open the app and dispatch `ScanTarget`.

## Phase 4: Cross-Device And Hosted Runtime Proof

Prove the product goal against real Finite Chat protocol participants.

Matrix:

- Electron daemon device and iOS app device share one Nostr account/npub but
  have different device ids.
- Hosted Docker runtime from `finitecomputer-v2` joins as its own runtime
  principal through the current runtime image shape.
- All participants use the deployed/default server unless explicitly testing a
  branch server.

Acceptance:

- Electron and iOS can both send as the same account; the runtime sees the same
  account/npub with distinct sender device ids.
- A Topic created from Electron is visible to iOS after sync.
- A Segment created in Electron is visible to iOS as an ordered boundary.
- Runtime replies and activity land in the selected Topic.
- Docker runtime restart preserves agent identity, invite room, topic state,
  Hermes memory, and Finite Chat state.

## Phase 5: Promote To iOS

Use the Electron-proven room/topic model to reshape the iOS app.

Scope:

- Add topic navigation to iOS over the same `AppState`.
- Move the single Start Chat flow toward selecting/creating Rooms and Topics.
- Render Segment boundaries in the transcript.
- Keep SwiftUI as a renderer and OS-capability bridge.

Acceptance:

- iOS uses the same topic/segment state and action names as Electron.
- Product harness proves topic create/open/send/segment on iOS simulator and
  then physical device.
- No Electron-only topic semantics remain.

## Phase 6: Hosted Web Bridge And TEE Candidate

Only after local Electron daemon semantics are stable, evaluate a hosted bridge
or TEE deployment.

Rules:

- A hosted bridge may decrypt on behalf of a web UI only under
  `hosted_trusted_server_client` disclosure.
- A TEE bridge must use the same daemon surface, storage layout, and runtime
  evidence gates as local daemon/Electron where possible.
- Do not fork the app protocol for hosted web.

Acceptance:

- Hosted bridge renders the same room/topic projection as a derived web surface.
- Trust disclosure is explicit.
- The same daemon tests run against the TEE/hosted deployment with only
  provider-specific state mount and ingress differences.

## Protocol Consolidation Gates

The Electron daemon is a protocol proving surface, not a place to hide protocol
drift. A build is not considered playable unless the current app, daemon,
server, and Hermes runtime image all pass the same finitechat contract:

- `/health` reports the expected `server_contract_version`, finitechat source
  commit, and `source_dirty: false` before any production/default-server test.
- Runtime clients treat `server_contract_version` as a minimum
  transport/admission contract, not an exact encrypted app protocol match.
  Exact commit/contract matching belongs to release deployment gates.
- Delivery `MemberId` remains the compact opaque `fcdev1` route id derived from
  typed `DeviceRef`; JSON `DeviceRef` blobs must not re-enter HTTP delivery
  routing.
- Identity-sensitive checks read typed Finite identity from KeyPackage
  metadata, Welcome payloads, encrypted application payloads, or
  room-membership projections, then verify any compact route id only as a
  routing projection.
- The real Docker runtime image must complete room admission and answer at
  least two Hermes turns with an Electron-style long device id before promotion.
- Home-server failures must not block room-server invite finalization when the
  invite's room server is healthy; sync should report failure only when no
  useful progress was possible.

These gates are deliberately product-level. If Electron, iOS, CLI, or Hermes
need a compatibility shim to talk to the current server, stop and fix the core
protocol or release alignment instead.

## Evaluation Design

Core tests:

- Topic create/open/rename/archive projection.
- Topic-scoped message send and selected-message window.
- Segment boundary projection and replay.
- Invite scan/join does not lose selected topic state.
- Same-account multi-device visibility.

Daemon tests:

- AppState JSON schema round-trip.
- Action dispatch and update stream.
- Restart survival across pending send, pending invite, selected room/topic, and
  outbox drain.
- Loopback auth rejects unauthenticated requests.

Frontend tests:

- Imported legacy composer can send text and attachments through daemon actions.
- New chat creates a Topic.
- Segment creation renders an ordered divider.
- Protocol handler opens invite scan flow.
- Responsive desktop/mobile-width layouts do not overlap text or controls.

Runtime image checks:

- Build or inspect the latest `finitecomputer-v2` runtime image shape.
- Verify the image packages `finitechat`, Hermes, plugin files, entrypoint,
  health server, `/healthz`, and `/invite`.
- Run the local Docker canary when credentials/environment are available.
- If full canary cannot run, record the missing env and run static image-shape
  checks plus unit tests that validate the `/invite` payload contract.
