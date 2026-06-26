#!/usr/bin/env python3
"""Validate restic backup env before running the expensive Docker smoke."""

from __future__ import annotations

import argparse
import json
import os
import sys
import time
from pathlib import Path
from urllib.parse import urlsplit, urlunsplit

DEFAULT_LOCAL_PASSWORD = "finite-docker-smoke-restic-key"
AWS_ENV_ALIASES = {
    "AWS_ACCESS_KEY_ID": "FINITE_DOCKER_RESTIC_AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY": "FINITE_DOCKER_RESTIC_AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN": "FINITE_DOCKER_RESTIC_AWS_SESSION_TOKEN",
    "AWS_REGION": "FINITE_DOCKER_RESTIC_AWS_REGION",
    "AWS_DEFAULT_REGION": "FINITE_DOCKER_RESTIC_AWS_DEFAULT_REGION",
}


def redact_repository(repository: str) -> str:
    if not repository.startswith("s3:"):
        return repository
    parsed = urlsplit(repository[3:])
    if not parsed.username and not parsed.password:
        return repository
    host = parsed.hostname or ""
    if parsed.port is not None:
        host = f"{host}:{parsed.port}"
    return "s3:" + urlunsplit((parsed.scheme, host, parsed.path, parsed.query, parsed.fragment))


def normalized_env(env: dict[str, str]) -> dict[str, str]:
    normalized = dict(env)
    for aws_name, finite_name in AWS_ENV_ALIASES.items():
        if not normalized.get(aws_name) and normalized.get(finite_name):
            normalized[aws_name] = normalized[finite_name]
    if (
        normalized.get("FINITE_DOCKER_RESTIC_BACKEND", "local").strip().lower() == "s3"
        and not normalized.get("FINITE_DOCKER_RESTIC_REPOSITORY", "").strip()
        and normalized.get("FINITE_LATITUDE_STORAGE_BUCKET", "").strip()
    ):
        endpoint = normalized.get(
            "FINITE_LATITUDE_OBJECT_ENDPOINT", "https://objects.nyc.storage.sh"
        ).rstrip("/")
        bucket = normalized["FINITE_LATITUDE_STORAGE_BUCKET"].strip().strip("/")
        prefix = normalized.get("FINITE_DOCKER_RESTIC_PREFIX", "hermes-docker-smoke")
        prefix = prefix.strip().strip("/")
        normalized["FINITE_DOCKER_RESTIC_REPOSITORY"] = f"s3:{endpoint}/{bucket}/{prefix}"
    return normalized


def validate(env: dict[str, str]) -> tuple[int, dict[str, object]]:
    env = normalized_env(env)
    backend = env.get("FINITE_DOCKER_RESTIC_BACKEND", "local").strip().lower()
    repository = env.get("FINITE_DOCKER_RESTIC_REPOSITORY", "").strip()
    password = env.get("FINITE_DOCKER_RESTIC_PASSWORD", "")
    errors: list[str] = []
    warnings: list[str] = []

    if backend not in {"local", "s3"}:
        errors.append("FINITE_DOCKER_RESTIC_BACKEND must be 'local' or 's3'")

    report: dict[str, object] = {
        "status": "ok",
        "generated_at_unix": int(time.time()),
        "backend": backend,
        "repository": None,
        "env": {
            "FINITE_DOCKER_RESTIC_BACKEND": bool(env.get("FINITE_DOCKER_RESTIC_BACKEND")),
            "FINITE_DOCKER_RESTIC_REPOSITORY": bool(repository),
            "FINITE_DOCKER_RESTIC_PASSWORD": bool(password),
            "AWS_ACCESS_KEY_ID": bool(env.get("AWS_ACCESS_KEY_ID")),
            "AWS_SECRET_ACCESS_KEY": bool(env.get("AWS_SECRET_ACCESS_KEY")),
            "AWS_SESSION_TOKEN": bool(env.get("AWS_SESSION_TOKEN")),
            "AWS_REGION": bool(env.get("AWS_REGION")),
            "AWS_DEFAULT_REGION": bool(env.get("AWS_DEFAULT_REGION")),
            "FINITE_DOCKER_RESTIC_AWS_ACCESS_KEY_ID": bool(
                env.get("FINITE_DOCKER_RESTIC_AWS_ACCESS_KEY_ID")
            ),
            "FINITE_DOCKER_RESTIC_AWS_SECRET_ACCESS_KEY": bool(
                env.get("FINITE_DOCKER_RESTIC_AWS_SECRET_ACCESS_KEY")
            ),
            "FINITE_LATITUDE_STORAGE_BUCKET": bool(env.get("FINITE_LATITUDE_STORAGE_BUCKET")),
        },
        "warnings": warnings,
        "errors": errors,
    }

    if backend == "local":
        if not password:
            warnings.append(
                "FINITE_DOCKER_RESTIC_PASSWORD is unset; the local Docker smoke "
                "will use its disposable default password"
            )
        return finish(report)

    if backend == "s3":
        if not repository:
            errors.append("FINITE_DOCKER_RESTIC_REPOSITORY is required for backend=s3")
        elif not repository.startswith("s3:"):
            errors.append("FINITE_DOCKER_RESTIC_REPOSITORY must start with 's3:'")
        else:
            report["repository"] = redact_repository(repository)
            parsed = urlsplit(repository[3:])
            if parsed.username or parsed.password:
                errors.append(
                    "FINITE_DOCKER_RESTIC_REPOSITORY must not contain URL userinfo; "
                    "pass object-storage credentials through AWS_* env vars"
                )
            if not parsed.scheme or not parsed.netloc or not parsed.path.strip("/"):
                errors.append(
                    "FINITE_DOCKER_RESTIC_REPOSITORY must look like "
                    "s3:https://endpoint/bucket/prefix"
                )
        if not password:
            errors.append("FINITE_DOCKER_RESTIC_PASSWORD is required for backend=s3")
        elif password == DEFAULT_LOCAL_PASSWORD:
            errors.append(
                "FINITE_DOCKER_RESTIC_PASSWORD must be user-owned for backend=s3, "
                "not the local smoke default"
            )
        for name in ("AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY"):
            if not env.get(name):
                errors.append(f"{name} is required for backend=s3")
        if not env.get("AWS_REGION") and not env.get("AWS_DEFAULT_REGION"):
            warnings.append(
                "AWS_REGION/AWS_DEFAULT_REGION is unset; S3-compatible endpoints may "
                "still work, but set one if the provider requires request signing region"
            )

    return finish(report)


def finish(report: dict[str, object]) -> tuple[int, dict[str, object]]:
    errors = report["errors"]
    if isinstance(errors, list) and errors:
        report["status"] = "failed"
        return 2, report
    return 0, report


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--report", help="optional JSON report path")
    args = parser.parse_args()
    status, report = validate(dict(os.environ))
    text = json.dumps(report, indent=2) + "\n"
    if args.report:
        path = Path(args.report)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text, encoding="utf-8")
    print(text, end="")
    return status


if __name__ == "__main__":
    sys.exit(main())
