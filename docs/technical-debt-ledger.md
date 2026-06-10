# Technical Debt Ledger

Status: active ledger.

This file is where tolerated integration debt goes before it becomes product
architecture by accident. A debt item is allowed only when it has:

- an observed source;
- why the shortcut is risky;
- the first proof that keeps it bounded;
- a delete condition.

Do not call something "temporary" without a delete condition. Do not add a new
finitecomputer integration shortcut without adding or updating a row here.

## Finitecomputer Integration Debt

| Debt | Observed Source | Why It Is Risky | First Proof | Delete Condition |
| --- | --- | --- | --- | --- |
| Plaintext relay and mirrored chat snapshots | `finited` exposes chat snapshot and SSE stream endpoints; `finitecomputer` docs call this Half-Moved | The snapshot can quietly become canonical chat state instead of a transition bridge | Finite Chat durable room-log store can replay the same dashboard DTOs from ordered encrypted events | Dashboard reads projection from durable Finite Chat room events; snapshot mirror is removed or becomes a derived cache with named source/invalidation |
| File-backed relay commands | `finited` relay currently stores events/results outside the Finite Chat room sequencer | Command/result delivery can drift from room ordering, idempotency, and runtime command ledger rules | Runtime command request/result DTOs are tested through the Finite Chat reducer/store before finitecomputer integration | Runtime commands that affect chat/runtime state are ordered durable room events; relay callbacks only wake sync or carry hosted-runner admin events |
| Plaintext `ChatRuntime` as canonical transcript | `finite-core::ChatRuntime` stores threads, messages, gateway inbox, and attachments in local SQLite | New protocol semantics can get bolted onto the old transcript schema, creating two chat models | Encrypted room projection can render the existing dashboard DTO contract without mutating plaintext `messages` | Plaintext `ChatRuntime` is read-only archive/import input or deleted for canary Projects |
| Stringly relay lanes and kinds | Dashboard relay calls use `{ lane, kind, payload }` strings such as `chat.send_message` and `runtime.inference.apply` | Command names and payloads can spread without typed validation or idempotency semantics | `finitechat-proto` owns typed command, topic, state, and message DTOs with invalid-data tests | Dashboard/server routes translate into typed Finite Chat events before runtime scheduling; untyped relay names are not the application protocol |
| Runtime-local attachment bytes | Current finitecomputer chat attachments are stored under the runtime and can travel through relay JSON/base64 paths | Attachments are not portable across clients, and storage policy is mixed into chat transport | `finitechat-blob` proves encrypted Blossom-compatible references, ciphertext upload verification, ciphertext-before-decrypt checks, plaintext-after-decrypt checks, v1 size rejection, and metadata hiding from the blob store | finitecomputer chat messages carry encrypted blob references; dashboard/runtime no longer fetch plaintext attachment bytes from runtime-specific stores |
| Dashboard status as request/response | Existing dashboard surfaces use runtime commands/status calls for some state | Page loads can become durable command spam and make "read status" look like "ask runtime to work" | `runtime.state.snapshot` projection tests prove non-notifying latest-state reads | Dashboard status cards read projected snapshots by state key; explicit refresh is a user command |
| Chat control coupled to Hermes health | Current finitecomputer chat loop routes through `ChatRuntime` and Hermes gateway behavior for ordinary replies | If Hermes breaks, users can lose the only practical control surface for observing and repairing the runtime | HTTP route and runtime-delivery tests prove durable opaque command/state delivery, command-inbox effects, liveness separation, and non-notifying snapshot reads without Hermes; full daemon crash-recovery proofs return when a production daemon entrypoint exists (the old `finitechat-sim` daemon-survival suite was retired with the fake delivery service) | Finite Chat daemon remains usable for status and recovery while host is online, even when Hermes and inference are down |
| Retired reducer kept as test fixture | `finitechat-testkit` preserves the old in-memory `DeliveryService` as an MLS message factory for 13 runtime-client HTTP tests | The fixture duplicates delivery semantics that production now gets from the Darkmatter HTTP path; drift between fixture and route behavior could make tests prove stale rules | The fixture is a dev-dependency of `finitechat-client` only, is documented as never-production, and the HTTP tests it serves assert against the real Axum/SQLite server | Rewrite the remaining client-test group setups to bootstrap rooms over the Darkmatter HTTP routes, then delete `finitechat-testkit` |
| Hosted-runner admin mixed with portable commands | finitecomputer still has hosted operations for routes, auth policy, runner images, and emergency pod work | Portable Finite Chat can accidentally depend on Finite Computer's hosting substrate | Integration docs classify each surface as portable finitec command, runtime state snapshot, or hosted-runner admin | A self-managed agent with only `finitec` can use chat, topics, commands, attachments, and state snapshots without hosted-runner APIs |
| Trusted-server hosted web decryption | Hosted finitecomputer web may use a server-side Rust Finite Chat client to decrypt and render DTOs | Product copy could imply E2EE where the hosted server has device secrets | Docs and UI copy call hosted mode "web chat" or "topics", not E2EE. finitecomputer account summaries expose `ProductTrustDisclosureV1` for `hosted_web_bridge`, with `may_claim_e2ee = false`, as a typed product contract. | E2EE language is used only for clients that keep Finite Chat device secrets on the user's device |
| Premature separate `finitechatd` process | A standalone daemon is the long-term product shape, but the first canary can embed Rust crates | A new process adds auth, deployment, logging, upgrade, and local-debug burden before protocol fit is proven | Embedded crates expose a daemon-shaped API and pass local-loop restart tests | Extract `finitechatd` only after the embedded boundary has stable DTOs, sync ticks, command ledger, and storage layout |
| Shared id validators permit empty strings | `finitechat-proto::validate_room_id` currently checks byte length but not non-empty | Integration crates may assume `validate_room_id` rejects empty IDs and silently accept impossible room state | `finitechat-hermes` adds its own non-empty ingress check and has an invalid empty-room test | Tighten proto ID validators or add explicit non-empty ID helpers, then remove duplicate bridge checks |
| Hermes `stop_typing` lacks thread metadata | Hermes base adapter API calls `stop_typing(chat_id)` without the metadata passed to `send_typing` | Topic-scoped activity can be harder to clear exactly when several conversations in one room are active | The Finite Chat plugin records the last room/topic activity route, refreshes every 30s, expires activity after 60s, and tests clear the simple topic case | Hermes passes thread metadata or an activity handle to `stop_typing`, or Finite Chat bridge returns an activity handle that the adapter can clear exactly |

## Review Rule

Before each finitecomputer integration checkpoint, review this ledger and answer:

- Did this checkpoint add a new shortcut?
- Did an existing shortcut gain a smaller delete condition?
- Did tests prove the shortcut is still bounded?
- Did any user-facing copy become less honest about hosted web versus true
  end-to-end encryption?
