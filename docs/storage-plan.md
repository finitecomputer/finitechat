# Storage Plan

## Decision

Use three storage profiles:

- Client/device: encrypted local SQLite for MLS client state, pending outbound
  work, inbound event cache, and device-linking state.
- Local/dev and first server proof: SQLite.
- Hosted production room server: Postgres.

SQLite is not "just client testing" in this repo. It is the first durable
server proof because it forces every reducer invariant through a transaction
and a restart boundary. That makes it useful before the HTTP API exists.

Postgres is still the production target for hosted room servers because it has
stronger operational fit for multi-process API nodes, queue workers, retention
jobs, migrations, backups, observability, and canary rollback.

## Current SQLite Scope

`finitechat-client` has the first local client SQLite store:

- `client_device_states`

The table stores one encrypted binary snapshot per account/device. The
plaintext snapshot contains the Nostr-rooted device profile metadata needed to
reload, the Finite Chat room id to MLS group id mapping, and OpenMLS storage
records for signer, group, and message-secret state. The wrapping key is
derived from the user's Nostr secret and device id using HKDF with Finite Chat
domain separation, and the account/device lookup key is bound into AEAD AAD.

This is application-level SQLite encryption for the client state snapshot, not
SQLCipher. SQLite metadata, row counts, WAL behavior, and account/device lookup
ids remain visible to the local machine. Production still needs the unlock
policy that decides whether the Nostr key comes from OS keychain, user
passphrase, hardware-backed storage, or an already-unlocked finitecomputer
runtime.

`finitechat-store` now uses normalized SQLite tables that mirror the intended
Postgres shape:

- `rooms`
- `direct_rooms`
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
membership, KeyPackages, Welcomes, and link sessions are schema rows. The
client store uses a bounded binary snapshot because OpenMLS storage is already
a local opaque provider snapshot, and encrypting it as one unit avoids leaking
OpenMLS storage-key names into SQLite indexes.
KeyPackage bytes are a `BLOB` column on `key_packages`. Welcome payload and
ratchet-tree bytes are `BLOB` columns on `welcomes`; the server keeps them
opaque and only enforces protocol bounds before mutation.
Account-level KeyPackage fanout is a bounded query over indexed schema state,
not a JSON scan: it claims one available package per device and leaves extra
packages for later group invites or retry flows.

## Production Schema Direction

The Postgres schema should keep this same model:

- `rooms`
- `direct_rooms`
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
