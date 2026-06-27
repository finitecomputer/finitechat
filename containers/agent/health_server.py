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


class Handler(BaseHTTPRequestHandler):
    def do_GET(self) -> None:
        if self.path != "/healthz":
            self._write(404, {"ready": False, "error": "not found"})
            return
        payload = identity()
        self._write(200 if payload["ready"] else 503, payload)

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
