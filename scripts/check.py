#!/usr/bin/env python3
"""Repository checks: skill package rules, Rust format/lint/tests, and a CLI
smoke run of the built executable.

Usage: python scripts/check.py

Standard library only. Private samples under fixtures/private/ are only
counted; output never names their files or prints their contents.
"""

from __future__ import annotations

import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SKILLS = ROOT / "skills"
PRIVATE = ROOT / "fixtures" / "private"
ALLOWED_FIELDS = {"name", "description", "license", "compatibility", "metadata", "allowed-tools"}
EXEMPT_DIRS = {"agents", "evals"}

errors: list[str] = []


def fail(msg: str) -> None:
    errors.append(msg)
    print(f"FAIL {msg}")


def check_skill(skill: Path) -> None:
    entry = skill / "SKILL.md"
    if not entry.is_file():
        fail(f"{skill.name}: missing SKILL.md")
        return
    text = entry.read_text(encoding="utf-8")
    m = re.match(r"---\n(.*?)\n---\n", text, re.S)
    if not m:
        fail(f"{skill.name}: SKILL.md lacks YAML frontmatter")
        return
    fields = {}
    for line in m.group(1).splitlines():
        key, _, value = line.partition(":")
        fields[key.strip()] = value.strip()
    if set(fields) - ALLOWED_FIELDS:
        fail(f"{skill.name}: unsupported frontmatter fields {sorted(set(fields) - ALLOWED_FIELDS)}")
    if fields.get("name") != skill.name:
        fail(f"{skill.name}: frontmatter name {fields.get('name')!r} must match directory")
    if not fields.get("description"):
        fail(f"{skill.name}: missing description")
    body = text[m.end():]
    for link in re.findall(r"\]\(<?([^)>]+)>?\)", body):
        if re.match(r"[a-z]+://", link) or link.startswith("#"):
            continue
        target = (skill / link).resolve()
        if not target.is_relative_to(skill.resolve()):
            fail(f"{skill.name}: link escapes the skill directory: {link}")
        elif not target.exists():
            fail(f"{skill.name}: broken link {link}")
    for path in skill.rglob("*"):
        rel = path.relative_to(skill)
        if path.is_symlink():
            fail(f"{skill.name}: symlink {rel}")
        if path.is_file() and rel.parts[0] not in EXEMPT_DIRS and rel.name != "SKILL.md" and "__pycache__" not in rel.parts:
            if rel.as_posix() not in body:
                fail(f"{skill.name}: runtime file {rel.as_posix()} is not named in SKILL.md")
    print(f"ok   package {skill.name}")


def run_cargo() -> None:
    for args in (
        ["fmt", "--all", "--check"],
        ["clippy", "--workspace", "--all-targets", "--locked", "--", "-D", "warnings"],
        ["test", "--workspace", "--locked"],
    ):
        proc = subprocess.run(["cargo", *args], cwd=ROOT)
        if proc.returncode != 0:
            fail(f"cargo {args[0]}")
        else:
            print(f"ok   cargo {args[0]}")


def executable() -> Path:
    """The debug `xldif` in Cargo's target directory, honoring CARGO_TARGET_DIR."""
    proc = subprocess.run(["cargo", "metadata", "--format-version", "1", "--no-deps"], cwd=ROOT, capture_output=True)
    target = Path(json.loads(proc.stdout)["target_directory"]) if proc.returncode == 0 else ROOT / "target"
    return target / "debug" / ("xldif.exe" if sys.platform == "win32" else "xldif")


def cli_smoke() -> None:
    """The built executable honors the process contract outside the test harness."""
    exe = executable()
    if not exe.is_file():
        fail(f"{exe} is missing; cargo test builds it")
        return
    before = len(errors)
    help_run = subprocess.run([str(exe)], capture_output=True)
    if help_run.returncode != 0 or not help_run.stdout.startswith(b"Compare Excel workbooks"):
        fail("smoke: bare xldif does not print help")
    bad = subprocess.run([str(exe), "no-such-command"], capture_output=True)
    try:
        env = json.loads(bad.stdout.decode("utf-8"))
    except ValueError:
        env = {}
    if bad.returncode != 2 or env.get("error", {}).get("code") != "invalid_invocation":
        fail("smoke: invalid invocation is not a JSON failure with exit 2")
    if len(errors) == before:
        print("ok   cli smoke")
    if PRIVATE.is_dir():
        books = [p for p in PRIVATE.rglob("*") if p.suffix.lower() in {".xlsx", ".xlsm"}]
        # No command reads workbooks yet; the corpus smoke run arrives with `inspect`.
        print(f"ok   private corpus present: {len(books)} workbooks (not yet exercised)")


def main() -> int:
    if SKILLS.is_dir():
        for skill in sorted(p for p in SKILLS.iterdir() if p.is_dir()):
            check_skill(skill)
    run_cargo()
    cli_smoke()
    if errors:
        print(f"\n{len(errors)} check(s) failed")
        return 1
    print("\nall checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
