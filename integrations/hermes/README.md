# Hermes ⇄ Finite Chat

The `finite-platform` plugin connects a [Hermes agent](https://github.com/NousResearch/hermes-agent)
to end-to-end-encrypted Finite Chat rooms. The dream flow (ADR 0006):

1. The agent prints a QR code, a `finite://join?...` URL, and a rotating
   6-digit PIN when the gateway starts.
2. You scan or paste it into the Finite Chat app and type the PIN.
3. The agent verifies the PIN proof *before* admitting you to the MLS group
   — then you're chatting, end-to-end encrypted, with MLS-authenticated
   sender identities. No public relay, no account registration: the agent's
   npub lives only on its home server.

## Install

```bash
# 1. The binary (one drop-in binary owns all crypto and state)
cargo install --path crates/finitechat-cli   # installs `finitechat-darkmatter`

# 2. The agent identity
finitechat-darkmatter hermes --home ~/.finite-agent init --server http://your-server:8787
export FINITECHAT_HOME=~/.finite-agent

# 3. The plugin (Hermes ≥ 0.16 plugin layout)
mkdir -p ~/.hermes/plugins
cp -r integrations/hermes/finite-platform ~/.hermes/plugins/finite
```

Enable it in `~/.hermes/config.yaml`:

```yaml
plugins:
  enabled:
    - finite

gateway:
  platforms:
    finite:
      enabled: true
```

Then `hermes gateway start` prints the invite QR/URL/PIN and the agent is
reachable from the Finite Chat app.

## How the pieces divide (ADR 0002)

The Python adapter stays thin: it shells to `finitechat-darkmatter hermes
<action> --json` and translates JSON to Hermes `MessageEvent`s. The Rust
binary owns identity, MLS encryption, invite verification, durable cursors,
and storage. The bridge actions are `init`, `invite`, `pin`, `poll`,
`send`, `edit`, and `activity`.
