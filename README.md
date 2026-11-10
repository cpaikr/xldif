# xldif

`xldif` compares two Excel workbooks (`.xlsx`, `.xlsm`) and reports what
changed: values, formulas, formatting, and workbook objects. Rows and columns
are matched by content, so an inserted row is one change, not every cell
below it. Agents read its JSON; people read its side-by-side HTML report.
No Excel installation is needed.

**Status:** milestone M1 in progress. The reader and the read views
(`inspect`, `cells`) work; `compare` comes next. See [PLAN.md](PLAN.md).

## Commands

- `xldif inspect BOOK` shows sheets (kind, visibility, used range, cell and
  formula counts), defined names, calculation settings, and the date system.
  It prints no cell contents.
- `xldif cells BOOK --sheet NAME [--range A1:F40] [--limit N]` lists cells
  with a value or formula: address, type, stored value, and formula text.

Both accept `--format text`. Run `xldif <command> --help` for details.

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
