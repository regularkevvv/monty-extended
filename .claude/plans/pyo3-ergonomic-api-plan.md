# PyO3-Style Ergonomic Extension API Plan

## Goal

Provide a declarative, PyO3-inspired authoring experience for native Monty extensions where authors think in **modules**, **classes**, **functions**, and **methods** — while the macro generates the exact same low-level ABI machinery (`StoredObject` enum, handle wrappers, extension struct, `MontyExtension` trait impl, C entry point) behind the scenes.

## Why

The current macro system (`monty_extension`, `monty_handles`) is powerful but exposes implementation details that extension authors shouldn't need to think about:

1. **Manual enum declaration** — authors must define a `StoredObject` enum and annotate it with `#[monty_handles]`.
2. **Manual extension struct** — authors must define a struct with `objects: Mutex<HashMap<u64, StoredObject>>` and `next_id: Mutex<u64>`.
3. **One giant impl block** — all functions AND methods for every class live in a single `#[monty_extension]` impl block, leading to 1000+ line files for large extensions.
4. **Handle-centric vocabulary** — authors work with "handles" and "stored objects" instead of "classes" and "methods".

PyO3 proves that authors think in:

1. **module** — the importable Python package
2. **class** — a Python type with data fields
3. **methods** — behaviour on a class
4. **functions** — top-level module functions

We want that mental model. The macro does the rest.

## Non-Goals

- No changes to Monty VM call semantics.
- No changes to ABI types (`ExtValue`, `ExtArgs`, `ExtManifest`, `MontyExtension`, `ExtHandle`).
- No host-extension behavior changes.
- No breaking changes — the classic macros (`monty_extension`, `monty_handles`, `function`, `method`, `shutdown`) stay active as compatibility aliases.

## Completed: Alias Layer (Phase 1)

Already implemented as a foundation:

- `#[monty_module]` on impl blocks (alias for `#[monty_extension]`)
- `#[monty_classes]` on enums (alias for `#[monty_handles]`)
- `#[monty_function]`, `#[monty_method]`, `#[monty_shutdown]` (aliases for `#[function]`, `#[method]`, `#[shutdown]`)
- Re-exports from `monty-extension-api`
- Smoke tests for both naming styles
- Native extension example + polars-monty migrated to new names

## Completed: Module-Level Macros (Phase 2)

```rust
use monty_extension_api::*;

#[monty_module(
    name = "datatools",
    version = "0.1.0",
    skill = SKILL_TEXT,
    stubs = TYPE_STUBS,
)]
mod datatools_ext {
    use super::*;

    /// A simple in-memory DataFrame with column-oriented storage.
    #[monty_class]
    struct DataFrame {
        columns: Vec<String>,
        data: HashMap<String, Vec<CellValue>>,
        row_count: usize,
    }

    /// Top-level module functions.
    #[monty_function]
    fn parse_csv(ext: &Extension, text: &str) -> Result<DataFrameHandle, ExtError> {
        let mut lines = text.lines();
        let header = lines.next().ok_or_else(|| ExtError::value_error("empty CSV"))?;
        let columns: Vec<String> = header.split(',').map(|c| c.trim().to_string()).collect();
        // ... parse rows ...
        Ok(ext.store_data_frame(DataFrame { columns, data, row_count }))
    }

    #[monty_function(name = "row_count")]
    fn row_count_fn(ext: &Extension, df: DataFrameHandle) -> Result<usize, ExtError> {
        ext.with_data_frame(&df, "row_count", |f| Ok(f.row_count))
    }

    /// Methods on DataFrame handles.
    #[monty_methods]
    impl DataFrame {
        fn row_count(ext: &Extension, df: DataFrameHandle) -> Result<usize, ExtError> {
            ext.with_data_frame(&df, "row_count", |f| Ok(f.row_count))
        }

        fn head(
            ext: &Extension,
            df: DataFrameHandle,
            n: Option<usize>,
        ) -> Result<Vec<HashMap<String, CellValue>>, ExtError> {
            ext.with_data_frame(&df, "head", |f| {
                // ...
                Ok(rows)
            })
        }

        fn filter_gt(
            ext: &Extension,
            df: DataFrameHandle,
            col: &str,
            threshold: f64,
        ) -> Result<DataFrameHandle, ExtError> {
            let filtered = ext.with_data_frame(&df, "filter_gt", |f| {
                // ...
                Ok(new_df)
            })?;
            Ok(ext.store_data_frame(filtered))
        }
    }

    #[monty_shutdown]
    fn shutdown(ext: &Extension) {
        ext.objects.lock().unwrap().clear();
    }
}
```

### What the author writes vs. what the macro generates

| Author writes | Macro generates |
|---|---|
| `#[monty_class] struct DataFrame { ... }` | `enum StoredObject { DataFrame(DataFrame) }` variant, `DataFrameHandle` wrapper, `store_data_frame()`, `with_data_frame()`, `with_data_frame_mut()` |
| (nothing — implicit) | `struct Extension { objects: Mutex<HashMap<u64, StoredObject>>, next_id: Mutex<u64> }` with `Extension::new()` |
| `#[monty_function] fn parse_csv(ext: &Extension, ...)` | Dispatch arm in `MontyExtension::call()`, manifest entry |
| `#[monty_methods] impl DataFrame { fn head(...) }` | Dispatch arm in `MontyExtension::call_method()` matched by `"datatools.DataFrame"` type name |
| `#[monty_shutdown] fn shutdown(ext: &Extension)` | `MontyExtension::shutdown()` body |
| `#[monty_module(...)] mod datatools_ext` | `MontyExtension` trait impl, `monty_extension_entry()` C symbol, `ExtManifest` construction |

### Key design decisions

1. **`Extension` is a generated type.** The user never defines `struct Extension` — the macro creates it with the required `objects` + `next_id` fields. The name `Extension` is a module-local alias that the user references in function signatures.

2. **Functions take `ext: &Extension` as their first parameter.** This replaces the `&self` pattern from the impl-level macro. The macro strips this parameter during dispatch code generation and passes the extension instance instead.

3. **`#[monty_methods] impl DataFrame` uses the struct name to infer the handle type.** The macro knows `DataFrame` is a `#[monty_class]`, so it maps to `"datatools.DataFrame"` type name and routes through `DataFrameHandle` for dispatch. Methods still take `ext: &Extension` and `df: DataFrameHandle` — the `impl DataFrame` block is organizational, not a real inherent impl.

4. **Multiple `#[monty_class]` structs supported.** Each generates its own enum variant, handle wrapper, and store/with helpers. Method dispatch groups by handle type name, same as today.

5. **`use super::*;`** — the mod is a real Rust module. Items defined outside (helper types, constants, etc.) are imported via standard Rust paths.

## Implementation Steps

### 1. New `monty_module` on `mod` in `monty-extension-macros`

The existing `monty_module` on `impl` blocks stays as-is. Add a new code path that detects `mod` items:

```rust
#[proc_macro_attribute]
pub fn monty_module(attr: TokenStream, item: TokenStream) -> TokenStream {
    // Try parsing as ItemImpl first (existing path)
    // If that fails, parse as ItemMod (new path)
}
```

The mod-level expansion:

1. **Parse the module** — collect all items, classify them:
   - `#[monty_class]` structs → class definitions
   - `#[monty_function]` free functions → top-level function exports
   - `#[monty_methods] impl ClassName` blocks → method exports grouped by class
   - `#[monty_shutdown]` function → shutdown hook
   - Everything else → pass through unchanged

2. **Generate the `StoredObject` enum** from all class definitions, same shape as `#[monty_handles]` produces today.

3. **Generate `Extension` struct** with `objects: Mutex<HashMap<u64, StoredObject>>` and `next_id: Mutex<u64>`.

4. **Generate handle wrappers** (`DataFrameHandle`, etc.) with `MontyHandleType`, `TryIntoExtValue`, `FromExtValue` impls — reuse the same generation logic from `expand_handle_variant`.

5. **Generate store/with helpers** on `Extension` — `store_data_frame()`, `with_data_frame()`, `with_data_frame_mut()`.

6. **Generate `MontyExtension` trait impl** on `Extension`:
   - `manifest()` — from module attributes + collected function/method names
   - `call()` — dispatch to `#[monty_function]` free functions, passing `&self` as `ext`
   - `call_method()` — dispatch to `#[monty_methods]` functions, grouped by class type name
   - `shutdown()` — delegate to `#[monty_shutdown]` function

7. **Generate `monty_extension_entry()`** and `__monty_create_extension()`.

8. **Output the module** with all original items (class structs, functions, helper types) plus all generated items.

### 2. Handle `#[monty_class]` as a no-op marker

`#[monty_class]` is consumed by the module-level parser and should be stripped from the output. The struct definition itself passes through unchanged into the generated module.

### 3. Handle `#[monty_methods] impl ClassName`

The `impl ClassName` block is NOT a real inherent impl — it's an organizational container. The macro:

- Strips the `impl` wrapper
- Extracts each method as a free function
- Tags each method with the class name for dispatch grouping
- Validates that the class name matches a `#[monty_class]` struct

### 4. Function signature rewriting

For dispatch, each function's `ext: &Extension` parameter is replaced with `&self` on the generated `Extension` type. The macro handles this during code generation — the user's function bodies remain unchanged because `ext` is just a local binding.

### 5. Tests

Add to `crates/monty-extension-api/tests/`:

- `module_level_smoke.rs` — full module-level macro test with:
  - Single class, multiple functions, multiple methods
  - Multiple classes with separate `#[monty_methods]` blocks
  - Shutdown hook
  - Manifest verification
  - Function dispatch
  - Method dispatch
  - Unknown method errors
  - Optional arguments

### 6. Migrate examples

- Rewrite `examples/native_extension/` to use the new module-level pattern.
- Migrate `polars-monty` to the new pattern (significant cleanup — ~1000 lines become ~500).

## Definition of Done

- `#[monty_module] mod ...` with `#[monty_class]`, `#[monty_function]`, `#[monty_methods]`, `#[monty_shutdown]` compiles and produces identical runtime behavior.
- All existing tests pass unchanged (classic macro style still works).
- New module-level smoke tests pass.
- `examples/native_extension/` uses the new style.
- `polars-monty` migrated and all 17 E2E tests pass.
- No ABI or runtime behavior changes.

## Risk and Mitigation

### Risk: Module parsing complexity

Parsing heterogeneous `mod` items and correlating class structs with their method impl blocks is more complex than parsing a single impl block.

Mitigation:
- Single-pass collection into typed buckets (classes, functions, methods, shutdown, other).
- Emit clear compile errors for unsupported patterns (e.g. `#[monty_methods]` on a non-class type).

### Risk: `Extension` name collisions

The generated `Extension` type lives inside the module, so it shouldn't collide with external names. But users might try to define their own `Extension` type.

Mitigation:
- Use a well-documented name (`Extension`).
- Emit a compile error if the user defines a struct named `Extension` inside the module.
- Consider making the name configurable via attribute argument in the future.

### Risk: Method impl blocks look like inherent impls but aren't

`#[monty_methods] impl DataFrame { fn head(...) }` looks like methods on `DataFrame`, but they're really free functions dispatched through handles. This could confuse authors.

Mitigation:
- Document clearly that `impl DataFrame` is organizational — methods take `ext: &Extension` and handle arguments explicitly.
- The signature makes it obvious: `fn head(ext: &Extension, df: DataFrameHandle, ...)` is not a `&self` method.
- Future phase could explore true `&self` methods where the macro auto-wraps with `ext.with_data_frame()`.

### Risk: Backward compatibility

Old-style macros must keep working.

Mitigation:
- `monty_module` on `impl` blocks still delegates to the existing `expand_monty_extension`.
- `monty_classes` / `monty_handles` on enums still works.
- Classic inner attributes (`#[function]`, `#[method]`, `#[shutdown]`) still recognized.
- All existing tests run unchanged.
