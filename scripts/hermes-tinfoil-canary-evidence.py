#!/usr/bin/env python3
"""Build live Tinfoil canary evidence JSON from observed artifacts."""

from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path
from typing import Any

DEFAULT_EVIDENCE_JSON = "target/hermes-docker-smoke/tinfoil-canary-evidence.json"
DEFAULT_HANDOFF_REPORT = "target/hermes-docker-smoke/tinfoil-handoff.json"
DEFAULT_CANARY_SUMMARY = "target/hermes-docker-smoke/tinfoil-canary/tinfoil-canary-summary.json"


def load_optional_json(path: str | None) -> dict[str, Any]:
    if not path:
        return {}
    if not Path(path).exists():
        return {}
    value = json.loads(Path(path).read_text(encoding="utf-8"))
    return value if isinstance(value, dict) else {}


def first_string(*values: Any) -> str | None:
    for value in values:
        if isinstance(value, str) and value.strip():
            return value.strip()
    return None


def first_bool(*values: Any) -> bool | None:
    for value in values:
        if isinstance(value, bool):
            return value
    return None


def object_dict(value: Any) -> dict[str, Any]:
    return value if isinstance(value, dict) else {}


def container_from_json(container: dict[str, Any]) -> dict[str, str | None]:
    return {
        "name": first_string(container.get("name"), container.get("container_name")),
        "status": first_string(container.get("status"), container.get("state")),
        "url": first_string(
            container.get("url"),
            container.get("container_url"),
            container.get("containerUrl"),
            container.get("containerURL"),
        ),
    }


def build_evidence(args: argparse.Namespace) -> dict[str, Any]:
    handoff = load_optional_json(args.handoff_report)
    canary_summary = load_optional_json(args.canary_summary)
    container_json = load_optional_json(args.container_json)
    health_json = load_optional_json(args.health_json)
    handoff_image = object_dict(handoff.get("image"))
    handoff_restore = object_dict(handoff.get("restore"))
    summary_container_name = canary_summary.get("container_name")
    summary_image_digest = canary_summary.get("image_digest")
    container = container_from_json(container_json)
    health_ready = (
        args.health_ready if args.health_ready is not None else first_bool(health_json.get("ready"))
    )
    health_npub = first_string(args.health_npub, health_json.get("npub"))
    npub_before = first_string(args.npub_before_restart, health_npub)
    npub_after = first_string(args.npub_after_restore, health_npub)
    evidence = {
        "generated_at_unix": int(time.time()),
        "sources": {
            "handoff_report": args.handoff_report,
            "canary_summary": args.canary_summary,
            "container_json": args.container_json,
            "health_json": args.health_json,
        },
        "container": {
            "name": first_string(args.container_name, container["name"], summary_container_name),
            "status": first_string(args.container_status, container["status"]),
            "url": first_string(args.container_url, container["url"]),
        },
        "image": {
            "digest": first_string(
                args.image_digest, summary_image_digest, handoff_image.get("digest")
            ),
        },
        "storage": {
            "backend": first_string(args.storage_backend, handoff_restore.get("backend")),
            "restore_tag": first_string(args.restore_tag, handoff_restore.get("restore_tag")),
        },
        "health": {
            "ready": health_ready is True,
            "npub": health_npub,
        },
        "chat": {
            "before_restart": {
                "ok": bool(args.chat_before_ok or args.chat_before_message_id),
                "message_id": args.chat_before_message_id,
            },
            "after_restart": {
                "ok": bool(args.chat_after_ok or args.chat_after_message_id),
                "message_id": args.chat_after_message_id,
            },
        },
        "restart_restore": {
            "npub_before_restart": npub_before,
            "npub_after_restore": npub_after,
            "same_npub": bool(npub_before and npub_after and npub_before == npub_after),
            "backup_observed": args.backup_observed,
            "restore_observed": args.restore_observed,
        },
    }
    return evidence


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--handoff-report", default=DEFAULT_HANDOFF_REPORT)
    parser.add_argument("--canary-summary", default=DEFAULT_CANARY_SUMMARY)
    parser.add_argument("--container-json")
    parser.add_argument("--health-json")
    parser.add_argument("--container-name")
    parser.add_argument("--container-status")
    parser.add_argument("--container-url")
    parser.add_argument("--image-digest")
    parser.add_argument("--storage-backend")
    parser.add_argument("--restore-tag")
    parser.add_argument("--health-ready", action="store_true", default=None)
    parser.add_argument("--health-npub")
    parser.add_argument("--npub-before-restart")
    parser.add_argument("--npub-after-restore")
    parser.add_argument("--chat-before-ok", action="store_true")
    parser.add_argument("--chat-before-message-id")
    parser.add_argument("--chat-after-ok", action="store_true")
    parser.add_argument("--chat-after-message-id")
    parser.add_argument("--backup-observed", action="store_true")
    parser.add_argument("--restore-observed", action="store_true")
    parser.add_argument("--evidence-json", default=DEFAULT_EVIDENCE_JSON)
    args = parser.parse_args()

    evidence = build_evidence(args)
    evidence_path = Path(args.evidence_json)
    evidence_path.parent.mkdir(parents=True, exist_ok=True)
    text = json.dumps(evidence, indent=2) + "\n"
    evidence_path.write_text(text, encoding="utf-8")
    print(text, end="")
    return 0


if __name__ == "__main__":
    sys.exit(main())
