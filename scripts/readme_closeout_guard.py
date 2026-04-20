#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path


REQUIRED_HEADINGS = [
    "## 当前状态",
    "## 本轮收口验证",
    "## 当前边界",
    "## 仓库卫生要求",
]

BLOCKED_RESIDUE = [
    ".chrome-devtools/",
    ".data/",
    ".omx/",
]


def run(cmd: list[str], cwd: Path) -> str:
    result = subprocess.run(
        cmd,
        cwd=str(cwd),
        text=True,
        capture_output=True,
        check=False,
    )
    if result.returncode != 0:
        raise RuntimeError(
            f"command failed: {' '.join(cmd)}\nstdout:\n{result.stdout}\nstderr:\n{result.stderr}"
        )
    return result.stdout


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("project_root")
    parser.add_argument("--expect-clean", action="store_true")
    args = parser.parse_args()

    root = Path(args.project_root).resolve()
    readme = root / "README.md"
    package_json = root / "package.json"

    if not readme.exists():
        print("ERROR: missing README.md at project root", file=sys.stderr)
        return 1

    try:
        run(["git", "rev-parse", "--is-inside-work-tree"], root)
    except RuntimeError as exc:
        print(f"ERROR: not a git repository\n{exc}", file=sys.stderr)
        return 1

    try:
        remotes = run(["git", "remote"], root).strip().splitlines()
    except RuntimeError as exc:
        print(f"ERROR: failed to read git remotes\n{exc}", file=sys.stderr)
        return 1
    if not remotes:
        print("ERROR: git remote is missing", file=sys.stderr)
        return 1

    text = readme.read_text(encoding="utf-8")

    for heading in REQUIRED_HEADINGS:
        if heading not in text:
            print(f"ERROR: README missing required heading: {heading}", file=sys.stderr)
            return 1

    if "唯一当前进度标准" not in text:
        print("ERROR: README must declare itself as the single current progress standard", file=sys.stderr)
        return 1

    if package_json.exists():
        try:
            version = json.loads(package_json.read_text(encoding="utf-8")).get("version")
        except json.JSONDecodeError as exc:
            print(f"ERROR: invalid package.json\n{exc}", file=sys.stderr)
            return 1
        if version and version not in text:
            print(
                f"ERROR: package.json version {version!r} not found in README.md",
                file=sys.stderr,
            )
            return 1

    for aux_name in ("SPEC.md", "PLAN.md"):
        aux_path = root / aux_name
        if not aux_path.exists():
            continue
        aux_text = aux_path.read_text(encoding="utf-8")
        if "STATUS.md" in aux_text:
            print(
                f"ERROR: {aux_name} still references STATUS.md instead of README.md",
                file=sys.stderr,
            )
            return 1

    status_output = run(["git", "status", "--short"], root)
    status_lines = [line for line in status_output.splitlines() if line.strip()]

    for line in status_lines:
        path = line[3:]
        normalized = path.rstrip("/")
        for blocked in BLOCKED_RESIDUE:
            blocked_normalized = blocked.rstrip("/")
            if normalized == blocked_normalized or normalized.startswith(f"{blocked_normalized}/"):
                print(
                    f"ERROR: runtime residue should not appear in git status: {path}",
                    file=sys.stderr,
                )
                return 1

    if args.expect_clean:
        if status_lines:
            print("ERROR: worktree is not clean", file=sys.stderr)
            print(status_output, file=sys.stderr)
            return 1
    else:
        if status_lines and not any(line[3:] == "README.md" for line in status_lines):
            print(
                "ERROR: working tree has changes but README.md was not updated in this round",
                file=sys.stderr,
            )
            return 1

    print("OK: README closeout guard passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
