# Protocol V1

## Entities

`Account`

A Nostr public key. This is user-level identity.

`Device`

One application install for one account. Every device is its own MLS leaf.

## Identity And Secret Roots

Finite Chat v1 uses the Nostr account key as the user identity root. WorkOS or
finitecomputer login may authorize product access, but cryptographic chat
identity is proof that the user controls the Nostr private key for the account
public key in the room.

The room server is authoritative for room ordering only. It is not authoritative
for who an account or device is. Identity claims are accepted by clients only
when the Nostr-rooted credential and MLS state validate locally.

Persistent Finite Chat device secrets must be rooted in that Nostr private key,
using explicit domain separation for Finite Chat, version, account, and device
purpose. MLS is still allowed to create ephemeral or per-epoch secrets internally;
those are MLS protocol state, not a replacement account identity.

`FiniteDeviceCredentialV1` is the credential payload carried in MLS credential
identity bytes. It binds:

- Nostr account public key;
- Finite Chat device id;
- MLS leaf signing public key or credential key material;
- credential version and expiry/rotation metadata;
- Nostr account signature over the binding.

Clients must reject MLS credentials whose Nostr account signature, device id,
or MLS leaf key binding does not match the expected account/device. Changed
LeafNodes use the same binding rule.

The Nostr key authenticates the device and any persistent device root. The MLS
key material performs room encryption. These are not independent identities.
They are one account identity with per-device MLS participation.

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
- The server is authoritative for ordering, not identity.
- The server validates only routing envelopes and structural metadata in v1.
- A Welcome is released only after the linked Commit row is durable.
- Mutations are idempotent by account, device, method, path, and key.
- Rejected mutations after idempotency admission are replayable.
- Removed devices can fetch through their removal Commit.
- Removed devices cannot send new events or Commits after the removal Commit is
  the room head.
- Removed devices must not be able to decrypt post-removal application
  ciphertext, even if they obtain those bytes outside normal sync.
- `NeedsRepair` blocks normal sends.
- Protocol limits are enforced before state mutation. Limit failures must not
  create log entries, consume KeyPackages, release Welcomes, or write
  idempotency responses.
- Encrypted application messages use MLS protection. Do not add an extra
  application-message encryption layer unless a future threat model names the
  additional boundary. Local database encryption is separate at-rest protection.

## V1 Limits

These are protocol constants, not tuning hints:

- envelope payload: `256 KiB`;
- sync page: `100` entries and `4 MiB` of envelope payload bytes;
- devices per account per room: `32`;
- direct room devices per account: `8`;
- explicit KeyPackage claims per request: `1`;
- account fanout KeyPackage claims per request: `8`, one available package per
  device;
- KeyPackage inventory per device: `64` unconsumed packages, counting
  available and leased packages;
- KeyPackage payload: `64 KiB`;
- Welcomes claimed per request: `32`;
- staged Welcomes per Commit: `32`;
- account room discovery page: `256` rooms;
- Welcome payload: `1 MiB`;
- ratchet-tree payload: `1 MiB`;
- idempotency records per room/device: `4096`;
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
- `POST /v1/devices/{device_id}/revoke`

Device records are a server-side control-plane ledger, not identity proof.
Clients still decide whether a device identity is valid by verifying its
Nostr-rooted MLS credential. The server records only whether a device is
currently usable for server mutations. Revocation is terminal in v1: a revoked
device cannot upload or claim KeyPackages, claim or activate Welcomes, create
rooms, send application events, submit Commits, or be added to a room again.
MLS remove Commits are still required for the cryptographic cutoff; the device
status ledger prevents the revoked install from acquiring new server-mediated
material while room removals fan out.

KeyPackages:

- `POST /v1/key-packages`
- `GET /v1/devices/{account_id}/{device_id}/key-packages/inventory`
- `POST /v1/key-packages/claim`
- `POST /v1/accounts/{account_id}/key-packages/claim`
- `POST /v1/key-packages/release`

Uploaded KeyPackages include opaque serialized MLS KeyPackage bytes plus the
metadata the server uses for routing/cache checks. Claiming a KeyPackage returns
those exact bytes to the adding client; clients parse and verify MLS credential
identity locally.

Each device has a bounded KeyPackage inventory. The cap counts available
packages plus leased packages because both are unconsumed server-held material;
accepted add Commits consume leased packages and free inventory space. Clients
use the inventory view to keep a small target number of available packages
without pushing an unbounded upload pile into the Delivery Service. Runtime
clients persist generated upload requests in encrypted local state before
uploading generated packages, then clear each request only after server
acceptance. Exact duplicate uploads are idempotent retry; a duplicate id with
different owner, ref, hash, or payload is rejected. V1 client helpers derive
package ids from the serialized MLS KeyPackage payload hash so replenishment
does not need a persisted counter.

Account fanout claim returns at most one available KeyPackage per registered
device for the target account, ordered deterministically by device id and
KeyPackage id. This is the invite primitive for multi-device users: the server
routes packages to devices, but the adding client still verifies every
Nostr-rooted MLS credential before constructing the Commit.

Rooms:

- `POST /v1/rooms`
- `GET /v1/accounts/{account_id}/rooms?after_room_id=...&limit=N`
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

A newly linked device joins existing rooms through normal add-device Commits.
Because MLS KeyPackages are single-use, the device must replenish enough
KeyPackages for the rooms it is being linked into; each accepted room add
releases a distinct Welcome for that room. The replenishment loop should query
inventory, upload at most the missing packages needed to reach the device's
target available count, and stay under the unconsumed inventory cap.
The account-room discovery endpoint is a control-plane helper for that worker:
it pages over current/pending membership rows for an account and returns room
head metadata plus the account's current devices. It is not an authorization
oracle for identity; clients still verify Nostr-rooted MLS credentials and the
server only orders the resulting Commits.

## History Policy

V1 room history starts for a device at that device's accepted add Commit. A
newly added device may sync the add Commit and later room entries, including
messages sent before it acked its Welcome, but the room server must not replay
pre-membership room log entries as ordinary history for that device.

Pre-invite history recovery is a separate product protocol. It must be provided
by encrypted backup or an explicit member-to-member history-share message, not
by making the server authoritative over old plaintext or hidden key access.

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

## Idempotency Capacity

Idempotency records are durable retry state. The room server must replay an
existing record even when the room/device ledger is full.

When a room/device already has `4096` idempotency records, a new mutation with a
new idempotency key is rejected with `IdempotencyCapacityExceeded` before side
effects. The server must not silently delete old records to make room, because
that would turn a lost response retry into a possible duplicate mutation.

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
The server uses it for cache and routing. Clients validate actual MLS effects
by processing ordered Commit log entries with OpenMLS before sending or
decrypting messages in the next epoch.

Required structural checks:

- `base_epoch == expected_epoch`;
- `post_commit_epoch == base_epoch + 1`;
- update/rekey Commits may have no membership delta rows;
- no duplicate add devices;
- no duplicate remove devices;
- no add and remove of the same device;
- every add has a KeyPackage id/ref/hash;
- every add has exactly one matching staged Welcome;
- every staged Welcome has non-empty opaque Welcome bytes and non-empty
  ratchet-tree bytes, both bounded to `1 MiB`;
- every remove has a removed leaf index;
- `commit_message_id` matches the submitted Commit envelope.

The room server stores staged Welcome and ratchet-tree bytes as opaque payloads
linked to the accepted Commit. It validates ids, sizes, and one-to-one matching
with membership adds; it does not parse or trust the MLS contents. Claiming a
Welcome returns these exact bytes to the recipient device.

For multi-device invites, one MLS Commit may add several devices from the same
account. Each added device receives its own Welcome record, but the opaque MLS
Welcome bytes may be the same batch Welcome containing secrets for all added
leaves. A device becomes a member interval at the accepted Commit seq even
before it acks the Welcome, so it can sync messages after that seq; it cannot
send until its own Welcome is claimed, activated, and acked.
