# Host Extension Example: ML Module

Demonstrates building a **host-backed extension** for Monty using `MontyModule` decorators. Sandboxed code can `import ml` to train and evaluate machine learning models — all function calls dispatch to Python via the VM's suspension mechanism.

## What This Shows

1. **`MontyModule` + `@function` decorators**: Declaring host extension functions with enforcement limits (timeout, return size cap)
2. **`HandleStore`**: Heavy objects (trained models) stay on the host side; sandboxed code receives lightweight handle dicts
3. **Type stubs**: Extension provides `.pyi`-style stubs for Monty's type checker
4. **Skill text**: Markdown description for AI agent prompt injection via `Monty.extension_skills()`
5. **Enforcement wrappers**: Timeout and return-size limits on host calls

## Files

- `ml_extension.py` — Extension definition: `MontyModule`, model classes, registered functions
- `main.py` — Runner that creates a `Monty` instance with the extension and executes sandbox code

## To Run

```bash
# From repo root — requires pydantic_monty to be installed
make dev-py
cd examples/host_extension
uv run python main.py
```
