"""Unit checks for GitHub Actions secret setup helper."""

from __future__ import annotations

import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
SCRIPT_PATH = REPO_ROOT / "scripts" / "hermes-github-secrets-setup.py"

spec = importlib.util.spec_from_file_location("hermes_github_secrets_setup", SCRIPT_PATH)
assert spec is not None
setup = importlib.util.module_from_spec(spec)
assert spec.loader is not None
spec.loader.exec_module(setup)


class GithubSecretsSetupTest(unittest.TestCase):
    def test_parse_env_file_supports_quotes_and_export(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_value:
            env_file = Path(tmp_value) / ".env"
            env_file.write_text(
                "\n".join(
                    [
                        "FINITE_DOCKER_RESTIC_PASSWORD='restore key'",
                        'export AWS_ACCESS_KEY_ID="access"',
                        "AWS_SECRET_ACCESS_KEY=secret",
                        "FINITE_LATITUDE_STORAGE_BUCKET=bucket",
                    ]
                ),
                encoding="utf-8",
            )

            values = setup.parse_env_file(env_file)

        self.assertEqual(values["FINITE_DOCKER_RESTIC_PASSWORD"], "restore key")
        self.assertEqual(values["AWS_ACCESS_KEY_ID"], "access")
        self.assertEqual(values["AWS_SECRET_ACCESS_KEY"], "secret")
        self.assertEqual(values["FINITE_LATITUDE_STORAGE_BUCKET"], "bucket")

    def test_build_report_is_ready_without_leaking_values(self) -> None:
        values = {
            "FINITE_DOCKER_RESTIC_PASSWORD": "restore-key",
            "AWS_ACCESS_KEY_ID": "access",
            "AWS_SECRET_ACCESS_KEY": "private-token-value",
            "FINITE_LATITUDE_STORAGE_BUCKET": "bucket",
        }

        status, report = setup.build_report(
            repo="finitecomputer/finitechat",
            env_file=Path(".env"),
            values=values,
            existing_secret_names=set(),
            existing_variable_names=set(),
            apply=False,
        )
        serialized = json.dumps(report)

        self.assertEqual(status, 0)
        self.assertEqual(report["status"], "ready")
        self.assertEqual(report["missing_required_secrets"], [])
        self.assertEqual(report["missing_required_variables"], [])
        self.assertIn("FINITE_DOCKER_RESTIC_AWS_ACCESS_KEY_ID", serialized)
        self.assertIn("FINITE_LATITUDE_OBJECT_ENDPOINT", serialized)
        self.assertNotIn("restore-key", serialized)
        self.assertNotIn("access", serialized)
        self.assertNotIn("private-token-value", serialized)
        self.assertNotIn("bucket", serialized)

    def test_build_report_fails_without_required_values(self) -> None:
        status, report = setup.build_report(
            repo="finitecomputer/finitechat",
            env_file=Path(".env"),
            values={},
            existing_secret_names=set(),
            existing_variable_names=set(),
            apply=False,
        )

        self.assertEqual(status, 2)
        self.assertEqual(report["status"], "failed")
        self.assertIn("FINITE_DOCKER_RESTIC_PASSWORD", report["missing_required_secrets"])
        self.assertIn(
            "FINITE_DOCKER_RESTIC_AWS_SECRET_ACCESS_KEY",
            report["missing_required_secrets"],
        )
        self.assertEqual(report["missing_required_variables"], ["FINITE_LATITUDE_STORAGE_BUCKET"])

    def test_build_report_accepts_existing_remote_names_without_local_values(self) -> None:
        status, report = setup.build_report(
            repo="finitecomputer/finitechat",
            env_file=Path(".env"),
            values={},
            existing_secret_names={
                "FINITE_DOCKER_RESTIC_PASSWORD",
                "FINITE_DOCKER_RESTIC_AWS_ACCESS_KEY_ID",
                "FINITE_DOCKER_RESTIC_AWS_SECRET_ACCESS_KEY",
            },
            existing_variable_names={"FINITE_LATITUDE_STORAGE_BUCKET"},
            apply=False,
        )

        self.assertEqual(status, 0)
        self.assertEqual(report["status"], "ready")
        self.assertEqual(report["missing_required_secrets"], [])
        self.assertEqual(report["missing_required_variables"], [])
        self.assertEqual(
            report["existing_required_secrets"],
            [
                "FINITE_DOCKER_RESTIC_AWS_ACCESS_KEY_ID",
                "FINITE_DOCKER_RESTIC_AWS_SECRET_ACCESS_KEY",
                "FINITE_DOCKER_RESTIC_PASSWORD",
            ],
        )
        self.assertEqual(report["existing_required_variables"], ["FINITE_LATITUDE_STORAGE_BUCKET"])


if __name__ == "__main__":
    unittest.main()
