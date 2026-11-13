# TODO

Open work after M1–M5. [PLAN.md](PLAN.md) holds the decisions and
milestones; this file holds what is left. Remove an item when it lands.

## Pending

- **First release.** The maintainer (`sjunepark`, the only actor the
  release workflow accepts) picks the version, sets it in `Cargo.toml`, and
  pushes its `v*` tag ([AGENTS.md](AGENTS.md#release)).

## Features

- **`x14` validation and conditional formatting.** Newer Excel writes some
  data validation and conditional formatting rules in the worksheet's
  extension list (`<extLst>`, `x14:` elements), which xldif does not read
  yet. A `not_compared` warning names the extension list when a workbook
  has one.
- **Tables.** Excel tables (`xl/tables/*.xml`) and their styles are not
  compared; the `not_compared` warning names them.
- **Row labels from date cells.** A row's label is its leftmost text. A row
  keyed by a date (a transaction list) has no label. Date-formatted column
  headers already read as ISO dates.

## Verification

Facts from memory that need an Excel-saved file before they count as
verified. See [PLAN.md](PLAN.md#risks-to-watch) for details:

- reader facts: `_xHHHH_` escapes in `<f>` and `<definedName>` (seen in
  strings only), macro-sheet roots, and a cell reference naming another row
  than its `<row>`;
- built-in number formats 27–36 and 50–58 other than 31 (only 31 has been
  observed in Excel), and the currency, negative-number, and accounting
  formats 5–8 and 37–44 under regional settings other than Korean;
- the `oleObject` and `control` relationship kinds named by the
  `not_compared` warning (`drawing`, `table`, `chartsheet`, `ctrlProp`, and
  `package` were observed in Excel-saved files);
- external links: the target Excel writes for a source outside the
  workbook's folder (only sources in the folder and a subfolder were
  observed, both relative), and DDE and OLE links, which are read as links
  that name no workbook file.
