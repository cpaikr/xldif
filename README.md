# xldif

`xldif` compares two Excel workbooks (`.xlsx`, `.xlsm`) and reports what
changed: values, formulas, formatting, and workbook objects. Rows and columns
are matched by content, so an inserted row is one change, not every cell
below it. Agents read its JSON; people read its side-by-side HTML report.
`.xlsx` and `.xlsm` need no Excel. On Windows with Excel installed, `.xls`,
`.xlsb`, and password-protected workbooks are read through a copy Excel
saves, and `--unsaved` reads a workbook as Excel has it open.

**Status:** milestones M1–M5 done; no release is published yet. `compare`
reports value, formula, and format changes; inserted, deleted, and moved
rows, columns, and sheets; row and column layout; and merges, comments,
data validation, conditional formatting, hyperlinks, sheet visibility,
defined names, the theme, and the files external links name.
`--format html -o report.html` writes a
self-contained side-by-side report for people, and the
[`xldif` agent skill](skills/xldif/SKILL.md) teaches agents to drive the
CLI. See [PLAN.md](PLAN.md).

## Commands

- `xldif inspect BOOK` shows sheets (kind, visibility, used range, cell and
  formula counts), defined names, calculation settings, the date system, and
  the files external links name. It prints no cell contents.
- `xldif compare BASE OTHER [--sheet NAME] [--kind value,formula,format,...]
  [--limit N] [--all] [--tolerance X] [--align content|position]` reports
  what changed. Rows and columns pair by their labels and values, so an
  inserted row is one `row` change; each change has a stable id, base and
  other locations, a numeric delta (also for numbers stored as text), and
  inferred row and column labels. Formulas are compared after Excel's own
  adjustment for inserted and deleted rows, so `=D30` becoming `=D31` under
  an inserted row is not a change, while a rewrite shows both formulas and
  the one Excel would have kept (`expected`). A reference to another
  workbook (`[1]Prices!A1`) is compared by the file its number names.
  `--format html -o FILE`
  writes the side-by-side report instead (it never replaces a compared
  workbook, and replaces another file only with `--force`).
- `xldif cells BOOK --sheet NAME [--range A1:F40] [--limit N]` lists cells
  with a value or formula: address, type, stored value, formula text, the
  text the cell shows when its number format changes it (`display`), and
  date-formatted numbers as ISO dates (`date`). Value changes in `compare`
  carry the same two fields.

All accept `--format text`. Run `xldif <command> --help` for details.

Excel, where installed (Windows), is used for two things, each reported in a
warning:

- A `.xls`, `.xlsb`, or password-protected workbook is read through a copy
  that a hidden Excel saves as `.xlsx` (about fifteen seconds a file). Set
  `XLDIF_PASSWORD` for a password to open.
- A workbook that is open in Excel is read as last saved. `--unsaved` reads
  what Excel holds now instead, unsaved edits included; the open workbook is
  not saved, closed, or changed.

| Path | Purpose |
|---|---|
| [crates/xldif](crates/xldif/) | The library: reader, alignment, diff, and reports |
| [crates/xldif-cli](crates/xldif-cli/) | The `xldif` executable |
| [crates/xldif-fixtures](crates/xldif-fixtures/) | Synthetic test workbooks |
| [skills/xldif](skills/xldif/SKILL.md) | The agent skill: routing, [comparing](skills/xldif/references/comparing.md), [installation](skills/xldif/references/installation.md) |
| [ARCHITECTURE.md](ARCHITECTURE.md) | Boundaries, invariants, and the process contract |
| [PLAN.md](PLAN.md) | Design decisions and milestones |
| [TODO.md](TODO.md) | Open items and pending verification |
| `fixtures/private/` | Git-ignored real workbooks for local checks only |

## Installing

Download the Windows release archive (once one is published), or build from
source on any platform; see
[installation](skills/xldif/references/installation.md).

## Building

```bash
cargo build --release
```

The executable is `target/release/xldif` (`xldif.exe` on Windows).

## Output

Every command prints one JSON object to stdout,
`{ok, command, result, warnings, error, metadata}`, and exits 0 on success,
1 on failure, and 2 on an invalid invocation. Help, `--version`, and
`--format text` views are plain text.

Values are the results Excel last saved; xldif never recalculates. Warnings
say when they may be stale.

## Checks

```bash
python scripts/check.py
```

The reader parity check needs `openpyxl` (`pip install openpyxl`).
