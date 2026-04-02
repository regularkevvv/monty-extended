"""Decorator framework for defining host-backed Monty extensions.

Host extensions are Python functions that sandboxed Monty code can call via
the standard ``import`` mechanism. The VM suspends on each call, dispatches
to the registered Python callable, and resumes with the result.

Example::

    from pydantic_monty import MontyModule

    math_ext = MontyModule(
        'mymath',
        skill='# MyMath\\nProvides add(a, b) -> int',
    )


    @math_ext.function()
    def add(a: int, b: int) -> int:
        return a + b


    # Pass to Monty via extensions=[math_ext.to_extension_dict()]
"""

from __future__ import annotations

from typing import Any, Callable

from pydantic_monty.enforcement import enforce_call_count, enforce_size, enforce_timeout


class MontyModule:
    """Declares a host-backed extension module.

    Each instance represents one importable module (e.g. ``import sklearn``).
    Use the :meth:`function` decorator to register callables.

    Args:
        module_name: The module name that sandboxed code will ``import``.
        skill: Markdown text describing the module's capabilities for AI agents.
            Injected into system prompts via ``Monty.extension_skills()``.
        type_stub: Optional Python type stub source for static type checking.
        version: Semantic version string for the extension.
    """

    def __init__(
        self,
        module_name: str,
        *,
        skill: str = '',
        type_stub: str | None = None,
        version: str = '0.0.0',
    ) -> None:
        self.module_name = module_name
        self.skill = skill
        self.type_stub = type_stub
        self.version = version
        self._functions: dict[str, _FunctionEntry] = {}

    def function(
        self,
        *,
        name: str | None = None,
        timeout_ms: int | None = None,
        max_return_bytes: int | None = None,
        max_calls: int | None = None,
    ) -> Callable[[Callable[..., Any]], Callable[..., Any]]:
        """Registers a Python callable as a host extension function.

        The decorated function will be called when sandboxed code invokes
        ``module.function_name(...)``. Arguments are converted from Monty
        values to Python objects, and the return value is converted back.

        Enforcement wrappers are applied automatically when
        :meth:`to_extension_dict` is called based on the limits specified here.

        Args:
            name: Override the function name exposed to Monty code.
                Defaults to the Python function's ``__name__``.
            timeout_ms: Maximum wall-clock time (ms) for this call.
                ``None`` means no timeout.
            max_return_bytes: Maximum shallow byte size of the return value.
                ``None`` means no size limit.
            max_calls: Maximum number of times this function may be called
                per ``Monty`` instance. ``None`` means unlimited.
        """

        def decorator(fn: Callable[..., Any]) -> Callable[..., Any]:
            fn_name = name or fn.__name__
            self._functions[fn_name] = _FunctionEntry(
                callable=fn,
                timeout_ms=timeout_ms,
                max_return_bytes=max_return_bytes,
                max_calls=max_calls,
            )
            return fn

        return decorator

    def to_extension_dict(self) -> dict[str, Any]:
        """Serializes this module into the dict format expected by ``Monty(extensions=[...])``.

        Enforcement wrappers (timeout, return-size cap, call-count budget)
        are applied to each callable based on the limits set via :meth:`function`.

        Returns:
            A dict with keys ``module_name``, ``functions``, ``skill``,
            ``version``, ``type_stub_source``, and ``callables``.
        """
        functions = [{'name': fn_name, 'is_native': False} for fn_name in self._functions]
        callables = {fn_name: entry.wrapped_callable() for fn_name, entry in self._functions.items()}
        result: dict[str, Any] = {
            'module_name': self.module_name,
            'functions': functions,
            'skill': self.skill,
            'version': self.version,
            'callables': callables,
        }
        if self.type_stub is not None:
            result['type_stub_source'] = self.type_stub
        return result


class _FunctionEntry:
    """Internal record for a registered host function with optional enforcement limits."""

    __slots__ = ('callable', 'timeout_ms', 'max_return_bytes', 'max_calls')

    def __init__(
        self,
        callable: Callable[..., Any],
        timeout_ms: int | None,
        max_return_bytes: int | None,
        max_calls: int | None,
    ) -> None:
        self.callable = callable
        self.timeout_ms = timeout_ms
        self.max_return_bytes = max_return_bytes
        self.max_calls = max_calls

    def wrapped_callable(self) -> Callable[..., Any]:
        """Returns the callable with enforcement decorators applied.

        Decorators are stacked inside-out: the function runs first,
        then size is checked, then timeout wraps everything. Call-count
        is the outermost layer so it decrements before any work starts.
        """
        fn = self.callable
        if self.max_return_bytes is not None:
            fn = enforce_size(self.max_return_bytes)(fn)
        if self.timeout_ms is not None:
            fn = enforce_timeout(self.timeout_ms)(fn)
        if self.max_calls is not None:
            fn = enforce_call_count(self.max_calls)(fn)
        return fn
