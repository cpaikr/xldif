# TODO

Open work after M1–M5. [PLAN.md](PLAN.md) holds the decisions and
milestones; this file holds what is left. Remove an item when it lands.

## Pending

- **First release.** The maintainer (`sjunepark`, the only actor the
  release workflow accepts) picks the version, sets it in `Cargo.toml`, and
  pushes its `v*` tag ([AGENTS.md](AGENTS.md#release)). It waits for the
  alignment and comparison-model items below.
- **Comparing a workbook that is open in Excel.** A saved workbook that
  Excel has open already compares (the last saved state). Undecided:
  a warning when a compared file has a `~$` lock file beside it, and
  comparing unsaved edits through an Excel snapshot (`SaveCopyAs`) as part
  of the converter below.

## Review follow-ups

From the project review of 2026-11-11, in order. Feature work waits for
the first two.

1. **Alignment.** Replace the three key-matching passes with one
   cost-model alignment anchored on unique exact rows, and add property
   tests: swapping the files mirrors the result, a workbook against itself
   is empty in JSON and in the HTML report. Known failures on synthetic
   inputs:
   - rows that share a label (a currency or entity column) pair by label
     before content, so one inserted row among edited rows shifts every
     pair below it;
   - when every row has a changed cell, nothing anchors, and a gap of about
     2,000 rows falls back to position;
   - the rarest-key fallback and the similarity backtrack are not symmetric
     under a swap;
   - `--tolerance` does not reach the alignment's tokens, so float noise
     alone can unpair rows;
   - the row and column maps carry a removed blank row onto the row that
     took its place: formulas over it are reported without `expected`, and
     ranges of merges, comments, and rules map there too.
2. **A typed comparison model.** Renderers receive serialized changes with
   string coordinates and the HTML re-derives one-sided rows from the
   workbooks, so it can disagree with the comparison (a row holding only an
   empty string shows as removed and added on identical workbooks). Give
   the renderers a typed model, serialize at the edge, and split `diff.rs`.
   Check whether a moved row and a row-layout change can share a change id.
3. **Excel as converter and oracle.** An optional step that has Excel save
   `.xls`, `.xlsb`, and password-protected inputs as `.xlsx` before
   comparing; the format and colour oracle against Excel's `DisplayFormat`
   that [PLAN.md](PLAN.md#decisions) planned; and an Excel-saved workbook
   with invented content among the public fixtures.
4. **Smaller defects.**
   - Built-in number formats 37–40 are locale-dependent too, and Excel does
     not write a code for them when it saves. Korean Excel shows a bare 37
     as `#,##0;-#,##0` (observed 2026-11-11, with 38–40 alike); xldif shows
     the ECMA-376 code with parentheses, and names that code as the format
     property. Found while fixing ids 5–8 and 41–44, which now render as
     Korean Excel does (`locale_builtin` in
     [numfmt.rs](crates/xldif/src/numfmt.rs)).
   - Between a 1900 and a 1904 workbook, date cells compare by the date
     they show, but the alignment's tokens are still the serials, so equal
     dates do not help pair rows there. It belongs with the alignment item.
   - External links are compared by the file they name in cell formulas
     and defined names, but validation and conditional-format formulas
     still compare the number as written, and so do the alignment's
     formula tokens (a renumbered link makes a formula another token).

## Features

- **`x14` validation and conditional formatting.** Newer Excel writes some
  data validation and conditional formatting rules in the worksheet's
  extension list (`<extLst>`, `x14:` elements), which xldif does not read
  yet. A `not_compared` warning names the extension list when a workbook
  has one.
- **Tables.** Excel tables (`xl/tables/*.xml`) and their styles are not
  compared; the `not_compared` warning names them.
- **`.xlsb` input.** Binary workbooks are rejected with `unsupported_format`
  until the converter above exists.
- **Row labels from date cells.** A row's label is its leftmost text. A row
  keyed by a date (a transaction list) has no label. Date-formatted column
  headers already read as ISO dates.

## Verification

Facts from memory that need an Excel-saved file before they count as
verified. See [PLAN.md](PLAN.md#risks-to-watch) for details:

- reader facts: strict-save `t="d"` dates, `_xHHHH_` escapes in `<f>` and
  `<definedName>`, macro-sheet roots, and a cell reference naming another
  row than its `<row>`;
- format facts: row `customFormat`, `dxf` `bgColor`, the threaded-comment
  fallback, and dynamic-array `cm`;
- built-in number formats 27–36 and 50–58 other than 31 (only 31 has been
  observed in Excel), and the currency and accounting formats 5–8 and 41–44
  under regional settings other than Korean;
- the `oleObject` and `control` relationship kinds named by the
  `not_compared` warning (`drawing`, `table`, `chartsheet`, `ctrlProp`, and
  `package` were observed in Excel-saved files);
- external links: the target Excel writes for a source outside the
  workbook's folder (only sources in the folder and a subfolder were
  observed, both relative), and DDE and OLE links, which are read as links
  that name no workbook file.
