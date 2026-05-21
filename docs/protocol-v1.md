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
- `GET /v1/rooms/{room_id}/events?after_seq=N&limit=M`
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
