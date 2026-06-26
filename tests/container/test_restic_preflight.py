"""Unit checks for Docker restic preflight env normalization."""

from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
PREFLIGHT_PATH = REPO_ROOT / "scripts" / "hermes-restic-preflight.py"

spec = importlib.util.spec_from_file_location("hermes_restic_preflight", PREFLIGHT_PATH)
assert spec is not None
preflight = importlib.util.module_from_spec(spec)
assert spec.loader is not None
spec.loader.exec_module(preflight)


class ResticPreflightTest(unittest.TestCase):
    def test_accepts_finite_prefixed_aws_credentials(self) -> None:
        status, report = preflight.validate(
            {
                "FINITE_DOCKER_RESTIC_BACKEND": "s3",
                "FINITE_DOCKER_RESTIC_REPOSITORY": (
                    "s3:https://objects.nyc.storage.sh/tinfoil-agent-spike/hermes"
                ),
                "FINITE_DOCKER_RESTIC_PASSWORD": "user-owned-restore-key",
                "FINITE_DOCKER_RESTIC_AWS_ACCESS_KEY_ID": "access",
                "FINITE_DOCKER_RESTIC_AWS_SECRET_ACCESS_KEY": "secret",
            }
        )

        self.assertEqual(status, 0)
        self.assertEqual(report["status"], "ok")
        self.assertEqual(
            report["repository"],
            "s3:https://objects.nyc.storage.sh/tinfoil-agent-spike/hermes",
        )
        env = report["env"]
        self.assertTrue(env["AWS_ACCESS_KEY_ID"])
        self.assertTrue(env["AWS_SECRET_ACCESS_KEY"])
        self.assertTrue(env["FINITE_DOCKER_RESTIC_AWS_ACCESS_KEY_ID"])
        self.assertTrue(env["FINITE_DOCKER_RESTIC_AWS_SECRET_ACCESS_KEY"])

    def test_derives_repository_from_latitude_bucket(self) -> None:
        status, report = preflight.validate(
            {
                "FINITE_DOCKER_RESTIC_BACKEND": "s3",
                "FINITE_DOCKER_RESTIC_PASSWORD": "user-owned-restore-key",
                "FINITE_DOCKER_RESTIC_AWS_ACCESS_KEY_ID": "access",
                "FINITE_DOCKER_RESTIC_AWS_SECRET_ACCESS_KEY": "secret",
                "FINITE_LATITUDE_STORAGE_BUCKET": "tinfoil-agent-spike",
                "FINITE_DOCKER_RESTIC_PREFIX": "hermes-docker-smoke/canary",
            }
        )

        self.assertEqual(status, 0)
        self.assertEqual(
            report["repository"],
            "s3:https://objects.nyc.storage.sh/tinfoil-agent-spike/hermes-docker-smoke/canary",
        )

    def test_s3_backend_still_requires_user_owned_password(self) -> None:
        status, report = preflight.validate(
            {
                "FINITE_DOCKER_RESTIC_BACKEND": "s3",
                "FINITE_DOCKER_RESTIC_REPOSITORY": (
                    "s3:https://objects.nyc.storage.sh/tinfoil-agent-spike/hermes"
                ),
                "FINITE_DOCKER_RESTIC_AWS_ACCESS_KEY_ID": "access",
                "FINITE_DOCKER_RESTIC_AWS_SECRET_ACCESS_KEY": "secret",
            }
        )

        self.assertEqual(status, 2)
        self.assertEqual(report["status"], "failed")
        self.assertIn("FINITE_DOCKER_RESTIC_PASSWORD is required for backend=s3", report["errors"])


if __name__ == "__main__":
    unittest.main()
