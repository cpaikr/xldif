# xldif

`xldif` compares two Excel workbooks (`.xlsx`, `.xlsm`) and reports what
changed: values, formulas, formatting, and workbook objects. Rows and columns
are matched by content, so an inserted row is one change, not every cell
below it. Agents read its JSON; people read its side-by-side HTML report.
No Excel installation is needed.

**Status:** scaffold (milestone M0). No commands exist yet; `inspect` and
`compare` arrive with the reader. See [PLAN.md](PLAN.md).

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
1 on failure, and 2 on an invalid invocation. Help and `--version` are plain
text.

## Checks

```bash
python scripts/check.py
```
