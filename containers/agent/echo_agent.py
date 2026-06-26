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
from typing import Any

logging.basicConfig(level=logging.INFO, format="%(asctime)s %(name)s %(message)s")
logger = logging.getLogger("echo-agent")

HOME = Path(os.environ["FINITECHAT_HOME"])
SERVER_URL = os.environ["FINITE_SERVER_URL"]
PLUGIN_DIR = Path(os.environ.get("FINITECHAT_PLUGIN_DIR", "/root/.hermes/plugins/finite"))
FINITECHAT_CMD = shlex.split(os.environ.get("FINITECHAT_BIN", "finitechat"))
HEALTH_HOST = os.environ.get("FINITE_AGENT_HTTP_HOST", "0.0.0.0")
HEALTH_PORT = int(os.environ.get("FINITE_AGENT_HTTP_PORT", "8080"))
STATUS: dict[str, Any] = {
    "ready": False,
    "npub": None,
    "account_id": None,
}


def finitechat_json(*args: str) -> dict[str, Any]:
    result = subprocess.run(
        [*FINITECHAT_CMD, *args],
        capture_output=True,
        text=True,
        check=True,
    )
    return json.loads(result.stdout)


def ensure_initialized() -> dict[str, Any]:
    if (HOME / "config.json").exists():
        logger.info("agent home already initialized at %s", HOME)
        return finitechat_json("identity", "--agent-home", str(HOME), "show")
    identity = finitechat_json(
        "hermes",
        "--home",
        str(HOME),
        "init",
        "--server",
        SERVER_URL,
    )
    logger.info("agent identity: %s", identity["npub"])
    print(f"AGENT_NPUB={identity['npub']}", flush=True)
    return identity


def set_identity_status(identity: dict[str, Any]) -> None:
    STATUS["npub"] = identity.get("npub")
    STATUS["account_id"] = identity.get("account_id")


async def handle_health(reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
    try:
        request_line = await asyncio.wait_for(reader.readline(), timeout=2)
        parts = request_line.decode("latin-1", errors="replace").split()
        path = parts[1] if len(parts) >= 2 else "/"
        while True:
            line = await asyncio.wait_for(reader.readline(), timeout=2)
            if line in {b"\r\n", b"\n", b""}:
                break
        status_code = 200 if path == "/healthz" and STATUS["ready"] else 503
        if path != "/healthz":
            status_code = 404
        reason = {200: "OK", 404: "Not Found", 503: "Service Unavailable"}[status_code]
        body = json.dumps(
            {
                "ready": STATUS["ready"],
                "npub": STATUS["npub"],
                "account_id": STATUS["account_id"],
            }
        ).encode("utf-8")
        writer.write(
            (
                f"HTTP/1.1 {status_code} {reason}\r\n"
                "content-type: application/json\r\n"
                f"content-length: {len(body)}\r\n"
                "connection: close\r\n"
                "\r\n"
            ).encode("ascii")
            + body
        )
        await writer.drain()
    except Exception as exc:  # pragma: no cover - defensive health server guard
        logger.warning("health request failed: %s", exc)
    finally:
        writer.close()
        await writer.wait_closed()


async def start_health_server() -> asyncio.AbstractServer:
    server = await asyncio.start_server(handle_health, HEALTH_HOST, HEALTH_PORT)
    logger.info("health endpoint listening on %s:%s", HEALTH_HOST, HEALTH_PORT)
    return server


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
    identity = ensure_initialized()
    set_identity_status(identity)
    health_server = await start_health_server()
    module = load_adapter_class()
    adapter = build_adapter(module)

    async def echo_handler(event):
        text = getattr(event, "text", "") or ""
        chat_id = getattr(getattr(event, "source", None), "chat_id", None)
        message_type = str(getattr(event, "message_type", "")).split(".")[-1].lower()
        media_types = list(getattr(event, "media_types", []) or [])
        if not chat_id:
            return None
        logger.info(
            "inbound from %s type=%s media_types=%s: %r",
            chat_id,
            message_type,
            media_types,
            text,
        )
        print(
            "ECHO_AGENT_INBOUND "
            f"message_type={message_type} "
            f"media_types={','.join(media_types)} "
            f"text={text}",
            flush=True,
        )
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
    STATUS["ready"] = True
    print("ECHO_AGENT_READY", flush=True)
    try:
        while True:
            await asyncio.sleep(3600)
    finally:
        health_server.close()
        await health_server.wait_closed()


if __name__ == "__main__":
    asyncio.run(main())
