#!/usr/bin/env python3
"""Checks xldif's display text against Excel's own (`Range.Text`).

Usage:
  python -I scripts/display_parity.py --probe OUT.xlsx SHOWN_EXE
      writes a synthetic workbook of number formats and values, then checks it
      (mismatches are printed in full: the content is invented);
  python -I scripts/display_parity.py SHOWN_EXE BOOK...
      checks existing books and prints counts only, so private workbooks
      never reach a log.

SHOWN_EXE is the `shown` example (`cargo build -p xldif --example shown`).
Needs Windows, Excel, and pywin32; openpyxl for --probe. Excel opens each
book read-only in its own process; columns are widened in memory (never
saved) so text is not cut to `####`.

Cells are skipped, and counted, when Excel's text is all `#` (too narrow
even when widened), or when the format is a built-in whose meaning depends
on the locale and Excel reports another code for it. Text that matches once
spaces are removed counts as matching: xldif drops `*` fill and trims edge
padding by design. Exit status 1 when anything else disagrees.
"""

from __future__ import annotations

import json
import re
import subprocess
import sys
from collections import Counter
from pathlib import Path

# Built-in codes (as xldif names them) that Excel shows per the system locale;
# `builtin:<id>` codes are locale-dependent too.
LOCALE_BUILTINS = {
    "mm-dd-yy",
    "d-mmm-yy",
    "d-mmm",
    "mmm-yy",
    "h:mm AM/PM",
    "h:mm:ss AM/PM",
    "h:mm",
    "h:mm:ss",
    "m/d/yy h:mm",
}

PROBE = [
    ("General", [0, 1, -1, 1234.5, 0.1 + 0.2, 1 / 3, 123456789012, 12345678901, 0.00001234567, 0.0000123, 1e-10, -0.5, 99999999999.5]),
    ("0", [2.5, -2.5, -0.4, 0, 1234567]),
    ("0.00", [2.675, -0.004, 1.005, 0]),
    ("#,##0", [1234567, -1234567, 0, 999.5]),
    ("#,##0.00", [-1234.5, 0.005]),
    ("#,###", [0, 1234]),
    ("0%", [0.5, -0.125]),
    ("0.0%", [0.1234]),
    ("#,##0,", [1234567, 499]),
    ("0.0,,", [12345678]),
    ("000-00-0000", [123456789]),
    ("#.##", [1, 1.5]),
    (".00", [1.5, 0.25]),
    ("00000", [42]),
    ("#,##0;(#,##0)", [-1234, 1234, 0]),
    ('0;-0;"zero"', [0, -3, 3]),
    ('"Total "0', [7, -7]),
    ('"fixed"', [5, -5]),
    ('_-* #,##0_-;-* #,##0_-;_-* "-"_-;_-@_-', [-1234, 0, 1234]),
    ("_(* #,##0.00_);_(* (#,##0.00);_(* \"-\"??_);_(@_)", [-1234.5, 0, 1234.5]),
    ("#,##0_);[Red]\\(#,##0\\)", [-1234, 1234]),
    ("[Red]0.00", [1.23456, -1.23456]),
    ("[$€-407] #,##0.00", [5]),
    ("[$-409]mmmm d, yyyy", [45292]),
    ("0.00E+00", [12345, 0.000123, 0, -12345]),
    ("##0.0E+0", [12345, 0.00012345, 1234567]),
    ("0.0E+00", [9.99]),
    ("# ?/?", [1.5, 0.25, 1, 0]),
    ("# ??/??", [2.14159, 0.5]),
    ("?/8", [0.375, 1.7]),
    ("# ?/?", [0.3, 0.7, 0.65, 2.999]),
    ("# ???/???", [3.14159265]),
    ("?/?", [0.33, 0.95, 1.25]),
    ("yyyy-mm-dd", [45292, 60, 61, 1, 45292.75]),
    ("m/d/yy", [45292]),
    ("d-mmm-yy", [45351]),
    ("dddd, mmmm d, yyyy", [45292]),
    ("mmmmm", [45292]),
    ("h:mm AM/PM", [0.75, 0, 0.5]),
    ("h:mm am/pm", [0.75]),
    ("h:mm a/p", [0.75]),
    ("hh:mm:ss", [0.5, 0.999999]),
    ("[h]:mm", [1.5]),
    ("[mm]:ss", [0.05]),
    ("mm:ss.0", [0.000011574]),
    ("h:mm", [0.999999]),
    ("ddd", [1, 45292]),
    ('yyyy"년" m"월" d"일"', [45292]),
    ("yy.mm.dd", [45292]),
    ('[>=1000]#,##0,"K";0', [12345, 12, -5]),
    ("[<0]\"neg\";\"pos\"", [-1, 1, 0]),
    ("[<=100]0;0", [-5, 5, 500]),
    ("[>100]0;0", [-5, 500]),
    ("[=-5]0;0", [-5, -6]),
    ("[<0]0;0", [-5, 5]),
    ("[>=0]0;0", [-5]),
    ("[<0]0;[>0]0;0", [-5, 0, 5]),
    ("[>0]0;[<0]0", [-5, 0]),
    ("[<=-1]0;[>=1]0;0", [-5, 0.5, -0.5]),
    ("[>100]0;[>50]0", [-5, 75]),
    ('0;"x"', [-5]),
    ('0;"x";"z"', [0]),
    ('"a"0"b";"c"', [-5]),
    ("0.00;-0.00;0;\"Note: \"@", ["x"]),
    ("@", [1.5, "text"]),
    ('"Name: "@', ["Ann"]),
    ("0;;", [5, -5, 0]),
    ("???.???", [1.5, 12.25]),
    ("0;[<0]0", [5, -5]),
    ("0.00;[<-10]0;0", [5, -5, -20]),
    ("#", [-0.4, 0]),
    ('"0"', [-5]),
    ('0" in 2024"', [-0.4]),
    ('0 "of 10"', [-0.2]),
    ("?/8", [-0.01, -0.5]),
    ("?/?", [12.5, 1.25]),
    ("??/??", [100.5]),
    ("hh:mm:ss.000", [0.5000014]),
    ("h:mm", [0.99999999]),
    ("0", [7]),
]


def write_probe(path: Path) -> None:
    import openpyxl

    wb = openpyxl.Workbook()
    ws = wb.active
    ws.title = "Probe"
    r = 1
    for code, values in PROBE:
        for v in values:
            ws.cell(r, 1, code)
            c = ws.cell(r, 2, v)
            c.number_format = code
            r += 1
    ws.column_dimensions["A"].width = 60
    ws.column_dimensions["B"].width = 60
    wb.save(path)


def shown(exe: str, book: Path) -> list[dict]:
    out = subprocess.run([exe, str(book)], capture_output=True, text=True, encoding="utf-8", check=True)
    return [json.loads(line) for line in out.stdout.splitlines()]


def mask(text: str) -> str:
    """Text reduced to its shape: digits to d, letters to a."""
    return re.sub(r"\d", "d", re.sub(r"[^\W\d_]", "a", text))


def mask_code(code: str) -> str:
    """A format code with its quoted text and escaped characters hidden."""
    return re.sub(r'"[^"]*"', '"…"', re.sub(r"\\.", "\\\\…", code))


def check(excel, exe: str, book: Path, show: bool, shape: bool = False) -> Counter:
    counts: Counter = Counter()
    ours = shown(exe, book)
    sep = str(excel.International[16])  # xlDateSeparator (17, 1-based)
    wb = excel.Workbooks.Open(str(book.resolve()), UpdateLinks=0, ReadOnly=True)
    try:
        sheets = {}
        for row in ours:
            name = row["sheet"]
            if name not in sheets:
                ws = wb.Worksheets(name)
                try:
                    ws.UsedRange.EntireColumn.ColumnWidth = 120
                except Exception:  # protected sheets keep their widths
                    pass
                sheets[name] = ws
            cell = sheets[name].Range(row["cell"])
            theirs = str(cell.Text)
            kind = "date" if row["date"] else ("general" if row["format"] == "General" else "number")
            if theirs == row["text"]:
                counts[f"match {kind}"] += 1
            elif theirs and set(theirs) == {"#"}:
                counts["skip narrow"] += 1
                if show:
                    print(f"  narrow: {row['format']!r} {cell.Value!r} xldif {row['text']!r}")
            elif (row["format"] in LOCALE_BUILTINS or row["format"].startswith("builtin:")) and str(
                cell.NumberFormat
            ) != row["format"]:
                counts["skip locale"] += 1
            elif row["date"] and theirs == row["text"].replace("/", sep):
                counts["skip locale date separator"] += 1
            elif theirs.replace(" ", "") == row["text"].replace(" ", ""):
                counts[f"match {kind} (spacing)"] += 1
            else:
                counts[f"MISMATCH {kind}"] += 1
                if show:
                    print(f"  {row['format']!r:45} {cell.Value!r:>22}  xldif {row['text']!r}  excel {theirs!r}")
                elif shape:
                    print(f"  shape: {mask_code(row['format'])!r}  xldif {mask(row['text'])!r}  excel {mask(theirs)!r}")
    finally:
        wb.Close(SaveChanges=False)
    return counts


def main(argv: list[str]) -> int:
    import win32com.client

    shape = argv[:1] == ["--shape"]
    if shape:
        argv = argv[1:]
    probe = None
    if argv[:1] == ["--probe"]:
        probe = Path(argv[1]).resolve()
        argv = argv[2:]
        write_probe(probe)
    exe, books = argv[0], ([probe] if probe else [Path(b) for b in argv[1:]])
    excel = win32com.client.DispatchEx("Excel.Application")
    excel.Visible = False
    excel.DisplayAlerts = False
    excel.AutomationSecurity = 3  # msoAutomationSecurityForceDisable: never run macros
    total: Counter = Counter()
    try:
        for i, book in enumerate(books):
            counts = check(excel, exe, book, show=probe is not None, shape=shape)
            total += counts
            if probe is None:
                bad = sum(v for k, v in counts.items() if k.startswith("MISMATCH"))
                print(f"book {i}: {sum(counts.values())} cells, {bad} mismatched")
    finally:
        excel.Quit()
    for k in sorted(total):
        print(f"{k}: {total[k]}")
    return 1 if any(k.startswith("MISMATCH") for k in total) else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
