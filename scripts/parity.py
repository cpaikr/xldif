#!/usr/bin/env python3
"""Checks xldif's reader against openpyxl, an independent implementation.

Usage: python -I scripts/parity.py [--show] XLDIF_EXE BOOK...

For every worksheet of every book, the set of cells with content, each
cell's stored value, and each formula's text must agree. Known, intended
differences are skipped and counted, not hidden:

- shared-formula cells after the anchor (xldif expands them in M2);
- cells openpyxl reads as dates are compared as Excel serial numbers;
- openpyxl drops cells whose only content is an empty string;
- openpyxl drops values stored in merged cells other than the top-left one,
  which Excel keeps (hidden) and xldif reads;
- openpyxl leaves OOXML `_xHHHH_` escapes undecoded in strings, where Excel
  and xldif decode them; those strings are decoded before comparing.

Without --show, output names books by index and reports counts only, so the
private corpus never reaches a log. Exit status 1 when anything disagrees.
"""

from __future__ import annotations

import datetime as dt
import json
import subprocess
import sys
from collections import Counter
from pathlib import Path

import re

import openpyxl
from openpyxl.cell.cell import MergedCell
from openpyxl.utils.datetime import to_excel
from openpyxl.worksheet.formula import ArrayFormula, DataTableFormula


def xldif(exe: str, *args: str) -> dict:
    stage = args[0]
    proc = subprocess.run([exe, *args], capture_output=True)
    try:
        env = json.loads(proc.stdout.decode("utf-8"))
    except ValueError:
        raise RuntimeError(f"xldif {stage} exited {proc.returncode} without a JSON envelope") from None
    if not env["ok"]:
        raise RuntimeError(f"xldif {stage} failed with {env['error']['code']}")
    return env["result"]


def theirs_formula(value, data_type: str) -> str | None:
    """The formula text openpyxl read, by its cell type: a text cell that
    merely starts with "=" is not a formula."""
    if data_type != "f" or isinstance(value, DataTableFormula):
        return None
    if isinstance(value, ArrayFormula):
        return value.text
    return value


XSTRING = re.compile(r"_x([0-9A-Fa-f]{4})_")


def decode_xstring(text: str) -> str:
    return XSTRING.sub(lambda m: chr(int(m.group(1), 16)), text)


def same_value(ours: dict, theirs, data_type: str, epoch) -> bool:
    """Strings must match openpyxl's after decoding the `_xHHHH_` escapes
    it leaves in place, and error literals must be typed as errors."""
    kind = ours["type"]
    value = ours.get("value")
    if isinstance(theirs, (dt.datetime, dt.date, dt.time, dt.timedelta)):
        return kind == "number" and abs(to_excel(theirs, epoch) - value) < 1e-6
    if theirs is None:
        return kind == "empty" or (kind == "text" and value == "")
    if isinstance(theirs, bool):
        return kind == "bool" and value == theirs
    if isinstance(theirs, (int, float)):
        return kind == "number" and float(theirs) == value
    if isinstance(theirs, str):
        expected_kind = "error" if data_type == "e" else "text"
        return kind == expected_kind and value == decode_xstring(theirs)
    return False


def check_book(exe: str, path: str, label: str, show: bool, tally: Counter) -> None:
    info = xldif(exe, "inspect", path)
    try:
        formulas = openpyxl.load_workbook(path, data_only=False, read_only=False)
        values = openpyxl.load_workbook(path, data_only=True, read_only=False)
    except Exception as e:  # noqa: BLE001 - name the stage, keep the message private
        raise RuntimeError(f"openpyxl could not load the book ({type(e).__name__})") from e
    epoch = values.epoch
    for sheet in info["sheets"]:
        if sheet["kind"] != "worksheet":
            continue
        name = sheet["name"]
        ours = {c["cell"]: c for c in xldif(exe, "cells", path, "--sheet", name, "--limit", "0")["cells"]}
        f_ws, v_ws = formulas[name], values[name]
        theirs = {}
        for row in f_ws.iter_rows():
            for cell in row:
                if cell.value is not None:
                    cached = v_ws[cell.coordinate]
                    theirs[cell.coordinate] = (cell.value, cell.data_type, cached.value, cached.data_type)
        for coord in sorted(set(ours) | set(theirs)):
            mine = ours.get(coord)
            if coord not in theirs:
                if mine and mine["type"] == "text" and mine.get("value") == "" and "formula" not in mine:
                    tally["skipped: empty string cell"] += 1
                    continue
                if isinstance(f_ws[coord], MergedCell):
                    tally["skipped: value hidden under a merge"] += 1
                    continue
                if mine and mine.get("formulaKind") == "shared" and mine.get("formula") == "":
                    tally["skipped: shared follower with empty cached value"] += 1
                    continue
                report(tally, show, label, name, coord, "cell only in xldif", mine, None)
                continue
            raw, raw_type, cached, cached_type = theirs[coord]
            if mine is None:
                report(tally, show, label, name, coord, "cell only in openpyxl", None, raw)
                continue
            expected_formula = theirs_formula(raw, raw_type)
            if mine.get("formulaKind") == "shared" and mine.get("formula") == "":
                tally["skipped: shared formula follower"] += 1
            elif mine.get("formulaKind") == "dataTable":
                tally["skipped: data table formula"] += 1
            elif expected_formula is not None or "formula" in mine:
                got = "=" + mine["formula"] if "formula" in mine else None
                if got != expected_formula:
                    report(tally, show, label, name, coord, "formula differs", got, expected_formula)
                    continue
            if not same_value(mine, cached, cached_type, epoch):
                report(tally, show, label, name, coord, "value differs", mine, cached)
                continue
            tally["agree"] += 1


def report(tally: Counter, show: bool, label: str, sheet: str, coord: str, what: str, ours, theirs) -> None:
    tally[what] += 1
    if show:
        print(f"  {label} {sheet}!{coord}: {what}: xldif={ours!r} openpyxl={theirs!r}")


def main(argv: list[str]) -> int:
    show = "--show" in argv
    args = [a for a in argv if a != "--show"]
    if len(args) < 2:
        print(__doc__)
        return 2
    exe, books = str(Path(args[0]).resolve()), args[1:]
    tally: Counter = Counter()
    failed_books = 0
    for i, path in enumerate(books):
        label = path if show else f"book {i + 1}"
        try:
            check_book(exe, path, label, show, tally)
        except Exception as e:  # noqa: BLE001 - one unreadable book must not hide the rest
            failed_books += 1
            # Messages above name only the stage and error code, never content.
            detail = e if show or isinstance(e, RuntimeError) else "details hidden"
            print(f"  {label}: could not be compared ({type(e).__name__}: {detail})")
    bad = sum(n for k, n in tally.items() if k != "agree" and not k.startswith("skipped"))
    summary = ", ".join(f"{k} {n}" for k, n in sorted(tally.items()))
    print(f"parity over {len(books)} book(s): {summary or 'no cells'}")
    return 1 if bad or failed_books else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
