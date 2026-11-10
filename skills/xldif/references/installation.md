# Installing xldif

`xldif` is a single executable with no runtime dependencies; it does not need
Excel.

## From a release

When the repository has published releases, download the archive for the
platform from its Releases page, extract `xldif` (`xldif.exe` on Windows),
and put it on `PATH`. Check with `xldif --version`.

## From source

With a Rust toolchain (1.88 or later):

```bash
cargo install --locked --git https://github.com/cpaikr/xldif xldif-cli
```

or, in a checkout:

```bash
cargo build --release
```

and use `target/release/xldif`.

## After installing

Run `xldif --version`, then resume the task. A comparison failure with an
`error.code` (for example `unsupported_format` for an `.xlsb` file) is not an
installation problem; read its `hint`.
