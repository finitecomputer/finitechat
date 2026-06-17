# Storage Plan

## Decision

Use three storage profiles:

- Client/device: encrypted local SQLite for MLS client state, pending outbound
  work, inbound event cache, and device-linking state.
- Local/dev, first server proof, and self-hosted single-node production: SQLite.
- Hosted multi-node room server: Postgres.

SQLite is not "just client testing" in this repo. It is the first durable
server proof because it forces every reducer invariant through a transaction and
a restart boundary. For self-hosted Finite Chat, the single-writer property is a
strength: it matches the room-server sequencer model, keeps deployment small,
and makes transaction order easy to reason about.

Postgres is still the production target for hosted multi-node room servers
because it has stronger operational fit for multiple API processes, queue
workers, retention jobs, migrations, backups, observability, and canary
rollback.

## Current SQLite Scope

`finitechat-client` has the first local client SQLite store:

- `client_device_states`
- `client_app_rooms`
- `client_app_state`
- `client_app_messages`
- `client_app_outbox`
- `client_app_profiles`

`client_device_states` stores one encrypted binary snapshot per
account/device. The plaintext snapshot contains the Nostr-rooted device
profile metadata needed to reload, the Finite Chat room id to MLS group id
mapping, the per-room applied server cursor, pending claimed Welcome payloads,
durable link-fanout plans and prepared Commit replay values, and OpenMLS
storage records for signer, group, and message-secret state.

`client_app_rooms` stores encrypted local application-room metadata that is not
MLS state but is required to render the chat list and resume room work after
restart. Rows are scoped to the owning account/device and keyed by `room_id`;
the payload stores the room display name, visible lifecycle/status, pending
join invite URL, and creator-owned invite watch URL. The row payload is sealed
with the same client-store key as the device snapshot, and the AEAD AAD binds
owner and room id so copied or tampered rows fail closed on load. Room
creation persists the device state and app-room metadata in one SQLite
transaction; scan, PIN submission, join-finalization, and invite-creation
transitions persist their visible room projection before returning to Swift.
Startup reconstructs the chat list from the union of persisted app-room rows
and MLS-known rooms, because a pending invite can be user-visible before the
device has joined the MLS group.

`client_app_state` stores encrypted single-row application navigation state
owned by Rust. The first field is `selected_room_id`, which lets a phone
force-close and reopen into the same selected transcript before any network
sync. Swift mirrors this field into native navigation; it does not decide which
room is selected or persist chat routing on its own. The AEAD AAD binds the row
to the owning account/device so copied state fails closed.

`client_app_messages` stores the bounded local application-message projection
that powers chat lists and room views. It is not an authoritative server log
and it is not a profile-style cache: each row is scoped to the owning
account/device, keyed by `(room_id, message_id)`, ordered by local insertion,
and contains the authenticated sender plus decrypted application plaintext
encrypted at rest. The row plaintext is sealed with the same client-store key
as the device snapshot, and the AEAD AAD binds owner, room id, sequence,
message id, and authenticated sender so copied or tampered rows fail closed
on load. The table has owner and owner/room/seq indexes so startup can load a
bounded recent projection without replaying room history.

`client_app_outbox` stores encrypted local pending/failed chat sends that have
not become accepted server log entries. Outbox rows are scoped to the owning
account/device and keyed by `(room_id, message_id)`, where `message_id` is a
local client id. The encrypted payload carries the sender, decrypted application
plaintext, and delivery state (`pending` or `failed` with a bounded reason).
Runtime startup merges these rows into the Rust-owned chat projection after
accepted messages/events, so a force-close after a failed send reopens with the
visible failed bubble instead of an empty transcript. When a send is accepted by
the room server, the runtime writes the accepted app message/event projection
and deletes the matching outbox row.

The wrapping key is derived from the user's Nostr secret and device id using
HKDF with Finite Chat domain separation. SQLite metadata, row counts, WAL
behavior, and account/device lookup ids remain visible to the local machine;
the client store is application-level SQLite encryption, not SQLCipher.

Client code should use store-backed operations for persisting claimed Welcomes,
Welcome activation, link-fanout preparation/completion, and ordered-log
application. These operations persist MLS state and the applied cursor together,
so a restart after a successful write can skip replayed log entries instead of
asking OpenMLS to reprocess an already-applied Commit or application message.
Persisting claimed Welcomes also means a device can recover after the server has
moved a Welcome from released to claimed, but before local activation has
completed. Persisting prepared fanout Commits means a device can restart after
creating local pending MLS state and still submit the exact server request that
matches that pending Commit.

Received application messages are inserted in the same SQLite transaction that
persists the device cursor that consumed them. Own sends are first inserted into
`client_app_outbox` before network delivery, then promoted by deleting the
outbox row after the server append is accepted and the accepted app
message/event projection is saved. Swift and the app runtime render the Rust
state and do not own persistence. Startup reads the bounded SQLite app-state,
room, message, outbox, and profile projections before network sync; delivery
failure during startup must return the saved chat list and selected transcript
as offline local state, not an empty UI. Full room-history sync remains a
repair/recovery path, not the ordinary way the UI gets messages after launch.

Production still needs the unlock policy that decides whether the Nostr key
comes from OS keychain, user passphrase, hardware-backed storage, or an
already-unlocked finitecomputer runtime.

`finitechat-blob` owns the first attachment/blob boundary. It does not add a
new database yet. Clients encrypt attachment bytes with per-attachment
AES-256-GCM key material, upload only ciphertext to a Blossom-compatible
content-addressed store, and put the blob reference, hashes, key, nonce,
filename, MIME type, and dimensions inside the encrypted application payload.
The blob store sees ciphertext bytes, a ciphertext content type, ciphertext
hash, object size, URL, timing, and requester metadata. It does not receive the
plaintext filename or MIME type in the Finite Chat abstraction.

The current proof uses an in-memory content-addressed store plus a
Blossom-shaped HTTP request/response boundary so the cryptographic and metadata
invariants are tested without adding a networking dependency. The
finitecomputer integration should replace runtime-local attachment bytes by
executing that boundary against real Blossom-compatible storage, not by
changing the encrypted reference shape.

`finitechat-store` now uses normalized SQLite tables that mirror the intended
Postgres shape:

- `rooms`
- `direct_rooms`
- `devices`
- `room_log_entries`
- `room_membership_intervals`
- `key_packages`
- `welcomes`
- `link_sessions`
- `idempotency_records`

The store still uses SQLite for local/dev and first-server proof, but the
authoritative state layout is no longer a JSON snapshot.
SQLite connections set `journal_mode = WAL` and `synchronous = FULL`
explicitly so tests do not inherit durability behavior from library defaults.
Write transactions use `BEGIN IMMEDIATE`, and room-head updates include the
epoch and sequence they consumed. Commit rows also have a partial unique index
on `(room_id, epoch)` for `kind = 'commit'`.

It proves:

- accepted and rejected idempotency responses survive reopen;
- idempotency capacity rejects new mutations without breaking existing replay;
- Commit side effects are persisted together;
- Commit transaction rollback after intermediate side effects converges on retry;
- same-epoch Commit losers cannot create duplicate log rows or Welcomes;
- device revocation status survives reopen and blocks new server-mediated
  device material or mutations;
- randomized in-memory-vs-SQLite operation sequences stay state-equivalent
  across mixed accepted/rejected mutations and exact idempotent retries;
- KeyPackage leases, consumption, and opaque payload bytes survive reopen;
- account-level KeyPackage fanout claims return one available package per
  device and persist the leases across reopen;
- Welcome release, claim, ack, failure, resume states, and opaque payload bytes
  survive reopen;
- direct-room identity constraints survive reopen;
- link-session state survives reopen.

The SQLite shape flushed out two production-schema requirements:

- membership rows need stable string/table keys, not serialized struct keys;
- replayable rejects require durable serialization of typed engine errors.

The only JSON stored by the server store is `idempotency_records.response_json`,
which is a bounded typed replay value. Room state, message ordering,
membership, device status, KeyPackages, Welcomes, and link sessions are schema
rows. The client store uses a bounded binary snapshot because OpenMLS storage
is already a local opaque provider snapshot, and encrypting it as one unit
avoids leaking OpenMLS storage-key names into SQLite indexes.
Device status is deliberately small: active or revoked. It is not identity
authority and does not replace client-side Nostr credential verification, but
it gives the server a durable way to block revoked installs from new
KeyPackages, Welcome activation, sends, Commits, and future add-device Commits.
KeyPackage bytes are a `BLOB` column on `key_packages`. Welcome payload and
ratchet-tree bytes are `BLOB` columns on `welcomes`; the server keeps them
opaque and only enforces protocol bounds before mutation.
Account-level KeyPackage fanout is a bounded query over indexed schema state,
not a JSON scan: it claims one available package per device and leaves extra
packages for later group invites or retry flows.
Account-room discovery follows the same rule. The server pages over indexed
current membership rows for an account, returns room head metadata plus the
account's current/pending devices, and rejects duplicate current/pending device
adds before consuming another KeyPackage or releasing another Welcome. The
general devices-per-account-per-room cap keeps group-room fanout bounded, while
direct rooms keep the tighter direct-room cap.

## Production Schema Direction

The Postgres schema should keep this same model:

- `rooms`
- `direct_rooms`
- `devices`
- `room_log_entries`
- `room_membership_intervals`
- `key_packages`
- `welcomes`
- `idempotency_records`
- `link_sessions`
- `repair_reports`
- `push_outbox`

The critical transaction remains the same:

1. lock room row;
2. validate expected epoch and sender membership;
3. validate KeyPackage leases for adds;
4. validate staged Welcome payloads for adds;
5. append exactly one log entry;
6. advance room epoch;
7. update membership interval cache;
8. consume KeyPackages;
9. release Welcomes with opaque Welcome and ratchet-tree bytes;
10. persist idempotency response;
11. enqueue opaque push wakes.

The mutation path must not reconstruct the full room log. Full log validation is
for read/replay paths; append and Commit validation use the indexed room head,
membership intervals, and idempotency rows needed for that single mutation.

Rejected mutations admitted under an idempotency key must also persist their
typed rejection result so client retries receive the same answer after restart.
