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
- M1 is done: the reader (`inspect`, `cells`, openpyxl parity clean on 29
  private workbooks) and values-only `compare` with content alignment
  (self-compare and swap symmetry clean on the corpus).
- M2 is done: the formula tokenizer, shared formulas expanded (all 13,954
  in the private corpus match openpyxl's expansion), and formulas compared
  through the alignment, including references to other sheets and renamed
  sheets, with runs folded.
- M3 is done: number-format display text and ISO dates (`display` and
  `date` on cells and value changes; the HTML grid shows display text),
  checked against Excel's `Range.Text` on a synthetic probe and the private
  corpus (every cell matches except locale-dependent built-in formats);
  formats (resolved per cell, colors symbolic, folded into rectangles),
  row and column layout, merges, comments (threaded ones replacing their
  legacy copy), data validation, conditional formatting (as rules),
  hyperlinks, sheet visibility, defined names (references carried through
  the alignment), and the theme, all mapped through the alignment.
- M4 is done: `compare --format html -o FILE` writes one self-contained
  report (aligned grids side by side, inserted/deleted/moved rows and
  columns marked, cells colored by change kind, next/previous navigation,
  sheet list, details panel, "changes only" view, large sheets trimmed to
  changed rows and their context, long added or removed runs folded). Checked in a browser on a synthetic
  demo pair (`cargo run -p xldif-fixtures --example write_demo -- DIR`).
- M5 is done except publishing: the `xldif` agent skill (`skills/xldif`,
  with evals on the synthetic demo pair) and a release workflow that builds
  the Windows x64 archive when a `v*` tag is pushed. Windows is the only
  release target: the tool is used beside Excel. CI still checks Linux, and
  other platforms build from source.
- The project review (2026-11-11) kept Rust and the own reader and put
  correctness of the comparison ahead of new features. Its first fixes
  are in; the rest, in order, is in [TODO.md](TODO.md).
- The review's follow-ups are done (alignment, the comparison model, Excel
  as converter and oracle, the smaller defects).
- Next action: the maintainer decides the first release. Features that
  wait on it are in [TODO.md](TODO.md#features).

## Decisions

| Decision | Choice | Why |
|---|---|---|
| Build or reuse | Build. Borrow ideas, and permissively licensed code with attribution | The GitHub survey (2026-11-10) found no tool that combines formatting diffs, row and column alignment, and an HTML report. workbooklens (Apache-2.0) compares by position; xlsx-review (MIT) aligns but skips formatting |
| Language and layout | Rust workspace: `crates/xldif` (library, no process I/O) and `crates/xldif-cli` (clap, process contract) | Same boundaries, lints, and release tooling as dsdai |
| Workbook reader | Own read-only OOXML reader on `zip` + `quick-xml` | `calamine` cannot read styles, comments, or validation. `umya-spreadsheet` is a read/write object model that doesn't resolve effective formatting and isn't built around input bounds. A diff needs exact control over style inheritance, formulas, and hostile-input bounds |
| File types | `.xlsx` and `.xlsm` (macros not diffed), read as saved. `.xls`, `.xlsb`, and password-protected workbooks are read through a copy Excel saves as `.xlsx`, where Excel is installed; elsewhere they fail with `unsupported_format` | A second reader for a binary format would double the format knowledge to keep right; Excel already has it, on every machine that runs xldif |
| Values | Stored (cached) values; no recalculation, no Excel needed | Deterministic and portable. Warn on manual calculation, `fullCalcOnLoad`, formulas with no cached value, and date-system mismatches |
| Scope | Two-file comparison, plus `series`: which saved versions of a workbook in a folder are worth keeping | Merging and range-to-range comparison are out of v1. `series` (2026-11-26) serves cleaning up folders of timestamped saves: it reuses the two-file comparison pairwise and only recommends; it never deletes |
| Series rules | Versions are names differing only by a trailing `_YYMMDD_HHMM` stamp, ordered by it. Each is compared with the last version kept, and kept with at least `--min-edits` edits: typed values between constants, formula changes, and rows, columns, and sheets added, removed, moved, or renamed. Recalculated values, formats, and objects are reported, not counted. The first, the newest, and unreadable versions are always kept | Recalculation ripple swamps raw counts (one changed rate changes thousands of values). Measuring from the last kept version stops a chain of small saves from all being dropped. Cells inside an array formula's or data table's range count as recalculated |
| JSON style | camelCase fields and dsdai's status words (`same`, `changed`, `onlyBase`, `onlyOther`, plus `moved`) | One vocabulary across the user's tools |
| Fixtures | Synthetic workbooks written by `rust_xlsxwriter` with explicit formula results, plus hand-made Excel-saved files with invented content | rust_xlsxwriter stores 0 as every formula result unless told otherwise. Shared formulas, threaded comments, dynamic arrays, and real style inheritance only come from Excel |
| Oracles | openpyxl for values and formulas; Excel COM on Windows for display strings (`Range.Text`) and for the cell's own format (`Range.Font`, `Interior`, `Borders`, alignment, protection) | Independent checks of the reader, not of xldif against itself. The format check reads the cell's own format, not `DisplayFormat`, which adds conditional formatting and table styles that xldif compares as rules or not at all. Theme and indexed colours stay symbolic in xldif and are not compared. Two cells share an edge and Excel reports one border for it, so a border agrees when it is the cell's own or its written neighbour's. On the private corpus (2026-11-13): 69,788 properties over 3,686 distinct formats agree, none differ |
| Excel's role | A converter for inputs the reader rejects and a test oracle; never the reader | Reviewed 2026-11-11 with Excel on every machine that runs xldif: Excel reads formats one cell at a time through COM, which is far too slow, recalculates on open, and cannot run in CI. The reader's stored values stay the basis of a comparison |
| Open workbooks | Read as last saved, with an `open_in_another_program` warning; `--unsaved` reads a copy the running Excel makes with `SaveCopyAs` | The saved file is what anyone else would get, so it stays the default. The copy leaves the open workbook unsaved and untouched. The workbook is found among registered running objects, never asked for by name, which would make Excel load a file that is not open. Not handled: a workbook opened from OneDrive or SharePoint (Excel knows it by its web address), Protected View, and Excel while a cell is being edited; each fails with `excel_failed` and a hint |
| Confidentiality | Same rule as dsdai: real workbooks only under git-ignored `fixtures/private/`. Their contents, file names, and derived values never appear in tests, docs, or commits | Real inputs are audit workpapers |

## Architecture

```text
skills/xldif ──(xldif …; JSON on stdout)──► crates/xldif-cli ──► crates/xldif
  routing, judgment                           process contract     reader, model,
                                                                   align, diff, render
```

`crates/xldif` modules, as built. [ARCHITECTURE.md](ARCHITECTURE.md) lists
every file and the invariants; this list keeps the choices made inside
them, including what the first plan named and the code does not do.

- **`package`, `xml`:** zip access with bounds (archive size, member count,
  each member's and the total decompressed size), relationship and
  content-type resolution, and a pull parser with a depth cap. The reader
  bounds the number of records (cells, rows, strings, sheets, names,
  styles).
- **`reader`:**
  - Workbook level: `workbook.xml` (sheet order, visibility, defined names,
    `calcPr`, and the `workbookPr date1904` flag), shared strings (rich text
    flattened to its text, phonetic runs dropped), `styles.xml` (through
    `styles`), the theme's colour scheme (kept as a fingerprint only),
    `persons.xml` for the authors of threaded comments, and the file each
    external link names (`externalReferences`, in the order formulas
    number them).
  - Each sheet's XML: cells, types, cached values, and formulas, and through
    `sheet_objects` merges, row and column records, data validation,
    conditional formatting, hyperlinks, legacy notes, and threaded comments.
  - Content with no place in the model is counted for the `not_compared`
    warning.
  - Not built: `indexedColors` (an indexed colour compares by its index,
    whatever palette the workbook defines) and `metadata.xml` (a dynamic
    array is not told from a legacy array formula).
- **`model`:** an immutable `Workbook` → `Sheet` → rows of written cells.
  - Each written cell holds a typed `Value` (number, text, bool, error,
    empty), an optional `Formula`, and a style index.
  - A `Formula` has a kind (normal, shared, array, data table), its text,
    and for an anchor the range it covers. A shared formula's text is
    expanded for each of its cells. An array formula sits on its anchor
    only; the other cells of its range are plain values with no mark of
    where they came from.
  - Row and column records keep their own style, height or width, and hidden
    flag.
- **`styles`:** resolves a cell's style index into comparable properties.
  - A written `<c>` always uses its own `s`, which defaults to 0.
  - Cells that aren't written get no format of their own: a row's style
    (kept when the row has `customFormat="1"`) and a column's style are
    compared as row and column records.
  - A `cellXfs` entry is taken as the whole format, which is how Excel
    writes it. The named-style parent (`cellStyleXfs`) contributes only its
    name, and the `applyX` flags are not consulted (unverified against
    files that rely on inheriting from the named style).
  - Colours are kept symbolic: a theme index with tint, an indexed colour,
    an RGB value, or auto. Nothing resolves them to RGB.
  - A `dxf` (conditional-formatting style) is kept as a canonical
    description of its XML, so rules compare by what they apply, not by
    index.
- **`formula`:**
  - A tokenizer for A1 references: sheet-qualified, absolute and relative,
    ranges, whole rows and columns, 3-D spans, and external workbooks'
    sheets. Structured table references and anything it doesn't know pass
    through verbatim.
  - It shifts a shared formula to each of its cells, rewrites references
    through a row and column map, and produces a position-free form
    (relative parts as offsets) for folding and for alignment tokens.
- **`align`:** row and column sequence alignment and move detection (see
  Alignment).
- **`links`:** external workbook links paired by the file they name, and
  a base formula's link numbers rewritten to the other workbook's.
- **`diff`, `diff_objects`:** sheet pairing, column-then-row alignment of
  each pair, value and formula changes, formats, layout, and objects over
  the alignment, plus context labels, change folding, and the summary.
- **`numfmt`:** an Excel number-format renderer for display strings,
  covering sections, conditions, dates and times, fractions, scientific
  notation, and literal text. Every code renders. Built-in ids whose code
  depends on the locale show dates in ISO form. The currency,
  negative-number, and accounting ids (5–8, 37–44) show as Excel does under
  Korean regional settings (`₩`, a leading minus, no parentheses), observed in Excel on 2026-11-11 for a workbook that
  names the ids without a code; Excel writes the code itself when it saves,
  so only files from other writers depend on this. What Excel shows under
  other regional settings (ECMA-376 lists `$` and parentheses for the US)
  is not observed and not reproduced.
  Column width (`####`), `*` fill, edge padding, colors, and the locale's
  date separator are not reproduced. Display strings are a convenience
  only: comparisons use typed values. Rules that could only be learned from
  Excel (the sign under conditional sections, negative zero, continued-
  fraction denominators, the case of `am/pm`) were observed through
  `scripts/display_parity.py` and are pinned by unit tests.
- **`views`, `html`:** the `inspect` and `cells` results, and the HTML
  report. Results are serde structs; the CLI serializes them as JSON or
  prints its own text listing.

The CLI owns argument parsing, the envelope, the text listings, exit codes,
and refusing to overwrite files. There is no `version` or `upgrade` command.

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
  for `.xlsb` and `.xls` where Excel cannot convert them, `excel_failed`
  for `--unsaved` when Excel does not hand the workbook over, and
  `internal_error` for a panic.
- `metadata.contract` starts at 1; new fields are added without removing old
  ones.

Commands:

- `xldif inspect BOOK` gives an agent its first look without cell contents:
  sheets (order, visibility, used range, cell, formula, and comment counts),
  defined names, calculation settings, date system, the files external
  links name, and reader warnings.
- `xldif compare BASE OTHER [--format json|text|html] [-o FILE] [--sheet NAME]
  [--kind KIND,...] [--limit N (default 500)] [--all] [--tolerance X]
  [--align content|position] [--force]`
  - HTML requires `-o`. With `-o`, stdout carries a summary envelope.
  - `--all` also lists sheets that are the same.
  - It never overwrites either input, even with `--force`.

### JSON result shape (sketch)

M1 implements this for `value`, `row`, `column`, and sheet changes. As
built, `listed` and `omitted` sit beside `summary`, the base side's row
label is `labels.baseRow`, and alignment runs list paired rows only (one-sided
rows are `row` changes). `xldif compare --help` describes the output.

There is one `kind` vocabulary, shared by `--kind` and the output: `value`,
`formula`, `format`, `row`, `column`, `sheet`, `name`, `comment`, `merge`,
`validation`, `conditionalFormat`, `hyperlink`, `theme`, `externalLink`
(`--kind external-link`).

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
    with `labels.baseRow` added when the base side's label differs.
  - `inferred: true` marks the labels as heuristic.
- **Folding:** a rectangular block of cells that share the same property
  change becomes a single change with a range. This applies to formats and
  to formulas adjusted by structure, so restyling a 50k-cell table is one
  entry.
- **Limits:** `--limit` caps the listed changes. `omitted` counts
  what was cut for each sheet and kind, so an agent knows where to narrow
  with `--sheet` and `--kind`.

## Alignment

The tool exists to handle inserted rows and columns, so whatever an
alignment signal is built from must survive a row or column insertion.
Implemented in M1 (`align.rs`, `diff.rs`); tuned on the private corpus.

1. **Sheet pairing.** Pair sheets by exact name first. Pair the remaining
   sheets by content similarity (at least 0.5) and report them as renamed.
   A paired sheet outside the longest in-order run is reported as moved.
   Visibility changes come with the other workbook objects in M3.
2. **Items.** Each sheet's content rows and columns become items with a
   label key and a multiset of content tokens:
   - A row's key is its leftmost text cell; a column's key is the header
     text (or a year stored as a number) in the sheet's top ten content rows.
   - Keys drop whitespace (Korean statements space out characters), case,
     and one leading outline marker ("Ⅰ.", "(1)", "1.", "가."), so a
     renumbered section still matches.
   - Column tokens are the column's values. Row tokens tie each value to its
     aligned column, so a value counts as shared only in the matching column.
   - Blank and formatting-only rows are not items, so spacer rows never
     anchor anything and inserting one is not a change.
3. **Order: columns, then rows.** Column alignment uses values only, which
   an inserted row barely changes; row alignment then uses the column map,
   so an inserted column cannot break row matching.
4. **Matching each axis.** Two items are the same row or column to the
   degree their similarity says (bigram label likeness averaged with token
   Jaccard). An alignment is the set of pairs, never crossing, with the
   greatest total similarity; a pair below 0.5 is never made. Two steps
   find it without comparing every item with every other:
   - **Anchors.** Items whose whole content (label plus tokens) occurs once
     on each side of a region are candidates to pair, and so are items
     whose label does: a statement line keeps its name while its figures
     change. The longest in-order run of all candidates anchors, so one
     line that moved, changed or not, cannot pull its neighbours out of
     line or split the region across every line it passed. Equal items at
     a region's two ends pair as well. Each anchor splits its region and
     the parts are searched again.
   - A label shared by several rows (a currency or entity column) never
     pairs two of them ahead of their content. An earlier version matched
     runs of equal labels first, which paired every row after an insertion
     with the row above its own.
   - **Gaps.** Between anchors a weighted LCS picks the pairs.
   - A leftover equal-sized run pairs by position for columns (headers often
     change wholesale between periods), and for rows when every row keeps
     its layout and shows some sign of being the same row (a shared value,
     or labels at least 0.25 alike): a row whose label alone was edited is
     one value change, while an unrelated row replacing another stays a
     deletion and an insertion.
   - `--tolerance` reaches the tokens: a number at least eight buckets
     from zero is put in a bucket eight tolerances wide, so rows that
     differ only by float noise still pair. Bucket edges sit off round
     decimals. Smaller numbers keep exact tokens, or rates under a
     tolerance sized for amounts would all be one token. Two numbers on
     either side of an edge differ as tokens; their row then pairs on the
     rest of its content.
5. **Moves.** Unpaired items with the same key and the same tokens, unique
   on each side, are moves (`rowMoves`, `colMoves`).
6. **Determinism and symmetry.** Hashes are fixed (FNV-1a) and every tie
   breaks by index, never by hash order. Ties broken by index would depend
   on which file is the base, so each sheet pair is aligned in a fixed
   order (by a hash of the two sheets' content) and the result turned round
   when that is not the order given: swapping the files mirrors the rows,
   columns, and values reported. Formula changes have a direction and do
   not mirror: a row inserted inside a total is a change, the same row
   deleted is not. Property tests over random workbooks hold both this and
   "a workbook against itself shows nothing, in JSON and in the report"
   (`crates/xldif/tests/properties.rs`); `scripts/check.py` counts the same
   on the private corpus.
7. **Bounds.** The gap pass compares every item of a gap with every other,
   up to 4 million pairs or 200 million token comparisons. A region with
   no anchor that is larger than 1 million pairs, or than that budget, is
   split on weaker evidence first, each kind tried in turn:
   - a run of 2 to 32 consecutive rows whose contents occur once, as a run,
     on each side (a table stacked twice);
   - a token that occurs once on each side, which votes for the two rows
     holding it: rows that are each other's first choice, and similar
     enough to pair, anchor (every row edited, but not in every cell);
   - a run of consecutive labels that occurs once on each side;
   - failing those, rows at the region's two ends, paired in file order for
     as long as they are similar enough to pair (labels repeating in a
     cycle, with a block inserted in the middle).

   A gap still over the budget is searched only near its diagonal (rows
   mostly keep their order and few come or go); with no room for even that
   it is paired by position. Pairing from the ends, the search near the
   diagonal, and pairing by position can each miss the best alignment, and
   nothing they find can show that they did not, so each sets
   `fellBack: true` and an `alignment_fallback` warning.
   `--align position` compares A1 with A1 throughout.
8. **Objects follow the alignment (M3).** Merges, comments, data
   validation, conditional formatting ranges, and hyperlinks are mapped
   through the row and column maps before they are compared, so an inserted
   row doesn't flag every comment below it.

## Cell and object comparison

- **Values:**
  - Compare by type, then by value. Numbers report a `delta`; `--tolerance`
    suppresses tiny float noise.
  - Numbers formatted as dates also carry an ISO date, taken from each
    side's own date system. A date1904 mismatch gives a warning, and cells
    formatted as dates on both sides compare by their ISO dates instead of
    the raw serials, with `delta` in days. Alignment still reads the
    serials.
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
  - As built in M2, adjusted formulas are counted in
    `summary.formulasAdjusted` and never listed. A deleted range corner
    moves inward to the nearest surviving row, as Excel shrinks a range; a
    deleted single reference becomes `#REF!`. References to other sheets
    follow those sheets' alignments, and a renamed sheet's references are
    renamed.
  - Array formulas compare by their anchor text; the spill range itself is
    not compared yet, and dynamic-array metadata is not read.
  - Unchanged text is a change when Excel would have widened one of its
    ranges over inserted rows or columns (a total no longer covering a new
    line), when a cell it referred to still exists under another address,
    or when its address now holds a cell that came from another row or
    column (a deleted row's place taken by the row below). A range is
    judged by its ends, and an end that stays inside the range does not
    count: rows reordered within a total leave it unchanged. Otherwise the
    unchanged text wins: a row edited in place can align as deleted plus
    inserted, and the formula was not touched.
  - `#REF!` (and `Sheet!#REF!`) from a deleted reference is an adjustment.
    3-D references (`Jan:Dec!B5`) and external ones (`[1]Prices!A1`) shift
    in shared formulas but never follow this workbook's alignment.
  - An external reference is compared by the file its number names, not by
    the number, which is only a position in each workbook's list of links.
    Links pair by target (without case); the base formula is read in the
    other workbook's numbering, so `[1]` against `[2]` for one file is no
    change. A link pointed at another file (`[1]` naming last year's file
    on one side and this year's on the other) is one `externalLink` change;
    the formulas through it are not listed one by one. A file the other
    workbook does not link to at all is written by name in `expected`
    (`[FY2024.xlsx]Sheet1!A1`), since no number there means it. This covers
    cell formulas and defined names, including external names
    (`[1]!Rate`), validation and conditional-format formulas, and the
    formula tokens rows are aligned on.
  - Blank rows between two paired rows are matched in order with the other
    side's blank rows in the same gap, so a spacer row next to an inserted
    or deleted line maps where Excel moved it. When the two gaps hold
    different numbers of blank rows, a blank row keeps its offset from the
    pair before it (a blank row someone typed into is still that row), and
    one carried past the end of the other side's gap is a deleted row: the
    blank row was removed, and formulas, merges, comments, and rules over
    it are treated as Excel treats a deleted row.
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
  - External links (`externalLink`), compared by the file each names: one
    linked on a single side is `onlyBase` or `onlyOther`; a number that
    names another file on each side is one `changed` with both targets.
    The id carries the numbers formulas use
    (`externalLink:<base>:<other>`), and `inspect` lists each workbook's
    links in that order. The target is the link part's `externalBook`
    relationship as Excel writes it, usually relative to the workbook's
    folder, so the same file reached by another path is another link.
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
  - A top bar with summary chips, and the comparison's warnings under it.
  - Previous and next change buttons (`N`/`P`/`J`/`K`).
  - A "changes only" toggle, and a details toggle (`I`).
  - A sheet list with a change count per sheet.
  - Two panes side by side, scrolling together, each named after its file.
- **Grid:**
  - Each pane shows its own row numbers and column letters.
  - Every column has the same width, so that as many as possible fit in a
    pane; a cell too narrow for its text shows all of it as a tooltip.
  - Inserted and deleted rows and columns appear as hatched placeholder rows
    and columns opposite them, and moved rows are marked.
  - Cells are coloured by change kind.
  - Clicking a cell shows its details in two halves, the base workbook's
    under the base pane and the other's under the other pane: the cell's
    address, formula, and shown text on each side, then each change's old
    and new value or formula (with `delta` and `expected`) and changed
    format properties.
  - A changed text or formula has only its changed parts coloured, compared
    word by word, when the two sides share at least half the words of the
    shorter one (by length) and at least three tokens; otherwise each side is coloured
    whole. The grid marks the
    cell, not parts of its text.
  - Cell text uses the display string; fonts and fills are drawn lightly.
- **Size:** the report draws what changed, so that a workbook of large
  sheets still opens and responds in a browser. Drawing every sheet whole
  made a report too heavy to use (seen on a real pair, 2026-11-13).
  - A sheet with no changes has no grid, and neither has one whose changes
    touch none of its rows (a sheet property, an object, the format of an
    empty column); those are listed in the details.
  - A changed sheet of up to 20,000 cells is drawn whole, and "changes
    only" hides its unchanged rows. A larger one has only its changed rows
    and two rows of context each way.
  - A run of more than 20 rows that are all added or all removed is folded
    to five rows at each end and one row that counts the rest. The folded
    rows' content is not in the report.
  - A sheet whose drawn rows still exceed 250,000 cells has no grid; its
    changes are listed.
  - Each sheet waits in a `<template>` and is laid out when first opened.

## Agent skill

`skills/xldif/SKILL.md`, structured like dsdai's `dart-dsd`:
- `SKILL.md` covers when to use the tool, `inspect` first, filtering by
  `--sheet` and `--kind` using `omitted`, reading the envelope, and
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
  - A Windows x64 release (`.github/workflows/release.yml`). Other targets
    were built at first and dropped: nobody uses them.
  - `version` and `upgrade` only if distribution needs them.

## Risks to watch

- **Alignment quality** on sheets without text labels: pure numeric grids
  and pivot-like layouts. Those fall back to value similarity alone, so
  tune and test them specifically in M1.
- **Values hidden under merges:** Excel keeps values in a merged range's
  non-anchor cells (the private corpus has them). The diff must decide
  whether to report changes there, and say they are hidden.
- **Unverified reader facts:** the root element of macro sheets, and
  whether a cell reference naming another row than its `<row>` really wins.
  Confirm each on an Excel-saved file. (Strict-save `t="d"` dates and
  `_xHHHH_` escapes in strings were observed in `fixtures/excel/`; the
  dates were read as text until then.)
- **Format facts:** row `customFormat`, `dxf` `bgColor`, the note Excel
  writes beside a threaded comment, and dynamic-array `cm` were observed in
  the Excel-saved fixtures and are asserted in
  `crates/xldif/tests/excel_saved.rs`, which is the record of what was seen.
- **Formula tokenizer:** edge cases include structured references, external
  workbooks, and names that look like cell references. Unknown tokens pass
  through, and one bad formula never fails a whole comparison.
- **Large workbooks:** the target is two 100k-cell workbooks compared in a
  few seconds. Measure in M1 before optimizing.
