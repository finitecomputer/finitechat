"""Agent-in-a-container end-to-end (ADR 0006 over Apple `container`).

Guest (Linux): latest pip `hermes-agent` + the finite-platform plugin + a
Linux build of the finitechat binary, running the echo agent.
Host (macOS): finitechat-server (the agent's home/room server) and a CLI
user who pairs via the invite URL + PIN and asserts the echo round trip.

Gated behind FINITE_CONTAINER_E2E=1 (run scripts/agent-container-e2e.sh);
requires `container system start` to have been run once.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import tempfile
import time
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
IMAGE = "finite-agent-e2e"
CONTAINER = "finite-agent-e2e-run"
SERVER_PORT = 18787
# The default vmnet subnet's host-side gateway address; the guest reaches
# the host server here, and the host can dial it too (it owns the address).
GATEWAY = os.environ.get("FINITE_CONTAINER_GATEWAY", "192.168.64.1")


def run(args, *, timeout=600, check=True, **kwargs):
    return subprocess.run(
        args, capture_output=True, text=True, timeout=timeout, check=check, **kwargs
    )


def stage_build_context(ctx: Path) -> None:
    for name, source in (("finitechat", REPO_ROOT),):
        run(
            [
                "rsync",
                "-a",
                "--exclude",
                ".git",
                "--exclude",
                "target",
                "--exclude",
                "__pycache__",
                "--exclude",
                ".DS_Store",
                f"{source}/",
                str(ctx / name),
            ]
        )


@unittest.skipUnless(
    os.environ.get("FINITE_CONTAINER_E2E") == "1",
    "set FINITE_CONTAINER_E2E=1 (scripts/agent-container-e2e.sh) to run",
)
class AgentContainerE2ETest(unittest.TestCase):
    def setUp(self):
        if shutil.which("container") is None:
            self.fail(
                "apple/container is not installed: "
                "sudo installer -pkg container-installer-signed.pkg -target /"
            )
        status = run(["container", "system", "status"], check=False)
        if status.returncode != 0:
            self.fail("container services are not running: `container system start`")
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.server_proc = None
        self.addCleanup(self._teardown_processes)

    def _teardown_processes(self):
        run(["container", "delete", "--force", CONTAINER], check=False, timeout=120)
        if self.server_proc is not None:
            self.server_proc.terminate()
            try:
                self.server_proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.server_proc.kill()

    def hermes_user(self, *args, timeout=120):
        result = run(
            [str(self.cli_bin), "hermes", "--home", str(self.user_home), *args],
            timeout=timeout,
        )
        return json.loads(result.stdout)

    def test_container_agent_pairs_via_invite_and_echoes(self):
        tmp = Path(self.tmp.name)

        # Host binaries (server + user CLI).
        run(
            ["cargo", "build", "--release", "-p", "finitechat-cli", "-p", "finitechat-server"],
            cwd=REPO_ROOT,
            timeout=1800,
        )
        self.cli_bin = REPO_ROOT / "target/release/finitechat"
        server_bin = REPO_ROOT / "target/release/finitechat-server"

        # Guest image: latest hermes-agent + plugin + Linux finitechat build.
        ctx = tmp / "ctx"
        ctx.mkdir()
        stage_build_context(ctx)
        run(
            [
                "container",
                "build",
                "--tag",
                IMAGE,
                "--file",
                str(ctx / "finitechat/containers/agent/Dockerfile"),
                str(ctx),
            ],
            timeout=3600,
        )

        # The agent's home server, reachable from the guest via the gateway.
        self.server_proc = subprocess.Popen(
            [
                str(server_bin),
                "serve",
                f"0.0.0.0:{SERVER_PORT}",
                "--sqlite",
                str(tmp / "server.sqlite3"),
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
        )
        self._wait_for_health(f"http://127.0.0.1:{SERVER_PORT}/health")

        run(
            [
                "container",
                "run",
                "--name",
                CONTAINER,
                "--detach",
                "--rm",
                "--env",
                f"FINITE_SERVER_URL=http://{GATEWAY}:{SERVER_PORT}",
                IMAGE,
            ],
            timeout=300,
        )
        self._wait_for_log("ECHO_AGENT_READY", timeout=120)

        # Fresh invite URL + PIN straight from the agent's stored invite.
        pin_info = json.loads(
            run(
                [
                    "container",
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
        invite_url = pin_info["url"]
        pin = pin_info["pin"]

        # The host-side user pairs exactly the way the app would.
        self.user_home = tmp / "user-home"
        self.hermes_user("init", "--server", f"http://127.0.0.1:{SERVER_PORT}")
        joined = self.hermes_user(
            "join",
            "--url",
            invite_url,
            "--pin",
            pin,
            "--name",
            "E2E User",
            "--timeout-ms",
            "90000",
            timeout=180,
        )
        self.assertEqual(joined["state"], "joined")
        room_id = joined["room_id"]

        # E2EE round trip through the real hermes-agent gateway base.
        self.hermes_user(
            "send",
            "--request-json",
            json.dumps(
                {
                    "room_id": room_id,
                    "conversation_id": None,
                    "text": "ping from the host",
                    "kind": "message",
                    "status": "complete",
                    "reply_to_message_id": None,
                }
            ),
        )
        deadline = time.monotonic() + 120
        echoed = None
        while time.monotonic() < deadline and echoed is None:
            poll = self.hermes_user(
                "poll",
                "--request-json",
                json.dumps({"timeout_millis": 10000}),
                timeout=60,
            )
            for event in poll.get("events", []):
                if event.get("text") == "echo: ping from the host":
                    echoed = event
        if echoed is None:
            logs = run(["container", "logs", CONTAINER], check=False, timeout=60)
            self.fail(f"no echo received; agent logs:\n{logs.stdout[-4000:]}")
        # The sender is the agent's MLS-authenticated identity.
        agent_config = json.loads(
            run(
                [
                    "container",
                    "exec",
                    CONTAINER,
                    "cat",
                    "/data/agent/config.json",
                ],
                timeout=60,
            ).stdout
        )
        self.assertEqual(echoed["source"]["user_id"], agent_config["account_id"])

    def _wait_for_health(self, url: str, timeout: float = 30) -> None:
        import urllib.request

        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                with urllib.request.urlopen(url, timeout=2) as response:
                    if response.status == 200:
                        return
            except Exception:
                time.sleep(0.2)
        self.fail(f"server at {url} never became healthy")

    def _wait_for_log(self, marker: str, timeout: float = 60) -> None:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            logs = run(["container", "logs", CONTAINER], check=False, timeout=60)
            if marker in (logs.stdout or ""):
                return
            time.sleep(2)
        logs = run(["container", "logs", CONTAINER], check=False, timeout=60)
        self.fail(f"container never printed {marker!r}; logs:\n{(logs.stdout or '')[-4000:]}")
