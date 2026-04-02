# monty-extended: Unified Native + Host Extension System

## Context

Monty is a sandboxed Python interpreter (Rust) where the only way code interacts with the outside world is through explicitly provided capabilities. We want to fork Monty to create `monty-extended` — a version that supports **two kinds of extensions** through the **same `import` mechanism**:

1. **Native extensions** — Rust `.so/.dylib` loaded at runtime. Functions execute directly in the VM loop (like `math.floor`). No suspension, no Python.
2. **Host extensions** — Python functions registered via decorators. Functions suspend the VM and call back to Python (like existing `external_functions`, but importable as modules).

Both feel identical to Monty code:
```python
import polars as pl      # native (Rust .so)
import sklearn           # host (Python)

df = pl.read_csv("x.csv")        # no suspension, pure Rust
model = sklearn.fit("logreg", df) # suspends, calls Python sklearn
```

A single module can mix both — some functions native, some host-backed.

---

## Upstream Monty Structure (actual)

```
monty/
├── Cargo.toml                              # Workspace: 7 crates
├── crates/
│   ├── monty/                              # Core VM (Rust lib, name="monty")
│   │   └── src/
│   │       ├── value.rs
│   │       ├── modules/                    # StandardLib, math.rs, json.rs, etc.
│   │       ├── bytecode/
│   │       │   ├── compiler.rs
│   │       │   └── vm/
│   │       │       ├── mod.rs
│   │       │       └── call.rs
│   │       ├── run.rs, run_progress.rs, object.rs, ...
│   │
│   ├── monty-python/                       # PyO3 bindings = THE pip package
│   │   ├── Cargo.toml                      # name="pydantic-monty", cdylib
│   │   ├── pyproject.toml                  # pip: "pydantic-monty", maturin build
│   │   ├── src/                            # Rust: monty_cls.rs, convert.rs, external.rs, ...
│   │   └── python/pydantic_monty/          # Python package shipped to users
│   │       ├── __init__.py                 # Re-exports + pure Python helpers
│   │       ├── _monty.pyi                  # Type stubs for Rust bindings
│   │       └── os_access.py                # OSAccess, MemoryFile (pure Python)
│   │
│   ├── monty-cli/
│   ├── monty-js/
│   ├── monty-type-checking/
│   ├── monty-typeshed/
│   └── fuzz/
│
└── examples/                               # sql_playground, web_scraper, expense_analysis
```

Key: `monty-python` is BOTH the Rust PyO3 crate AND the Python package. Maturin compiles Rust into `pydantic_monty._monty`, ships Python from `python/pydantic_monty/`. One crate = one pip package.

---

## Our Fork Structure

We follow the exact same pattern. ONE new crate added (`monty-extension-api`). Everything else is modifications to existing crates. New Python files go inside `monty-python/python/` alongside the existing `os_access.py`.

```
monty-extended/                           # Fork of monty
├── Cargo.toml                              # Workspace: +1 new member
├── crates/
│   ├── monty/                              # MODIFIED: core VM
│   │   ├── Cargo.toml                      # +dep: monty-extension-api
│   │   └── src/
│   │       ├── value.rs                    # MODIFIED: +3 Value variants
│   │       ├── extensions.rs               # NEW: ExtensionRegistry + Value↔ExtValue bridge
│   │       ├── modules/mod.rs              # MODIFIED: import fallback to registry
│   │       ├── bytecode/
│   │       │   ├── compiler.rs             # MODIFIED: emit LoadExtensionModule
│   │       │   └── vm/
│   │       │       ├── mod.rs              # MODIFIED: LoadExtensionModule opcode handler
│   │       │       └── call.rs             # MODIFIED: is_native dispatch branch
│   │       └── ...                         # Everything else unchanged
│   │
│   ├── monty-extension-api/                # NEW: stable public crate
│   │   ├── Cargo.toml                      # Deps: abi_stable ONLY
│   │   └── src/lib.rs                      # ExtValue, ExtArgs, ExtResult, MontyExtension trait
│   │
│   ├── monty-python/                       # MODIFIED: Python bindings + framework
│   │   ├── Cargo.toml                      # +deps: monty-extension-api, libloading
│   │   ├── pyproject.toml                  # Renamed pip package: "monty-extended"
│   │   ├── src/
│   │   │   ├── monty_cls.rs               # MODIFIED: extension loading API
│   │   │   └── ...                         # convert.rs, external.rs, etc. mostly unchanged
│   │   └── python/monty_extended/        # Python package (renamed from pydantic_monty)
│   │       ├── __init__.py                 # MODIFIED: enhanced Monty class + re-exports
│   │       ├── _monty.pyi                  # MODIFIED: updated type stubs
│   │       ├── os_access.py                # Unchanged from upstream
│   │       ├── decorators.py               # NEW: MontyModule, @function decorator
│   │       ├── handles.py                  # NEW: HandleStore
│   │       └── enforcement.py              # NEW: resource enforcement wrappers
│   │
│   ├── monty-cli/                          # Unchanged
│   ├── monty-js/                           # Unchanged
│   ├── monty-type-checking/                # Unchanged
│   ├── monty-typeshed/                     # Unchanged
│   └── fuzz/                               # Unchanged
│
└── examples/
    ├── sql_playground/                     # Unchanged from upstream
    ├── web_scraper/                        # Unchanged from upstream
    ├── native_extension/                   # NEW: example native Rust extension
    │   ├── Cargo.toml                      # Deps: monty-extension-api, polars
    │   └── src/lib.rs
    └── host_extension/                     # NEW: example host Python extension
        ├── pyproject.toml
        └── monty_ext_sklearn/__init__.py   # Uses MontyModule decorator
```

---

## The 3 Crates

| Crate | Published to | Purpose | Depended on by |
|---|---|---|---|
| `monty` | Not published | The VM | Only `monty-python` |
| `monty-extension-api` | crates.io | Stable types + trait for writing native extensions | Third-party native extension authors |
| `monty-python` | PyPI as `monty-extended` | PyO3 bindings + Python framework | End users + host extension authors |

`monty-extension-api` exists so native extension authors **never depend on monty internals**. They see only `ExtValue`, `ExtArgs`, `MontyExtension` trait.

---

## Key Design: Skills — Every Extension Has a One-Page Skill Doc

Every extension (native or host) carries a **skill**: a markdown string that describes the extension's capabilities for AI agents. This is not documentation for humans — it's prompt context for LLMs.

**Why:** When an AI agent uses monty-extended to run code, it needs to know what APIs are available, what patterns to use, and what constraints exist (e.g., "DataFrames are handles, use `.head()` to see data"). Type stubs tell the type checker; skills tell the agent.

**No auto-discovery.** Extensions are explicitly passed to `Monty()`. The agent builder curates which extensions are available — no accidental capability leakage from a random pip install.

```python
from monty_extended import Monty
from monty_ext_polars import polars_extension
from monty_ext_sklearn import sklearn_extension

# Explicit — you choose what's available
m = Monty(
    code=agent_generated_code,
    extensions=[polars_extension, sklearn_extension],
)

# Collect skills for injection into the agent's system prompt
skills = m.extension_skills()
# Returns: "# Polars — DataFrame Operations\n\nYou have access to..."
```

Both native and host extensions carry the skill the same way:

```rust
// Native (Rust) — skill is a field on ExtManifest
#[repr(C)] #[derive(StableAbi)]
pub struct ExtManifest {
    pub module_name: RString,
    pub functions: RVec<ExtFunctionDecl>,
    pub type_stub_source: ROption<RString>,
    pub skill: RString,                      // markdown for AI agents
    pub version: RString,
}
```

```python
# Host (Python) — skill is a field on MontyModule
sklearn = MontyModule(
    "sklearn",
    skill="""
    # scikit-learn — Machine Learning

    You have access to `import sklearn` for training and evaluating models.

    ## Available functions
    - `sklearn.fit(model: str, data: DataFrame, target: str) -> Model`
    - `sklearn.predict(model: Model, data: DataFrame) -> list[float]`
    - `sklearn.score(model: Model, data: DataFrame, target: str) -> dict`

    ## Patterns
    - Models are handles — you can't inspect internals, use `sklearn.score()` for metrics.
    - Supported model names: "logistic_regression", "random_forest", "xgboost", "kmeans"

    ## Example
    ```python
    import sklearn
    model = sklearn.fit("logistic_regression", train_df, target="churned")
    metrics = sklearn.score(model, test_df, target="churned")
    print(metrics)  # {"accuracy": 0.87, "f1": 0.83}
    ```
    """,
    type_stub="def fit(model: str, data: Any, target: str) -> Any: ...",
)
```

The `Monty.extension_skills()` method concatenates all skills with `---` separators, ready for prompt injection.

---

## Key Design: The `is_native` Flag

Every extension function in the VM carries one boolean:

```rust
ExtensionFunction { registry_index: u32, function_name: StringId, is_native: bool }
```

At call time in `call.rs`:
- `is_native = true` → convert args → call Rust fn pointer → convert result → `CallResult::Value` (no suspension)
- `is_native = false` → `CallResult::External("ext:<idx>:<name>", args)` (VM suspends, Python host dispatches)

Same module can mix both. Same bytecode. Compiler doesn't care which type a function is.

---

## Rust Core Changes (crates/monty/)

### value.rs — 3 new Value variants
```rust
ExtensionFunction(ExtensionFunctionId)    // { registry_index, function_name, is_native }
ExtensionHandle(ExtensionHandleValue)     // { registry_index, type_name, handle_id }
BoundExtensionMethod { handle, method_name, is_native }
```

### extensions.rs — NEW file (~300 lines)
- `ExtensionRegistry`: stores loaded extensions
  - `register_native(extension, library)` — from abi_stable trait object
  - `register_host(manifest)` — manifest only, Python handles dispatch
  - `lookup(module_name) -> Option<u32>` — compiler checks this
  - `create_module(index) -> Module` — builds real Module on heap
  - Merge logic: same-name modules from native + host → `Mixed` entry
- Bridge functions: `value_to_ext()` and `ext_to_value()` — bidirectional Value ↔ ExtValue

### modules/mod.rs — import fallback
```
import X → 1. StandardLib? → LoadModule (existing)
           2. ExtensionRegistry.lookup(X)? → LoadExtensionModule(idx) (NEW)
           3. else → RaiseImportError (existing)
```

### bytecode/compiler.rs — new opcode emission
For known extensions, emit `LoadExtensionModule(registry_index)` instead of `RaiseImportError`.

### bytecode/vm/mod.rs — new opcode handler
`LoadExtensionModule(idx)`: calls `registry.create_module(idx)`, allocates on heap, pushes to stack.

### bytecode/vm/call.rs — dispatch branch
```
Value::ExtensionFunction(ef) =>
  if ef.is_native → direct Rust call → CallResult::Value
  else → CallResult::External("ext:{idx}:{name}", args)
```

---

## monty-extension-api Crate (crates/monty-extension-api/)

Stable, minimal, `abi_stable`-based:

```rust
#[repr(C)] #[derive(StableAbi)]
pub enum ExtValue { None, Bool(bool), Int(i64), Float(f64), Str(RString), Bytes(RVec<u8>),
                    List(RVec<ExtValue>), Dict(RVec<(RString, ExtValue)>), Handle(ExtHandle) }

#[repr(C)] #[derive(StableAbi)]
pub struct ExtArgs { pub positional: RVec<ExtValue>, pub keyword: RVec<(RString, ExtValue)> }

pub type ExtResult = RResult<ExtValue, ExtError>;

#[repr(C)] #[derive(StableAbi)]
pub struct ExtHandle { pub type_name: RString, pub handle_id: u64, pub extension_id: RString }

#[repr(C)] #[derive(StableAbi)]
pub struct ExtContext { pub budget: ResourceBudget }

#[repr(C)] #[derive(StableAbi)]
pub struct ExtManifest {
    pub module_name: RString,
    pub functions: RVec<ExtFunctionDecl>,
    pub type_stub_source: ROption<RString>,
    pub skill: RString,                      // markdown for AI agent prompts
    pub version: RString,
}

#[sabi_trait]
pub trait MontyExtension: Send + Sync {
    fn manifest(&self) -> ExtManifest;
    fn call(&self, function_name: RStr<'_>, args: ExtArgs, ctx: &ExtContext) -> ExtResult;
    fn call_method(&self, handle: &ExtHandle, method: RStr<'_>, args: ExtArgs, ctx: &ExtContext) -> ExtResult;
    fn shutdown(&self);
}

#[repr(C)] #[derive(StableAbi)]
pub struct ExtensionEntry { pub api_version: u32, pub create: extern "C" fn() -> MontyExtension_TO<...> }
```

---

## Python Framework (crates/monty-python/python/monty_extended/)

### decorators.py — Host extension API
```python
sklearn = MontyModule(
    "sklearn",
    skill="# scikit-learn\n\nYou have access to...",
    type_stub="def fit(...) -> Any: ...",
)

@sklearn.function(timeout_ms=60_000)
def fit(model_name: str, data) -> dict:
    return handle_store.register(model, "sklearn.Model")
```

### handles.py — Heavy object registry
Thread-safe `{handle_id: object}` store. Extensions register Python objects, get lightweight handle dicts back.

### enforcement.py — Resource enforcement
Wraps host extension calls with wall-clock timeout, return size cap, call count budget.

### __init__.py — Enhanced Monty class
```python
class Monty:
    def __init__(self, code: str, *, extensions: list[Extension] = [], **kwargs):
        # Extensions are EXPLICIT — no auto-discovery
        for ext in extensions:
            if is_native(ext):
                self._inner.load_native_extension(ext.library_path())
            else:
                self._inner.register_host_manifest(ext.to_manifest())

    def extension_skills(self) -> str:
        """Collect all extension skills for AI agent prompt injection."""
        return "\n\n---\n\n".join(ext.skill for ext in self._extensions)

    def run(self, **kwargs) -> Any: ...
    async def run_async(self, **kwargs) -> Any: ...
```

---

## How Host Calls Reuse Existing Infrastructure

No new `RunProgress` variants. Host extension calls piggyback on `FunctionCall`:

1. VM encounters `ExtensionFunction { is_native: false }` → `CallResult::External("ext:3:fit", args)`
2. Host run loop receives `RunProgress::FunctionCall` with name `"ext:3:fit"`
3. Host parses prefix, finds the registered module, calls the callable
4. Wraps with timeout/size enforcement
5. Resumes VM with result

Same mechanism as `external_functions`, just routed through the extension registry.

**Host-language agnostic.** The Rust VM doesn't know or care whether the host is Python or JavaScript. It just suspends and yields.

**Native extensions work in both Python and JS automatically.** The `.so/.dylib` is loaded by the Rust VM via `libloading` in the `monty` core crate — not in the language bindings. The host language is never involved:

```
JS app  → monty-js (NAPI)   → Rust VM → native fn pointer → Rust polars code
Python  → monty-python (PyO3) → Rust VM → same fn pointer → same Rust polars code
```

The only difference is distribution: the same `.so` ships via `pip install monty-ext-polars` (maturin wheel) for Python users and `npm install monty-ext-polars` (prebuild binary) for JS users. Same Rust source, same compiled output, two packaging targets.

**Host extensions are per-language by nature.** A Python host extension wrapping sklearn is a Python function — it can't run in JS. A JS host extension wrapping a JS library can't run in Python.

| Extension type | Python host | JS host | Extension author does |
|---|---|---|---|
| Native (Rust .so) | Works | Works | Nothing extra — loaded by Rust VM |
| Host (Python) | Works | N/A | Python function, only runs in Python |
| Host (JS) | N/A | Works | JS function, only runs in JS |

Host extension callables are registered per-language:
- **Python**: `MontyModule` + `@function` decorators in `monty_extended`
- **JavaScript**: equivalent registration API in `monty-js`

The manifest (module name, function declarations, skill, type stubs) is language-neutral — it lives in Rust on `ExtManifest`. Only the callable implementations are language-specific.

---

## Resource Enforcement

- **Native calls**: Pre-call budget check + post-call time accounting in the VM. `ExtContext` exposes remaining budget for cooperative checking.
- **Host calls**: Python wrapper enforces wall-clock timeout, return size cap, call count budget.
- Monty's built-in bytecode limits (allocations, memory, recursion) apply between all calls.

---

## Implementation Phases

### Phase 1: Foundation (~1 week)
- [ ] Fork monty, add `monty-extension-api` crate to workspace
- [ ] Define ExtValue, ExtArgs, ExtResult, MontyExtension trait (with abi_stable)
- [ ] Add 3 Value variants to `crates/monty/src/value.rs`
- [ ] Add `crates/monty/src/extensions.rs` (registry + bridge)
- [ ] Add `LoadExtensionModule` opcode + compiler fallback
- [ ] Add call dispatch branch in `call.rs`

### Phase 2: Python integration (~1 week)
- [ ] Add libloading in `monty-python/src/monty_cls.rs`
- [ ] Expose `load_native_extension(path)` and `register_host_manifest(dict)` to Python
- [ ] Add `decorators.py`, `handles.py`, `enforcement.py` to `python/monty_extended/`
- [ ] Update `__init__.py` with enhanced Monty class (explicit extensions, `extension_skills()`)

### Phase 3: Resource enforcement + stubs (~3-5 days)
- [ ] Pre/post-call budget checks for native calls
- [ ] Timeout + size cap wrappers for host calls
- [ ] Type stub collection from both extension types
- [ ] Inject into compiler via `type_check_stubs`

### Phase 4: Example extensions (~1 week)
- [ ] `examples/native_extension/` — polars binding (Rust, links polars-core)
- [ ] `examples/host_extension/` — sklearn binding (Python, wraps sklearn)

### Phase 5: Tests (~3-5 days)
- [ ] Registry, bridge, dispatch unit tests
- [ ] Integration: native import + call
- [ ] Integration: host import + call
- [ ] Integration: mixed module
- [ ] Resource limit enforcement

---

## Verification

1. `import polars as pl; pl.read_csv("test.csv")` — native, no suspension
2. `import sklearn; sklearn.fit("logreg", data)` — host, suspends to Python
3. Mixed module with native + host functions, both work
4. Resource limits: timeout/size violations raise errors
5. Type checking: `Monty(code, type_check=True)` validates extension calls
6. Explicit registration: `Monty(code, extensions=[polars_ext])` → `import polars` works
7. Skills: `m.extension_skills()` returns combined markdown from all registered extensions
8. Backward compat: `external_functions={}` still works unchanged

---

## Files Changed/Created

**New files:**
- `crates/monty-extension-api/Cargo.toml` + `src/lib.rs`
- `crates/monty/src/extensions.rs`
- `crates/monty-python/python/monty_extended/decorators.py`
- `crates/monty-python/python/monty_extended/handles.py`
- `crates/monty-python/python/monty_extended/enforcement.py`

**Modified files:**
- `Cargo.toml` — add workspace member
- `crates/monty/Cargo.toml` — add dep on monty-extension-api
- `crates/monty/src/value.rs` — 3 new variants
- `crates/monty/src/modules/mod.rs` — import fallback
- `crates/monty/src/bytecode/compiler.rs` — emit LoadExtensionModule
- `crates/monty/src/bytecode/vm/mod.rs` — opcode handler
- `crates/monty/src/bytecode/vm/call.rs` — is_native dispatch
- `crates/monty-python/Cargo.toml` — add deps (monty-extension-api, libloading)
- `crates/monty-python/pyproject.toml` — rename to monty-extended
- `crates/monty-python/src/monty_cls.rs` — extension loading API
- `crates/monty-python/python/monty_extended/__init__.py` — enhanced Monty class
