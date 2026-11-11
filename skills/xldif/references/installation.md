# Installing xldif

`xldif` is a single executable with no runtime dependencies; it does not need
Excel.

## From a release

Releases carry one archive, for Windows x64. When the repository has
published releases, download it from the Releases page, extract
`xldif.exe`, and put it on `PATH`. Check with `xldif --version`. On other
platforms, build from source.

## From source

With a Rust toolchain (1.88 or later):

```bash
cargo install --locked --git https://github.com/cpaikr/xldif xldif-cli
```

or, in a checkout:

```bash
cargo build --release
```

and use `target/release/xldif` (`xldif.exe` on Windows).

## After installing

Run `xldif --version`, then resume the task. A comparison failure with an
`error.code` (for example `unsupported_format` for an `.xlsb` file on a
machine without Excel) is not an installation problem; read its `hint`.
