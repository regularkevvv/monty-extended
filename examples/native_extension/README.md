# Native Extension Example: datatools

Demonstrates building a **native Rust extension** for Monty using the proc-macro authoring API in `monty-extension-api`. The extension provides in-memory CSV/DataFrame operations that execute directly in the VM loop with no suspension.

## What This Shows

1. **`#[monty_handles(...)]`**: Declaring stored object variants once and getting typed handle wrappers plus object-store helpers.
2. **`#[monty_extension(...)]`**: Generating the manifest, top-level dispatch, method dispatch, shutdown wiring, and C ABI entry point.
3. **Typed Rust signatures**: Writing exported functions as normal Rust methods such as `fn parse_csv(&self, text: &str)`.
4. **Opaque extension handles**: DataFrames stay in the extension's memory; sandboxed code only sees lightweight handles.
5. **Type stubs and skill text**: Injected into Monty's type checker and skill collection from the macro declaration.

## Functions

| Function | Description |
|---|---|
| `parse_csv(text)` | Parse CSV text into a DataFrame handle |
| `row_count(df)` | Number of rows |
| `columns(df)` | List of column names |
| `head(df, n=5)` | First N rows as list of dicts |
| `column_sum(df, col)` | Sum of a numeric column |
| `column_mean(df, col)` | Mean of a numeric column |
| `filter_gt(df, col, threshold)` | Filter rows where column > threshold |

## Building

```bash
cd examples/native_extension
cargo build --release
```

This produces `target/release/libmonty_ext_datatools.dylib` (macOS) or `target/release/libmonty_ext_datatools.so` (Linux).

## Usage

The compiled `.so`/`.dylib` is loaded at runtime by passing it to `Monty()` via the `extensions` parameter.

```python
from pydantic_monty import Monty

m = Monty(
    code,
    extensions=[{'library_path': 'path/to/libmonty_ext_datatools.dylib'}],
)
```
