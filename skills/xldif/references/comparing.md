# Comparing workbooks

## Run the comparison

Start broad, then narrow:

1. `xldif compare BASE OTHER --format text` for an overview: the header line
   counts every change by kind, then each changed sheet lists its changes.
2. `xldif compare BASE OTHER --sheet NAME --kind value,formula` (JSON) to
   work with one sheet's figures. `--limit 0` lists everything; the default
   lists 500 changes and `omitted` counts the rest by sheet and kind.
3. `xldif cells OTHER --sheet NAME --range B10:F30 --format text` to read the
   surroundings of a change.
4. `xldif compare BASE OTHER --format html -o REPORT.html` when a person will
   review the result. The report shows every change regardless of `--limit`
   and never replaces a compared workbook; `--force` replaces another
   existing file.

`.xls`, `.xlsb`, and password-protected workbooks work where Excel is
installed (Windows): xldif reads a copy Excel saves as `.xlsx`, which takes
about fifteen seconds a file, and says so in a `converted_by_excel` warning.
Values are then as Excel calculated them on opening, and font and colour
differences against a file that was not converted may come from the
conversion. A password to open goes in the `XLDIF_PASSWORD` environment
variable; ask the user to set it rather than putting a password on a command
line.

A workbook that is open in Excel is read as last saved, with an
`open_in_another_program` warning. When the user wants what is on their
screen, edits not yet saved included, add `--unsaved`: xldif reads a copy
the running Excel makes, and the open workbook is not saved or changed. It
fails with `excel_failed` when Excel is busy (a cell being edited, a dialog
open) or the workbook was opened from OneDrive or SharePoint; follow the
`hint`.

`--tolerance 0.005` ignores rounding noise between numbers.
`--align position` compares A1 with A1 when the user explicitly wants a
cell-by-cell comparison without row matching.

## Read a change

Each change has a stable `id` (`s0:value:D14:D15` = sheet 0, base D14, other
D15), a `kind`, a `status`, `base` and `other` locations, and often `labels`
and `properties`. A value change's locations carry the stored `value` and,
when the number format changes how it reads, `display` (`1,234.50`, `(500)`);
date-formatted numbers also carry `date` (ISO 8601). Quote `display` or
`date` to a person and `value` for arithmetic. When the workbooks use
different date systems (a `date_system_mismatch` warning), the same date has
two serial numbers: date cells are then compared by `date`, and their
`delta` is in days.

| kind | meaning |
|---|---|
| `value` | a stored value changed on a cell present in both (`delta` for numbers, also numbers written as text) |
| `formula` | a formula was rewritten, added (`onlyOther`), or replaced by a typed value (`onlyBase`); `expected` is the base formula as Excel would have adjusted it |
| `format` | number format, font, fill, borders, alignment, protection, or named style; `properties` lists each one; neighbouring cells with the same change fold into a range |
| `row`, `column` | inserted (`onlyOther`), deleted (`onlyBase`), `moved`, or `changed` (height/width, hidden, row/column style) |
| `merge`, `comment`, `validation`, `conditionalFormat`, `hyperlink` | objects, carried through the alignment before comparing |
| `sheet` | visibility; added, removed, renamed, and moved sheets are sheet statuses |
| `name`, `theme` | workbook-level, under `objects` |
| `externalLink` | workbook-level, under `objects`: a file another workbook is linked to on one side only (`onlyBase`, `onlyOther`), or a link that names another file on each side (`changed`, with both targets) |

`summary.formulasAdjusted` counts formulas whose text differs only because
Excel adjusted them for inserted or deleted rows; they are not listed and
are not changes.

## Explain what matters

- Lead with the figures the user cares about: totals, the line items that
  explain them, then formulas that were rewritten or replaced by typed
  values (`formula` `onlyBase`): those break a model silently.
- A formula whose text did not change but now reads a different cell, or a
  total that no longer covers an inserted row, is listed with `expected`;
  explain it as "Excel would have adjusted this to …, but it still reads …".
- A formula names another workbook by number (`[1]Prices!A1`), and the
  number is only a position in that workbook's list of links: xldif
  compares the files, so a renumbered link is not a change. A link
  pointed at another file is one `externalLink` change that names both
  files (`externalLink:1:1`); the formulas through it are not listed, and
  every one of them reads the new file. Say so when reporting it.
  `xldif inspect BOOK` lists each workbook's links in order. A file name
  in a formula's `expected` (`[FY2024.xlsx]Sheet1!A1`) means the other
  workbook no longer links to that file at all.
- Moved rows are not changes in content; mention them only when order
  matters.
- Format-only changes are rarely material; summarize them unless asked.

## When the result looks wrong

- Many value changes on one sheet together with `alignment.fellBack: true`
  or an `alignment_fallback` warning: part of the sheet was too large to
  align fully by content, and rows there may be paired with the wrong ones.
  The warning alone, without a run of implausible value changes, usually
  means the pairing is fine. Narrow the question or say so.
- A sheet reported as removed and another added with similar content:
  sheets pair by name, then by content; if the content changed too much,
  they are reported separately. Compare the two sheets' cells directly.
- Rows whose labels repeat (a currency or entity column) or tables without
  labels align by their values; heavily edited rows can then show as a
  deletion plus an insertion. Check with `cells` before concluding a row
  was deleted.
- Swapping BASE and OTHER mirrors the rows, columns, and values reported.
  Formula changes do not mirror: a row inserted inside a total is a change,
  the same row deleted is not.
- Warnings about stale values mean the stored results may not match the
  formulas; say that figures come from the last save.
- A `not_compared` warning names content xldif does not read in that
  workbook (charts, tables, pivot tables, protection, newer conditional
  formats and validations, links to something other than a workbook,
  macros).
  "No changes" does not cover it; say what was left out.
