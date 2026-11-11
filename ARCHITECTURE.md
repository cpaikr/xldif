# Architecture

`xldif` is one Rust implementation of Excel workbook reading and comparison,
shipped as a standalone executable. [README.md](README.md) owns commands and
building; [PLAN.md](PLAN.md) owns the design decisions and milestones. This
file describes what exists now and the boundaries new code must respect.

## Boundaries

```text
skills/xldif ──(xldif …; JSON on stdout)──► crates/xldif-cli ──► crates/xldif
  routing, judgment                           process contract     workbook rules
```

- [`crates/xldif`](crates/xldif/src/lib.rs) owns every workbook rule and has
  no process I/O:
  - [package.rs](crates/xldif/src/package.rs): the bounded ZIP archive,
    member lookup, content types, and relationships;
  - [xml.rs](crates/xldif/src/xml.rs): a pull parser with local names,
    resolved references, and a depth cap;
  - [reader.rs](crates/xldif/src/reader.rs): workbook, shared strings,
    sheet parts, and the files external links name, into the model;
  - [model.rs](crates/xldif/src/model.rs): the immutable workbook model;
  - [cellref.rs](crates/xldif/src/cellref.rs): A1 references and ranges;
  - [views.rs](crates/xldif/src/views.rs): `inspect` and `cells` results;
  - [align.rs](crates/xldif/src/align.rs): sequence alignment of rows or
    columns (key anchors, similarity in the gaps, moves);
  - [formula.rs](crates/xldif/src/formula.rs): formula tokens, shifting
    (shared formulas), remapping through an alignment, and position-free
    forms for folding;
  - [links.rs](crates/xldif/src/links.rs): external workbook links paired
    by the file they name, so a formula's `[1]` is read in the other
    workbook's numbering before it is compared;
  - [styles.rs](crates/xldif/src/styles.rs): cell formats from `styles.xml`,
    resolved into comparable properties with symbolic colors;
  - [numfmt.rs](crates/xldif/src/numfmt.rs): number format codes rendered
    as the text Excel shows, and date serials as ISO dates;
  - [sheet_objects.rs](crates/xldif/src/sheet_objects.rs): row and column
    records, merges, comments, validation, conditional formats, hyperlinks;
  - [diff_objects.rs](crates/xldif/src/diff_objects.rs): formats, layout, and
    those objects compared through the alignment;
  - [html.rs](crates/xldif/src/html.rs): the self-contained side-by-side
    report, built from a comparison listing and the two workbooks;
  - [diff.rs](crates/xldif/src/diff.rs): sheet pairing, column-then-row
    alignment, value and formula changes, labels, folding, and the
    `compare` listing;
  - [error.rs](crates/xldif/src/error.rs): stable error categories.

  The [`xldif` skill](skills/xldif/SKILL.md) never reads workbooks itself:
  it chooses commands, reads envelopes, and owns what to report.
- [`crates/xldif-cli`](crates/xldif-cli/src/main.rs) parses arguments (clap),
  runs one command, and writes one stdout document
  ([envelope.rs](crates/xldif-cli/src/envelope.rs)). It also owns the one
  place xldif starts another program
  ([excel.rs](crates/xldif-cli/src/excel.rs), with the PowerShell script
  beside it): having Excel save a copy of a workbook the reader rejects, or
  of a workbook as Excel has it open.
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
  the stack. Alignment work is capped per gap; a gap beyond the cap pairs by
  position with an `alignment_fallback` warning.
- **Excel converts; it never reads.** Where a workbook reaches the reader
  only through Excel, Excel saves a copy and that copy goes through the same
  reader as any other file, in a scratch directory removed before the
  command returns. A warning names the route (`converted_by_excel`,
  `unsaved_state`), because Excel recalculates on opening and rewrites
  fonts. An open workbook is looked up among the objects running programs
  have registered and copied with `SaveCopyAs`; it is never saved, closed,
  or loaded by xldif. Excel is asked for a conversion only when the reader
  has already rejected the file as a compound or binary workbook, and the
  copy is refused unless Excel reports one of its own workbook formats: it
  would otherwise open any file as text.
- **Deterministic results.** Content hashes are fixed (FNV-1a) and every
  alignment tie breaks by index, so the same inputs always give the same
  changes and ids. Change ids derive from locations, never from list order.
- **Stored values only.** The reader takes each cell's cached result as
  written; nothing is recalculated. `inspect` warns when values may be stale
  (manual calculation, recalculate-on-open, formulas without a result).
- **Say what is not read.** Content the model has no place for (charts and
  other drawings, chart sheets, tables, pivot tables, embedded objects,
  sheet and workbook protection, worksheet extension lists, external links
  that name no workbook file, VBA) is counted while reading and named in
  one `not_compared` warning per workbook, so an empty comparison never
  silently covers it
  ([reader.rs](crates/xldif/src/reader.rs)). The HTML report shows every
  warning under its header. Autofilter criteria are the
  exception: they are not read and not named, because the filter's range
  (a defined name) and the rows it hides (row layout) are compared.
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
  `invalid_xml`, `invalid_argument`, `output_exists`, `io_error`,
  `excel_failed` when Excel would not hand over an open workbook, and
  `internal_error` for a panic, which is a bug); 2 is an invalid invocation
  (`invalid_invocation`).
- Bare `xldif` prints exactly `xldif --help`.
- `metadata` is `{tool: "xldif", version, contract}`.

## Verification

- [`crates/xldif-cli/tests/cli.rs`](crates/xldif-cli/tests/cli.rs) runs the
  executable as an external process and checks the process contract.
- [`crates/xldif/tests/reader.rs`](crates/xldif/tests/reader.rs) reads
  synthetic workbooks, including hand-assembled parts in shapes writer
  libraries never produce.
- [`crates/xldif/tests/excel_saved.rs`](crates/xldif/tests/excel_saved.rs)
  reads `fixtures/excel/`: one invented workbook that Excel built and saved,
  in the usual format and as Strict Open XML
  ([`scripts/excel_fixture.py`](scripts/excel_fixture.py)). Its assertions
  are the file-format facts observed in Excel's own output.
- [`crates/xldif-cli/tests/excel.rs`](crates/xldif-cli/tests/excel.rs)
  drives Excel: a `.xls` and a `.xlsb` come back with the figures they were
  saved from, and a workbook held open with an unsaved edit reads as saved,
  or with the edit under `--unsaved`, and is byte-for-byte unchanged on disk
  afterwards. It runs only with `XLDIF_TEST_EXCEL=1`, which
  `scripts/check.py` sets where Excel is installed.
- [`scripts/parity.py`](scripts/parity.py) compares every cell's value and
  formula with openpyxl, an independent reader, and counts the known,
  intended differences it skips.
- [`crates/xldif/tests/formulas.rs`](crates/xldif/tests/formulas.rs) covers
  formulas adjusted by inserted rows and by other sheets' rows, renamed
  sheets, rewrites with `expected`, formulas replaced by values, unchanged
  text that reads a different cell after rows moved, malformed formulas,
  folding, and external links renumbered or pointed at another file.
- [`crates/xldif/tests/objects.rs`](crates/xldif/tests/objects.rs) covers
  formats, layout, and every object kind on `rust_xlsxwriter` output,
  including an inserted row that every object must follow.
- [`crates/xldif/tests/compare.rs`](crates/xldif/tests/compare.rs) covers each
  alignment case: inserted, deleted, and moved rows; an inserted column with
  inserted rows; spacer rows; renumbered labels; renamed and moved sheets;
  swap symmetry.
- [`scripts/display_parity.py`](scripts/display_parity.py) compares display
  text with Excel's own (`Range.Text`, through COM on Windows) on a
  synthetic probe of format codes, and on given workbooks with counts only.
- [`scripts/check.py`](scripts/check.py) runs fmt, clippy, tests, a CLI smoke
  run, parity over the public fixtures and, when present, the private corpus,
  and a corpus compare smoke run: every workbook against itself must show no
  changes, and consecutive versions must compare, counting pairs that do not
  mirror when swapped (counts only). Where Excel is installed it also runs
  the display probe; `--excel` adds the private corpus.
