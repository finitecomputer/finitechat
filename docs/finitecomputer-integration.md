# Finitecomputer Integration

## Existing Seam

Do not replace the current outbound relay shape. It is the right boundary for
hosted runtimes:

```text
Dashboard route
  -> finited admin API
  -> machine relay event
  -> finitec relay run inside runtime
  -> ChatRuntime / Hermes gateway
  -> finitec gateway send/edit
  -> relay snapshot / stream
  -> Dashboard route
```

Finite Chat should first replace the semantics behind "chat event" and "chat
message", not the dashboard shell.

The hosted finitecomputer web path is a trusted-server-client mode, not true
end-to-end encryption. A server-side Rust Finite Chat client may decrypt room
state and expose the existing dashboard DTOs so the web frontend changes as
little as possible. Only local daemon, Electron, native mobile, or other clients
that keep Finite Chat device secrets on the user's device should be described
as end-to-end encrypted.

## Command/RPC Mapping

Dashboard and Hermes commands ride inside Finite Chat durable application
events, not a separate RPC channel. The dashboard sends
`runtime.command.request` as an encrypted durable event in the Project runtime
room. The runtime device syncs ordered room entries, decrypts the request,
validates sender and target policy locally, persists a request ledger entry, and
then schedules execution.

Intermediate states such as thinking, working, tool-running, upload progress,
or runtime presence use ephemeral activity events with `push_policy = never`.
User-visible output, durable checkpoints, terminal success, terminal failure,
and cancellation results are durable application events.

The current relay shell can still wake/poll the runtime. A wake only triggers
sync; it must not directly execute work from an external event callback. Optional
cleartext wake hints may wake a specific runtime device, but the decrypted
command target and local policy remain authoritative.

Transport should start with HTTP mutations, cursor-based pull sync, and SSE
hints. WebSockets are not needed for v1. SSE and relay wakes only trigger sync;
durable ordered room state remains the source of truth.

## Proposed Landing Shape

Add an encrypted mode in finitecomputer with four layers:

1. `finitechat-proto`: shared DTOs used by dashboard server routes, finited,
   finitec, and tests.
2. `finitechat-engine`: reducer/store used by `finited` in local/dev and by a
   future canary room server.
3. `finitechatd`: local daemon that owns device secrets, MLS state, sync,
   projections, command ledger, and attachment download/upload.
4. `finitec encrypted-chat`: runtime/client commands that manage device state,
   KeyPackages, Welcome claim/ack, room sync, and Hermes gateway bridge.

The dashboard should keep the current `FiniteChat` component contract as long as
possible. The server route can translate encrypted room state into the existing
render model while the encrypted transcript becomes canonical.

The standalone Finite Chat product should grow from the CLI/daemon first. The
finitecomputer integration can consume that daemon surface initially; Electron
and native apps can reuse the same Rust core when the local true-E2EE clients
are ready.

## Mapping To Current Files

`crates/finite-core/src/chat.rs`

- Keep current render DTOs for the UI.
- Add encrypted room DTOs separately. Do not overload plaintext message structs
  with MLS envelope fields.
- Model receipts as encrypted durable `chat.receipt` events with
  `push_policy = never`, not as user-notifying messages.
- Replace current attachment semantics with encrypted Blossom-compatible blob
  references in decrypted message DTOs.

`crates/finite-core/src/chat_runtime.rs`

- Keep as the plaintext fallback.
- Add an encrypted runtime store instead of mutating `messages` in place.
- The gateway inbox can be fed from decrypted application messages after the
  runtime device processes room sync.
- Drive encrypted device maintenance through `finitechat_client::run_runtime_sync_tick`:
  it replenishes KeyPackages, persists replayable pending uploads with local
  MLS state before upload, claims and activates Welcomes, retries pending
  Welcome acks, and applies bounded ordered room pages into the encrypted client
  store.
- Drive later-device room fanout through `finitechat_client::run_link_fanout_tick`:
  after a target device is registered and replenished, an existing device pages
  the account's rooms, claims one target-device KeyPackage per room, persists
  the room plan and prepared MLS Commit, submits idempotently, and completes
  from the ordered room log.

`crates/finite-core/src/relay.rs` and `crates/finited/src/main.rs`

- Current relay events are short-lived file-backed commands.
- Encrypted room logs need durable ordered storage, not only event files.
- Add a room-log store with transaction semantics before production use.
- Keep machine polling for command/result flow.
- Add SSE as a hint channel for local/dev and hosted routes, but keep pull sync
  as the repair path.

`crates/fc/src/main.rs`

- Add commands under a separate namespace first:
  - `finitec encrypted-chat device register`
  - `finitec encrypted-chat keypackages upload`
  - `finitec encrypted-chat rooms sync`
  - `finitec encrypted-chat gateway poll`
  - `finitec encrypted-chat gateway send`
- Once stable, route existing `finitec gateway` through encrypted chat for
  canary Projects.

`integrations/hermes/finite-platform/adapter.py`

- Keep the adapter CLI-only.
- It should not import finitechat internals or start a local HTTP service.
- The command it shells out to can change from `gateway` to encrypted gateway
  subcommands after the finitec surface is stable.

`apps/dashboard/src/lib/finite-relay-client.ts`

- Keep the relay client shape for dashboard calls.
- Add feature-gated encrypted endpoints only in server-side routes, not in the
  client component, until the render model needs new states such as
  `NeedsRepair` or device linking.

## Migration Strategy

Start with new canary rooms only.

- Do not import old Pika rooms.
- Do not transparently convert existing finitecomputer plaintext threads.
- For existing users, create a fresh encrypted Project chat and expose old
  finitecomputer plaintext threads as read-only archived chats until a separate
  migration plan exists.

## Runtime State

Suggested runtime directories:

```text
$HOME/.finite/chat/plaintext/       # current fallback
$HOME/.finite/chat/encrypted/
  device.json                       # account/device identity metadata
  client.sqlite3                    # local MLS and sync state, encrypted at rest
  attachments/
  keypackages/
```

Server/control-plane state should not live in the runtime directory. Room logs,
Welcome delivery records, and idempotency results belong to the room server or
`finited` local dev store.

## Feature Flags

Use explicit flags during integration:

- `FINITE_CHAT_MODE=plaintext|encrypted|dual`
- `FINITE_CHAT_ROOM_SERVER_URL`
- `FINITE_CHAT_DEVICE_ID`
- `FINITE_CHAT_STATE_DIR`

`dual` is for local validation only: write encrypted rooms while preserving the
current plaintext render path.

## First Finitecomputer PR

Keep it boring:

1. add crates and docs;
2. wire compile/test only;
3. add no dashboard behavior change;
4. add one local CLI smoke that creates a fake-MLS room and proves Commit
   ordering.

After that, add the room server API and dashboard local-loop feature flag.
