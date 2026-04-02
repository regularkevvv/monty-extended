# Extensions Architecture

This document describes the architecture of Monty's extension system — how sandboxed Python code can `import` modules backed by native Rust or host-language (Python/JS) functions.

## Overview

The extension system has two execution paths that share the same import mechanism:

```
┌──────────────────────────────────────────────────────┐
│  Sandboxed Python Code                               │
│  import polars as pl                                 │
│  df = pl.read_csv(data)     # native — instant       │
│  model = ml.fit(data)       # host — VM suspends     │
└───────────┬──────────────────────────┬───────────────┘
            │                          │
     ┌──────▼──────┐          ┌───────▼────────┐
     │ Native Path │          │  Host Path     │
     │ Rust → Rust │          │ Rust → Python  │
     │ No suspend  │          │ VM suspends    │
     │ Full speed  │          │ Host dispatches│
     └─────────────┘          └────────────────┘
```

**Native extensions** are compiled Rust `cdylib` libraries (`.so`/`.dylib`) loaded at runtime. Function calls execute directly in Rust — no VM suspension, no serialization overhead for handles.

**Host extensions** are Python (or JS) functions registered with the VM. When called, the VM suspends, yields to the host language, the host executes the function, and resumes the VM with the result.

Both paths produce the same result from the sandboxed code's perspective: an importable module with callable functions that can return primitive values or opaque handles.

## Architecture Layers

```
┌─────────────────────────────────────────────────────────────┐
│  Layer 1: Extension API (monty-extension-api crate)         │
│  ABI-stable types and traits for extension authors          │
├─────────────────────────────────────────────────────────────┤
│  Layer 2: Registry & Value Bridge (monty/src/extensions.rs) │
│  Central multiplexer, name interning, value conversion      │
├─────────────────────────────────────────────────────────────┤
│  Layer 3: Bytecode (compiler.rs, op.rs)                     │
│  Import resolution, LoadExtensionModule opcode              │
├─────────────────────────────────────────────────────────────┤
│  Layer 4: Call Dispatch (vm/call.rs)                         │
│  Native calls (direct) and host calls (suspension)          │
├─────────────────────────────────────────────────────────────┤
│  Layer 5: Value Representation (value.rs, heap_data.rs)     │
│  ExtensionFunction, ExtensionHandle, MontyObject bridge     │
├─────────────────────────────────────────────────────────────┤
│  Layer 6: Execution Engine (run.rs, run_progress.rs)        │
│  Registry wiring, snapshot/resume for host calls            │
├─────────────────────────────────────────────────────────────┤
│  Layer 7: Python Bindings (monty-python/src/monty_cls.rs)   │
│  Native loading, host dispatch, handle detection            │
├─────────────────────────────────────────────────────────────┤
│  Layer 8: Python Helpers (decorators.py, handles.py, etc.)  │
│  MontyModule, HandleStore, enforcement wrappers             │
└─────────────────────────────────────────────────────────────┘
```

## Layer 1: Extension API

**Crates**: `crates/monty-extension-api/`, `crates/monty-extension-macros/`

The public, ABI-stable API that extension authors depend on. Uses `abi_stable` internally for C-compatible types and `monty-extension-macros` for proc-macro code generation. Extension authors never import `abi_stable` directly — all ABI details are hidden behind higher-level constructors, `From` impls, and the `__private` re-export module used only by generated code.

Two authoring styles are supported (see "Writing a Native Extension" below):

- **Module-level (recommended)**: `#[monty_module]` on a `mod` block with `#[monty_class]`, `#[monty_function]`, `#[monty_methods]`, `#[monty_shutdown]`. The macro generates `Extension`, `StoredObject`, typed handles, store/with helpers, the `MontyExtension` trait impl, and the C entry point.
- **Impl-level (classic)**: `#[monty_classes]` / `#[monty_handles]` on an enum, `#[monty_module]` / `#[monty_extension]` on an impl block. Still fully supported.

### Core types

| Type | Purpose |
|------|---------|
| `ExtValue` | ABI-stable mirror of Monty's `Value`. Variants: `None`, `Bool`, `Int`, `Float`, `Str`, `Bytes`, `List`, `Dict`, `Handle`. Provides `string()`, `list()`, `dict()` constructors, `From` impls for common Rust types, and `as_str()`/`as_int()`/`as_float()`/`as_bool()` accessors. |
| `ExtKeyValue` | Key-value pair for dicts and keyword arguments. Use `ExtKeyValue::new(key, value)` to construct. |
| `ExtHandle` | Opaque reference to an extension-managed object. Contains `type_name`, `handle_id`, and `extension_id`. |
| `ExtArgs` | Positional + keyword arguments with typed extraction via `get()`, `argument()`, `optional_argument()`, `kwarg()`. |
| `ExtError` | Exception with `exception_type` and `message`. Use constructors: `value_error()`, `type_error()`, `key_error()`, `attribute_error()`, `missing_argument()`, `argument_type()`, `invalid_handle()`. |
| `ExtResult` | ABI-stable `Result<ExtValue, ExtError>` — the return type for all extension calls. |
| `ExtContext` | Execution context with `ResourceBudget` (remaining time, remaining allocations). |
| `ExtManifest` | Module metadata: name, function declarations, type stubs, skill text, version. |
| `ExtFunctionDecl` | Declares a function with `name` and `is_native` flag. |
| `ExtensionEntry` | C-ABI entry point struct with `api_version` and factory function. |

### Ergonomic value construction

Extension authors construct `ExtValue` instances using standard Rust types — the API handles conversion to ABI-stable types internally:

```rust
// String values — no RString needed
ExtValue::string("hello")
ExtValue::from("hello")
ExtValue::from(format!("count: {n}"))

// Numeric values
ExtValue::from(42_i64)
ExtValue::from(3.14_f64)
ExtValue::from(true)

// Lists — no RVec needed
ExtValue::list(vec![ExtValue::from(1), ExtValue::from(2)])
ExtValue::list(items.iter().map(|x| ExtValue::from(*x)))

// Dicts — no RString/RVec needed
ExtValue::dict(vec![
    ExtKeyValue::new("name", ExtValue::from("Alice")),
    ExtKeyValue::new("age", ExtValue::from(30)),
])

// Reading values
if let Some(s) = value.as_str() { /* ... */ }
if let Some(n) = value.as_int() { /* ... */ }

// Errors — use provided constructors
ExtError::value_error("something went wrong")
ExtError::type_error(format!("{func}() expected a str"))
ExtError::attribute_error("DataFrame", "no_such_method")
```

### MontyExtension trait

```rust
#[sabi_trait]
pub trait MontyExtension: Send + Sync {
    fn manifest(&self) -> ExtManifest;
    fn call(&self, function_name: RStr<'_>, args: ExtArgs, ctx: &ExtContext) -> ExtResult;
    fn call_method(&self, handle: &ExtHandle, method: RStr<'_>, args: ExtArgs, ctx: &ExtContext) -> ExtResult;
    fn shutdown(&self);
}
```

Every native extension implements this trait. The `abi_stable` crate generates an ABI-safe trait object wrapper (`MontyExtension_TO`) for cross-library dispatch.

### Entry point convention

Every native extension exports a single C function:

```rust
#[unsafe(no_mangle)]
pub extern "C" fn monty_extension_entry() -> ExtensionEntry {
    ExtensionEntry {
        api_version: API_VERSION,
        create: create_extension,  // factory function
    }
}
```

## Layer 2: Extension Registry

**File**: `crates/monty/src/extensions.rs`

The registry is the central hub connecting extensions to the VM.

### Data structures

```
ExtensionRegistry
├── entries: Vec<ExtensionEntry>          // All registered extensions
│   └── ExtensionEntry
│       ├── manifest: ExtManifest         // Module metadata
│       ├── native: Option<TraitObject>   // Native extension (None for host)
│       └── library: Option<Library>      // Keeps .so/.dylib alive
├── name_to_index: HashMap<String, u16>   // Module name → registry index
└── interned: Vec<InternedExtension>      // Pre-interned string IDs
    └── InternedExtension
        ├── module_name: StringId
        └── function_names: Vec<StringId>
```

### Key operations

1. **`register_native(extension, library)`** — Stores the trait object and library handle. Returns the registry index.

2. **`register_host(manifest)`** — Stores the manifest only (no trait object). Returns the registry index.

3. **`intern_names(intern_table)`** — Called before compilation. Pre-interns all module and function names so the compiler and runtime can use `StringId`s without mutable access to the intern table.

4. **`lookup(module_name)`** — Called by the compiler during import resolution. Returns `Some(registry_index)` if the module is a registered extension.

5. **`create_module(registry_index, heap, interns)`** — Called at runtime by the `LoadExtensionModule` opcode. Allocates a `Module` on the heap and populates it with `Value::ExtensionFunction` entries for each function in the manifest.

6. **`call_native(registry_index, function_index, ext_args, ctx)`** — For native calls: delegates to `extension.call()`.

7. **`call_method_native(handle_data, method_name, ext_args, budget)`** — For native method calls: constructs an `ExtHandle` from the stored handle data and delegates to `extension.call_method()`.

### Value bridge

Two conversion functions handle the boundary:

- **`value_to_ext(value, heap, interns)`** — Converts Monty `Value` → `ExtValue` for passing to extensions. Handles primitives, interned/heap strings, lists, dicts, and handles.

- **`ext_to_value(ext_value, heap, interns)`** — Converts `ExtValue` → Monty `Value` after extensions return. Allocates heap space for complex types. Converts `ExtHandle` → `HeapData::ExtensionHandle`.

## Layer 3: Bytecode

### Compiler (`crates/monty/src/bytecode/compiler.rs`)

The compiler receives an optional `&ExtensionRegistry` reference. During import resolution:

```
compile_import("polars")
  → self.lookup_extension("polars")
    → registry.lookup("polars")
      → Some(0)  // registry index
  → emit Opcode::LoadExtensionModule(0)
```

If the module is not found in the registry (and is not a builtin), the compiler emits `RaiseImportError` — but this is deferred to runtime, allowing `if TYPE_CHECKING:` imports to compile without errors.

### Opcode (`crates/monty/src/bytecode/op.rs`)

```
LoadExtensionModule(u16)  // operand is the registry index
```

## Layer 4: Call Dispatch

**File**: `crates/monty/src/bytecode/vm/call.rs`

### Function calls

When `Value::ExtensionFunction(ef)` is called:

```
if ef.is_native:
    1. Convert each arg: value_to_ext()
    2. Build ResourceBudget from tracker
    3. registry.call_native(ef.registry_index, ef.function_index, ext_args, ctx)
    4. Convert result: ext_to_value()
    5. Check time limits (extension may have consumed wall-clock time)
    → CallResult::Value(result)

else (host):
    1. Build prefixed name: "ext:<registry_index>:<function_name>"
    2. Package args as-is
    → CallResult::External(prefixed_name, args)
    (VM suspends, host handles the call)
```

### Method calls

When sandboxed code calls `handle.method(args)`:

1. `py_call_attr()` on `HeapReadOutput::ExtensionHandle` extracts the handle data
2. Calls `vm.call_extension_method(handle_data, handle_heap_id, method_name, args)`
3. Dispatch follows the same native/host split as function calls
4. For host methods, the handle is prepended to the positional args so the host can identify which object is being called

## Layer 5: Value Representation

### Stack values

```rust
enum Value {
    // ... existing variants ...
    ExtensionFunction(ExtensionFunctionId),  // inline, 8 bytes
}

struct ExtensionFunctionId {
    registry_index: u16,   // which extension
    function_index: u16,   // which function within the extension
    function_name: StringId, // interned name (for error messages + host dispatch)
    is_native: bool,       // native (Rust) or host (Python/JS)
}
```

### Heap data

```rust
enum HeapData {
    // ... existing variants ...
    ExtensionHandle(ExtensionHandleData),
}

struct ExtensionHandleData {
    registry_index: u16,   // identifies the owning extension
    type_name: String,     // e.g. "polars.DataFrame"
    handle_id: u64,        // extension-internal ID (opaque to Monty)
}
```

### Bridge type

```rust
enum MontyObject {
    // ... existing variants ...
    ExtensionHandle {
        registry_index: u16,
        type_name: String,
        handle_id: u64,
    },
}
```

Used for serialization between the VM and host language bindings (Python/JS). Converted to `HeapData::ExtensionHandle` when loaded into the VM.

## Layer 6: Execution Engine

### Wiring (`crates/monty/src/run.rs`)

```
Monty(code, extensions=[...])
  → build_extension_registry(extensions)
    → registry.intern_names(intern_table)  // before compilation
    → Compiler::new(registry)              // import resolution
    → VM::new(registry)                    // runtime dispatch
```

### Snapshot/resume (`crates/monty/src/run_progress.rs`)

The registry is `#[serde(skip)]` — it contains live trait objects and library handles that can't be serialized. On resume, the registry must be re-attached.

When a host extension call suspends the VM:

```
VM executes → CallResult::External("ext:0:predict", args)
  → RunProgress::FunctionCall {
      function_name: "ext:0:predict",
      args: [MontyObject::...],
      snapshot: Snapshot { heap, globals, locals, stack },
    }
  → Host handles the call
  → resume(ExtFunctionResult::Return(result))
  → VM continues from the suspension point
```

## Layer 7: Python Bindings

**File**: `crates/monty-python/src/monty_cls.rs`

### Extension registration

`build_extension_registry()` processes the `extensions` list from `Monty(extensions=[...])`:

**For native extensions** (dict has `library_path` key):
```python
{'module_name': 'polars', 'library_path': '/path/to/libpolars_monty.dylib'}
```
1. Load `.so`/`.dylib` with `libloading::Library::new()`
2. Look up `monty_extension_entry` symbol
3. Call factory function to get trait object
4. Validate `api_version`
5. Register with `registry.register_native()`

**For host extensions** (dict has `functions` and `callables` keys):
```python
{'module_name': 'mymath', 'functions': [...], 'callables': {'add': <fn>}, ...}
```
1. Build `ExtManifest` from dict fields
2. Register with `registry.register_host()`
3. Store callables in `host_extension_callables` map keyed by `"ext:<idx>:<name>"`

### Host call dispatch

`dispatch_host_extension_call()` handles VM suspension:

1. Parse registry index from `"ext:<idx>:<name>"` prefix
2. Look up Python callable in `host_extension_callables`
3. Convert `MontyObject` args → Python objects
4. Call the Python function
5. Convert result → `MontyObject`
6. **Handle detection**: `maybe_convert_handle_dict()` checks if the result is a dict with `handle_id`/`type_name`/`extension_id` keys — if so, promotes to `MontyObject::ExtensionHandle` to enable method syntax in the sandbox

### Async and REPL support

- `async_dispatch.rs` — Handles `ext:*` calls in async execution mode
- `repl.rs` — Supports extensions in interactive REPL mode

## Layer 8: Python Helpers

### MontyModule (`decorators.py`)

Decorator-based API for defining host extensions:

```python
ext = MontyModule('mymath', skill='# MyMath\nProvides add(a, b)')

@ext.function(timeout_ms=5000, max_return_bytes=1_000_000)
def add(a: int, b: int) -> int:
    return a + b

monty = Monty(code, extensions=[ext.to_extension_dict()])
```

`to_extension_dict()` produces:
```python
{
    'module_name': 'mymath',
    'functions': [{'name': 'add', 'is_native': False}],
    'callables': {'add': <wrapped_fn>},
    'skill': '# MyMath\nProvides add(a, b)',
    'version': '0.0.0',
}
```

### HandleStore (`handles.py`)

Thread-safe Python-side store for opaque objects:

```python
store = HandleStore()

def fit(model_name, data):
    model = train_model(model_name, data)
    return store.register(model, 'ml.Model', extension_id='ml')
    # Returns: {'handle_id': 1, 'type_name': 'ml.Model', 'extension_id': 'ml'}

def predict(handle_dict, inputs):
    model = store.get(handle_dict['handle_id'])
    return model.predict(inputs)
```

The handle dict is detected by `maybe_convert_handle_dict()` and promoted to `ExtensionHandle` in the VM, enabling `model.predict(inputs)` syntax in sandboxed code.

### Enforcement wrappers (`enforcement.py`)

Three decorators applied to host extension functions:

- **`enforce_timeout(ms)`** — Runs in daemon thread with deadline
- **`enforce_size(max_bytes)`** — Checks `sys.getsizeof(result)` after return
- **`enforce_call_count(n)`** — Thread-safe counter, raises when exhausted

Applied automatically when limits are specified in `@ext.function(timeout_ms=..., ...)`.

## Data Flow: Native Extension Call

```
Sandboxed code: df = polars.read_csv(data)

1. Compile time:
   compile_import("polars")
   → registry.lookup("polars") → Some(0)
   → emit LoadExtensionModule(0)

2. Runtime - import:
   LoadExtensionModule(0)
   → registry.create_module(0)
   → allocate Module on heap
   → Module["read_csv"] = Value::ExtensionFunction(idx=0, fn=0, native=true)

3. Runtime - call:
   call polars.read_csv(data)
   → Value::ExtensionFunction(is_native=true)
   → value_to_ext(data) → ExtValue::Str(data)
   → registry.call_native(0, 0, ExtArgs{[ExtValue::Str(data)]}, ctx)
     → extension.call("read_csv", args, ctx)
     → returns ExtValue::Handle(ExtHandle{type_name="polars.DataFrame", id=1})
   → ext_to_value(handle) → HeapData::ExtensionHandle{registry=0, type="polars.DataFrame", id=1}
   → push Value::Ref(heap_id)

4. Runtime - method call:
   df.head(5)
   → py_call_attr on ExtensionHandle
   → call_extension_method(handle_data, "head", args)
   → registry.call_method_native(handle_data, "head", ...)
     → extension.call_method(ext_handle, "head", args, ctx)
   → result converted back, pushed to stack
```

## Data Flow: Host Extension Call

```
Python setup:
   ext = MontyModule('ml')
   @ext.function()
   def fit(model_name, data): ...
   monty = Monty(code, extensions=[ext.to_extension_dict()])

1. Build phase:
   build_extension_registry([ext_dict])
   → register_host(manifest) → index 0
   → host_callables["ext:0:fit"] = <wrapped_fit>

2. Compile time:
   compile_import("ml") → registry.lookup("ml") → Some(0)
   → emit LoadExtensionModule(0)

3. Runtime - call:
   model = ml.fit("logreg", data)
   → Value::ExtensionFunction(is_native=false)
   → CallResult::External("ext:0:fit", args)
   → VM suspends

4. Host dispatch:
   → RunProgress::FunctionCall{name="ext:0:fit", args=[...]}
   → dispatch_host_extension_call("ext:0:fit", args, host_callables)
   → look up host_callables["ext:0:fit"] → <wrapped_fit>
   → call fit("logreg", data) in Python
   → result: {'handle_id': 1, 'type_name': 'ml.Model', 'extension_id': 'ml'}
   → maybe_convert_handle_dict() → MontyObject::ExtensionHandle
   → resume(Return(ExtensionHandle))

5. VM resumes:
   → MontyObject::ExtensionHandle → HeapData::ExtensionHandle
   → push Value::Ref(heap_id)
   → model now supports .predict() method syntax
```

## Writing a Native Extension

There are two authoring styles. Both produce identical runtime behaviour and ABI output.

### Module-level style (recommended)

Apply `#[monty_module]` to a `mod` block. The macro generates the `Extension` struct, `StoredObject` enum, typed handles, dispatch tables, and the C entry point automatically.

```rust
use monty_extension_api::*;

#[monty_module(name = "myext", version = "0.1.0")]
mod myext {
    use monty_extension_api::{ExtArgs, ExtContext, ExtError, ExtValue};

    #[monty_class]
    struct Counter(pub(crate) u64);

    #[monty_function()]
    fn greet(ext: &Extension, name: &str) -> Result<ExtValue, ExtError> {
        Ok(ExtValue::string(format!("Hello, {name}!")))
    }

    #[monty_function(name = "new_counter")]
    fn new_counter(ext: &Extension) -> Result<CounterHandle, ExtError> {
        Ok(ext.store_counter(Counter(0)))
    }

    #[monty_methods]
    impl Counter {
        #[monty_method(name = "increment")]
        fn increment(ext: &Extension, c: CounterHandle) -> Result<i64, ExtError> {
            ext.with_counter_mut(c, "increment", |counter| {
                counter.0 += 1;
                Ok(counter.0 as i64)
            })
        }
    }

    #[monty_shutdown()]
    fn shutdown(ext: &Extension) {
        ext.objects.lock().unwrap().clear();
    }
}
```

### Impl-level style (classic)

Use `#[monty_classes]` / `#[monty_handles]` on an enum and `#[monty_module]` / `#[monty_extension]` on an impl block. This is the original API and remains fully supported.

```rust
use monty_extension_api::*;

#[monty_classes(extension = MyExtension, module = "myext")]
enum StoredObject { Counter(u64) }

struct MyExtension {
    objects: Mutex<HashMap<u64, StoredObject>>,
    next_id: Mutex<u64>,
}

#[monty_module(name = "myext", version = "0.1.0")]
impl MyExtension {
    fn new() -> Self { /* ... */ }

    #[monty_function()]
    fn greet(&self, args: &ExtArgs, _ctx: &ExtContext) -> Result<ExtValue, ExtError> {
        let name = args.get::<&str>(0, "greet", "name")?;
        Ok(ExtValue::string(format!("Hello, {name}!")))
    }

    #[monty_shutdown()]
    fn shutdown(&self) { self.objects.lock().unwrap().clear(); }
}
```

### Building and loading

Build as `cdylib`. The only required dependency is `monty-extension-api` — `abi_stable` is handled internally by the API crate and its macros:
```toml
[lib]
crate-type = ["cdylib"]

[dependencies]
monty-extension-api = "0.1"
```

Load from Python:
```python
from pydantic_monty import Monty

m = Monty('import myext\nmyext.greet("World")', extensions=[{
    'module_name': 'myext',
    'library_path': '/path/to/libmyext.dylib',
}])
result = m.run()  # "Hello, World!"
```

## Writing a Host Extension

```python
from pydantic_monty import Monty, MontyModule, HandleStore

store = HandleStore()
ml = MontyModule('ml', skill='# ML\nProvides fit(name) and predict(model, x)')

@ml.function()
def fit(model_name: str) -> dict:
    model = {'name': model_name, 'weights': [1.0, 2.0]}
    return store.register(model, 'ml.Model', extension_id='ml')

@ml.function()
def predict(handle_dict: dict, x: float) -> float:
    model = store.get(handle_dict['handle_id'])
    return sum(w * x for w in model['weights'])

m = Monty("""
import ml
model = ml.fit('linear')
result = model.predict(3.0)
result
""", extensions=[ml.to_extension_dict()])

result = m.run()  # 9.0
```

## Security Model

Extensions execute **outside** the sandbox but are controlled by it:

- Native extensions run in the same Rust process with full access to the host OS, but the sandboxed code can only call functions declared in the manifest.
- Host extensions run in the host Python/JS process. The sandbox communicates only through the `ExtValue`/`MontyObject` serialization boundary.
- No raw pointers, file handles, or memory addresses cross the boundary — only values and opaque handle IDs.
- Resource limits (time, allocations) are enforced on both sides: native extensions receive budgets via `ExtContext`, host extensions are wrapped with enforcement decorators.
- Extension authors are trusted — they choose what capabilities to expose. The sandbox protects against untrusted *Python code*, not untrusted extensions.
