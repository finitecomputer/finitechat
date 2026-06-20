#!/usr/bin/env bash
# Local Hermes adapter media end-to-end:
#   real pip hermes-agent package + finite-platform plugin + finitechat binaries
#   finitechat user joins via invite/PIN, sends image media, then receives
#   agent text and image media replies.
# This test installs an echo set_message_handler callback. It proves adapter
# transport/media wiring, not real Hermes gateway/model behavior.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

cargo build -p finitechat-cli -p finitechat-server

exec env \
    FINITE_HERMES_AGENT_MEDIA_E2E=1 \
    FINITECHAT_BIN="$REPO_ROOT/target/debug/finitechat" \
    FINITECHAT_SERVER_BIN="$REPO_ROOT/target/debug/finitechat-server" \
    uvx --no-config --with hermes-agent python -m unittest \
    tests.hermes.test_live_hermes_agent_media_e2e -v
