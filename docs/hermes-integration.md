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
finitechat hermes activity --json
```

Requests are JSON on stdin. Responses are one JSON object on stdout. The Rust
contract lives in `crates/finitechat-hermes`.

`poll`

- Input: `HermesPollOptionsV1`.
- Output: `HermesPollResponseV1`.
- Returns decrypted, Hermes-ready events for one room.

`ack`

- Input: `HermesAckRequestV1`.
- Acks `(room_id, seq, message_id)` only after Hermes `handle_message` returns.

`send`

- Input: `HermesSendRequestV1`.
- Appends a durable user-visible reply, tool output, or media message.

`edit`

- Input: `HermesEditRequestV1`.
- Updates a previously sent message. `finalize=true` marks stream completion.

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
plugin uses `activity` so these states do not create unread counts or push
notifications.

## Test Strategy

CI runs both sides of the boundary:

- `cargo test --workspace` validates the Rust DTOs, limits, invalid data, JSON
  round trips, and room/conversation mapping.
- `python3 -m unittest discover -s tests -p '*test*.py'` validates the Hermes
  plugin without requiring a Hermes checkout.

The plugin tests prove:

- registration exposes the `finite` platform contract;
- `FINITECHAT_BIN`/`FINITECHAT_ROOM_ID` are the required bridge inputs;
- outbound sends preserve room, topic, reply, attachments, and metadata;
- inbound poll events map room to chat and topic to thread;
- wrong-room events are not dispatched or acked;
- ephemeral activity is used for working state instead of durable status
  messages.
