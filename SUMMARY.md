# Summary of Extension System Work

This document summarizes all work done to add a unified extension system to Monty.

## Goal

Enable sandboxed Python code running in Monty to `import` modules backed by either native Rust code or host-language (Python/JS) functions, all through the standard `import` mechanism.

## What was built

### Phase 1: Foundation (Rust core)

Built the core extension infrastructure inside the Monty VM:

- **`monty-extension-api` crate** — A standalone, ABI-stable public API for extension authors. Uses `abi_stable` for C-compatible types (`ExtValue`, `ExtArgs`, `ExtResult`, `ExtHandle`, `ExtManifest`) and the `MontyExtension` trait. Extensions depend only on this crate — never on Monty internals.

- **`ExtensionRegistry`** — Central multiplexer in `crates/monty/src/extensions.rs`. Stores both native and host extensions, handles name interning (before compilation), module creation (at runtime), and value conversion between Monty's `Value` and the ABI-stable `ExtValue`.

- **`LoadExtensionModule` opcode** — New bytecode opcode. The compiler queries the registry during import resolution; if a module name matches a registered extension, it emits this opcode instead of `RaiseImportError`.

- **`Value::ExtensionFunction`** — New inline value variant carrying a packed `ExtensionFunctionId` (registry index, function index, interned name, is_native flag).

- **`HeapData::ExtensionHandle`** — Heap-allocated variant for opaque handles to extension-managed objects (DataFrames, models, etc.).

- **Call dispatch** — In `crates/monty/src/bytecode/vm/call.rs`:
  - **Native path**: Convert args → `ExtValue`, call `extension.call()`, convert result back. No VM suspension.
  - **Host path**: Return `CallResult::External("ext:<idx>:<name>", args)`. VM suspends, host language dispatches the call, resumes with the result.

- **Method dispatch** — `call_extension_method()` handles `handle.method(args)` syntax for both native and host extensions.

- **Registry wiring** — Threaded through `Executor → Compiler → VM` in `run.rs` and `run_progress.rs`. Registry survives snapshot/resume cycles (skipped during serialization, re-attached on resume).

### Phase 2: Python Integration

Built the Python-facing API in `crates/monty-python/`:

- **`extensions=` parameter** on `Monty()` — Accepts a list of extension dicts. `build_extension_registry()` processes each, supporting both native (`library_path` key) and host (function dicts with callables) extensions.

- **Native extension loading** — `load_native_extension()` uses `libloading` to load `.so`/`.dylib` files, look up `monty_extension_entry`, validate the API version, and register the trait object.

- **Host extension dispatch** — `dispatch_host_extension_call()` handles VM suspension: parses the `ext:<idx>:<name>` prefix, looks up the Python callable, converts args, calls Python, converts results back.

- **`MontyModule` decorator** (`decorators.py`) — Pythonic API for defining host extensions:
  ```python
  ext = MontyModule('mymath', skill='...')
  @ext.function()
  def add(a: int, b: int) -> int: return a + b
  ```

- **`HandleStore`** (`handles.py`) — Thread-safe Python-side store for opaque objects. Returns handle dicts that get converted to `ExtensionHandle` values in the VM.

- **`MontyObject::ExtensionHandle`** — Bridge variant in `object.rs` for passing extension handles between Python and the VM. `maybe_convert_handle_dict()` detects handle-shaped dicts from host callables and promotes them.

- **Updated exports and type stubs** — `__init__.py`, `_monty.pyi`

### Phase 3: Resource Enforcement + Type Stubs

- **Native call budgets** — `ExtContext` carries `ResourceBudget` (remaining time, remaining allocations) to extension calls. Post-call time check in the VM.

- **Host call enforcement** (`enforcement.py`) — Three decorator wrappers:
  - `enforce_timeout(ms)` — Runs function in daemon thread with deadline
  - `enforce_size(max_bytes)` — Checks `sys.getsizeof(result)` after return
  - `enforce_call_count(n)` — Thread-safe counter, raises when exhausted

- **Type stub injection** — Extensions provide stub source in their manifest. Collected by `extension_type_stubs()` and injected into the type checker.

- **Skill collection** — `extension_skills()` collects markdown documentation from all extensions for AI agent prompt injection.

### Phase 4: Example Extensions

- **Native extension** (`examples/native_extension/`) — `datatools` module with CSV parsing, DataFrame operations (row_count, columns, head, column_sum, column_mean, filter_gt). Demonstrates handles, method dispatch, and the full native pattern.

- **Host extension** (`examples/host_extension/`) — `ml` module with model fitting, prediction, and stateful handles via `HandleStore`. Demonstrates VM suspension, host dispatch, and handle method calls.

### Phase 5: Tests

- **54 formal pytest tests** (`crates/monty-python/tests/test_extensions.py`) — Covers: native function calls, host function calls, skills, type stubs, enforcement (timeout, size, call count), handle methods, error propagation, keyword args, nested data, mixed native+host, async dispatch, REPL support.

- **Native E2E tests** (`examples/native_extension/test_e2e.py`) — 9 tests

- **Host E2E tests** (`examples/host_extension/test_e2e.py`) — 11 tests

### Phase 6: Real-world Validation

- **`polars-monty`** (`../polars-monty/`) — Native Rust extension wrapping the real Polars DataFrame library. 45MB cdylib linking against `polars = "0.46"`. Implements comprehensive API surface: `read_csv`, `DataFrame`, `Series`, expressions (`col`, `lit`, arithmetic, comparisons, aggregations), `group_by().agg()`, `join`, `sort`, `filter`, `with_columns`, `concat`, lazy API, and more. 17 E2E tests pass.

- **`polars-monty-demo`** (`../polars-monty-demo/`) — pydantic-ai agent demo. A uv Python project with:
  - `main.py` — Agent with `run_code` tool that executes Python in the Monty sandbox with Polars
  - System prompt includes Polars API skills extracted from the extension
  - Agent generates Polars code, sandbox executes it at native Rust speed, results returned
  - Sample sales dataset for demo analysis

### Phase 7: Proc-Macro Ergonomic API

Built a PyO3-inspired declarative authoring experience for native extensions via `crates/monty-extension-macros/`.

- **Phase 7a: Alias layer** — `#[monty_module]`, `#[monty_classes]`, `#[monty_function]`, `#[monty_method]`, `#[monty_shutdown]` as semantic aliases for the classic `#[monty_extension]`/`#[monty_handles]`/`#[function]`/`#[method]`/`#[shutdown]` names. Re-exports from `monty-extension-api`. Smoke tests for both naming styles.

- **Phase 7b: Module-level macros** — `#[monty_module]` on a `mod` block with `#[monty_class]` structs, `#[monty_function]` free functions, `#[monty_methods] impl ClassName` blocks, and `#[monty_shutdown]`. The macro auto-generates:
  - `StoredObject` enum (one variant per class)
  - `Extension` struct with `objects: Mutex<HashMap<u64, StoredObject>>` + `next_id: Mutex<u64>`
  - Typed handle wrappers (`DataFrameHandle`, etc.) with `TryIntoExtValue`/`FromExtValue` impls
  - `store_*`, `with_*`, `with_*_mut` helper methods on `Extension`
  - `MontyExtension` trait impl with function/method dispatch
  - `monty_extension_entry()` C ABI entry point
  - Both classic (impl-level) and new (mod-level) styles produce identical runtime behavior

- **Module-level smoke tests** (`crates/monty-extension-api/tests/module_level_smoke.rs`) — Covers manifest generation, function dispatch, optional arguments, handle methods, multiple classes, method name overrides, error handling, and shutdown.

- **Migrated `examples/native_extension/`** to the new module-level pattern.

### Phase 8: polars-monty Migration to Module-Level API

Migrated `../polars-monty/` from the classic impl-level pattern to the new module-level `#[monty_module] mod` pattern:

- **`src/lib.rs`** — Replaced `#[monty_classes]` enum + manual `PolarsExtension` struct + `mod extension_impl` with a single `#[monty_module] mod polars_ext { ... }` block containing:
  - 5 newtype wrapper structs with `#[monty_class]` + `Deref`/`DerefMut` (`DataFrame`, `Series`, `Expr`, `LazyFrame`, `GroupBy`)
  - ~90 dispatch shims organized by type: `#[monty_methods] impl DataFrame`, `impl Series`, `impl Expr`, `impl LazyFrame`, `impl GroupBy`
  - Module functions (`read_csv`, `DataFrame`, `Series`, `col`, `lit`, aggregations, `concat`)
  - Re-exports `Extension as PolarsExtension` and `StoredObject` so internal modules needed minimal changes
- **Deleted `src/extension_impl.rs`** — 1077-line file replaced by the organized mod block
- **Internal modules** (`dataframe.rs`, `series.rs`, `expr.rs`, `dispatch.rs`, `helpers.rs`) — Updated `StoredObject` pattern matches to access inner types via `.0` on the newtype wrappers
- All 17 polars-monty E2E tests pass, all 4 polars-monty-demo sandbox tests pass

### Phase 9: Hiding `abi_stable` from Extension Authors

Eliminated the need for extension authors to depend on `abi_stable` directly. Previously, extensions that built `ExtValue` variants manually (e.g. `ExtValue::Str(RString::from(...))`) had to import `abi_stable::std_types::{RString, RVec}` and list `abi_stable` in their `Cargo.toml`. Now the extension API provides higher-level constructors that hide the ABI-stable types entirely.

- **`ExtValue` constructors** — `ExtValue::string(s)`, `ExtValue::list(items)`, `ExtValue::dict(pairs)` accept standard Rust types (`impl Into<String>`, `impl IntoIterator<Item = ExtValue>`, etc.) and handle `RString`/`RVec` conversion internally.

- **`ExtValue` accessors** — `as_str()`, `as_int()`, `as_float()`, `as_bool()` for reading values without pattern-matching against ABI types.

- **`From` impls on `ExtValue`** — `From<bool>`, `From<i32>`, `From<i64>`, `From<u32>`, `From<f32>`, `From<f64>`, `From<&str>`, `From<String>`, `From<Vec<ExtValue>>`, `From<Vec<ExtKeyValue>>` for idiomatic Rust construction.

- **`ExtKeyValue::new(key, value)`** — Constructor accepting `impl Into<String>` so extension authors never construct `RString` by hand.

- **Migrated `polars-monty`** — Replaced all direct `abi_stable` usage across 4 source files (`convert.rs`, `helpers.rs`, `dataframe.rs`, `series.rs`). Also replaced manual `ExtError` struct construction with the existing `ExtError::value_error()` / `ExtError::type_error()` / `ExtError::attribute_error()` constructors. Removed `abi_stable = "0.11"` from `polars-monty/Cargo.toml`. All 17 E2E tests pass.

- **Extension `Cargo.toml`** now only needs:
  ```toml
  [dependencies]
  monty-extension-api = "0.1"
  ```
  No `abi_stable` dependency required. The `__private` module in `monty-extension-api` re-exports what the generated (macro) code needs, but extension authors never interact with it.

## Other changes

- **Async dispatch** — `async_dispatch.rs` updated to handle `ext:*` prefix for host extension calls in async execution mode.
- **REPL support** — Both `crates/monty/src/repl.rs` and `crates/monty-python/src/repl.rs` updated to support extensions in interactive mode.
- **JS bindings** — `crates/monty-js/src/convert.rs` updated to handle `MontyObject::ExtensionHandle`.
- **`.gitignore`** — Updated to exclude `.DS_Store`, `__pycache__/`, `**/target/`.

## Files changed/added

### New files
| File | Purpose |
|------|---------|
| `crates/monty-extension-api/Cargo.toml` | Extension API crate manifest |
| `crates/monty-extension-api/src/lib.rs` | ABI-stable extension API + macro re-exports |
| `crates/monty-extension-macros/Cargo.toml` | Proc-macro crate manifest |
| `crates/monty-extension-macros/src/lib.rs` | Proc macros: `monty_module`, `monty_class`, `monty_methods`, etc. |
| `crates/monty-extension-api/tests/module_level_smoke.rs` | Module-level macro smoke tests |
| `crates/monty/src/extensions.rs` | Extension registry + value bridge |
| `crates/monty-python/python/pydantic_monty/decorators.py` | MontyModule decorator |
| `crates/monty-python/python/pydantic_monty/handles.py` | HandleStore for host objects |
| `crates/monty-python/python/pydantic_monty/enforcement.py` | Timeout/size/call-count enforcement |
| `examples/native_extension/` | Native extension example (module-level API) + E2E tests |
| `examples/host_extension/` | Host extension example + E2E tests |
| `crates/monty-python/tests/test_extensions.py` | 54 formal pytest tests |
| `EXTENSIONS_ARCHITECTURE.md` | Extension system architecture doc |
| `SUMMARY.md` | This file |

### Modified files
| File | Changes |
|------|---------|
| `Cargo.toml` (workspace) | Added `monty-extension-api` member |
| `Cargo.lock` | Updated dependencies |
| `crates/monty/Cargo.toml` | Added `monty-extension-api` + `libloading` deps |
| `crates/monty/src/lib.rs` | Added `extensions` module |
| `crates/monty/src/value.rs` | Added `ExtensionFunction` variant |
| `crates/monty/src/heap_data.rs` | Added `ExtensionHandle` variant + `py_call_attr` dispatch |
| `crates/monty/src/heap.rs` | Handle `ExtensionHandle` in heap operations |
| `crates/monty/src/object.rs` | Added `MontyObject::ExtensionHandle` variant |
| `crates/monty/src/bytecode/op.rs` | Added `LoadExtensionModule` opcode |
| `crates/monty/src/bytecode/compiler.rs` | Extension import resolution |
| `crates/monty/src/bytecode/vm/mod.rs` | `LoadExtensionModule` handler |
| `crates/monty/src/bytecode/vm/call.rs` | Extension call + method dispatch |
| `crates/monty/src/run.rs` | Registry wiring through executor |
| `crates/monty/src/run_progress.rs` | Registry in snapshot/resume |
| `crates/monty/src/resource.rs` | Budget extraction for extensions |
| `crates/monty/src/types/type.rs` | Extension type support |
| `crates/monty-python/Cargo.toml` | Added `libloading` dep |
| `crates/monty-python/src/monty_cls.rs` | Extension bindings, native loading, host dispatch |
| `crates/monty-python/python/pydantic_monty/__init__.py` | Updated exports |
| `crates/monty-python/python/pydantic_monty/_monty.pyi` | Updated type stubs |

## Current state

Everything is working. All tests pass (924 core test cases, 54 extension pytest tests, 17 polars E2E tests, 4 polars-monty-demo tests). The extension system and proc-macro API are both ready for production use. Both authoring styles (module-level and impl-level) produce identical runtime behavior. Extension authors depend only on `monty-extension-api` — `abi_stable` is fully hidden behind the API and macro layers.
