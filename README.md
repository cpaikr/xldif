# xldif

`xldif` compares two Excel workbooks (`.xlsx`, `.xlsm`) and reports what
changed: values, formulas, formatting, and workbook objects. Rows and columns
are matched by content, so an inserted row is one change, not every cell
below it. Agents read its JSON; people read its side-by-side HTML report.
No Excel installation is needed.

**Status:** milestones M1–M3 done. `compare` reports value, formula, and
format changes; inserted, deleted, and moved rows, columns, and sheets;
row and column layout; and merges, comments, data validation, conditional
formatting, hyperlinks, sheet visibility, defined names, and the theme.
The HTML report comes next. See
[PLAN.md](PLAN.md).

## Commands

- `xldif inspect BOOK` shows sheets (kind, visibility, used range, cell and
  formula counts), defined names, calculation settings, and the date system.
  It prints no cell contents.
- `xldif compare BASE OTHER [--sheet NAME] [--kind value,formula,format,...]
  [--limit N] [--all] [--tolerance X] [--align content|position]` reports
  what changed. Rows and columns pair by their labels and values, so an
  inserted row is one `row` change; each change has a stable id, base and
  other locations, a numeric delta (also for numbers stored as text), and
  inferred row and column labels. Formulas are compared after Excel's own
  adjustment for inserted and deleted rows, so `=D30` becoming `=D31` under
  an inserted row is not a change, while a rewrite shows both formulas and
  the one Excel would have kept (`expected`).
- `xldif cells BOOK --sheet NAME [--range A1:F40] [--limit N]` lists cells
  with a value or formula: address, type, stored value, and formula text.

All accept `--format text`. Run `xldif <command> --help` for details.

| Path | Purpose |
|---|---|
| [crates/xldif](crates/xldif/) | The library: reader, alignment, diff, and reports |
| [crates/xldif-cli](crates/xldif-cli/) | The `xldif` executable |
| [crates/xldif-fixtures](crates/xldif-fixtures/) | Synthetic test workbooks |
| [ARCHITECTURE.md](ARCHITECTURE.md) | Boundaries, invariants, and the process contract |
| [PLAN.md](PLAN.md) | Design decisions and milestones |
| `fixtures/private/` | Git-ignored real workbooks for local checks only |

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
