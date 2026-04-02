# monty-extended — Work In Progress

## What is this?

A fork of [Monty](https://github.com/pydantic/monty) that adds a unified extension system. Extensions let sandboxed Python code `import` modules backed by native Rust or host-language (Python/JS) functions — all through the same `import` mechanism.

```python
import polars as pl      # native Rust extension — no VM suspension
import sklearn           # host Python extension — VM suspends, calls back to Python

df = pl.read_csv("x.csv")
model = sklearn.fit("logreg", df)
```

## Current status

**All phases complete. The extension system is fully implemented, macro API finalized, and polars-monty migrated.**

### Latest work

- Implemented the module-level `#[monty_module] mod` macro (Phase 2 of the PyO3-ergonomic API plan) in `crates/monty-extension-macros/`. Authors now write `#[monty_class]` structs, `#[monty_function]` free functions, `#[monty_methods] impl ClassName` blocks, and `#[monty_shutdown]` inside a `mod` block — the macro generates `Extension`, `StoredObject`, typed handles, dispatch, and the C entry point.
- Migrated `examples/native_extension/` to the new module-level pattern.
- Added comprehensive module-level smoke tests (`crates/monty-extension-api/tests/module_level_smoke.rs`).
- **Migrated `../polars-monty/`** from the classic impl-level pattern to the module-level API:
  - Replaced `#[monty_classes]` enum + manual struct + 1077-line `extension_impl.rs` with a single organized `#[monty_module] mod polars_ext { ... }` block
  - 5 newtype wrapper structs with `Deref`/`DerefMut` for transparent access to inner Polars types
  - ~90 dispatch shims grouped by type (`#[monty_methods] impl DataFrame`, etc.)
  - All 17 polars-monty E2E tests and 4 polars-monty-demo sandbox tests pass
- Re-verified the full path with:
  `cargo test -p monty-extension-api`,
  `uv run pytest crates/monty-python/tests/test_extensions.py -q`,
  `uv run pytest examples/native_extension/test_e2e.py -q`,
  `make lint-rs`,
  `../polars-monty/test_e2e.py`,
  and `../polars-monty-demo/test_sandbox.py`.

### Next step

- None — all planned features are implemented and validated.

| Phase | Description | Status |
|-------|-------------|--------|
| Phase 1 | Foundation (registry, opcodes, compiler, VM dispatch) | Complete |
| Phase 2 | Python Integration (PyO3 bindings, MontyModule, HandleStore) | Complete |
| Phase 3 | Resource Enforcement + Type Stubs | Complete |
| Phase 4 | Example Extensions (native datatools + host ML) | Complete |
| Phase 5 | Tests (54 pytest tests, E2E tests for both patterns) | Complete |
| Phase 6 | Real-world validation (polars-monty + pydantic-ai demo) | Complete |
| Phase 7 | Proc-macro ergonomic API (alias layer + module-level macros) | Complete |
| Phase 8 | polars-monty migration to module-level API | Complete |

### What's implemented

| Component | Files |
|---|---|
| `monty-extension-api` crate | `crates/monty-extension-api/src/lib.rs` |
| `monty-extension-macros` proc-macro crate | `crates/monty-extension-macros/src/lib.rs` |
| `ExtensionFunction` Value variant | `crates/monty/src/value.rs` |
| `ExtensionHandle` HeapData variant | `crates/monty/src/heap_data.rs`, `heap.rs` |
| Extension registry | `crates/monty/src/extensions.rs` |
| `LoadExtensionModule` opcode | `crates/monty/src/bytecode/op.rs` |
| Compiler import fallback | `crates/monty/src/bytecode/compiler.rs` |
| VM opcode handler | `crates/monty/src/bytecode/vm/mod.rs` |
| Native call dispatch | `crates/monty/src/bytecode/vm/call.rs` |
| Host call dispatch (via `CallResult::External`) | `crates/monty/src/bytecode/vm/call.rs` |
| Value ↔ ExtValue bridge (incl. Dict) | `crates/monty/src/extensions.rs` |
| Keyword argument forwarding | `crates/monty/src/bytecode/vm/call.rs` |
| Skill collection (`extension_skills()`) | `crates/monty/src/extensions.rs` |
| Registry wired through Executor → Compiler → VM | `crates/monty/src/run.rs`, `run_progress.rs` |
| Registry survives snapshot/resume cycles | `crates/monty/src/run_progress.rs` |
| PyO3 bindings (`extensions=` param, `extension_skills()`) | `crates/monty-python/src/monty_cls.rs` |
| Host extension dispatch in Python run loop | `crates/monty-python/src/monty_cls.rs` |
| `MontyModule` decorator framework | `crates/monty-python/python/pydantic_monty/decorators.py` |
| `HandleStore` for opaque objects | `crates/monty-python/python/pydantic_monty/handles.py` |
| Enforcement wrappers (timeout, size, call count) | `crates/monty-python/python/pydantic_monty/enforcement.py` |
| Native call resource budgets | `crates/monty/src/resource.rs`, `crates/monty/src/extensions.rs` |
| Post-native-call time check | `crates/monty/src/bytecode/vm/call.rs` |
| Type stub collection + injection | `crates/monty/src/extensions.rs`, `monty_cls.rs` |
| Handle method calls (native + host) | `heap_data.rs`, `call.rs`, `extensions.rs`, `object.rs` |
| `MontyObject::ExtensionHandle` bridge variant | `object.rs`, `monty_cls.rs`, `convert.rs` |
| Native extension loading from Python (`library_path`) | `crates/monty-python/src/monty_cls.rs` |
| Async dispatch loop `ext:*` support | `crates/monty-python/src/async_dispatch.rs` |
| REPL extension support | `crates/monty/src/repl.rs`, `crates/monty-python/src/repl.rs` |
| Native extension example (datatools) | `examples/native_extension/` |
| Host extension example (ml) | `examples/host_extension/` |

### External validation

- **polars-monty** (`../polars-monty/`) — Native Rust extension wrapping the real Polars DataFrame library (45MB cdylib). Uses the module-level `#[monty_module] mod` API with newtype wrappers around Polars types. 17 E2E tests covering DataFrames, Series, expressions, group-by, joins, CSV I/O.
- **polars-monty-demo** (`../polars-monty-demo/`) — pydantic-ai agent that analyzes data using Polars inside the Monty sandbox. Demonstrates the full stack: LLM generates code → sandbox executes with Polars → results returned. 4 sandbox tests.

### Known TODOs

- None — all planned features are implemented and validated.

## Architecture

See `EXTENSIONS_ARCHITECTURE.md` for the full extension system design document.
See `.claude/plans/implementation-plan.md` for the original implementation plan.

## Usage Example

```python
from pydantic_monty import Monty, MontyModule

# Define a host extension
mymath = MontyModule('mymath', skill='# MyMath\nProvides add(a, b)')

@mymath.function()
def add(a: int, b: int) -> int:
    return a + b

# Create Monty with the extension
m = Monty(
    'import mymath\nresult = mymath.add(1, 2)',
    extensions=[mymath.to_extension_dict()],
)
result = m.run()  # returns 3

# Get skills for AI agent prompts
skills = m.extension_skills()  # '# MyMath\nProvides add(a, b)'
```

### Native extension (compiled .so/.dylib)

Load from Python:
```python
from pydantic_monty import Monty

m = Monty(
    'import polars as pl\ndf = pl.read_csv("a,b\\n1,2")\ndf.height()',
    extensions=[{
        'module_name': 'polars',
        'library_path': '/path/to/libpolars_monty.dylib',
    }],
)
result = m.run()  # returns 2
```

Author in Rust (module-level style — recommended):
```rust
use monty_extension_api::*;

#[monty_module(name = "myext", version = "0.1.0")]
mod myext {
    use monty_extension_api::{ExtArgs, ExtContext, ExtError, ExtValue};

    #[monty_class]
    struct Item(pub(crate) String);

    #[monty_function()]
    fn create(ext: &Extension, args: &ExtArgs, _ctx: &ExtContext) -> Result<ExtValue, ExtError> {
        let name = args.expect_str(0, "create", "name")?;
        let h = ext.store_item(Item(name.to_string()));
        Ok(h.try_into_ext_value()?)
    }

    #[monty_methods]
    impl Item {
        #[monty_method(name = "name")]
        fn item_name(ext: &Extension, h: ItemHandle, _a: &ExtArgs, _c: &ExtContext) -> Result<ExtValue, ExtError> {
            ext.with_item(h, "name", |item| Ok(ExtValue::Str(item.0.as_str().into())))
        }
    }

    #[monty_shutdown()]
    fn shutdown(ext: &Extension) { ext.objects.lock().unwrap().clear(); }
}
```
