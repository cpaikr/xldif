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

- [`crates/xldif`](crates/xldif/src/lib.rs) owns every workbook rule. Today it
  holds the stable error categories ([error.rs](crates/xldif/src/error.rs)).
  The reader, model, alignment, diff, and renderers arrive with M1–M4 (see
  PLAN.md › Architecture). It has no process I/O.
- [`crates/xldif-cli`](crates/xldif-cli/src/main.rs) parses arguments (clap),
  runs one command, and writes one stdout document
  ([envelope.rs](crates/xldif-cli/src/envelope.rs)).
- [`crates/xldif-fixtures`](crates/xldif-fixtures/src/lib.rs) writes synthetic
  workbooks with `rust_xlsxwriter` for tests. No shipped crate depends on it.

## Invariants

- **No writes to inputs.** xldif only reads workbooks. A report written with
  `-o` never replaces an input, even with `--force`.
- **Bounds.** Archive size, member count, decompressed size, XML depth, cell
  count, and alignment work will be capped. Hostile input fails with an
  envelope, or alignment falls back to position-based matching with a
  warning; it never exhausts memory, time, or the stack.

## Process contract

Contract version 1 (`metadata.contract`), the same shape as `dsd`:

- stdout carries exactly one newline-terminated document: the JSON envelope
  `{ok, command, result, warnings, error, metadata}`, or plain text for help
  and `--version` (and `--format text` views once they exist). Failures are
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
- [`scripts/check.py`](scripts/check.py) runs fmt, clippy, tests, and a CLI
  smoke run; the private corpus is only counted until `inspect` exists.
