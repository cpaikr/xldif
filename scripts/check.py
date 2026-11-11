#!/usr/bin/env python3
"""Repository checks: skill package rules, Rust format/lint/tests, and a CLI
smoke run of the built executable.

Usage: python scripts/check.py [--excel]

With Excel, pywin32, and Windows, display text is also checked against
Excel on a synthetic probe; --excel adds the private corpus (slower).

Standard library only, except that the reader parity check runs
scripts/parity.py, which needs openpyxl. Output never names private
workbooks or prints their contents.
"""

from __future__ import annotations

import json
import os
import re
import shutil
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


def excel_installed() -> bool:
    """Whether Excel's automation class is registered (Windows only)."""
    if sys.platform != "win32":
        return False
    query = ["reg", "query", r"HKCR\Excel.Application"]
    return subprocess.run(query, capture_output=True).returncode == 0


def run_cargo() -> None:
    # The tests that drive Excel (conversion, open workbooks) run only where
    # it is installed: crates/xldif-cli/tests/excel.rs.
    env = {**os.environ, "XLDIF_TEST_EXCEL": "1"} if excel_installed() else None
    for args in (
        ["fmt", "--all", "--check"],
        ["clippy", "--workspace", "--all-targets", "--locked", "--", "-D", "warnings"],
        ["test", "--workspace", "--locked"],
    ):
        proc = subprocess.run(["cargo", *args], cwd=ROOT, env=env)
        if proc.returncode != 0:
            fail(f"cargo {args[0]}")
        else:
            print(f"ok   cargo {args[0]}")


def target_dir() -> Path:
    """Cargo's target directory, honoring CARGO_TARGET_DIR."""
    proc = subprocess.run(["cargo", "metadata", "--format-version", "1", "--no-deps"], cwd=ROOT, capture_output=True)
    return Path(json.loads(proc.stdout)["target_directory"]) if proc.returncode == 0 else ROOT / "target"


TARGET = target_dir()
EXE = TARGET / "debug" / ("xldif.exe" if sys.platform == "win32" else "xldif")


def cli_smoke() -> None:
    """The built executable honors the process contract outside the test harness."""
    exe = EXE
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


def private_books() -> list[Path]:
    if not PRIVATE.is_dir():
        return []
    return sorted(p for p in PRIVATE.rglob("*") if p.suffix.lower() in {".xlsx", ".xlsm"} and not p.name.startswith("~$"))


def parity() -> None:
    """The reader agrees with openpyxl on the public fixtures and the private
    corpus. The private run prints counts only (see scripts/parity.py)."""
    have_openpyxl = subprocess.run([sys.executable, "-I", "-c", "import openpyxl"], capture_output=True).returncode == 0
    if not have_openpyxl:
        fail("parity: openpyxl is not installed (pip install openpyxl)")
        return
    # A directory of xldif's own, emptied first, so no stale or foreign
    # workbook from a shared target directory joins the run.
    fixtures = TARGET / "xldif-parity-fixtures"
    shutil.rmtree(fixtures, ignore_errors=True)
    proc = subprocess.run(
        ["cargo", "run", "-q", "--locked", "-p", "xldif-fixtures", "--example", "write_fixtures", "--", str(fixtures)],
        cwd=ROOT,
    )
    if proc.returncode != 0:
        fail("parity: writing the fixtures failed")
        return
    script = str(ROOT / "scripts" / "parity.py")
    public = sorted(str(p) for p in fixtures.glob("*.xlsx"))
    # Public fixtures may show details; the private corpus reports counts only.
    runs = [("public fixtures", ["--show"], public)]
    private = [str(p) for p in private_books()]
    if private:
        runs.append((f"private corpus ({len(private)} workbooks)", [], private))
    for label, flags, books in runs:
        proc = subprocess.run([sys.executable, "-I", script, *flags, str(EXE), *books])
        if proc.returncode != 0:
            fail(f"parity: {label}")
        else:
            print(f"ok   parity {label}")


def compare_smoke() -> None:
    """On the private corpus: every workbook compared with itself shows no
    changes, and each pair of consecutive versions in a series compares
    without error. Swapped pairs should mirror each other; mismatches are
    counted for information (ties in ambiguous sheets may legitimately
    break differently). Prints counts only."""
    books = private_books()
    if not books:
        return

    def run(a: Path, b: Path) -> dict | None:
        proc = subprocess.run([str(EXE), "compare", str(a), str(b), "--limit", "1"], capture_output=True)
        try:
            env = json.loads(proc.stdout.decode("utf-8"))
        except ValueError:
            return None
        return env["result"] if env.get("ok") else None

    before = len(errors)
    self_changes = sum(1 for b in books if (r := run(b, b)) is None or any(r["summary"].values()))
    if self_changes:
        fail(f"compare smoke: {self_changes} workbook(s) differ from themselves or failed")
    by_series: dict[Path, list[Path]] = {}
    for b in books:
        by_series.setdefault(b.parent, []).append(b)
    pairs = [(s[i], s[i + 1]) for s in by_series.values() for i in range(len(s) - 1)]
    failed = asymmetric = 0
    mirror = {
        "rowsOnlyBase": "rowsOnlyOther",
        "columnsOnlyBase": "columnsOnlyOther",
        "rowsMoved": "rowsMoved",
        "columnsMoved": "columnsMoved",
        "value": "value",
    }
    for a, b in pairs:
        ab, ba = run(a, b), run(b, a)
        if ab is None or ba is None:
            failed += 1
        elif any(ab["summary"][k] != ba["summary"][v] for k, v in mirror.items()):
            asymmetric += 1
    # Every version pair also renders as an HTML report (written to the
    # target directory, never printed).
    reports = TARGET / "xldif-corpus-reports"
    shutil.rmtree(reports, ignore_errors=True)
    reports.mkdir(parents=True)
    html_failed = 0
    for k, (a, b) in enumerate(pairs):
        out = reports / f"pair-{k}.html"
        proc = subprocess.run([str(EXE), "compare", str(a), str(b), "--format", "html", "-o", str(out)], capture_output=True)
        if proc.returncode != 0 or not out.is_file() or b"<script src" in out.read_bytes():
            html_failed += 1
    shutil.rmtree(reports, ignore_errors=True)
    if html_failed:
        fail(f"compare smoke: {html_failed} of {len(pairs)} HTML report(s) failed")
    if failed:
        fail(f"compare smoke: {failed} of {len(pairs)} version pair(s) failed to compare")
    if len(errors) == before:
        print(f"ok   compare smoke ({len(books)} self-compares, {len(pairs)} version pairs, "
              f"{asymmetric} not mirrored when swapped)")


def display_parity(corpus: bool) -> None:
    """Display text agrees with Excel's own on a synthetic probe of number
    formats, and with --excel on the private corpus (counts only). Needs
    Windows, Excel, and pywin32; skipped with a note when they are absent."""
    probe = "import win32com.client as w; e = w.DispatchEx('Excel.Application'); e.Quit()"
    if sys.platform != "win32" or subprocess.run([sys.executable, "-I", "-c", probe], capture_output=True).returncode:
        print("skip display parity: needs Windows, Excel, and pywin32")
        return
    proc = subprocess.run(["cargo", "build", "-q", "--locked", "-p", "xldif", "--example", "shown"], cwd=ROOT)
    if proc.returncode != 0:
        fail("display parity: building the shown example failed")
        return
    shown = str(TARGET / "debug" / "examples" / "shown.exe")
    script = str(ROOT / "scripts" / "display_parity.py")
    runs = [("probe", ["--probe", str(TARGET / "xldif-display-probe.xlsx"), shown])]
    private = [str(p) for p in private_books()]
    if corpus and private:
        runs.append((f"private corpus ({len(private)} workbooks)", [shown, *private]))
    for label, args in runs:
        proc = subprocess.run([sys.executable, "-I", script, *args], capture_output=True, text=True, encoding="utf-8")
        if proc.returncode != 0:
            # The probe is synthetic, so its details are safe to print; the
            # corpus stays silent.
            if label == "probe":
                print(proc.stdout)
                print(proc.stderr, file=sys.stderr)
            fail(f"display parity: {label}")
        else:
            print(f"ok   display parity {label}")


def main() -> int:
    if SKILLS.is_dir():
        for skill in sorted(p for p in SKILLS.iterdir() if p.is_dir()):
            check_skill(skill)
    run_cargo()
    cli_smoke()
    parity()
    compare_smoke()
    display_parity(corpus="--excel" in sys.argv[1:])
    if errors:
        print(f"\n{len(errors)} check(s) failed")
        return 1
    print("\nall checks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
