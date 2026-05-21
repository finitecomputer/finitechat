# Source Notes

Research date: 2026-05-21.

Local sources read:

- `https://github.com/justinmoon/finite`, cloned to a temp directory for the
  finite-chat protocol plan.
- `https://github.com/justinmoon/pika`, cloned to a temp directory for Pika,
  Marmot, MDK, OpenClaw, and mobile/runtime lessons.
- `../finitecomputer`, local checkout for the current plaintext chat relay,
  dashboard shape, finitec runtime connector, and Hermes platform adapter.

## Justin's Finite Planning Repo

The finite planning repo is the protocol authority for this seed. Its plan says:

- build a new Rust workspace;
- keep Pika and Cordn as prior art only;
- use MLS with Nostr account identity;
- make one room server the authoritative sequencer for each room;
- start with deterministic fake-MLS simulation before OpenMLS;
- keep Signal-level metadata protection out of v1;
- use explicit per-device identity and device binding from day one;
- treat DMs as MLS rooms;
- require Welcome release to be coupled to durable Commit acceptance.

Important planning files:

- `todos/finite-chat-protocol/plan.md`
- `docs/protocol/00-overview.md`
- `docs/protocol/02-protocol-sketch.md`
- `docs/protocol/03-decisions-and-risks.md`
- `docs/protocol/05-server-api.md`
- `docs/protocol/06-state-machines.md`
- `docs/protocol/07-simulator-plan.md`
- `docs/protocol/08-data-minimization.md`

## Pika And Marmot Lessons

Pika proves the product and client-side MLS shape:

- Rust core owns business state and mobile apps render snapshots.
- MDK/Marmot provides MLS group operations and encrypted messages.
- Nostr relays carry encrypted events without plaintext.
- KeyPackages and Welcomes are working product primitives.
- The OpenClaw/Pikachat sidecar shows a practical JSONL daemon boundary for
  external agent adapters.
- Local encrypted state uses SQLite plus keyring/file-key handling.
- Notification Service Extension code can decrypt push-related media, but
  notification previews should not define the core protocol.

The part to reject for Finite Chat v1 is relay-derived Commit consensus. MLS
needs all clients to process handshake operations in the same epoch order.
Eventually consistent Nostr relay delivery made that the brittle point.

Useful Pika references:

- `README.md`
- `docs/architecture.md`
- `rust/src/mdk_support.rs`
- `crates/pika-marmot-runtime`
- `crates/pikachat-sidecar/src/protocol.rs`
- `pikachat-openclaw/README.md`
- `pikachat-openclaw/todos/ship-marmot.md`

## Finitecomputer Current Shape

Finitecomputer already has the correct high-level transport boundary for hosted
runtime chat:

- dashboard uses Next routes under `apps/dashboard/src/app/api/chat/...`;
- dashboard calls the host relay through `apps/dashboard/src/lib/finite-relay-client.ts`;
- `finited` exposes machine-authenticated outbound polling and admin event APIs;
- `finitec relay run` polls from inside the runtime and handles typed events;
- `finite-core::ChatRuntime` stores plaintext threads/messages under
  `$HOME/.finite/chat`;
- the Hermes plugin talks only through `finitec gateway`, not a runtime-local
  HTTP server.

That is the right integration seam. Finite Chat should replace the chat payload
and room state inside the runtime/control-plane contract without making the
dashboard reach into the machine.

Relevant files:

- `docs/canary-roadmap.md`
- `docs/chat-local-dev.md`
- `crates/finite-core/src/chat.rs`
- `crates/finite-core/src/chat_runtime.rs`
- `crates/finite-core/src/relay.rs`
- `crates/finited/src/main.rs`
- `crates/fc/src/main.rs`
- `integrations/hermes/finite-platform/adapter.py`
- `apps/dashboard/src/lib/finite-relay-client.ts`
- `apps/dashboard/src/lib/chat-proxy.ts`

