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

It proves:

- accepted and rejected idempotency responses survive reopen;
- Commit side effects are persisted together;
- KeyPackage leases and consumption survive reopen;
- Welcome release, claim, ack, failure, and resume states survive reopen;
- direct-room identity constraints survive reopen;
- link-session state survives reopen.

The SQLite shape flushed out two production-schema requirements:

- membership rows need stable string/table keys, not serialized struct keys;
- replayable rejects require durable serialization of typed engine errors.

The only JSON stored by the server store is `idempotency_records.response_json`,
which is a bounded typed replay value. Room state, message ordering,
membership, KeyPackages, Welcomes, and link sessions are schema rows.

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
4. append exactly one log entry;
5. advance room epoch;
6. update membership interval cache;
7. consume KeyPackages;
8. release Welcomes;
9. persist idempotency response;
10. enqueue opaque push wakes.

Rejected mutations admitted under an idempotency key must also persist their
typed rejection result so client retries receive the same answer after restart.
