"""iOS Simulator smoke against the real Docker runtime image.

Gated behind FINITE_IOS_DOCKER_RUNTIME_E2E=1. This complements the Docker CLI
smoke by proving the packaged runtime can pair with the native iOS app, receive
an encrypted image message, and send a decryptable reply.
"""

from __future__ import annotations

import base64
import contextlib
import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path
from typing import Any
from urllib.parse import parse_qsl, urlencode, urlsplit, urlunsplit

from tests.container.test_agent_container_e2e import run, stage_build_context
from tests.hermes.test_live_hermes_agent_media_e2e import (
    FINITECHAT_BIN,
    FINITECHAT_SERVER_BIN,
    free_local_port,
    run_json,
    wait_for_health,
)
from tests.hermes.test_live_ios_simulator_hermes_media_e2e import (
    BUNDLE_ID,
    FINITECHAT_RMP_BIN,
    booted_simulator_udid,
    run_cmd,
)

REPO_ROOT = Path(__file__).resolve().parents[2]
IMAGE = os.environ.get("FINITE_IOS_DOCKER_IMAGE", "finite-ios-docker-runtime-e2e")
CONTAINER = os.environ.get("FINITE_IOS_DOCKER_CONTAINER", "finite-ios-docker-runtime-e2e-run")
DOCKER_HOST = os.environ.get("FINITE_DOCKER_HOST", "host.docker.internal")
HERMES_AGENT_VERSION = os.environ.get("FINITE_HERMES_AGENT_VERSION", "0.17.0")
DEFAULT_REPORT = REPO_ROOT / "target/ios-hermes-docker-runtime-e2e/report.json"
IOS_DEVICE_ID = "ios-docker-runtime-sim"
IOS_CAPTION = "ios docker runtime hello"
RUNTIME_ECHO_TEXT = f"echo: {IOS_CAPTION}"
PNG_1X1 = base64.b64decode(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMB/6X7z9kAAAAASUVORK5CYII="
)
PROOF_LAYERS = [
    "docker image build",
    "hermes-agent 0.17 runtime",
    "finitechat binary in image",
    "finite-platform plugin in image",
    "finitechat-server on host",
    "real Docker runtime agent container",
    "iOS Simulator app joins runtime invite",
    "iOS sends encrypted media message",
    "runtime agent receives iOS media",
    "iOS decrypts runtime agent reply",
]


def rewrite_invite_server(invite_url: str, server_url: str) -> str:
    parts = urlsplit(invite_url)
    query = dict(parse_qsl(parts.query, keep_blank_values=True))
    if "s" not in query:
        raise ValueError("Finite invite URL is missing server query parameter 's'")
    query["s"] = server_url
    return urlunsplit(
        (
            parts.scheme,
            parts.netloc,
            parts.path,
            urlencode(query),
            parts.fragment,
        )
    )


def docker_args() -> list[str]:
    return ["docker", "run", "--rm", "--add-host", f"{DOCKER_HOST}:host-gateway"]


def shared_server_host() -> str:
    if host := os.environ.get("FINITE_IOS_DOCKER_SERVER_HOST"):
        return host
    if sys.platform == "darwin":
        result = subprocess.run(
            ["ipconfig", "getifaddr", "en0"],
            capture_output=True,
            text=True,
            check=False,
            timeout=5,
        )
        if result.returncode == 0 and result.stdout.strip():
            return result.stdout.strip()
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sock:
        sock.connect(("8.8.8.8", 80))
        return str(sock.getsockname()[0])


def launch_ios_app(
    *,
    udid: str,
    support_root: Path,
    server_url: str,
    invite_url: str,
    pin: str,
    image_path: Path,
) -> None:
    run_cmd([str(FINITECHAT_RMP_BIN), "run", "ios", "--udid", udid], timeout=600)
    subprocess.run(
        ["xcrun", "simctl", "terminate", udid, BUNDLE_ID],
        capture_output=True,
        text=True,
    )
    run_cmd(
        [
            "xcrun",
            "simctl",
            "launch",
            udid,
            BUNDLE_ID,
            "--finitechat-product-harness-root",
            str(support_root),
            "--finitechat-server",
            server_url,
            "--finitechat-device",
            IOS_DEVICE_ID,
            "--finitechat-auto-join",
            invite_url,
            "--finitechat-pin",
            pin,
            "--finitechat-auto-send-attachment-file",
            str(image_path),
            "--finitechat-auto-send-attachment-caption",
            IOS_CAPTION,
        ],
        timeout=60,
    )


def read_ios_app_state(support_root: Path, server_url: str) -> dict[str, Any]:
    return run_json(
        [
            str(FINITECHAT_BIN),
            "app",
            "--data-dir",
            str(support_root / "FiniteChatStore"),
            "--server",
            server_url,
            "--device-id",
            IOS_DEVICE_ID,
            "state",
        ],
        timeout=30,
    )


def ios_texts(state: dict[str, Any]) -> list[str]:
    return [message.get("text") or "" for message in state.get("messages", [])]


class DockerIosSmokeReport:
    def __init__(self) -> None:
        configured = os.environ.get("FINITE_IOS_DOCKER_RUNTIME_E2E_REPORT")
        self.path = Path(configured) if configured else DEFAULT_REPORT
        self.started = time.monotonic()
        self.facts: dict[str, Any] = {}
        self.steps: list[dict[str, Any]] = []

    def fact(self, key: str, value: Any) -> None:
        self.facts[key] = value

    def time(self, name: str, fn):
        started = time.monotonic()
        value = fn()
        self.step(name, started)
        return value

    def step(self, name: str, started: float) -> None:
        self.steps.append({"name": name, "elapsed_ms": int((time.monotonic() - started) * 1000)})

    def finish(self) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self.path.write_text(
            json.dumps(
                {
                    "status": "passed",
                    "name": "ios_simulator_docker_runtime_e2e",
                    "elapsed_ms": int((time.monotonic() - self.started) * 1000),
                    "proof_layers": PROOF_LAYERS,
                    "facts": self.facts,
                    "steps": self.steps,
                },
                indent=2,
            )
            + "\n",
            encoding="utf-8",
        )


class InviteRewriteTest(unittest.TestCase):
    def test_rewrite_invite_server_preserves_join_fields(self) -> None:
        rewritten = rewrite_invite_server(
            "finite://join?v=1&s=http%3A%2F%2Fhost.docker.internal%3A18789&r=room&i=invite&t=token&a=npub1agent",
            "http://127.0.0.1:18789",
        )

        parts = urlsplit(rewritten)
        query = dict(parse_qsl(parts.query))
        self.assertEqual(parts.scheme, "finite")
        self.assertEqual(query["s"], "http://127.0.0.1:18789")
        self.assertEqual(query["r"], "room")
        self.assertEqual(query["i"], "invite")
        self.assertEqual(query["t"], "token")
        self.assertEqual(query["a"], "npub1agent")

    def test_rewrite_invite_server_requires_server_field(self) -> None:
        with self.assertRaisesRegex(ValueError, "missing server"):
            rewrite_invite_server("finite://join?v=1&r=room", "http://127.0.0.1:18789")


@unittest.skipUnless(
    os.environ.get("FINITE_IOS_DOCKER_RUNTIME_E2E") == "1",
    "run scripts/ios-hermes-docker-runtime-e2e.sh to enable this Docker+iOS smoke",
)
class IosDockerRuntimeE2ETest(unittest.TestCase):
    def setUp(self) -> None:
        if shutil.which("docker") is None:
            self.fail("Docker is not installed")
        status = run(["docker", "info"], check=False, timeout=60)
        if status.returncode != 0:
            self.fail("Docker daemon is not running")
        self.tmp = tempfile.TemporaryDirectory(dir=REPO_ROOT / "target")
        self.addCleanup(self.tmp.cleanup)
        self.server_proc: subprocess.Popen[str] | None = None
        self.agent_volume = f"{CONTAINER}-agent-{int(time.time() * 1000)}"
        self.addCleanup(self._teardown)

    def _teardown(self) -> None:
        run(["docker", "rm", "-f", CONTAINER], check=False, timeout=120)
        run(["docker", "volume", "rm", "-f", self.agent_volume], check=False, timeout=120)
        if self.server_proc is not None:
            self.server_proc.terminate()
            with contextlib.suppress(subprocess.TimeoutExpired):
                self.server_proc.wait(timeout=10)
            if self.server_proc.poll() is None:
                self.server_proc.kill()

    def test_ios_simulator_chats_with_real_docker_runtime(self) -> None:
        self.assertTrue(FINITECHAT_BIN.exists(), f"missing {FINITECHAT_BIN}")
        self.assertTrue(FINITECHAT_SERVER_BIN.exists(), f"missing {FINITECHAT_SERVER_BIN}")
        self.assertTrue(FINITECHAT_RMP_BIN.exists(), f"missing {FINITECHAT_RMP_BIN}")
        udid = booted_simulator_udid()
        report = DockerIosSmokeReport()
        tmp = Path(self.tmp.name)
        support_root = tmp / "ios-support"
        support_root.mkdir()
        ios_image = tmp / "ios-docker-runtime-image.png"
        ios_image.write_bytes(PNG_1X1)
        report.fact("platform", "ios_simulator")
        report.fact("simulator_udid", udid)
        report.fact("ios_device_id", IOS_DEVICE_ID)
        report.fact("bundle_id", BUNDLE_ID)

        ctx = tmp / "ctx"
        ctx.mkdir()
        report.time("stage_docker_context", lambda: stage_build_context(ctx))
        report.time(
            "docker_image_build",
            lambda: run(
                [
                    "docker",
                    "build",
                    "--build-arg",
                    f"HERMES_AGENT_VERSION={HERMES_AGENT_VERSION}",
                    "--tag",
                    IMAGE,
                    "--file",
                    str(ctx / "finitechat/containers/agent/Dockerfile"),
                    str(ctx),
                ],
                timeout=3600,
            ),
        )
        image_metadata = self.docker_image_metadata()
        report.fact("image", IMAGE)
        report.fact("image_id", image_metadata["id"])
        report.fact("image_metadata", image_metadata)

        port = free_local_port()
        shared_url = os.environ.get("FINITE_IOS_DOCKER_SERVER_URL") or (
            f"http://{shared_server_host()}:{port}"
        )
        ios_server_url = shared_url
        docker_server_url = shared_url
        report.fact("server_url_from_ios", ios_server_url)
        report.fact("server_url_from_docker", docker_server_url)
        report.fact("invite_rewritten_for_ios", True)
        server_log_path = tmp / "server.log"
        with server_log_path.open("w") as server_log:
            self.server_proc = subprocess.Popen(
                [
                    str(FINITECHAT_SERVER_BIN),
                    "serve",
                    f"0.0.0.0:{port}",
                    "--sqlite",
                    str(tmp / "server.sqlite3"),
                ],
                stdout=server_log,
                stderr=subprocess.STDOUT,
                text=True,
            )
            report.time("host_server_ready", lambda: wait_for_health(f"{ios_server_url}/health"))

            report.time(
                "agent_container_start", lambda: self.start_agent_container(docker_server_url)
            )
            report.time("agent_ready_log", lambda: self.wait_for_log("ECHO_AGENT_READY", 180))
            hermes_version = run(
                [
                    "docker",
                    "exec",
                    CONTAINER,
                    "python",
                    "-c",
                    "import importlib.metadata; print(importlib.metadata.version('hermes-agent'))",
                ],
                timeout=60,
            ).stdout.strip()
            self.assertEqual(hermes_version, HERMES_AGENT_VERSION)
            report.fact("hermes_agent_version_actual", hermes_version)
            identity = self.agent_identity()
            health = report.time("agent_http_health", self.agent_http_health)
            self.assertTrue(health["ready"])
            self.assertEqual(health["npub"], identity["npub"])
            report.fact("agent_npub", identity["npub"])
            report.fact("runtime_health", health)

            pin_info = json.loads(
                run(
                    [
                        "docker",
                        "exec",
                        CONTAINER,
                        "finitechat",
                        "hermes",
                        "--home",
                        "/data/agent",
                        "pin",
                    ],
                    timeout=60,
                ).stdout
            )
            invite_for_ios = rewrite_invite_server(pin_info["url"], ios_server_url)
            report.fact("invite_url_present", bool(invite_for_ios))
            report.fact("pin_present", bool(pin_info.get("pin")))

            started = time.monotonic()
            launch_ios_app(
                udid=udid,
                support_root=support_root,
                server_url=ios_server_url,
                invite_url=invite_for_ios,
                pin=pin_info["pin"],
                image_path=ios_image,
            )
            report.step("ios_app_launch", started)
            inbound_logs = report.time(
                "agent_receive_ios_media",
                lambda: self.wait_for_log("ECHO_AGENT_INBOUND message_type=photo", 120),
            )
            self.assertIn("media_types=image/png", inbound_logs)
            report.fact("agent_received_message_type", "photo")
            report.fact("agent_received_media_types", ["image/png"])

            deadline = time.monotonic() + 60
            started = time.monotonic()
            last_state: dict[str, Any] | None = None
            while time.monotonic() < deadline:
                last_state = read_ios_app_state(support_root, ios_server_url)
                texts = ios_texts(last_state)
                if RUNTIME_ECHO_TEXT in texts:
                    report.step("ios_receive_runtime_reply", started)
                    report.fact("ios_received_text", texts)
                    report.finish()
                    return
                time.sleep(1)

            self.fail(
                "iOS app store did not persist the Docker runtime reply; "
                f"messages={[(m.get('text'), len(m.get('media') or [])) for m in (last_state or {}).get('messages', [])]!r}"
            )

    def start_agent_container(self, server_url: str) -> None:
        run(["docker", "rm", "-f", CONTAINER], check=False, timeout=120)
        run(
            [
                "docker",
                "run",
                "--name",
                CONTAINER,
                "--detach",
                "--add-host",
                f"{DOCKER_HOST}:host-gateway",
                "--mount",
                f"type=volume,src={self.agent_volume},dst=/data/agent",
                "--env",
                f"FINITE_SERVER_URL={server_url}",
                "--env",
                "FINITECHAT_HERMES_INBOUND_STREAM=1",
                IMAGE,
            ],
            timeout=300,
        )

    def wait_for_log(self, token: str, timeout: float) -> str:
        deadline = time.monotonic() + timeout
        last = ""
        while time.monotonic() < deadline:
            logs = run(["docker", "logs", CONTAINER], check=False, timeout=30)
            last = f"{logs.stdout}\n{logs.stderr}"
            if token in last:
                return last
            time.sleep(0.5)
        raise AssertionError(f"container log did not contain {token!r}; logs={last[-4000:]}")

    def agent_identity(self) -> dict[str, Any]:
        return json.loads(
            run(
                [
                    "docker",
                    "exec",
                    CONTAINER,
                    "finitechat",
                    "identity",
                    "--agent-home",
                    "/data/agent",
                    "show",
                ],
                timeout=60,
            ).stdout
        )

    def agent_http_health(self) -> dict[str, Any]:
        return json.loads(
            run(
                [
                    "docker",
                    "exec",
                    CONTAINER,
                    "python",
                    "-c",
                    (
                        "import urllib.request; "
                        "print(urllib.request.urlopen("
                        "'http://127.0.0.1:8080/healthz', timeout=5"
                        ").read().decode())"
                    ),
                ],
                timeout=60,
            ).stdout
        )

    def docker_image_metadata(self) -> dict[str, Any]:
        inspected = json.loads(run(["docker", "image", "inspect", IMAGE], timeout=60).stdout)[0]
        return {
            "id": inspected.get("Id"),
            "repo_tags": inspected.get("RepoTags") or [],
            "architecture": inspected.get("Architecture"),
            "os": inspected.get("Os"),
        }


if __name__ == "__main__":
    unittest.main()
