#!/usr/bin/env python3
"""Checks the cell formats xldif resolves against Excel's own.

Usage:
  python -I scripts/format_parity.py [--show] FORMATS_EXE BOOK...

FORMATS_EXE is the `formats` example (`cargo build -p xldif --example
formats`), which prints one written cell for each distinct format of a
workbook. Excel opens each book read-only in its own hidden process and is
asked for the same cell's font, fill, borders, alignment, and protection.

Counts are printed by property. With --show, each disagreement is printed
too: use it only on workbooks with invented content. Exit status 1 when
anything disagrees.

What is not checked, and counted as skipped:
- colours xldif holds symbolically (theme and indexed colours): only an
  explicit RGB colour can be compared with the colour Excel reports;
- an edge xldif reads as having no border, or a lighter one than Excel
  reports: two cells share an edge, and Excel reports the heavier of their
  two borders for both (seen on the private corpus: a thin border beside a
  neighbour's medium one);
- cells inside a merge, whose alignment Excel reports for the merged area;
- fill patterns other than none and solid, and number formats, which
  `display_parity.py` checks through the text they produce.

The cell's own format is compared (`Range.Font`, not `DisplayFormat`), so
conditional formatting and table styles, which xldif compares as rules or
not at all, do not enter.
"""

from __future__ import annotations

import json
import subprocess
import sys
from collections import Counter
from pathlib import Path

NONE = -4142  # xlNone, xlLineStyleNone, xlUnderlineStyleNone
UNDERLINE = {"none": NONE, "single": 2, "double": -4119, "singleAccounting": 4, "doubleAccounting": 5}
HORIZONTAL = {
    "none": 1,
    "general": 1,
    "left": -4131,
    "center": -4108,
    "right": -4152,
    "fill": 5,
    "justify": -4130,
    "centerContinuous": 7,
    "distributed": -4117,
}
VERTICAL = {"none": -4107, "bottom": -4107, "top": -4160, "center": -4108, "justify": -4130, "distributed": -4117}
# OOXML border style -> (Excel line style, weight)
BORDER = {
    "hair": (1, 1),
    "thin": (1, 2),
    "medium": (1, -4138),
    "thick": (1, 4),
    "double": (-4119, 4),
    "dotted": (-4118, 2),
    "dashed": (-4115, 2),
    "dashDot": (4, 2),
    "dashDotDot": (5, 2),
    "mediumDashed": (-4115, -4138),
    "mediumDashDot": (4, -4138),
    "mediumDashDotDot": (5, -4138),
    "slantDashDot": (13, -4138),
}
EDGES = {"border.left": 7, "border.top": 8, "border.bottom": 9, "border.right": 10}


def rgb(color: str) -> int | None:
    """`rgb:AARRGGBB` as the integer Excel reports (blue high), else None."""
    if not color.startswith("rgb:") or len(color) != 12:
        return None
    r, g, b = (int(color[i : i + 2], 16) for i in (6, 8, 10))
    return r | g << 8 | b << 16


def flag(text: str) -> bool:
    return text == "yes"


def heavier(weight, than) -> bool:
    """Whether one Excel border weight is heavier than another."""
    order = {1: 0, 2: 1, -4138: 2, 4: 3}  # hairline, thin, medium, thick
    return order.get(weight, -1) > order.get(than, 9)


def compare(cell, ours: dict) -> list[tuple[str, str, object, object]]:
    """`(property, outcome, ours, excel)` for each property, outcome one of
    agree, differ, skipped."""
    out = []

    def check(name: str, expected, actual, comparable: bool = True) -> None:
        if not comparable or expected is None:
            out.append((name, "skipped", ours.get(name), actual))
        else:
            out.append((name, "agree" if expected == actual else "differ", ours.get(name), actual))

    font = cell.Font
    check("font.name", ours["font.name"], font.Name)
    check("font.size", float(ours["font.size"] or 0), float(font.Size or 0), bool(ours["font.size"]))
    check("font.bold", flag(ours["font.bold"]), bool(font.Bold))
    check("font.italic", flag(ours["font.italic"]), bool(font.Italic))
    check("font.underline", UNDERLINE.get(ours["font.underline"]), font.Underline)
    check("font.strike", flag(ours["font.strike"]), bool(font.Strikethrough))
    check("font.vertAlign", ours["font.vertAlign"],
          "superscript" if font.Superscript else "subscript" if font.Subscript else "none")
    check("font.color", rgb(ours["font.color"]), int(font.Color or 0))
    interior = cell.Interior
    pattern = ours["fill.pattern"]
    check("fill.pattern", {"none": NONE, "solid": 1}.get(pattern), interior.Pattern)
    check("fill.fg", rgb(ours["fill.fg"]), int(interior.Color or 0), pattern == "solid")
    for name, edge in EDGES.items():
        style = ours[name].split(" ")[0]
        border = cell.Borders(edge)
        theirs = (border.LineStyle, border.Weight)
        expected = BORDER.get(style)
        # A heavier line than this cell's own is the neighbour's.
        neighbours = expected is not None and theirs != expected and heavier(theirs[1], expected[1])
        check(name, expected, theirs, style != "none" and not neighbours)
    check("alignment.horizontal", HORIZONTAL.get(ours["alignment.horizontal"]), cell.HorizontalAlignment)
    check("alignment.vertical", VERTICAL.get(ours["alignment.vertical"]), cell.VerticalAlignment)
    check("alignment.wrap", flag(ours["alignment.wrap"]), bool(cell.WrapText))
    check("protection.locked", flag(ours["protection.locked"]), bool(cell.Locked))
    check("protection.hidden", flag(ours["protection.hidden"]), bool(cell.FormulaHidden))
    return out


def main() -> int:
    args = sys.argv[1:]
    show = "--show" in args
    args = [a for a in args if a != "--show"]
    if len(args) < 2:
        print(__doc__)
        return 2
    exe, books = str(Path(args[0]).resolve()), [Path(a) for a in args[1:]]
    import win32com.client as win32

    counts: Counter = Counter()
    cells = 0
    excel = win32.DispatchEx("Excel.Application")
    try:
        excel.Visible = False
        excel.DisplayAlerts = False
        excel.AutomationSecurity = 3  # macros stay disabled
        for book in books:
            run = subprocess.run([exe, str(book)], capture_output=True, text=True, encoding="utf-8", check=True)
            samples = [json.loads(line) for line in run.stdout.splitlines()]
            wb = excel.Workbooks.Open(str(book.resolve()), UpdateLinks=0, ReadOnly=True)
            try:
                sheets = {}
                for sample in samples:
                    name = sample["sheet"]
                    if name not in sheets:
                        sheets[name] = wb.Worksheets(name)
                    cell = sheets[name].Range(sample["cell"])
                    if cell.MergeCells:
                        counts[("cell", "skipped")] += 1
                        continue
                    cells += 1
                    for prop, outcome, ours, theirs in compare(cell, sample["format"]):
                        counts[(prop, outcome)] += 1
                        if outcome == "differ" and show:
                            print(f"{book.name} {name}!{sample['cell']} {prop}: xldif {ours!r}, Excel {theirs!r}")
            finally:
                wb.Close(SaveChanges=False)
    finally:
        excel.Quit()
    props = sorted({p for p, _ in counts})
    differ = sum(n for (_, outcome), n in counts.items() if outcome == "differ")
    print(f"format parity over {len(books)} book(s), {cells} cell(s): "
          f"agree {sum(n for (_, o), n in counts.items() if o == 'agree')}, differ {differ}, "
          f"skipped {sum(n for (_, o), n in counts.items() if o == 'skipped')}")
    for prop in props:
        d, s = counts[(prop, "differ")], counts[(prop, "skipped")]
        if d or s:
            print(f"  {prop}: agree {counts[(prop, 'agree')]}, differ {d}, skipped {s}")
    return 1 if differ else 0


if __name__ == "__main__":
    sys.exit(main())
