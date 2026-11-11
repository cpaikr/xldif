# TODO

Open work after M1–M5. [PLAN.md](PLAN.md) holds the decisions and
milestones; this file holds what is left. Remove an item when it lands.

## Pending

- **Project review.** The maintainer is assessing the whole project before
  more features land. Feature work below waits for that review.
- **First release.** The maintainer (`sjunepark`, the only actor the
  release workflow accepts) picks the version, sets it in `Cargo.toml`, and
  pushes its `v*` tag ([AGENTS.md](AGENTS.md#release)).

## Features

- **`x14` validation and conditional formatting.** Newer Excel writes some
  data validation and conditional formatting rules in the worksheet's
  extension list (`<extLst>`, `x14:` elements), which xldif does not read
  yet, so changes to those rules go unreported.
- **Table styles.** Excel tables (`xl/tables/*.xml`) and their styles are not
  compared.
- **`.xlsb` input.** Binary workbooks are rejected with `unsupported_format`.
  Add only if real files need it.
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
  observed in Excel).
