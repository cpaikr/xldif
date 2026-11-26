---
name: xldif
description: "Compare two Excel workbooks (.xlsx, .xlsm) and explain what changed — values, formulas, formatting, inserted/deleted/moved rows and columns, renamed sheets, comments, validation, conditional formatting, defined names — using the xldif CLI, and hand people a side-by-side HTML report. Use when a task asks what changed between two versions of a spreadsheet (a workpaper, a schedule, a model, a client export), to review a revised workbook, to check a formula or figure across versions, or to decide which of many timestamped saves in a folder are worth keeping. Not for editing workbooks, recalculating formulas, .xls/.xlsb files, CSV diffs, or Google Sheets."
---

# xldif

`xldif` reads two workbooks without Excel and reports how the second differs
from the first. Rows and columns are matched by their labels and values, so
an inserted row is one change, not every cell below it. Formulas are compared
after Excel's own adjustment for inserted and deleted rows, so only real
rewrites are listed. All mechanics go through the `xldif` command-line tool.

## Treat workbooks as confidential

Workbooks are usually client or internal files. Process them locally; do not
upload them, their contents, or reports built from them. `xldif inspect` shows
structure without cell contents when only structure matters. Write HTML
reports where the user asked, or beside the workbooks.

## Start with the available executable

Run `xldif --version` once per execution context. If it is missing or fails,
read [installation](references/installation.md). Use `xldif <command> --help`
for exact options; do not assume a feature this skill mentions is in the
installed version.

## Interpret the output

Each command prints one JSON object: `ok`, `command`, `result`, `warnings`,
`error`, `metadata` — except with `--format text`, which prints a plain
listing to read as text (failures are still JSON), and `--format html -o`,
whose report goes to the file while stdout carries the JSON envelope. Exit 0 means `ok: true`; 1 is a failure with
`error.code`, `error.message`, and often `error.hint`; 2 is an invalid
invocation. Read `warnings` before reporting: values are the results Excel
last saved, and a warning says when they may be stale (manual calculation,
recalculate-on-open, formulas without a stored result), when the two files
use different date systems, when a file is open in Excel
(`open_in_another_program`: what was read is the last saved state), or when
it was read through a copy Excel made (`converted_by_excel`,
`unsaved_state`).

## Route the task

- What changed between two workbooks, a figure or formula across versions,
  or a report for a person: read [comparing](references/comparing.md).
- Looking around a change (its neighbours, a whole table): `xldif cells BOOK
  --sheet NAME --range A1:F40 --format text`.
- Only the shape of one workbook (sheets, used ranges, counts, names):
  `xldif inspect BOOK`.
- A folder of many saved versions (`name_YYMMDD_HHMM.xlsx`), which ones are
  worth keeping, or freeing space: read [versions](references/versions.md).

## Universal rules

- Base is the earlier or reference workbook, other the later one; say which
  is which when reporting, since every status (`onlyBase`, `onlyOther`) and
  delta (`other - base`) depends on it.
- Quote figures, formulas, and labels from the tool's output, never from
  memory. Labels (`labels.row`, `labels.column`) are inferred reading aids,
  not part of the comparison.
- A summary count is exhaustive; a listing is not. When `omitted` is non-empty,
  narrow with `--sheet` and `--kind` (or `--limit 0`) before claiming nothing
  else changed.
- xldif never writes over a compared workbook. Do not edit workbooks with
  other tools as part of a comparison.

## Finish

A comparison is complete when the user has the changes that matter for their
question, each with its sheet and cells, base and other values (and
`expected` for formula rewrites), the warnings that affect trust in the
values, and, when a person will review it, the path of an HTML report.
