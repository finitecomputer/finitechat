#!/usr/bin/env python3
"""Audit Hermes hardening evidence reports without overstating completion."""

from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path
from typing import Any

REQUIRED_DOCKER_PROOF_LAYERS = {
    "docker image build",
    "hermes-agent 0.17 runtime",
    "finitechat binary in image",
    "finite-platform plugin in image",
    "E2EE echo round trip before restore",
    "entrypoint restic encrypted agent state snapshot on shutdown",
    "restic repository check",
    "agent state volume wipe",
    "fresh agent container with empty local state",
    "entrypoint restic latest-by-tag restore into fresh volume",
    "same agent npub after restore",
    "runtime HTTP health endpoint after restore",
    "E2EE echo round trip after restore",
}
REQUIRED_SIDECAR_PROOF_LAYERS = {
    "finitechat-server",
    "finitechat hermes CLI",
    "encrypted client stores",
    "finitechat hermes serve",
    "sidecar /v1/hermes/inbound NDJSON",
    "ack/drain",
    "agent reply",
    "user decrypt",
}
REQUIRED_MEDIA_E2E_STEPS = {
    "server_ready",
    "agent_init",
    "adapter_connect",
    "user_join",
    "user_send_media",
    "agent_receive_media",
    "user_receive_agent_replies",
}
REQUIRED_IOS_MEDIA_E2E_STEPS = {
    "server_ready",
    "agent_init",
    "adapter_connect",
    "ios_app_launch",
    "agent_receive_ios_media",
    "ios_receive_agent_replies",
}
REQUIRED_TINFOIL_PROOF_LAYERS = {
    "Tinfoil container running",
    "digest-pinned runtime image",
    "S3 restic repository",
    "attested health proxy ready",
    "agent npub observed before restart",
    "Finite Chat round trip before restart",
    "entrypoint backup observed on clean stop",
    "latest-by-tag restore observed after restart",
    "same agent npub after restore",
    "Finite Chat round trip after restore",
}


def load_optional_json(path: Path) -> dict[str, Any] | None:
    if not path.exists():
        return None
    return json.loads(path.read_text(encoding="utf-8"))


def add_check(
    checks: list[dict[str, Any]],
    *,
    name: str,
    status: str,
    evidence: str | None = None,
    detail: str | None = None,
) -> None:
    checks.append(
        {
            "name": name,
            "status": status,
            "evidence": evidence,
            "detail": detail,
        }
    )


def missing_layers(report: dict[str, Any], required: set[str]) -> list[str]:
    present = set(report.get("proof_layers") or [])
    return sorted(required - present)


def list_detail(value: Any) -> str:
    if isinstance(value, list):
        return ", ".join(str(item) for item in value)
    return str(value) if value else ""


def step_names(report: dict[str, Any]) -> set[str]:
    steps = report.get("steps")
    if not isinstance(steps, list):
        return set()
    return {
        str(step.get("name"))
        for step in steps
        if isinstance(step, dict) and isinstance(step.get("name"), str)
    }


def audit(args: argparse.Namespace) -> dict[str, Any]:
    sidecar_path = Path(args.sidecar_report)
    media_e2e_path = Path(args.media_e2e_report)
    ios_media_e2e_path = Path(args.ios_media_e2e_report)
    docker_path = Path(args.docker_report)
    github_setup_path = Path(args.github_setup_report)
    github_publish_gate_path = Path(args.github_publish_gate_report)
    preflight_path = Path(args.preflight_report)
    publish_path = Path(args.publish_report)
    handoff_path = Path(args.handoff_report)
    canary_summary_path = Path(args.canary_summary)
    tinfoil_result_path = Path(args.tinfoil_result)

    sidecar = load_optional_json(sidecar_path)
    media_e2e = load_optional_json(media_e2e_path)
    ios_media_e2e = load_optional_json(ios_media_e2e_path)
    docker = load_optional_json(docker_path)
    github_setup = load_optional_json(github_setup_path)
    github_publish_gate = load_optional_json(github_publish_gate_path)
    preflight = load_optional_json(preflight_path)
    publish = load_optional_json(publish_path)
    handoff = load_optional_json(handoff_path)
    canary_summary = load_optional_json(canary_summary_path)
    tinfoil_result = load_optional_json(tinfoil_result_path)

    checks: list[dict[str, Any]] = []
    add_check(
        checks,
        name="local_sidecar_smoke",
        status="passed"
        if sidecar
        and sidecar.get("status") == "passed"
        and not missing_layers(sidecar, REQUIRED_SIDECAR_PROOF_LAYERS)
        else "missing",
        evidence=str(sidecar_path) if sidecar else None,
        detail=None
        if sidecar and not missing_layers(sidecar, REQUIRED_SIDECAR_PROOF_LAYERS)
        else f"missing layers: {', '.join(missing_layers(sidecar or {}, REQUIRED_SIDECAR_PROOF_LAYERS))}",
    )

    media_facts = media_e2e.get("facts", {}) if isinstance(media_e2e, dict) else {}
    media_steps_missing = sorted(REQUIRED_MEDIA_E2E_STEPS - step_names(media_e2e or {}))
    media_texts = media_facts.get("user_received_text")
    media_types = media_facts.get("agent_received_media_types")
    media_passed = (
        bool(media_e2e)
        and media_e2e.get("status") == "passed"
        and not media_steps_missing
        and media_facts.get("adapter_inbound_stream") is True
        and media_facts.get("adapter_service_url_present") is True
        and isinstance(media_types, list)
        and "image/png" in media_types
        and isinstance(media_texts, list)
        and "agent text echo: user media hello" in media_texts
        and "agent media echo" in media_texts
        and int(media_facts.get("user_received_media_count") or 0) >= 1
    )
    add_check(
        checks,
        name="local_hermes_agent_media_e2e",
        status="passed" if media_passed else "missing",
        evidence=str(media_e2e_path) if media_e2e else None,
        detail=None
        if media_passed
        else (
            "requires live hermes-agent media e2e report with sidecar stream, "
            f"text and image replies, and user decrypt; missing steps: {', '.join(media_steps_missing)}"
        ),
    )

    ios_facts = ios_media_e2e.get("facts", {}) if isinstance(ios_media_e2e, dict) else {}
    ios_steps_missing = sorted(REQUIRED_IOS_MEDIA_E2E_STEPS - step_names(ios_media_e2e or {}))
    ios_texts = ios_facts.get("ios_received_text")
    ios_media_types = ios_facts.get("agent_received_media_types")
    ios_passed = (
        bool(ios_media_e2e)
        and ios_media_e2e.get("status") == "passed"
        and ios_media_e2e.get("name") == "ios_simulator_hermes_agent_media_e2e"
        and not ios_steps_missing
        and ios_facts.get("platform") == "ios_simulator"
        and ios_facts.get("adapter_inbound_stream") is True
        and ios_facts.get("adapter_service_url_present") is True
        and bool(ios_facts.get("simulator_udid"))
        and isinstance(ios_media_types, list)
        and "image/png" in ios_media_types
        and isinstance(ios_texts, list)
        and "agent text echo: ios media hello" in ios_texts
        and "agent media echo" in ios_texts
        and int(ios_facts.get("ios_received_media_count") or 0) >= 1
    )
    add_check(
        checks,
        name="ios_simulator_media_e2e",
        status="passed" if ios_passed else "missing",
        evidence=str(ios_media_e2e_path) if ios_media_e2e else None,
        detail=None
        if ios_passed
        else (
            "requires live iOS Simulator media e2e report with sidecar stream, "
            f"native store decrypt, and text/image replies; missing steps: {', '.join(ios_steps_missing)}"
        ),
    )

    docker_missing = missing_layers(docker or {}, REQUIRED_DOCKER_PROOF_LAYERS)
    docker_facts = docker.get("facts", {}) if isinstance(docker, dict) else {}
    docker_passed = (
        bool(docker)
        and docker.get("status") == "passed"
        and not docker_missing
        and docker_facts.get("hermes_agent_version_actual") == "0.17.0"
        and docker_facts.get("agent_npub") == docker_facts.get("agent_npub_after_restore")
        and docker_facts.get("agent_state_backup", {}).get("source") == "entrypoint_backup_on_exit"
    )
    add_check(
        checks,
        name="docker_runtime_local_or_s3_smoke",
        status="passed" if docker_passed else "missing",
        evidence=str(docker_path) if docker else None,
        detail=None if docker_passed else f"missing layers: {', '.join(docker_missing)}",
    )
    s3_smoke = docker_passed and docker_facts.get("restic_backend") == "s3"

    github_setup_ready = (
        bool(github_setup)
        and github_setup.get("status") in {"ready", "applied"}
        and not github_setup.get("missing_required_secrets")
        and not github_setup.get("missing_required_variables")
    )
    github_setup_detail = (
        "requires GitHub secret/variable setup for S3 CI gate"
        if not github_setup
        else list_detail(github_setup.get("errors"))
        or "requires GitHub secret/variable setup for S3 CI gate"
    )
    add_check(
        checks,
        name="github_actions_s3_setup_ready",
        status="passed" if github_setup_ready or s3_smoke else "missing",
        evidence=str(github_setup_path) if github_setup else None,
        detail=None if github_setup_ready or s3_smoke else github_setup_detail,
    )

    github_publish_gate_ready = (
        bool(github_publish_gate) and github_publish_gate.get("status") == "passed"
    )
    github_publish_gate_detail = (
        "requires pushed branch and successful GitHub publish-gate run"
        if not github_publish_gate
        else list_detail(github_publish_gate.get("errors"))
        or f"publish-gate status={github_publish_gate.get('status')!r}"
    )
    add_check(
        checks,
        name="github_publish_gate_ready",
        status="passed" if github_publish_gate_ready or s3_smoke else "missing",
        evidence=str(github_publish_gate_path) if github_publish_gate else None,
        detail=None if github_publish_gate_ready or s3_smoke else github_publish_gate_detail,
    )

    add_check(
        checks,
        name="docker_runtime_s3_smoke",
        status="passed" if s3_smoke else "missing",
        evidence=str(docker_path) if docker else None,
        detail=f"restic_backend={docker_facts.get('restic_backend')!r}; expected 's3'",
    )

    preflight_s3 = (
        bool(preflight) and preflight.get("status") == "ok" and preflight.get("backend") == "s3"
    )
    add_check(
        checks,
        name="s3_restic_preflight",
        status="passed" if preflight_s3 else "missing",
        evidence=str(preflight_path) if preflight else None,
        detail=None
        if preflight_s3
        else f"backend={preflight.get('backend') if preflight else None!r}",
    )

    publish_passed = (
        bool(publish)
        and publish.get("status") == "published"
        and publish.get("source_image_id") == docker_facts.get("image_id")
        and bool(publish.get("repo_digests"))
    )
    add_check(
        checks,
        name="proven_image_published",
        status="passed" if publish_passed else "missing",
        evidence=str(publish_path) if publish else None,
        detail=None
        if publish_passed
        else "requires published image report with repo digest matching Docker smoke image id",
    )

    handoff_ready = bool(handoff) and handoff.get("status") == "ready"
    add_check(
        checks,
        name="tinfoil_handoff_ready",
        status="passed" if handoff_ready else "missing",
        evidence=str(handoff_path) if handoff else None,
        detail=None
        if handoff_ready
        else "requires ready handoff from S3 smoke and published digest",
    )

    canary_ready = bool(canary_summary) and canary_summary.get("status") == "ready"
    add_check(
        checks,
        name="tinfoil_canary_artifacts_ready",
        status="passed" if canary_ready else "missing",
        evidence=str(canary_summary_path) if canary_summary else None,
        detail=None if canary_ready else "requires generated digest-pinned Tinfoil config/runbook",
    )

    tinfoil_missing = missing_layers(tinfoil_result or {}, REQUIRED_TINFOIL_PROOF_LAYERS)
    tinfoil_facts = tinfoil_result.get("facts", {}) if isinstance(tinfoil_result, dict) else {}
    tinfoil_passed = (
        bool(tinfoil_result)
        and tinfoil_result.get("status") == "passed"
        and not tinfoil_missing
        and tinfoil_facts.get("restic_backend") == "s3"
        and tinfoil_facts.get("restore_tag") == "finite-agent-state"
        and tinfoil_facts.get("agent_npub_before_restart")
        == tinfoil_facts.get("agent_npub_after_restore")
        and tinfoil_facts.get("health_npub") == tinfoil_facts.get("agent_npub_after_restore")
        and tinfoil_facts.get("chat_before_restart") is True
        and tinfoil_facts.get("chat_after_restart") is True
    )
    add_check(
        checks,
        name="tinfoil_canary_runtime",
        status="passed" if tinfoil_passed else "missing",
        evidence=str(tinfoil_result_path) if tinfoil_result else None,
        detail=None
        if tinfoil_passed
        else (
            "requires validated live Tinfoil start/health/chat/backup/restart/restore/chat "
            f"evidence; missing layers: {', '.join(tinfoil_missing)}"
        ),
    )

    missing = [check for check in checks if check["status"] != "passed"]
    return {
        "status": "complete" if not missing else "incomplete",
        "generated_at_unix": int(time.time()),
        "checks": checks,
        "missing": [check["name"] for check in missing],
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--sidecar-report",
        default="target/hermes-sidecar-smoke/report.json",
    )
    parser.add_argument(
        "--media-e2e-report",
        default="target/hermes-agent-media-e2e/report.json",
    )
    parser.add_argument(
        "--ios-media-e2e-report",
        default="target/ios-hermes-agent-media-e2e/report.json",
    )
    parser.add_argument(
        "--docker-report",
        default="target/hermes-docker-smoke/report.json",
    )
    parser.add_argument(
        "--github-setup-report",
        default="target/hermes-github-secrets-setup.json",
    )
    parser.add_argument(
        "--github-publish-gate-report",
        default="target/hermes-github-publish-gate/report.json",
    )
    parser.add_argument(
        "--preflight-report",
        default="target/hermes-docker-smoke/restic-preflight.json",
    )
    parser.add_argument(
        "--publish-report",
        default="target/hermes-docker-smoke/image-publish.json",
    )
    parser.add_argument(
        "--handoff-report",
        default="target/hermes-docker-smoke/tinfoil-handoff.json",
    )
    parser.add_argument(
        "--canary-summary",
        default="target/hermes-docker-smoke/tinfoil-canary/tinfoil-canary-summary.json",
    )
    parser.add_argument(
        "--tinfoil-result",
        default="target/hermes-docker-smoke/tinfoil-canary-result.json",
    )
    parser.add_argument("--report", default="target/hermes-hardening-audit.json")
    parser.add_argument("--require-complete", action="store_true")
    args = parser.parse_args()

    report = audit(args)
    report_path = Path(args.report)
    report_path.parent.mkdir(parents=True, exist_ok=True)
    text = json.dumps(report, indent=2) + "\n"
    report_path.write_text(text, encoding="utf-8")
    print(text, end="")
    if args.require_complete and report["status"] != "complete":
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
