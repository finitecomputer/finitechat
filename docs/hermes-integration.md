# Hermes Integration

Finite Chat owns the Hermes platform plugin and the Rust bridge contract that
the plugin speaks. finitecomputer can import the plugin or vendor it into its
runtime image, but it should not fork the transport semantics.

## Bridge Commands

The plugin calls a Finite Chat CLI/daemon boundary:

```text
finitechat hermes poll --json
finitechat hermes ack --json
finitechat hermes send --json
finitechat hermes edit --json
finitechat hermes recover --json
finitechat hermes activity --json
```

Requests are JSON on stdin. Responses are one JSON object on stdout. The Rust
contract lives in `crates/finitechat-hermes`.

`poll`

- Input: `HermesPollOptionsV1`.
- Output: `HermesPollResponseV1`.
- Syncs room logs, stores decrypted Hermes-ready inbound events in the agent
  home's durable inbox, and returns unacked events for one room or all rooms.
  Events redeliver until `ack`.

`ack`

- Input: `HermesAckRequestV1`.
- Acks `(room_id, seq, message_id)` only after Hermes `handle_message` returns.

`send`

- Input: `HermesSendRequestV1`.
- Appends a durable user-visible reply, tool output, or media message.

`edit`

- Input: `HermesEditRequestV1`.
- Updates a previously sent message. `finalize=true` marks stream completion.

`recover`

- Finalizes locally tracked `running` Hermes messages after a gateway restart
  by appending an explicit recovery edit on the same visible message id.

`activity`

- Input: `HermesActivityRequestV1`.
- Sets or clears non-notifying ephemeral state such as `working`.

## Mapping

Finite Chat room maps to Hermes `source.chat_id`.

Finite Chat conversation/topic maps to Hermes `source.thread_id`.

Hermes outbound `chat_id` is interpreted as `room_id`. Hermes outbound metadata
`thread_id` or `conversation_id` is interpreted as `conversation_id`. Reserved
adapter metadata such as `_finitechat_kind`, `_finitechat_status`, and
`attachments` is consumed by the bridge and not stored as user metadata.

Attachments are typed as `HermesAttachmentV1`. They may contain a local path,
a URL, or a Finite Chat encrypted blob reference. The Python adapter only
passes the reference through; Finite Chat owns blob verification and
materialization.

Typing, thinking, and working indicators are not durable chat messages. The
plugin uses room-scoped `activity` so these states do not create unread counts
or push notifications. Topic-scoped activity is out of scope for v1; topics can
be represented as rooms.

## Test Strategy

CI runs both sides of the boundary:

- `cargo test --workspace` validates the Rust DTOs, limits, invalid data, JSON
  round trips, and room/conversation mapping.
- `python3 -m unittest discover -s tests -p '*test*.py'` validates the Hermes
  plugin without requiring a Hermes checkout.
- `scripts/hermes-agent-media-e2e.sh` installs the real upstream
  `hermes-agent` package with `uvx`, starts a live `finitechat-server`, pairs a
  CLI user through the agent invite/PIN, sends image media through the Finite
  Chat adapter, and asserts transport/media round trips. It installs an echo
  `set_message_handler` callback, so it is adapter transport coverage, not real
  Hermes model behavior.
- `scripts/ios-hermes-agent-media-e2e.sh` repeats that adapter transport/media
  round trip through the iOS Simulator app. It also uses an echo callback and
  must not be cited as proof that the real Hermes gateway answered.
- `scripts/ios-device-hermes-agent-media-e2e.sh` is the physical-phone version
  of the same echo-handler transport test. It requires an already installed
  `computer.finite.finitechat` build, an unlocked/awake paired iPhone, and a
  Mac LAN server URL so the phone talks to the same configured server instead
  of Mac loopback.
- `scripts/hermes-real-gateway-demo.sh` is the repo-local real Hermes runner:
  it starts a local Finite Chat server, initializes a Hermes agent home, loads
  the finite-platform plugin into a prepared Hermes checkout, and runs
  `hermes gateway run` without a test echo callback.

The plugin tests prove:

- registration exposes the `finite` platform contract;
- `FINITECHAT_HOME` is required, and `FINITECHAT_ROOM_ID` is an optional room
  filter;
- outbound sends preserve room, topic, reply, attachments, and metadata;
- outbound sends infer Hermes tool/status kind and running/complete status when
  Hermes metadata is missing;
- inbound poll events map room to chat and topic to thread, then ack only after
  dispatch succeeds;
- wrong-room events are not dispatched or acked;
- ephemeral activity is used for working state instead of durable status
  messages.

See `docs/oops-i-faked-it-audit.md` for the current line between echo-handler
transport coverage and real Hermes gateway proof.
