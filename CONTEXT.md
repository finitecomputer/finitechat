# Finite Chat

Finite Chat is an encrypted chat and command transport. Its language separates
cryptographic delivery boundaries from user-facing conversation structure.

## Language

**Room**:
An MLS group plus one server-ordered delivery log.
_Avoid_: Chat, topic, conversation

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

## Relationships

- A **Room** contains zero or more **Conversations**.
- A **Topic** is a **Conversation** presented as a named lane.
- A **Conversation** contains one or more **Segments** when an app supports context resets.
- A **Segment** belongs to exactly one **Conversation**.
- **Activity** may be scoped to a **Room** or to one **Conversation**.
- **Runtime State** belongs to one agent runtime device and is projected by key.
- A **Finite Chat Daemon** owns one or more **Devices** and may observe an
  agent runtime, but it is not the agent or its inference provider.

## Example Dialogue

> **Dev:** "If the user runs `/new` in the Deploys topic, do we create a new topic?"
> **Domain expert:** "No. Deploys stays the same Topic; `/new` starts a new Segment inside it."

> **Dev:** "Do we send a command whenever the dashboard needs status?"
> **Domain expert:** "No. The runtime publishes Runtime State, and the dashboard reads the latest projection."

> **Dev:** "If Hermes is broken, is chat broken?"
> **Domain expert:** "No. The Finite Chat Daemon still owns sync, Runtime State, and recovery commands while the host is online."

## Flagged Ambiguities

- "New chat" can mean creating a new **Topic** from the app shell, or starting a
  new **Segment** inside an existing **Topic**. Resolved: app-level "New chat"
  creates a Topic; `/new` inside a Topic creates a Segment.
