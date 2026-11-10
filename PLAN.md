# xldif build plan

`xldif` compares two Excel workbooks and reports what changed: values,
formulas, formatting, and workbook objects. Rows and columns are matched by
content, so an inserted row is one change, not every cell below it. It ships
as a standalone CLI with two outputs: JSON for agents, and a self-contained
side-by-side HTML report for people. An agent skill teaches agents to drive
it. The process contract and report style follow `../dsdai`'s `dsd compare`
(`render_html` in `crates/dsd/src/compare.rs`). The grid-alignment
algorithm is new.

Status:
- M0 (scaffold) is done.
- M1 is in progress. Done: `package`, the values and formula-text reader,
  `inspect`, `cells`, and openpyxl parity (clean on 29 private workbooks).
- Next action: the values-only alignment spike and `compare`.

## Decisions

| Decision | Choice | Why |
|---|---|---|
| Build or reuse | Build. Borrow ideas, and permissively licensed code with attribution | The GitHub survey (2026-11-10) found no tool that combines formatting diffs, row and column alignment, and an HTML report. workbooklens (Apache-2.0) compares by position; xlsx-review (MIT) aligns but skips formatting |
| Language and layout | Rust workspace: `crates/xldif` (library, no process I/O) and `crates/xldif-cli` (clap, process contract) | Same boundaries, lints, and release tooling as dsdai |
| Workbook reader | Own read-only OOXML reader on `zip` + `quick-xml` | `calamine` cannot read styles, comments, or validation. `umya-spreadsheet` is a read/write object model that doesn't resolve effective formatting and isn't built around input bounds. A diff needs exact control over style inheritance, formulas, and hostile-input bounds |
| File types | `.xlsx` and `.xlsm` (macros not diffed). `.xlsb` and `.xls` are rejected with `unsupported_format` | `.xlsb` is a separate binary format; add it later only if real files need it |
| Values | Stored (cached) values; no recalculation, no Excel needed | Deterministic and portable. Warn on manual calculation, `fullCalcOnLoad`, formulas with no cached value, and date-system mismatches |
| Scope | Two-file comparison only | Merging, comparing a series of versions, and range-to-range comparison are out of v1 |
| JSON style | camelCase fields and dsdai's status words (`same`, `changed`, `onlyBase`, `onlyOther`, plus `moved`) | One vocabulary across the user's tools |
| Fixtures | Synthetic workbooks written by `rust_xlsxwriter` with explicit formula results, plus hand-made Excel-saved files with invented content | rust_xlsxwriter stores 0 as every formula result unless told otherwise. Shared formulas, threaded comments, dynamic arrays, and real style inheritance only come from Excel |
| Oracles | openpyxl for values and formulas; Excel COM (`Range.Text`, `DisplayFormat`, colours) on Windows for display strings and effective formats | Independent checks of the reader, not of xldif against itself |
| Confidentiality | Same rule as dsdai: real workbooks only under git-ignored `fixtures/private/`. Their contents, file names, and derived values never appear in tests, docs, or commits | Real inputs are audit workpapers |

## Architecture

```text
skills/xldif ──(xldif …; JSON on stdout)──► crates/xldif-cli ──► crates/xldif
  routing, judgment                           process contract     reader, model,
                                                                   align, diff, render
```

`crates/xldif` modules:

- **`package`:** zip access with bounds (archive size, member count,
  decompressed size, XML depth, cell count), and relationship and
  content-type resolution.
- **`reader`:**
  - Workbook level: `workbook.xml` (sheet order, visibility, defined names,
    `calcPr`, and the `workbookPr date1904` flag), shared strings (rich text
    flattened, runs kept for display), `styles.xml` (including
    `indexedColors`), `theme1.xml`, `metadata.xml` (dynamic arrays), and
    `persons.xml`.
  - Each sheet's XML: cells, types, cached values, formulas, merges, row and
    column records, data validation, conditional formatting, and hyperlinks.
  - Comments: legacy notes and threaded comments.
- **`model`:** an immutable `Workbook` → `Sheet` → sparse cell grid.
  - Each written cell holds a typed `Value` (number, string, bool, error,
    empty), a `Formula`, and a `StyleId`.
  - `Formula` is one of: none; plain; shared (expanded to each cell); or an
    array anchor, which stores the anchor cell, its range, and whether it is
    a dynamic or legacy (CSE) array. Cells inside an array range are values
    marked "spilled from" their anchor and are not given their own formula.
  - Row and column records keep their own style, height or width, and hidden
    flag.
- **`format`:** resolves a cell's style into comparable properties.
  - A written `<c>` always uses its own `s`, which defaults to 0.
  - Inheritance applies only to cells that aren't written: the row style when
    the row has `customFormat="1"`, else the column style, else 0.
  - Cell formats are resolved through `cellStyleXfs` (the named-style parent)
    according to the `applyX` flags. The named style's name is a property of
    its own.
  - Colours are kept symbolic: a theme index with tint, an indexed colour,
    an RGB value, or auto. Resolved RGB is for display only.
  - The theme-colour index order is lt1, dk1, lt2, dk2, then accents. That is
    not the order of `clrScheme` in theme1.xml.
  - A `dxf` (conditional-formatting style) is a set of optional overrides,
    not a full format. A solid `dxf` fill keeps its colour in `bgColor`.
- **`formula`:**
  - A tokenizer for A1 references: sheet-qualified, absolute and relative,
    ranges, whole rows and columns, structured and external references, and
    `_xlfn.` names. Tokens it doesn't know pass through verbatim.
  - It produces a reference-free shape: functions, operators, and constants,
    with references abstracted away.
  - It can rewrite references through a row and column map.
- **`align`:** sheet pairing, row and column alignment, and move detection
  (see Alignment). This includes a port of dsdai's `difflib.rs` matcher and
  its non-crossing key pairing (same owner, so no licence issue).
- **`diff`:** cell, row, column, and object changes over an alignment, plus
  context labels, change folding, and the summary.
- **`numfmt`:** an Excel number-format renderer for display strings,
  covering sections, conditions, dates and times, and literal text. A code
  it can't render falls back to the raw value with the code noted, never an
  error. Display strings are a convenience only: comparisons use typed
  values.
- **`render`:** text, JSON (serde structs), and HTML.

The CLI owns argument parsing, the envelope, exit codes, and refusing to
overwrite files. Release checks and `upgrade` follow dsdai later, in M5.

## Process contract

Based on dsdai's contract version 1:

- stdout carries exactly one newline-terminated document: the JSON envelope
  `{ok, command, result, warnings, error, metadata}`, or plain text for help,
  `--version`, and `--format text`. Failures are always JSON.
- **Exit codes:** 0 means ok, 1 an execution failure, 2 an invalid invocation
  (`invalid_invocation`).
- **Error codes:** dsdai's `file_not_found`, `invalid_archive` (also used for
  bounds violations, as in dsdai), `invalid_xml`, `invalid_argument`,
  `output_exists`, and `io_error`, plus `unsupported_format`, which is new
  for `.xlsb` and `.xls`.
- `metadata.contract` starts at 1; new fields are added without removing old
  ones.

Commands:

- `xldif inspect BOOK` gives an agent its first look without cell contents:
  sheets (order, visibility, used range, cell, formula, and comment counts),
  defined names, calculation settings, date system, and reader warnings.
- `xldif compare BASE OTHER [--format json|text|html] [-o FILE] [--sheet NAME]
  [--kind KIND,...] [--limit N (default 500)] [--all] [--tolerance X]
  [--align auto|position] [--force]`
  - HTML requires `-o`. With `-o`, stdout carries a summary envelope.
  - `--all` also lists sheets that are the same.
  - It never overwrites either input, even with `--force`.

### JSON result shape (sketch)

There is one `kind` vocabulary, shared by `--kind` and the output: `value`,
`formula`, `format`, `row`, `column`, `sheet`, `name`, `comment`, `merge`,
`validation`, `conditionalFormat`, `hyperlink`, `theme`.

```jsonc
{
  "base": {"file": "a.xlsx", "sheets": 5, "date1904": false},
  "other": {"file": "b.xlsx", "sheets": 6, "date1904": false},
  "summary": {"sheetsChanged": 2, "onlyOther": 1, "rowsOnlyBase": 1, "rowsOnlyOther": 3,
              "rowsMoved": 1, "value": 12, "formula": 4, "format": 7, "comment": 2,
              "listed": 26, "omitted": {"BS": {"format": 0}}},
  "sheets": [{
    "id": "s0",                      // sheet-pair index: base order first, then only-other sheets
    "status": "changed",             // same | changed | onlyBase | onlyOther
    "base": "BS", "other": "BS", "renamed": false, "moved": false,
    "alignment": {
      "method": "content",           // content | position (fallback, with a warning)
      "rows": [[1,1,40],[41,null,1],[42,41,60]],   // runs of [base start, other start, length]
      "rowMoves": [[105,12,1]],      // moved runs, kept separate from the ordered runs
      "cols": [[1,1,12]], "colMoves": []
    },
    "changes": [
      {"id": "s0:value:D14:D15", "kind": "value", "status": "changed",
       "base": {"cell": "D14", "value": 1200, "type": "number", "display": "1,200"},
       "other": {"cell": "D15", "value": 1350, "type": "number", "display": "1,350"},
       "delta": 150,
       "labels": {"row": "Trade receivables", "column": "2025", "inferred": true}},
      {"id": "s0:formula:D20:D21", "kind": "formula", "status": "changed",
       "base": {"cell": "D20", "formula": "SUM(D10:D19)"},
       "other": {"cell": "D21", "formula": "SUM(D10:D18)"},
       "expected": "SUM(D10:D20)"},  // base formula rewritten through the alignment
      {"id": "s0:format:B3:H40:B3:H40", "kind": "format", "status": "changed",
       "base": {"range": "B3:H40"}, "other": {"range": "B3:H40"},
       "properties": [{"name": "fill.fg", "base": "theme:4", "other": "rgb:FFFF00"}]},
      {"id": "s0:row:-:41", "kind": "row", "status": "onlyOther", "other": {"row": 41},
       "labels": {"row": "Prepayments", "inferred": true}}
    ]
  }],
  "objects": [{"id": "name:TaxRate:workbook", "kind": "name", "status": "changed",
               "base": {"refersTo": "Inputs!$B$2"}, "other": {"refersTo": "Inputs!$B$3"}}]
}
```

- **Ids:** an id is built from the change's location (sheet pair, kind, base
  reference, other reference), so it stays the same across filters, limits,
  and reruns. Sheet names never go inside an id.
- **Labels:**
  - `labels.row` is the first text cell to the left in the same row.
  - `labels.column` is the nearest text cell above in the same column.
  - Both are searched within a small window and taken from the other side,
    with `labels.base` added when the base side's label differs.
  - `inferred: true` marks the labels as heuristic.
- **Folding:** a rectangular block of cells that share the same property
  change becomes a single change with a range. This applies to formats and
  to formulas adjusted by structure, so restyling a 50k-cell table is one
  entry.
- **Limits:** `--limit` caps the listed changes. `summary.omitted` counts
  what was cut for each sheet and kind, so an agent knows where to narrow
  with `--sheet` and `--kind`.

## Alignment

The tool exists to handle inserted rows and columns, so whatever an
alignment signal is built from must survive a row or column insertion.

1. **Sheet pairing.** Pair sheets by exact name first. Pair the remaining
   sheets by content similarity above a threshold and report them as
   renamed. Report order and visibility changes separately.
2. **Columns:**
   - Keep columns in position when both sides have the same used width and
     their header rows match closely.
   - Otherwise, key each column by its header text: the text cells in the
     first rows that look like headers.
   - Then match by similarity: the Jaccard similarity of the multiset of the
     column's values, sampled over rows whose label key appears on both
     sides.
   - Use difflib-style matching, and never let pairs cross.
3. **Rows:**
   - Key each row by its label: the leading text cells, normalized for
     whitespace and case. This is dsdai's account-label approach, and it
     survives an inserted column.
   - Match the keys with the difflib matcher and no crossing pairs.
   - In the gaps, pair rows whose value similarity is at least a threshold,
     measured over the matched columns (Jaccard on multisets of value and
     formula-shape tokens).
   - Blank and formatting-only rows are left out of matching and then fill
     the gaps between their matched neighbours in position, so runs of
     spacer rows never anchor unrelated sections.
4. **Cost.** When several alignments are possible (column-first against
   row-first, or a pairing against delete plus insert), xldif picks the one
   with the lowest cost.
   - The cost adds a weight for each row or column inserted or deleted and a
     weight for each changed cell within matched pairs.
   - An inserted row costs less than a matched row in which most cells
     changed.
   - The weights are constants tuned on the private corpus and recorded in
     `ARCHITECTURE.md`.
5. **Moves.** After alignment, an unmatched base row and an unmatched other
   row with the same unique key and matching value shape are reported as
   moved (in `rowMoves`), not as delete plus insert. Columns are handled the
   same way.
6. **Determinism and symmetry.** Every tie-break orders candidate pairs
   canonically (by the pair of indices, base then other), never by search
   order. Comparing a file with itself yields no changes. On fixtures with
   an unambiguous alignment, swapping base and other gives the inverse
   alignment and the same change counts.
7. **Bounds.** The matching work is capped. When the cap is exceeded, xldif
   falls back to position-based matching for that sheet, with a warning and
   `alignment.method: "position"`.
8. **Objects follow the alignment.** Merges, comments, data validation,
   conditional formatting ranges, and hyperlinks are mapped through the row
   and column maps before they are compared, so an inserted row doesn't flag
   every comment below it.

## Cell and object comparison

- **Values:**
  - Compare by type, then by value. Numbers report a `delta`; `--tolerance`
    suppresses tiny float noise.
  - Numbers formatted as dates also carry an ISO date, taken from each
    side's own date system. A date1904 mismatch gives a warning, and the
    ISO dates are compared instead of the raw serials.
  - A change of type is a change of its own.
- **Formulas:**
  - Rewrite the base formula's references through the row and column maps
    into other-side coordinates, then compare token streams with the other
    formula.
  - When the base formula's text differs from the other formula's but the
    rewritten one matches, Excel adjusted it for an insertion or deletion.
    That is not reported by default. It is folded into a
    `formula`/`adjusted` change that `--kind formula` with `--all` shows.
  - A real rewrite reports the base and other formulas plus `expected`.
  - Array formulas are compared as anchor plus range, and dynamic arrays
    flag spill-range growth once.
  - Cached values are compared separately, as values.
- **Formatting:**
  - Compare each written cell's resolved properties: number format, font
    (name, size, bold, italic, underline, strike, colour), fill (pattern and
    colours), each border edge (style and colour), alignment, protection,
    and the named style.
  - Row and column styles, heights, widths, and hidden flags are compared as
    `row` and `column` records. They are not expanded to every unwritten
    cell.
  - Colours compare symbolically.
  - Any change to theme1.xml is one `theme` change. Themed cells don't each
    report it.
  - Conditional formatting is compared as rules: range, type, formula, `dxf`
    overrides, and priority. Rules are not evaluated against cells.
  - Table styles are out of v1.
- **Workbook objects:**
  - Sheets: onlyBase, onlyOther, renamed, reordered, visibility.
  - Defined names, compared by name and scope, with `refersTo`.
  - Comments:
    - A threaded comment is compared as its thread: author, resolved through
      `persons.xml`, and each reply's text.
    - Excel also saves a legacy-note copy of each threaded comment. That
      copy is suppressed so the comment isn't reported twice.
    - Plain legacy notes are compared on their own.
  - Merged ranges, data validation (type, operator, formulas, list), and
    hyperlinks.

## HTML report

- **Format:** one self-contained file with no external assets, rendered in
  Rust as static HTML plus a small inline script, the way dsdai's
  `render_html` is.
- **Layout:** the same as dsdai's report. Generate a public reference with
  `dsd compare` from dsdai's public fixture.
  - A top bar with summary chips.
  - Previous and next change buttons (`N`/`P`/`J`/`K`).
  - A "changes only" toggle, and a details toggle (`I`).
  - A sheet list with a change count per sheet.
  - Two panes side by side, scrolling together, each named after its file.
- **Grid:**
  - Each pane shows its own row numbers and column letters.
  - Inserted and deleted rows and columns appear as hatched placeholder rows
    and columns opposite them, and moved rows are marked.
  - Cells are coloured by change kind.
  - Clicking a cell shows old and new value, formula (with `expected`), and
    changed format properties.
  - Cell text uses the display string; fonts and fills are drawn lightly.
- **Size:** runs of unchanged rows collapse to "… N same rows" with a few
  context rows around each change. Hard limits on rendered cells add a
  visible warning instead of producing a huge file.

## Agent skill

`skills/xldif/SKILL.md`, structured like dsdai's `dart-dsd`:
- `SKILL.md` covers when to use the tool, `inspect` first, filtering by
  `--sheet` and `--kind` using `summary.omitted`, reading the envelope, and
  handing people the HTML.
- A reference file explains the change kinds and how alignment can mislead:
  position fallback, inferred labels, `adjusted` formulas, and stale cached
  values.
- Evals pose realistic questions such as "what changed in the BS sheet
  between v1 and vFinal?"

`.agents/skills/` and `.claude/skills/` stay generated by `sjskills`.

## Milestones

Each milestone ends with `python scripts/check.py` green: fmt, clippy
pedantic, tests, and the CLI smoke run. Alignment is the largest risk, so it
is proven on real files before the full reader is built.

- **M0 Scaffold.**
  - `git init` and the workspace with dsdai's lints and release profile.
  - `AGENTS.md`, `ARCHITECTURE.md`, `README.md`, and a `.gitignore` that
    covers `fixtures/private/`.
  - `scripts/check.py`, and the fixture helper on `rust_xlsxwriter`.
- **M1 Values reader and alignment spike.**
  - Build `package`, plus a `reader` limited to values and formula text,
    `inspect`, and `align`.
  - Run a values-only `compare --format json` with a temporary
    `--format text` dump.
  - Done when:
    - a parity script matches values and formulas against openpyxl on every
      fixture and the private corpus;
    - on version pairs in the private corpus, the alignment is reviewed by
      hand and the cost weights are recorded;
    - fixtures with inserted, deleted, and moved rows and columns, an
      inserted column combined with inserted rows, and spacer rows give
      exactly the expected alignment.
- **M2 Formulas.**
  - The `formula` module: tokenizer, shape, and rewriting through the
    alignment. Shared and array formulas, dynamic arrays, and `adjusted`
    detection.
  - Done when: fixtures with absolute references, relative references, and
    ranges spanning an insertion show no false formula changes, and real
    rewrites are reported with `expected`.
- **M3 Formatting and objects.**
  - `format` (style precedence, symbolic colours, `dxf`), `numfmt`, and the
    objects (names, comments with threaded handling, merges, validation,
    conditional formatting, hyperlinks, theme).
  - Change folding, limits, labels, and the full JSON.
  - Done when: an Excel COM oracle script checks display strings and
    effective formats per cell on the hand-made files; a variant fixture for
    each object kind gives exactly the expected changes; self-compare and
    the symmetry criterion pass; and a private-corpus smoke run passes
    without printing content.
- **M4 HTML report.**
  - The renderer and its script.
  - Done when: reports for the fixtures are checked in a browser
    (navigation, synced scrolling, details, collapse, mobile width), and a
    structure test asserts the report has no external URLs and stays within
    the size limits.
- **M5 Skill and release.**
  - The skill with its evals.
  - Windows x64 release first, using dsdai's `release.py` pattern, then
    other targets.
  - `version` and `upgrade` only if distribution needs them.

## Risks to watch

- **Alignment quality** on sheets without text labels: pure numeric grids
  and pivot-like layouts. Those fall back to value similarity alone, so
  tune and test them specifically in M1.
- **Values hidden under merges:** Excel keeps values in a merged range's
  non-anchor cells (the private corpus has them). The diff must decide
  whether to report changes there, and say they are hidden.
- **Format facts from memory:** row `customFormat`, `dxf` `bgColor`, the
  threaded-comment fallback, and dynamic-array `cm` are not yet checked on
  real Excel files. Confirm each on a hand-made Excel file in the milestone
  that implements it, and record what was observed in a format reference,
  as dsdai does.
- **Formula tokenizer:** edge cases include structured references, external
  workbooks, and names that look like cell references. Unknown tokens pass
  through, and one bad formula never fails a whole comparison.
- **Large workbooks:** the target is two 100k-cell workbooks compared in a
  few seconds. Measure in M1 before optimizing.
