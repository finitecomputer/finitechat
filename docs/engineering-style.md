# Engineering Style

This repo borrows the parts of Tiger Style that fit Finite Chat's risk profile:
do the production-shaped thing early, keep control flow explicit, and make
invariants executable.

Reference: https://github.com/tigerbeetle/tigerbeetle/blob/main/docs/TIGER_STYLE.md

## Local Rules

- Authoritative server state must use schema, constraints, and transactions.
  Do not add JSON blobs for state that the room server must query, lock,
  constrain, or recover.
- JSON is allowed for wire DTOs, encrypted application payloads, and bounded
  typed replay values such as idempotency responses.
- Store APIs must not hide database or corruption errors behind `Option`.
- Every mutation that changes room state must have a test covering the positive
  path and at least one negative/replay path.
- Prefer explicit branch structure for validation. Avoid clever `Option` or
  iterator control flow where the code is enforcing safety properties.
- Persist replayable errors intentionally. Rejected mutations admitted under an
  idempotency key are part of durable server state.
- Keep fake-MLS tests honest: if a behavior will later depend on OpenMLS, mark
  the fake gate and keep the server-side invariant separate from crypto truth.

## Assert Boundary

Use handled errors for client mistakes and operating conditions:

- wrong epoch;
- stale KeyPackage;
- duplicate idempotency key with a different body;
- missing Welcome;
- room needing repair.

Use assertions or corruption errors for internal contradictions:

- room `last_seq` does not match persisted log rows;
- membership table key does not match the stored device;
- a Welcome ack has no corresponding inactive membership interval;
- persisted idempotency response kind disagrees with its operation.

The goal is not to maximize asserts mechanically. The goal is to keep invalid
states from becoming ordinary states.

