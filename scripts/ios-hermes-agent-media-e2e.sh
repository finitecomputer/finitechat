#!/usr/bin/env bash
set -euo pipefail

# Real iOS Simulator + real pip hermes-agent + finite-platform plugin.
# The app joins the agent invite, sends an image attachment with a caption,
# then receives agent text and image replies.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

cd "$REPO_ROOT"

cargo build -p finitechat-cli -p finitechat-server -p finitechat-rmp

env \
    FINITE_IOS_HERMES_AGENT_MEDIA_E2E=1 \
    FINITECHAT_BIN="$REPO_ROOT/target/debug/finitechat" \
    FINITECHAT_SERVER_BIN="$REPO_ROOT/target/debug/finitechat-server" \
    FINITECHAT_RMP_BIN="$REPO_ROOT/target/debug/finitechat-rmp" \
    uvx --no-config --with hermes-agent python -m unittest \
    tests.hermes.test_live_ios_simulator_hermes_media_e2e -v
