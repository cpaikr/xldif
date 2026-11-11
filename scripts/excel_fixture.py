#!/usr/bin/env python3
"""Has Excel write the public fixtures under fixtures/excel/.

Usage:
  python -I scripts/excel_fixture.py

Every value is invented. Excel builds the workbook from nothing and saves it
twice: `excel-saved.xlsx` (the usual format) and `excel-strict.xlsx` (Strict
Open XML). They exist because a file-format fact counts as verified only once
it has been seen in a file Excel saved: `crates/xldif/tests/excel_saved.rs`
reads them and states what was seen.

The files are committed, so this script runs only to change them. It needs
Windows, Excel, and pywin32. Excel stamps a workbook with its author and the
folder it was saved in; the script removes the personal information, saves
through a neutral folder, and refuses to finish if the user's name, account,
or home folder appears anywhere in the files.
"""

from __future__ import annotations

import getpass
import os
import shutil
import sys
import tempfile
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "fixtures" / "excel"


def build(xl, saved: Path, strict: Path) -> list[str]:
    """Fills a new workbook and saves it in both formats. Returns notes on
    features this Excel could not write."""
    notes = []
    wb = xl.Workbooks.Add()
    while wb.Worksheets.Count > 1:
        wb.Worksheets(wb.Worksheets.Count).Delete()
    ws = wb.Worksheets(1)
    ws.Name = "Ledger"

    # Values of each kind, under a header row.
    ws.Range("A1:E1").Value = (("Account", "Opened", "Balance", "Rate", "Checked"),)
    rows = [
        ("Cash", 45292, 1200.5, 0.035, True),
        ("Receivables", 45323, -310.25, 0.0425, False),
        ("Inventory", 45352, 98000, 0.1, True),
    ]
    for r, row in enumerate(rows, start=2):
        ws.Range(f"A{r}:E{r}").Value = (row,)
    ws.Range("B2:B4").NumberFormat = "yyyy-mm-dd"
    ws.Range("C2:C5").NumberFormat = "#,##0.00"
    ws.Range("D2:D4").NumberFormat = "0.00%"
    ws.Range("A5").Value = "Total"
    ws.Range("C5").Formula = "=SUM(C2:C4)"
    # Text that needs escaping: a carriage return, and text that looks like
    # an escape.
    ws.Range("A7").Value = "line one\r\nline two"
    ws.Range("A8").Value = "_x000D_ as typed"
    ws.Range("A9").Value = "'0042"
    ws.Range("B9").Formula = "=1/0"
    ws.Range("C9").Formula = '=A2&" on hand"'

    # A formula filled down (Excel stores it as shared), an array formula,
    # and a dynamic array that spills.
    ws.Range("F1").Value = "Interest"
    ws.Range("F2").Formula = "=C2*D2"
    ws.Range("F2:F4").FillDown()
    ws.Range("G2:G4").FormulaArray = "=C2:C4*2"
    try:
        ws.Range("H2").Formula2 = "=SORT(C2:C4)"
    except Exception as e:  # noqa: BLE001 - older Excel has no dynamic arrays
        notes.append(f"dynamic array not written: {e}")

    # Formats: a bold header with a fill and borders, a whole row and a
    # whole column formatted, widths, heights, and a hidden row.
    header = ws.Range("A1:F1")
    header.Font.Bold = True
    header.Interior.Color = 0xD9E1F2
    header.Borders(9).LineStyle = 1  # bottom edge, continuous
    ws.Rows(5).Font.Italic = True
    ws.Columns(3).HorizontalAlignment = -4152  # right
    ws.Columns(1).ColumnWidth = 18
    ws.Rows(1).RowHeight = 24
    ws.Rows(8).Hidden = True

    # Workbook objects.
    ws.Range("A11:C11").Merge()
    ws.Range("A11").Value = "Merged note"
    ws.Range("C2").AddComment("Agreed to the bank letter")
    try:
        ws.Range("C3").AddCommentThreaded("Aged listing attached")
    except Exception as e:  # noqa: BLE001 - not in every Excel
        notes.append(f"threaded comment not written: {e}")
    ws.Range("E2:E4").Validation.Add(3, 1, 1, "TRUE,FALSE")  # a list
    rule = ws.Range("C2:C4").FormatConditions.Add(1, 6, "=0")  # cell value < 0
    rule.Interior.Color = 0xC7CEFF
    rule.Font.Bold = True
    ws.Hyperlinks.Add(ws.Range("A13"), "https://example.com/policy", "", "", "Policy")
    wb.Names.Add("Balances", "=Ledger!$C$2:$C$4")

    # Content xldif does not compare, which it must name in a warning.
    table = wb.Worksheets.Add(After=ws)
    table.Name = "Listing"
    table.Range("A1:B3").Value = (("Item", "Amount"), ("Paper", 12), ("Ink", 30))
    table.ListObjects.Add(1, table.Range("A1:B3"), None, 1).Name = "Items"
    chart = table.Shapes.AddChart2(201, 51).Chart  # a clustered column chart
    chart.SetSourceData(table.Range("A1:B3"))
    hidden = wb.Worksheets.Add(After=table)
    hidden.Name = "Workings"
    hidden.Range("A1").Value = 42
    hidden.Visible = 0  # hidden, not very hidden

    # Authors become "Author" on saving; the document properties go. (Not
    # "remove everything", which also deletes the comments.)
    wb.RemovePersonalInformation = True
    for what in (8, 4):  # document properties, personal information
        try:
            wb.RemoveDocumentInformation(what)
        except Exception as e:  # noqa: BLE001
            notes.append(f"document information {what} not removed: {e}")
    wb.SaveAs(str(saved), 51)
    wb.SaveAs(str(strict), 61)
    wb.Close(False)
    return notes


def personal_strings(excel_user: str) -> list[str]:
    """The names a file must not carry: the account, its home folder, the
    machine, and the name Excel signs comments with (whole and in parts)."""
    home = Path.home()
    found = {getpass.getuser(), home.name, str(home), os.environ.get("USERDOMAIN", ""), os.environ.get("COMPUTERNAME", "")}
    found |= {excel_user, *excel_user.replace(",", " ").split()}
    # "user" as an account name would match `userId` in every file.
    return sorted(s for s in found if len(s) >= 3 and s.lower() not in {"user", "users"})


def leaks(path: Path, needles: list[str]) -> list[str]:
    out = []
    with zipfile.ZipFile(path) as z:
        for name in z.namelist():
            data = z.read(name)
            for form in ("utf-8", "utf-16-le"):
                text = data.decode(form, errors="ignore").lower()
                out += [f"{path.name}:{name} holds {n!r}" for n in needles if n.lower() in text]
    return sorted(set(out))


def main() -> int:
    import win32com.client as win32

    # A neutral folder: Excel records the folder a workbook was saved in.
    work = Path(tempfile.mkdtemp(prefix="x", dir=os.environ.get("SystemDrive", "C:") + "\\"))
    saved, strict = work / "excel-saved.xlsx", work / "excel-strict.xlsx"
    xl = win32.DispatchEx("Excel.Application")
    try:
        xl.Visible = False
        xl.DisplayAlerts = False
        excel_user = str(xl.UserName)
        notes = build(xl, saved, strict)
    finally:
        xl.Quit()
    try:
        needles = personal_strings(excel_user)
        problems = leaks(saved, needles) + leaks(strict, needles)
        if problems:
            print("\n".join(problems))
            print("not written: the files would carry personal information")
            return 1
        OUT.mkdir(parents=True, exist_ok=True)
        for f in (saved, strict):
            shutil.copyfile(f, OUT / f.name)
            print(f"wrote {OUT / f.name} ({f.stat().st_size} bytes)")
        for n in notes:
            print(f"note: {n}")
    finally:
        shutil.rmtree(work, ignore_errors=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
