#!/usr/bin/env bash
# Agent-in-a-Linux-container end-to-end (Apple `container` runtime):
#   host: finitechat-server + a CLI user
#   guest: latest hermes-agent + finite-platform plugin + finitechat binary
# Pairs via invite URL + PIN, then asserts an E2EE echo round trip.
#
# Requires: https://github.com/apple/container installed and
# `container system start` run once (first start installs a Linux kernel).
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"
exec env FINITE_CONTAINER_E2E=1 python3 -m unittest \
    tests.container.test_agent_container_e2e -v
