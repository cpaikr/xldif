# Pruning saved versions

`xldif series FOLDER` finds workbooks saved as timestamped copies
(`name_YYMMDD_HHMM.xlsx`, optionally with a suffix such as `_review`) and
suggests which versions are worth keeping. It only reports; it never moves or
deletes a file.

## Run it

1. `xldif series FOLDER --format text` for the overview: totals, then one
   table per series with each version's verdict and counts. Add `--recursive`
   when versions sit in subfolders; each folder's versions are their own
   series.
2. Read the JSON (`xldif series FOLDER`) when the counts drive a decision:
   each version has `keep`, `reason`, `edits`, `comparedWith`, `bytes`, and the
   full comparison `summary`.
3. When a version's verdict needs explaining, compare it with the version it
   was measured from: `xldif compare COMPARED_WITH FILE --format text`.

A large workbook takes seconds to compare, so a folder of dozens of
versions takes minutes; run it once and work from its output.

## How versions are judged

Each version is compared with the last version kept, not with the one saved
just before it, so many small saves add up until they amount to a revision.
`edits` counts what a person changed: values typed, changed, or cleared
between constants (`summary.valueInput`), formula changes (a fill down
counted once), and rows, columns, and sheets added, removed, moved, or
renamed. Values that formulas recalculated (`summary.valueComputed`),
formats, and workbook objects are listed in the counts but do not count:
one changed input can change thousands of computed values.

A version is kept when it is the first, the newest readable one, could not be read
(`unreadable`, with its `error`), or has at least `--min-edits` edits
(`edited`; default 200). The rest have reason `fewEdits` and `keep: false`.
`removableBytes` is the space they take. Lock files (`~$name.xlsx`, a
workbook open in Excel) and workbooks without a stamp are listed under
`skipped` and never judged. Names alike apart from case are one series,
and a name's last stamp is its version (`Plan_260101_0900_261123_1102`
belongs to `Plan_260101_0900`).

The default of 200 edits suits a working model saved every few minutes:
small saves fold into the next real revision. Lower it for small workbooks
whose revisions are a handful of figures.

## Before anything is deleted

- Present the suggested removals with their stamps, edits, and the total
  size, and let the user decide. Deleting workpapers is theirs to confirm,
  file by file or as a list they approve.
- Prefer the Recycle Bin or an archive folder over permanent deletion.
- Raise or lower `--min-edits` and rerun when the user wants fewer or more
  versions kept; each threshold gives its own chain of comparisons.
- Point out versions with a suffix (`_review`, `_final`, `_formatted`) before
  suggesting their removal: a person named them, often as a milestone.
- Mention versions whose formats or objects changed a lot while `edits` stayed
  low: a formatting pass may still matter to the user.
- A version that is open in Excel (`open_in_another_program`) was judged as
  last saved; `--unsaved` reads it as Excel holds it now.
