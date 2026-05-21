# Protocol V1

## Entities

`Account`

A Nostr public key. This is user-level identity.

`Device`

One application install for one account. Every device is its own MLS leaf.

`Room`

One MLS group plus one server-ordered log. V1 has exactly one authoritative
server per room.

`Room Server`

Delivery Service for KeyPackages, ordered room log entries, Welcomes, sessions,
membership intervals, repair reports, and push wake outbox records.

## Invariants

- A room has one canonical server sequence.
- At most one Commit is accepted per room epoch.
- Clients process entries in sequence order.
- Clients validate cryptography and application policy.
- The server validates only routing envelopes and structural metadata in v1.
- A Welcome is released only after the linked Commit row is durable.
- Mutations are idempotent by account, device, method, path, and key.
- Rejected mutations after idempotency admission are replayable.
- Removed devices can fetch through their removal Commit.
- `NeedsRepair` blocks normal sends.
- Protocol limits are enforced before state mutation. Limit failures must not
  create log entries, consume KeyPackages, release Welcomes, or write
  idempotency responses.

## V1 Limits

These are protocol constants, not tuning hints:

- envelope payload: `256 KiB`;
- sync page: `100` entries and `4 MiB` of envelope payload bytes;
- direct room devices per account: `8`;
- KeyPackages claimed per request: `1`;
- Welcomes claimed per request: `32`;
- link-session payload: `1 MiB`;
- idempotency key: `128` bytes;
- account id, device id, room id, MLS group id, object ids: `128` bytes each.

The numbers are intentionally small for v1. They keep WASM memory behavior
predictable, bound retry/fanout work, and make accidental full-room reads show
up as test failures.

## Server API Sketch

Session:

- `POST /v1/session/challenge`
- `POST /v1/session/login`
- `POST /v1/devices`

KeyPackages:

- `POST /v1/key-packages`
- `POST /v1/key-packages/claim`
- `POST /v1/key-packages/release`

Rooms:

- `POST /v1/rooms`
- `GET /v1/rooms/{room_id}/events?after_seq=N`
- `POST /v1/rooms/{room_id}/events`
- `POST /v1/rooms/{room_id}/commits`

Welcomes:

- `POST /v1/welcomes/claim`
- `POST /v1/welcomes/{welcome_id}/ack`
- `POST /v1/welcomes/{welcome_id}/release`

Repair:

- `POST /v1/rooms/{room_id}/repair-reports`

Device linking:

- `POST /v1/link-sessions`
- `POST /v1/link-sessions/{id}/payload`
- `POST /v1/link-sessions/{id}/claim`
- `POST /v1/link-sessions/{id}/ack`

## Message Ids

`seq` is a room-local cursor. It is not a stable message id.

`message_id` is derived from serialized message bytes:

```text
SHA256("finite-message-id-v1" || canonical_finite_envelope_bytes)
```

`message_id` is unique per room log. A second mutation with a different
idempotency key but identical envelope bytes is rejected as a duplicate message,
not appended as a second log entry.

## Sync Page

Sync returns an explicit page:

```json
{
  "entries": [],
  "next_after_seq": 42,
  "has_more": false
}
```

Clients must use `next_after_seq` as their next cursor, not the last visible
entry they happened to receive. This matters for removed devices: the server may
scan entries after the requested cursor that the requester is no longer allowed
to receive, and the requester must still be able to advance past those filtered
entries.

`has_more` means the server stopped because a page bound was reached. It does
not mean the room is quiescent forever.

## Application/RPC Payloads

Finite Chat orders encrypted application messages. The room server sees the
`FiniteEnvelope` routing fields and opaque payload bytes; clients decrypt and
interpret the plaintext.

Finitecomputer dashboard/runtime RPC should live inside the encrypted
application payload. The plaintext can be JSON because it is client-owned
application data, not authoritative room-server state.

Example plaintext before MLS encryption:

```json
{
  "type": "finitecomputer.command.v1",
  "request_id": "req_123",
  "command": "dashboard.send_message",
  "body": {
    "project_id": "proj_abc",
    "text": "run tests"
  }
}
```

Server-side invariants still live in schema rows and transactions, not inside
this JSON.

## Membership Delta

Commit requests carry cleartext `MembershipDeltaV1` beside the opaque Commit.
The server uses it for cache and routing. Clients validate actual MLS effects.

Required structural checks:

- `base_epoch == expected_epoch`;
- `post_commit_epoch == base_epoch + 1`;
- no duplicate add devices;
- no duplicate remove devices;
- no add and remove of the same device;
- every add has a KeyPackage id/ref/hash;
- every add has a matching staged Welcome;
- every remove has a removed leaf index;
- `commit_message_id` matches the submitted Commit envelope.
