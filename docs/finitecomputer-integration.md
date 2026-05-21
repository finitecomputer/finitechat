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

## Proposed Landing Shape

Add an encrypted mode in finitecomputer with three layers:

1. `finitechat-proto`: shared DTOs used by dashboard server routes, finited,
   finitec, and tests.
2. `finitechat-engine`: reducer/store used by `finited` in local/dev and by a
   future canary room server.
3. `finitec encrypted-chat`: runtime/client commands that manage device state,
   KeyPackages, Welcome claim/ack, room sync, and Hermes gateway bridge.

The dashboard should keep the current `FiniteChat` component contract as long as
possible. The server route can translate encrypted room state into the existing
render model while the encrypted transcript becomes canonical.

## Mapping To Current Files

`crates/finite-core/src/chat.rs`

- Keep current render DTOs for the UI.
- Add encrypted room DTOs separately. Do not overload plaintext message structs
  with MLS envelope fields.

`crates/finite-core/src/chat_runtime.rs`

- Keep as the plaintext fallback.
- Add an encrypted runtime store instead of mutating `messages` in place.
- The gateway inbox can be fed from decrypted application messages after the
  runtime device processes room sync.

`crates/finite-core/src/relay.rs` and `crates/finited/src/main.rs`

- Current relay events are short-lived file-backed commands.
- Encrypted room logs need durable ordered storage, not only event files.
- Add a room-log store with transaction semantics before production use.
- Keep machine polling for command/result flow.

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
- For existing users, create a fresh encrypted Project chat and keep old
  transcript access as plaintext archive until a separate migration plan exists.

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

