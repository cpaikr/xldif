# Architecture

`xldif` is one Rust implementation of Excel workbook reading and comparison,
shipped as a standalone executable. [README.md](README.md) owns commands and
building; [PLAN.md](PLAN.md) owns the design decisions and milestones. This
file describes what exists now and the boundaries new code must respect.

## Boundaries

```text
skills/xldif ──(xldif …; JSON on stdout)──► crates/xldif-cli ──► crates/xldif
  routing, judgment (M5)                      process contract     workbook rules
```

- [`crates/xldif`](crates/xldif/src/lib.rs) owns every workbook rule and has
  no process I/O:
  - [package.rs](crates/xldif/src/package.rs): the bounded ZIP archive,
    member lookup, content types, and relationships;
  - [xml.rs](crates/xldif/src/xml.rs): a pull parser with local names,
    resolved references, and a depth cap;
  - [reader.rs](crates/xldif/src/reader.rs): workbook, shared strings, and
    sheet parts into the model;
  - [model.rs](crates/xldif/src/model.rs): the immutable workbook model;
  - [cellref.rs](crates/xldif/src/cellref.rs): A1 references and ranges;
  - [views.rs](crates/xldif/src/views.rs): `inspect` and `cells` results;
  - [error.rs](crates/xldif/src/error.rs): stable error categories.

  Alignment, diff, and the renderers arrive with M1–M4 (see PLAN.md ›
  Architecture).
- [`crates/xldif-cli`](crates/xldif-cli/src/main.rs) parses arguments (clap),
  runs one command, and writes one stdout document
  ([envelope.rs](crates/xldif-cli/src/envelope.rs)).
- [`crates/xldif-fixtures`](crates/xldif-fixtures/src/lib.rs) writes synthetic
  workbooks with `rust_xlsxwriter` for tests. No shipped crate depends on it.

## Invariants

- **No writes to inputs.** xldif only reads workbooks. A report written with
  `-o` never replaces an input, even with `--force`.
- **Bounds.** File size, member count, each member's decompressed size
  (measured while reading), total decompressed size, XML depth, and cell
  count are capped ([package.rs](crates/xldif/src/package.rs),
  [xml.rs](crates/xldif/src/xml.rs), [reader.rs](crates/xldif/src/reader.rs)).
  Hostile input fails with `invalid_archive` instead of exhausting memory or
  the stack. Alignment work will be capped the same way.
- **Stored values only.** The reader takes each cell's cached result as
  written; nothing is recalculated. `inspect` warns when values may be stale
  (manual calculation, recalculate-on-open, formulas without a result).
- **Read what Excel keeps.** Values stored under a merged range's hidden
  cells and phonetic runs are file content: the former are read, the latter
  are excluded from text, as Excel displays it.

## Process contract

Contract version 1 (`metadata.contract`), the same shape as `dsd`:

- stdout carries exactly one newline-terminated document: the JSON envelope
  `{ok, command, result, warnings, error, metadata}`, or plain text for help,
  `--version`, and successful `--format text` views. Failures are
  always JSON. stderr stays empty.
- Exit 0 means `ok: true`; 1 is an execution failure with a stable
  `error.code` (`file_not_found`, `invalid_archive`, `unsupported_format`,
  `invalid_xml`, `invalid_argument`, `output_exists`, `io_error`); 2 is an
  invalid invocation (`invalid_invocation`).
- Bare `xldif` prints exactly `xldif --help`.
- `metadata` is `{tool: "xldif", version, contract}`.

## Verification

- [`crates/xldif-cli/tests/cli.rs`](crates/xldif-cli/tests/cli.rs) runs the
  executable as an external process and checks the process contract.
- [`crates/xldif/tests/reader.rs`](crates/xldif/tests/reader.rs) reads
  synthetic workbooks, including hand-assembled parts in shapes writer
  libraries never produce.
- [`scripts/parity.py`](scripts/parity.py) compares every cell's value and
  formula with openpyxl, an independent reader, and counts the known,
  intended differences it skips.
- [`scripts/check.py`](scripts/check.py) runs fmt, clippy, tests, a CLI smoke
  run, and parity over the public fixtures and, when present, the private
  corpus (counts only).
