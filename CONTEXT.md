# Finite Chat

Finite Chat is an encrypted chat and command transport. Its language keeps
Room as the first-level chat object while naming finer structure inside a Room.

## Language

**Room**:
A user-visible chat space backed by one MLS group and one server-ordered delivery log.
_Avoid_: Topic, conversation, direct room

**DM**:
A Room whose members happen to be two accounts. Not a distinct protocol
concept; several named Rooms with the same person are legal and useful.
_Avoid_: Direct room (retired server concept)

**Nostr Profile**:
Public user profile metadata attached to a Nostr account identity.
_Avoid_: Device identity, Room metadata

**Device List**:
The user-facing list of a user's active and revoked Finite Chat devices.
_Avoid_: Profile, account settings

**Invite**:
A shareable Room entry credential that lets another device request admission to a Room.
_Avoid_: QR, link

**Pending Room**:
A Room row created from a Scan Target before the local device can send messages in that Room.
_Avoid_: Half-joined room, unfinalized room

**Room Admission**:
The Room owner's approval of a Pending Room into a sendable Room.
_Avoid_: Accept button, finalize step

**Room State**:
The user-visible readiness of a Room row: connected, waiting, joining, needs attention, or offline.
_Avoid_: Sync status, protocol phase

**Scan Target**:
A scanned or pasted value that the app routes into the appropriate Room flow.
_Avoid_: Join invite

**Conversation**:
An application-level session inside a room.
_Avoid_: Room, MLS group

**Topic**:
A first-class user-facing conversation lane inside a room.
_Avoid_: Thread, room

**Segment**:
An app-owned context boundary inside a conversation.
_Avoid_: Topic, conversation, room

**Activity**:
TTL-bound encrypted intermediate state inside a room or conversation.
_Avoid_: Message, notification

**Runtime State**:
Structured current condition published by an agent runtime.
_Avoid_: Command, message, activity

**Finite Chat Daemon**:
The local or runtime-resident control surface that owns a Finite Chat device.
_Avoid_: Hermes, agent, inference provider

**Dev Diagnostics**:
A hidden surface for protocol, server, device, and local-state inspection.
_Avoid_: Settings, profile

## Relationships

- A **Room** contains zero or more **Conversations**.
- A **DM** is a **Room**, not a separate kind; per-topic lanes with one
  person are **Topics** inside a Room, or separate named Rooms — both legal.
- A **Nostr Profile** describes an account, not an individual device.
- A **Device List** belongs to an account and is where users revoke devices.
- A **Pending Room** is a **Room** from the user's point of view, but its
  local device is still waiting for admission to complete.
- **Room Admission** is automatic for a valid Invite plus correct PIN.
- **Room State** names whether a Room is ready to use; it does not expose
  protocol maintenance phases.
- A **Topic** is a **Conversation** presented as a named lane.
- A **Conversation** contains one or more **Segments** when an app supports context resets.
- A **Segment** belongs to exactly one **Conversation**.
- **Activity** may be scoped to a **Room** or to one **Conversation**.
- **Runtime State** belongs to one agent runtime device and is projected by key.
- A **Finite Chat Daemon** owns one or more **Devices** and may observe an
  agent runtime, but it is not the agent or its inference provider.
- **Dev Diagnostics** may expose server and device details; normal users should
  not need those details to use Rooms.

## Example Dialogue

> **Dev:** "If the user runs `/new` in the Deploys topic, do we create a new topic?"
> **Domain expert:** "No. Deploys stays the same Topic; `/new` starts a new Segment inside it."

> **Dev:** "Do we send a command whenever the dashboard needs status?"
> **Domain expert:** "No. The runtime publishes Runtime State, and the dashboard reads the latest projection."

> **Dev:** "If Hermes is broken, is chat broken?"
> **Domain expert:** "No. The Finite Chat Daemon still owns sync, Runtime State, and recovery commands while the host is online."

> **Dev:** "After scanning an invite and entering the PIN, does the inviter need to tap Accept?"
> **Domain expert:** "No. The correct PIN is the approval ceremony; valid Room Admission is automatic."

## Flagged Ambiguities

- "New chat" can mean creating a new **Topic** from the app shell, or starting a
  new **Segment** inside an existing **Topic**. Resolved: app-level "New chat"
  creates a Room; `/new` inside a Topic creates a Segment.
