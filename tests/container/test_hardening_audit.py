"""Unit checks for the Hermes hardening evidence audit."""

from __future__ import annotations

import json
import subprocess
import tempfile
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
AUDIT_SCRIPT = REPO_ROOT / "scripts" / "hermes-hardening-audit.py"
IMAGE_ID = "sha256:local-image"
IMAGE_DIGEST = "ghcr.io/finitecomputer/finite-chat-hermes-runtime:v0.1.0@sha256:published"

SIDECAR_LAYERS = [
    "finitechat-server",
    "finitechat hermes CLI",
    "encrypted client stores",
    "finitechat hermes serve",
    "sidecar /v1/hermes/inbound NDJSON",
    "ack/drain",
    "agent reply",
    "user decrypt",
]
DOCKER_LAYERS = [
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
]
DOCKER_IOS_LAYERS = [
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
TINFOIL_LAYERS = [
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
]
MEDIA_STEPS = [
    "server_ready",
    "agent_init",
    "adapter_connect",
    "user_join",
    "user_send_media",
    "agent_receive_media",
    "user_receive_agent_replies",
]
IOS_MEDIA_STEPS = [
    "server_ready",
    "agent_init",
    "adapter_connect",
    "ios_app_launch",
    "agent_receive_ios_media",
    "ios_receive_agent_replies",
]
DOCKER_IOS_STEPS = [
    "host_server_ready",
    "docker_image_build",
    "agent_container_start",
    "agent_ready_log",
    "ios_app_launch",
    "agent_receive_ios_media",
    "ios_receive_runtime_reply",
]
ADAPTER_REGRESSION_LAYERS = [
    "plain message mapping",
    "redelivery dedupe",
    "ack retry without duplicate dispatch",
    "transient poll recovery",
    "sidecar startup",
    "service fallback",
    "service serialization",
    "media attachments",
    "outbound edit route",
    "typing activity",
    "room filtering",
    "group sender identity",
    "receipt/control stream filtering",
    "inbound stream fallback",
]


def write_json(path: Path, value: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value) + "\n", encoding="utf-8")


def sidecar_report() -> dict:
    return {"status": "passed", "proof_layers": SIDECAR_LAYERS}


def adapter_regression_report() -> dict:
    return {
        "status": "passed",
        "proof_layers": ADAPTER_REGRESSION_LAYERS,
        "test_count": len(ADAPTER_REGRESSION_LAYERS),
    }


def media_e2e_report() -> dict:
    return {
        "status": "passed",
        "facts": {
            "adapter_inbound_stream": True,
            "adapter_service_url_present": True,
            "agent_received_media_types": ["image/png"],
            "user_received_text": ["agent text echo: user media hello", "agent media echo"],
            "user_received_media_count": 1,
        },
        "steps": [{"name": name, "elapsed_ms": 1} for name in MEDIA_STEPS],
    }


def ios_media_e2e_report() -> dict:
    return {
        "status": "passed",
        "name": "ios_simulator_hermes_agent_media_e2e",
        "facts": {
            "platform": "ios_simulator",
            "simulator_udid": "booted-simulator",
            "adapter_inbound_stream": True,
            "adapter_service_url_present": True,
            "agent_received_media_types": ["image/png"],
            "ios_received_text": ["agent text echo: ios media hello", "agent media echo"],
            "ios_received_media_count": 1,
        },
        "steps": [{"name": name, "elapsed_ms": 1} for name in IOS_MEDIA_STEPS],
    }


def docker_report(restic_backend: str = "s3") -> dict:
    return {
        "status": "passed",
        "proof_layers": DOCKER_LAYERS,
        "facts": {
            "image_id": IMAGE_ID,
            "restic_backend": restic_backend,
            "hermes_agent_version_actual": "0.17.0",
            "agent_npub": "npub1agent",
            "agent_npub_after_restore": "npub1agent",
            "agent_state_backup": {"source": "entrypoint_backup_on_exit"},
        },
    }


def docker_ios_report() -> dict:
    return {
        "status": "passed",
        "name": "ios_simulator_docker_runtime_e2e",
        "proof_layers": DOCKER_IOS_LAYERS,
        "facts": {
            "platform": "ios_simulator",
            "simulator_udid": "booted-simulator",
            "hermes_agent_version_actual": "0.17.0",
            "agent_npub": "npub1agent",
            "runtime_health": {"ready": True, "npub": "npub1agent"},
            "agent_received_message_type": "photo",
            "agent_received_media_types": ["image/png"],
            "invite_rewritten_for_ios": True,
            "ios_received_text": ["echo: ios docker runtime hello"],
        },
        "steps": [{"name": name, "elapsed_ms": 1} for name in DOCKER_IOS_STEPS],
    }


def tinfoil_result() -> dict:
    return {
        "status": "passed",
        "proof_layers": TINFOIL_LAYERS,
        "facts": {
            "restic_backend": "s3",
            "restore_tag": "finite-agent-state",
            "health_npub": "npub1agent",
            "agent_npub_before_restart": "npub1agent",
            "agent_npub_after_restore": "npub1agent",
            "chat_before_restart": True,
            "chat_after_restart": True,
        },
    }


def run_audit(tmp: Path, *, require_complete: bool = False) -> tuple[int, dict]:
    args = [
        str(AUDIT_SCRIPT),
        "--adapter-regression-report",
        str(tmp / "adapter-regressions.json"),
        "--sidecar-report",
        str(tmp / "sidecar.json"),
        "--media-e2e-report",
        str(tmp / "media-e2e.json"),
        "--ios-media-e2e-report",
        str(tmp / "ios-media-e2e.json"),
        "--docker-report",
        str(tmp / "docker.json"),
        "--docker-ios-report",
        str(tmp / "docker-ios.json"),
        "--github-setup-report",
        str(tmp / "github-setup.json"),
        "--github-publish-gate-report",
        str(tmp / "github-publish-gate.json"),
        "--preflight-report",
        str(tmp / "preflight.json"),
        "--publish-report",
        str(tmp / "publish.json"),
        "--handoff-report",
        str(tmp / "handoff.json"),
        "--canary-summary",
        str(tmp / "canary-summary.json"),
        "--tinfoil-result",
        str(tmp / "tinfoil-result.json"),
        "--report",
        str(tmp / "audit.json"),
    ]
    if require_complete:
        args.append("--require-complete")
    result = subprocess.run(args, capture_output=True, text=True, check=False)
    return result.returncode, json.loads((tmp / "audit.json").read_text(encoding="utf-8"))


class HardeningAuditTest(unittest.TestCase):
    def test_audit_marks_local_only_smoke_incomplete(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_value:
            tmp = Path(tmp_value)
            write_json(tmp / "sidecar.json", sidecar_report())
            write_json(tmp / "docker.json", docker_report(restic_backend="local"))
            write_json(tmp / "preflight.json", {"status": "ok", "backend": "local"})
            status, audit = run_audit(tmp, require_complete=True)

        self.assertEqual(status, 2)
        self.assertEqual(audit["status"], "incomplete")
        self.assertIn("adapter_focused_regressions", audit["missing"])
        self.assertIn("local_hermes_agent_media_e2e", audit["missing"])
        self.assertIn("ios_simulator_media_e2e", audit["missing"])
        self.assertIn("docker_runtime_ios_e2e", audit["missing"])
        self.assertIn("github_actions_s3_setup_ready", audit["missing"])
        self.assertIn("github_publish_gate_ready", audit["missing"])
        self.assertIn("docker_runtime_s3_smoke", audit["missing"])
        self.assertIn("proven_image_published", audit["missing"])
        self.assertIn("tinfoil_canary_runtime", audit["missing"])

    def test_audit_marks_complete_evidence_complete(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_value:
            tmp = Path(tmp_value)
            write_json(tmp / "adapter-regressions.json", adapter_regression_report())
            write_json(tmp / "sidecar.json", sidecar_report())
            write_json(tmp / "media-e2e.json", media_e2e_report())
            write_json(tmp / "ios-media-e2e.json", ios_media_e2e_report())
            write_json(tmp / "docker.json", docker_report())
            write_json(tmp / "docker-ios.json", docker_ios_report())
            write_json(tmp / "github-setup.json", {"status": "ready"})
            write_json(tmp / "github-publish-gate.json", {"status": "passed"})
            write_json(tmp / "preflight.json", {"status": "ok", "backend": "s3"})
            write_json(
                tmp / "publish.json",
                {
                    "status": "published",
                    "source_image_id": IMAGE_ID,
                    "repo_digests": [IMAGE_DIGEST],
                },
            )
            write_json(tmp / "handoff.json", {"status": "ready"})
            write_json(tmp / "canary-summary.json", {"status": "ready"})
            write_json(tmp / "tinfoil-result.json", tinfoil_result())
            status, audit = run_audit(tmp, require_complete=True)

        self.assertEqual(status, 0)
        self.assertEqual(audit["status"], "complete")
        self.assertEqual(audit["missing"], [])

    def test_audit_rejects_unvalidated_tinfoil_success_flag(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_value:
            tmp = Path(tmp_value)
            write_json(tmp / "adapter-regressions.json", adapter_regression_report())
            write_json(tmp / "sidecar.json", sidecar_report())
            write_json(tmp / "media-e2e.json", media_e2e_report())
            write_json(tmp / "ios-media-e2e.json", ios_media_e2e_report())
            write_json(tmp / "docker.json", docker_report())
            write_json(tmp / "docker-ios.json", docker_ios_report())
            write_json(tmp / "github-setup.json", {"status": "ready"})
            write_json(tmp / "github-publish-gate.json", {"status": "passed"})
            write_json(tmp / "preflight.json", {"status": "ok", "backend": "s3"})
            write_json(
                tmp / "publish.json",
                {
                    "status": "published",
                    "source_image_id": IMAGE_ID,
                    "repo_digests": [IMAGE_DIGEST],
                },
            )
            write_json(tmp / "handoff.json", {"status": "ready"})
            write_json(tmp / "canary-summary.json", {"status": "ready"})
            write_json(tmp / "tinfoil-result.json", {"status": "passed"})
            status, audit = run_audit(tmp, require_complete=True)

        self.assertEqual(status, 2)
        self.assertEqual(audit["status"], "incomplete")
        self.assertIn("tinfoil_canary_runtime", audit["missing"])

    def test_audit_reports_github_setup_errors_before_s3_smoke(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_value:
            tmp = Path(tmp_value)
            write_json(tmp / "adapter-regressions.json", adapter_regression_report())
            write_json(tmp / "sidecar.json", sidecar_report())
            write_json(tmp / "media-e2e.json", media_e2e_report())
            write_json(tmp / "ios-media-e2e.json", ios_media_e2e_report())
            write_json(tmp / "docker.json", docker_report(restic_backend="local"))
            write_json(tmp / "docker-ios.json", docker_ios_report())
            write_json(
                tmp / "github-setup.json",
                {
                    "status": "failed",
                    "errors": ["missing required secret values: FINITE_DOCKER_RESTIC_PASSWORD"],
                },
            )
            write_json(
                tmp / "github-publish-gate.json",
                {
                    "status": "not_ready",
                    "errors": ["remote workflow ref is missing; push the branch before dispatch"],
                },
            )
            status, audit = run_audit(tmp, require_complete=True)

        self.assertEqual(status, 2)
        details = {check["name"]: check["detail"] for check in audit["checks"]}
        self.assertIn("FINITE_DOCKER_RESTIC_PASSWORD", details["github_actions_s3_setup_ready"])
        self.assertIn("remote workflow ref is missing", details["github_publish_gate_ready"])

    def test_audit_rejects_ios_success_flag_without_native_store_evidence(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_value:
            tmp = Path(tmp_value)
            write_json(tmp / "adapter-regressions.json", adapter_regression_report())
            write_json(tmp / "sidecar.json", sidecar_report())
            write_json(tmp / "media-e2e.json", media_e2e_report())
            write_json(
                tmp / "ios-media-e2e.json",
                {
                    "status": "passed",
                    "name": "ios_simulator_hermes_agent_media_e2e",
                },
            )
            write_json(tmp / "docker.json", docker_report())
            write_json(tmp / "docker-ios.json", docker_ios_report())
            write_json(tmp / "github-setup.json", {"status": "ready"})
            write_json(tmp / "github-publish-gate.json", {"status": "passed"})
            write_json(tmp / "preflight.json", {"status": "ok", "backend": "s3"})
            write_json(
                tmp / "publish.json",
                {
                    "status": "published",
                    "source_image_id": IMAGE_ID,
                    "repo_digests": [IMAGE_DIGEST],
                },
            )
            write_json(tmp / "handoff.json", {"status": "ready"})
            write_json(tmp / "canary-summary.json", {"status": "ready"})
            write_json(tmp / "tinfoil-result.json", tinfoil_result())
            status, audit = run_audit(tmp, require_complete=True)

        self.assertEqual(status, 2)
        self.assertEqual(audit["status"], "incomplete")
        self.assertIn("ios_simulator_media_e2e", audit["missing"])

    def test_audit_rejects_adapter_regression_report_missing_layers(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_value:
            tmp = Path(tmp_value)
            report = adapter_regression_report()
            report["proof_layers"] = [
                layer for layer in ADAPTER_REGRESSION_LAYERS if layer != "typing activity"
            ]
            write_json(tmp / "adapter-regressions.json", report)
            write_json(tmp / "sidecar.json", sidecar_report())
            write_json(tmp / "media-e2e.json", media_e2e_report())
            write_json(tmp / "ios-media-e2e.json", ios_media_e2e_report())
            write_json(tmp / "docker.json", docker_report())
            write_json(tmp / "docker-ios.json", docker_ios_report())
            write_json(tmp / "github-setup.json", {"status": "ready"})
            write_json(tmp / "github-publish-gate.json", {"status": "passed"})
            write_json(tmp / "preflight.json", {"status": "ok", "backend": "s3"})
            write_json(
                tmp / "publish.json",
                {
                    "status": "published",
                    "source_image_id": IMAGE_ID,
                    "repo_digests": [IMAGE_DIGEST],
                },
            )
            write_json(tmp / "handoff.json", {"status": "ready"})
            write_json(tmp / "canary-summary.json", {"status": "ready"})
            write_json(tmp / "tinfoil-result.json", tinfoil_result())
            status, audit = run_audit(tmp, require_complete=True)

        self.assertEqual(status, 2)
        self.assertEqual(audit["status"], "incomplete")
        self.assertIn("adapter_focused_regressions", audit["missing"])

    def test_audit_rejects_docker_ios_success_flag_without_native_runtime_evidence(
        self,
    ) -> None:
        with tempfile.TemporaryDirectory() as tmp_value:
            tmp = Path(tmp_value)
            write_json(tmp / "adapter-regressions.json", adapter_regression_report())
            write_json(tmp / "sidecar.json", sidecar_report())
            write_json(tmp / "media-e2e.json", media_e2e_report())
            write_json(tmp / "ios-media-e2e.json", ios_media_e2e_report())
            write_json(tmp / "docker.json", docker_report())
            write_json(
                tmp / "docker-ios.json",
                {
                    "status": "passed",
                    "name": "ios_simulator_docker_runtime_e2e",
                },
            )
            write_json(tmp / "github-setup.json", {"status": "ready"})
            write_json(tmp / "github-publish-gate.json", {"status": "passed"})
            write_json(tmp / "preflight.json", {"status": "ok", "backend": "s3"})
            write_json(
                tmp / "publish.json",
                {
                    "status": "published",
                    "source_image_id": IMAGE_ID,
                    "repo_digests": [IMAGE_DIGEST],
                },
            )
            write_json(tmp / "handoff.json", {"status": "ready"})
            write_json(tmp / "canary-summary.json", {"status": "ready"})
            write_json(tmp / "tinfoil-result.json", tinfoil_result())
            status, audit = run_audit(tmp, require_complete=True)

        self.assertEqual(status, 2)
        self.assertEqual(audit["status"], "incomplete")
        self.assertIn("docker_runtime_ios_e2e", audit["missing"])


if __name__ == "__main__":
    unittest.main()
