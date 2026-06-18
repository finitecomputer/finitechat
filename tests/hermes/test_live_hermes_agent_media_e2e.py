from __future__ import annotations

import asyncio
import contextlib
import importlib.util
import io
import json
import os
import socket
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path
from typing import Any
from urllib.request import urlopen

REPO_ROOT = Path(__file__).resolve().parents[2]
ADAPTER_PATH = REPO_ROOT / "integrations" / "hermes" / "finite-platform" / "adapter.py"
FINITECHAT_BIN = Path(os.environ.get("FINITECHAT_BIN", REPO_ROOT / "target/debug/finitechat"))
FINITECHAT_SERVER_BIN = Path(
    os.environ.get("FINITECHAT_SERVER_BIN", REPO_ROOT / "target/debug/finitechat-server")
)


def run_json(args: list[str], *, timeout: int = 120) -> dict[str, Any]:
    result = subprocess.run(args, capture_output=True, text=True, timeout=timeout)
    if result.returncode != 0:
        raise AssertionError(
            "command failed:\n"
            f"  args={args!r}\n"
            f"  stdout={result.stdout}\n"
            f"  stderr={result.stderr}"
        )
    return json.loads(result.stdout)


def free_local_port() -> int:
    sock = socket.socket()
    sock.bind(("127.0.0.1", 0))
    port = sock.getsockname()[1]
    sock.close()
    return int(port)


def wait_for_health(url: str, *, timeout: float = 30) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            with urlopen(url, timeout=2) as response:
                if response.status == 200:
                    return
        except Exception:
            time.sleep(0.1)
    raise AssertionError(f"server at {url} never became healthy")


def load_adapter_module():
    spec = importlib.util.spec_from_file_location("finite_platform_live_media_e2e", ADAPTER_PATH)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"failed to load adapter from {ADAPTER_PATH}")
    module = importlib.util.module_from_spec(spec)
    sys.modules["finite_platform_live_media_e2e"] = module
    spec.loader.exec_module(module)
    return module


class RecordingPluginContext:
    def __init__(self) -> None:
        self.entries: list[dict[str, Any]] = []

    def register_platform(self, **kwargs):
        self.entries.append(kwargs)


@unittest.skipUnless(
    os.environ.get("FINITE_HERMES_AGENT_MEDIA_E2E") == "1",
    "run scripts/hermes-agent-media-e2e.sh to install hermes-agent and enable this e2e",
)
class LiveHermesAgentMediaE2ETest(unittest.IsolatedAsyncioTestCase):
    async def test_real_hermes_agent_exchanges_text_and_image_media(self) -> None:
        asyncio.get_running_loop().slow_callback_duration = 5
        self.assertTrue(FINITECHAT_BIN.exists(), f"missing {FINITECHAT_BIN}")
        self.assertTrue(FINITECHAT_SERVER_BIN.exists(), f"missing {FINITECHAT_SERVER_BIN}")

        with tempfile.TemporaryDirectory(prefix="finite-hermes-agent-media-") as tmp_value:
            tmp = Path(tmp_value)
            server_url = f"http://127.0.0.1:{free_local_port()}"
            server_log_path = tmp / "server.log"
            with server_log_path.open("w") as server_log:
                server = subprocess.Popen(
                    [
                        str(FINITECHAT_SERVER_BIN),
                        "serve",
                        server_url.removeprefix("http://"),
                        "--sqlite",
                        str(tmp / "server.sqlite3"),
                    ],
                    stdout=server_log,
                    stderr=subprocess.STDOUT,
                    text=True,
                )
                try:
                    wait_for_health(f"{server_url}/health")
                    await self._run_media_round_trip(tmp, server_url)
                finally:
                    server.terminate()
                    with contextlib.suppress(subprocess.TimeoutExpired):
                        server.wait(timeout=5)
                    if server.poll() is None:
                        server.kill()

    async def _run_media_round_trip(self, tmp: Path, server_url: str) -> None:
        from gateway.config import PlatformConfig

        agent_home = tmp / "agent-home"
        user_home = tmp / "user-home"
        user_image = tmp / "user-diagram.png"
        agent_image = tmp / "agent-reply.png"
        user_image.write_bytes(b"\x89PNG\r\n\x1a\nfinite-user-image")
        agent_image.write_bytes(b"\x89PNG\r\n\x1a\nfinite-agent-image")

        await asyncio.to_thread(
            run_json,
            [
                str(FINITECHAT_BIN),
                "hermes",
                "--home",
                str(agent_home),
                "init",
                "--server",
                server_url,
            ],
        )
        adapter = self._build_adapter(agent_home, PlatformConfig)
        agent_received = asyncio.get_running_loop().create_future()

        async def handle_agent_message(event):
            chat_id = getattr(getattr(event, "source", None), "chat_id", None)
            received = {
                "text": getattr(event, "text", ""),
                "message_type": str(getattr(event, "message_type", "")).split(".")[-1].lower(),
                "media_urls": list(getattr(event, "media_urls", []) or []),
                "media_types": list(getattr(event, "media_types", []) or []),
                "chat_id": chat_id,
            }
            if chat_id:
                text_result = await adapter.send(
                    chat_id,
                    f"agent text echo: {getattr(event, 'text', '')}",
                )
                media_result = await adapter.send_image_file(
                    chat_id,
                    str(agent_image),
                    caption="agent media echo",
                )
                received["agent_text_send_success"] = bool(getattr(text_result, "success", False))
                received["agent_media_send_success"] = bool(getattr(media_result, "success", False))
                received["agent_media_send_error"] = getattr(media_result, "error", None)
            if not agent_received.done():
                agent_received.set_result(received)
            return None

        if hasattr(adapter, "set_message_handler"):
            adapter.set_message_handler(handle_agent_message)
        else:
            adapter.handle_message = handle_agent_message

        with contextlib.redirect_stdout(io.StringIO()):
            connected = await adapter.connect()
        self.assertTrue(connected)
        try:
            pin_info = await asyncio.to_thread(
                run_json,
                [str(FINITECHAT_BIN), "hermes", "--home", str(agent_home), "pin"],
            )
            await asyncio.to_thread(
                run_json,
                [
                    str(FINITECHAT_BIN),
                    "hermes",
                    "--home",
                    str(user_home),
                    "init",
                    "--server",
                    server_url,
                ],
            )
            joined = await asyncio.to_thread(
                run_json,
                [
                    str(FINITECHAT_BIN),
                    "hermes",
                    "--home",
                    str(user_home),
                    "join",
                    "--url",
                    pin_info["url"],
                    "--pin",
                    pin_info["pin"],
                    "--name",
                    "Hermes Media User",
                    "--timeout-ms",
                    "30000",
                ],
                timeout=60,
            )
            room_id = joined["room_id"]
            await asyncio.to_thread(
                run_json,
                [
                    str(FINITECHAT_BIN),
                    "hermes",
                    "--home",
                    str(user_home),
                    "send",
                    "--request-json",
                    json.dumps(
                        {
                            "room_id": room_id,
                            "conversation_id": None,
                            "text": "user media hello",
                            "kind": "media",
                            "status": "complete",
                            "attachments": [
                                {
                                    "kind": "image",
                                    "path": str(user_image),
                                    "name": "user-diagram.png",
                                    "mime_type": "image/png",
                                }
                            ],
                            "reply_to_message_id": None,
                        }
                    ),
                ],
                timeout=60,
            )

            received = await asyncio.wait_for(agent_received, timeout=30)
            self.assertEqual(received["text"], "user media hello")
            self.assertEqual(received["message_type"], "photo")
            self.assertEqual(received["media_types"], ["image/png"])
            self.assertTrue(received["agent_text_send_success"])
            self.assertTrue(received["agent_media_send_success"], received["agent_media_send_error"])

            user_received_text: list[str] = []
            user_received_media_count = 0
            deadline = time.monotonic() + 30
            while time.monotonic() < deadline:
                poll = await asyncio.to_thread(
                    run_json,
                    [
                        str(FINITECHAT_BIN),
                        "hermes",
                        "--home",
                        str(user_home),
                        "poll",
                        "--request-json",
                        json.dumps({"timeout_millis": 1000}),
                    ],
                    timeout=30,
                )
                for event in poll.get("events", []):
                    user_received_text.append(event.get("text") or "")
                    user_received_media_count += len(event.get("attachments") or [])
                if (
                    "agent text echo: user media hello" in user_received_text
                    and "agent media echo" in user_received_text
                    and user_received_media_count >= 1
                ):
                    return

            self.fail(
                "user did not receive both Hermes replies; "
                f"text={user_received_text!r} media_count={user_received_media_count}"
            )
        finally:
            await adapter.disconnect()

    def _build_adapter(self, agent_home: Path, platform_config):
        module = load_adapter_module()
        ctx = RecordingPluginContext()
        module.register(ctx)
        factory = ctx.entries[0]["adapter_factory"]
        try:
            config = platform_config(
                enabled=True,
                extra={
                    "home": str(agent_home),
                    "finitechat_bin": str(FINITECHAT_BIN),
                    "poll_timeout_secs": 1,
                },
            )
        except TypeError:
            import types

            config = types.SimpleNamespace(
                enabled=True,
                extra={
                    "home": str(agent_home),
                    "finitechat_bin": str(FINITECHAT_BIN),
                    "poll_timeout_secs": 1,
                },
            )
        return factory(config)


if __name__ == "__main__":
    unittest.main()
