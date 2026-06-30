#!/usr/bin/env python3
"""Small runtime health endpoint for container/Tinfoil probes."""

from __future__ import annotations

import json
import os
import subprocess
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any

AGENT_HOME = Path(os.environ.get("FINITECHAT_HOME", "/data/agent"))
FINITECHAT_BIN = os.environ.get("FINITECHAT_BIN", "/usr/local/bin/finitechat")
HOST = os.environ.get("FINITE_AGENT_HTTP_HOST", "0.0.0.0")
PORT = int(os.environ.get("FINITE_AGENT_HTTP_PORT", "8080"))
INVITE_FILE = AGENT_HOME / "current-invite.json"
ROOM_NAME = os.environ.get(
    "FINITECHAT_HERMES_ROOM_NAME",
    os.environ.get("FINITE_AGENT_NAME", "Finite Agent"),
)


def identity() -> dict[str, Any]:
    config_path = AGENT_HOME / "config.json"
    if not config_path.exists():
        return {"ready": False, "error": "agent home is not initialized"}
    try:
        proc = subprocess.run(
            [
                FINITECHAT_BIN,
                "identity",
                "--agent-home",
                str(AGENT_HOME),
                "show",
            ],
            capture_output=True,
            check=True,
            text=True,
            timeout=5,
        )
        value = json.loads(proc.stdout)
    except Exception as exc:
        return {"ready": False, "error": str(exc)}
    return {
        "ready": True,
        "npub": value.get("npub"),
        "account_id": value.get("account_id"),
    }


def finitechat_json(args: list[str]) -> dict[str, Any]:
    proc = subprocess.run(
        [FINITECHAT_BIN, *args],
        capture_output=True,
        check=True,
        text=True,
        timeout=10,
    )
    return json.loads(proc.stdout)


def invite() -> dict[str, Any]:
    identity_payload = identity()
    if not identity_payload.get("ready"):
        return identity_payload
    try:
        if INVITE_FILE.exists():
            invite_payload = json.loads(INVITE_FILE.read_text(encoding="utf-8"))
        else:
            invite_payload = finitechat_json([
                "hermes",
                "--agent-home",
                str(AGENT_HOME),
                "invite",
                "--room-name",
                ROOM_NAME,
                "--json",
            ])
            INVITE_FILE.write_text(json.dumps(invite_payload, indent=2), encoding="utf-8")
    except Exception as exc:
        return {"ready": False, "error": f"invite unavailable: {exc}"}
    return {
        "ready": True,
        "agent_npub": identity_payload.get("npub"),
        "account_id": identity_payload.get("account_id"),
        "room_id": invite_payload.get("room_id"),
        "invite_id": invite_payload.get("invite_id"),
        "url": invite_payload.get("url"),
    }


class Handler(BaseHTTPRequestHandler):
    def do_GET(self) -> None:
        if self.path == "/healthz":
            payload = identity()
            self._write(200 if payload["ready"] else 503, payload)
            return
        if self.path == "/invite":
            payload = invite()
            self._write(200 if payload["ready"] else 503, payload)
            return
        else:
            self._write(404, {"ready": False, "error": "not found"})
            return

    def log_message(self, fmt: str, *args: object) -> None:
        return

    def _write(self, status: int, payload: dict[str, Any]) -> None:
        body = json.dumps(payload, sort_keys=True).encode("utf-8")
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


def main() -> None:
    server = ThreadingHTTPServer((HOST, PORT), Handler)
    server.serve_forever()


if __name__ == "__main__":
    main()
