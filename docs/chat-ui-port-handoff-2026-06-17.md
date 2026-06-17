# Chat UI Port Handoff

Date: 2026-06-17
Audience: next Codex session or human continuing the FiniteChat iOS work

## Read First

Start with these files:

- `docs/engineering-style.md`
- `docs/real-state-offline-plan.md`
- `docs/storage-plan.md`
- `docs/technical-debt-ledger.md`
- `docs/adr/0007-hint-channel-abstraction.md`
- `docs/adr/0008-rust-owned-app-runtime.md`
- `docs/feature-audit-marmot-pika.md`
- `README.md`

The governing architecture is RMP: Rust owns app state, networking,
persistence, protocol decisions, retry policy, and user-visible state
derivation. Swift renders projected state and performs bounded OS capability
bridges such as file pickers, photo saving, previews, camera/QR scanning, and
audio capture.

## Current Repo State

Workspace:

```text
/Users/futurepaul/dev/finite/finite-chat-darkmatter
```

Current branch when this handoff was written:

```text
import-pika
```

Recent commits from the Pika-quality chat UI port and persistence hardening:

```text
d2bcd99 Repair stale room state on app startup
2ba8054 Keep cached chats readable in iOS app
e62e27a Move media gallery into Rust state
065c746 Add chat media gallery
7a4a960 Prove force-close chat persistence
4a38db5 Add save photo action for chat media
145b0b6 Persist iOS runtime identity across relaunch
02ec442 Port tappable reply previews
3036bbc Preserve stable chat relaunch state
b0624d2 Protect stable chat relaunch identity
ddd214c Add voice transcript captions
2f7605c Persist chat timestamps through app relaunch
8ff1658 Restore core-created chats on app relaunch
0e35907 Add live typing activity to chat UI
1bae372 Persist iOS launch config for stable relaunch
7bb11d8 Retry failed outbox messages after restart
c3a867a Prevent diagnostic launches from poisoning app persistence
eeb7aac Persist failed outbound media sends
d74f71e Isolate iOS launch automation stores
904ecbe Hide runtime diagnostics from chat UI
d12e653 Prove app relaunch persistence
ec802c7 Add Rust-backed poll chat UI
a2501df Port Pika voice message composer
9d0426a Load local chat state before launch automation
4352f6f Port Pika paste-aware composer input
a2ee04d Port Pika staged media composer
c6c7a19 Port Pika input accessory transcript behavior
65295b2 Persist local chat outbox across restarts
30f1d4d Persist Rust-owned selected room state
5833cec Persist server-backed profile cache
c0f3ae0 Persist Rust-owned unread state
```

## What Has Been Built

Rust-owned app/runtime state:

- selected room persisted in encrypted client SQLite;
- room summaries persisted and repaired on startup;
- local transcript projection persisted before startup sync;
- pending/failed outbound text and media outbox persisted;
- server-backed/stale Nostr profile cache persisted;
- unread state persisted and clearable offline;
- raw/display timestamps persisted through relaunch;
- media gallery moved into Rust-projected selected-room state;
- verified attachment download cache and local path projection;
- room details/device list projection for settings/details views.

iOS UI:

- chat list rows instead of protocol controls;
- room transcript with performant collection-backed rows;
- input accessory composer behavior;
- staged multi-photo/video and file attachments;
- pasted image/GIF staging;
- replies and tappable reply previews;
- reactions, read receipts, polls/votes;
- voice recording, voice playback, and optional transcript captions;
- attachment previews and save-to-Photos bridge;
- media gallery screen;
- hidden Developer settings for raw runtime diagnostics and persistence data.

Protocol/product flow:

- user-facing flow is New Room, Invite, Scan/Paste, PIN, Chat;
- no user-facing manual sync, accept, or finalize action;
- SSE hint loop is behind Rust `wait_for_update`;
- server-backed Nostr profiles are the v1 profile source, with local cache;
- server is the first-class full-service backer; Nostr relay compatibility is
  deferred.

## Verification Already Run

Recently reported passing gates:

```text
cargo test -p finitechat-core
cargo clippy -p finitechat-core --all-targets -- -D warnings
full iOS simulator test suite: 51/51
```

Recent live simulator proof:

- app launched without injected args;
- list showed persisted room state from the stable store;
- room opened with normal composer when local state was healthy;
- hidden config showed the intended stable server/device identity.

Do not treat that as enough. The missing proof is the full real-state
online/offline product matrix in `docs/real-state-offline-plan.md`.

## Important Recent Fixes

`2ba8054 Keep cached chats readable in iOS app`:

- added room-details projection;
- made cached/degraded transcript readable;
- stopped invite failure from mutating an existing connected room into
  `NeedsAttention`;
- changed chat-list rows to neutral room avatars and less misleading preview
  copy.

`d2bcd99 Repair stale room state on app startup`:

- repaired stale persisted app-room rows when local MLS exists;
- projected rooms without local MLS as read-only cached state instead of
  pretending they are active;
- kept non-authoritative invite maintenance and activity/profile fetch failures
  from poisoning ordinary chat state;
- isolated iOS test config paths when tests inject `applicationSupportURL`.

## Current Risk

The product has been moving quickly and several dev/test launch paths existed
at once. That created states where a simulator could show a room row that did
not match local MLS membership or could inherit a stale server/device config.
Those states are unacceptable as product behavior and must not be normalized.

Treat any new occurrence of these as high priority:

- a connected local room becomes `NeedsAttention` because the server is down;
- a force-close relaunch loses a saved transcript;
- a server-off launch hides rooms that were visible with the server on;
- a sendable room becomes read-only because of transport failure;
- a test or launch automation writes fake server/device config into the stable
  product app support directory;
- the normal chat UI shows raw HTTP diagnostics.

## Next Work

1. Run `git status --short` and inspect any dirty files before editing.
2. Read `docs/real-state-offline-plan.md`.
3. Build the canonical product-state E2E harness before more UI features.
4. Audit `AppRoomState::Offline` and `NeedsAttention` transitions. Connected
   local membership should survive server outages.
5. Update user-facing delivery copy from failed/error language to saved
   undelivered language for retryable outbound messages.
6. Add automatic Rust-owned outbox drain on startup/reconnect/open-room if it
   is not already complete for the product path.
7. Prove the server-on/server-off matrix with a stable app store, then repeat
   with a real phone when attached.

## Local Runbook

Start the server:

```sh
cargo run -p finitechat-server -- serve 127.0.0.1:8787 --sqlite .state/finitechat.sqlite3
```

Run the iOS app through RMP:

```sh
FINITECHAT_SERVER_URL=http://127.0.0.1:8787 cargo run -p finitechat-rmp -- run ios
```

The app and RMP runner use bundle id `computer.finite.finitechat`. Do not add a
debug bundle suffix for ordinary persistence testing.

Useful checks:

```sh
cargo test -p finitechat-core
cargo clippy -p finitechat-core --all-targets -- -D warnings
cargo run -p finitechat-rmp -- doctor
cargo run -p finitechat-rmp -- bindings swift
uvx --no-config ruff format --check .
uvx --no-config ruff check .
uvx --no-config --with hermes-agent basedpyright
python3 -m unittest discover -s tests -p '*test*.py'
```

## Friction And Debt To Keep Visible

Primary debt rows:

- `In-memory-only app runtime projections`
- `Product app state can be polluted by non-product launch paths`
- `Transport failure conflated with room lifecycle`
- `Chat transcript UI ahead of product projection commands`
- `Rebuilt selected-room media gallery`
- `Pika composer media parity is incomplete`

Deferred decisions or still-open work:

- live outbound upload progress;
- indexed client media-gallery table;
- revoked-device marks moved from in-memory runtime state into encrypted
  client SQLite projection;
- product-grade automatic outbox drain proof across real iOS app relaunch and
  server restart;
- byte-level download/upload progress through Rust projection;
- physical-phone E2E against local server and then hosted/Hermes paths;
- eventual Postgres hosted server proof;
- future Nostr relay compatibility after server-backed profiles/messages are
  stable.

## Definition Of Done For The Next Goal

The next goal should be considered done only when:

- a single documented product-state harness exists;
- the app passes the online/offline matrix in `docs/real-state-offline-plan.md`;
- text offline send is durable through force close and drains exactly once when
  the server returns;
- attachment offline send has the same durable proof or is explicitly still
  blocked in the debt ledger;
- normal UI copy treats server outages as connectivity, not room failure;
- hidden Developer settings retain enough diagnostics to debug bad stores
  without showing them to users;
- the debt ledger has smaller delete conditions or removed rows for every
  completed piece.
