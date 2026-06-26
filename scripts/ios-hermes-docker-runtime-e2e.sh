#!/usr/bin/env bash
# Real iOS Simulator + real Docker runtime image.
#
# Builds containers/agent/Dockerfile, starts the runtime echo agent in Docker,
# joins from the native iOS app, sends an image attachment, and verifies the app
# decrypts the runtime agent's reply.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

cd "$REPO_ROOT"

cargo build -p finitechat-cli -p finitechat-server -p finitechat-rmp

env \
    FINITE_IOS_DOCKER_RUNTIME_E2E=1 \
    FINITE_IOS_DOCKER_RUNTIME_E2E_REPORT="$REPO_ROOT/target/ios-hermes-docker-runtime-e2e/report.json" \
    FINITECHAT_BIN="$REPO_ROOT/target/debug/finitechat" \
    FINITECHAT_SERVER_BIN="$REPO_ROOT/target/debug/finitechat-server" \
    FINITECHAT_RMP_BIN="$REPO_ROOT/target/debug/finitechat-rmp" \
    python3 -m unittest tests.container.test_ios_docker_runtime_e2e -v
