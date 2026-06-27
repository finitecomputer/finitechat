"""Container test helpers shared by Docker runtime smokes."""

from __future__ import annotations

import subprocess
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]


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
