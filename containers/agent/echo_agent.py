"""Echo agent for the container e2e: the finite-platform plugin running
against the REAL hermes-agent gateway base classes (pip `hermes-agent`),
with a trivial echo handler standing in for the LLM.

Boot sequence: `hermes init` against FINITE_SERVER_URL (idempotent),
construct the adapter through the plugin's `register(ctx)` contract,
connect (which prints the invite QR/URL/PIN), and echo every inbound
message back into its room. The e2e driver pairs from the host via
`hermes join` and asserts the echo round trip.
"""

from __future__ import annotations

import asyncio
import importlib.util
import json
import logging
import os
import shlex
import subprocess
import sys
import types
from pathlib import Path

logging.basicConfig(level=logging.INFO, format="%(asctime)s %(name)s %(message)s")
logger = logging.getLogger("echo-agent")

HOME = Path(os.environ["FINITECHAT_HOME"])
SERVER_URL = os.environ["FINITE_SERVER_URL"]
PLUGIN_DIR = Path(os.environ.get("FINITECHAT_PLUGIN_DIR", "/root/.hermes/plugins/finite"))
FINITECHAT_CMD = shlex.split(os.environ.get("FINITECHAT_BIN", "finitechat"))


def ensure_initialized() -> None:
    if (HOME / "config.json").exists():
        logger.info("agent home already initialized at %s", HOME)
        return
    result = subprocess.run(
        [
            *FINITECHAT_CMD,
            "hermes",
            "--home",
            str(HOME),
            "init",
            "--server",
            SERVER_URL,
        ],
        capture_output=True,
        text=True,
        check=True,
    )
    identity = json.loads(result.stdout)
    logger.info("agent identity: %s", identity["npub"])
    print(f"AGENT_NPUB={identity['npub']}", flush=True)


def load_adapter_class():
    """Load FiniteChatAdapter against the real hermes gateway modules."""
    spec = importlib.util.spec_from_file_location("finite_platform", PLUGIN_DIR / "adapter.py")
    if spec is None or spec.loader is None:
        raise RuntimeError(f"failed to load finite platform plugin from {PLUGIN_DIR}")
    module = importlib.util.module_from_spec(spec)
    sys.modules["finite_platform"] = module
    spec.loader.exec_module(module)
    return module


class RecordingCtx:
    """Minimal plugin context capturing register_platform kwargs, used when
    we drive the adapter directly instead of through `hermes gateway`."""

    def __init__(self):
        self.entries = []

    def register_platform(self, **kwargs):
        self.entries.append(kwargs)


def build_adapter(module):
    from gateway.config import PlatformConfig  # real hermes-agent module

    ctx = RecordingCtx()
    module.register(ctx)
    assert ctx.entries and ctx.entries[0]["name"] == "finite"
    factory = ctx.entries[0]["adapter_factory"]
    try:
        config = PlatformConfig(enabled=True, extra={"home": str(HOME)})
    except TypeError:
        config = types.SimpleNamespace(enabled=True, extra={"home": str(HOME)})
    return factory(config)


async def main() -> None:
    ensure_initialized()
    module = load_adapter_class()
    adapter = build_adapter(module)

    async def echo_handler(event):
        text = getattr(event, "text", "") or ""
        chat_id = getattr(getattr(event, "source", None), "chat_id", None)
        if not chat_id:
            return None
        logger.info("inbound from %s: %r", chat_id, text)
        result = await adapter.send(chat_id, f"echo: {text}")
        logger.info("echo sent: %s", result)
        return None

    # The real BasePlatformAdapter exposes set_message_handler; fall back to
    # overriding handle_message if the API ever shifts.
    if hasattr(adapter, "set_message_handler"):
        adapter.set_message_handler(echo_handler)
    else:  # pragma: no cover
        adapter.handle_message = echo_handler

    connected = await adapter.connect()
    if not connected:
        logger.error("adapter failed to connect")
        sys.exit(1)
    print("ECHO_AGENT_READY", flush=True)
    while True:
        await asyncio.sleep(3600)


if __name__ == "__main__":
    asyncio.run(main())
