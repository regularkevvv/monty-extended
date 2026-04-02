"""Resource enforcement wrappers for host extension calls.

These wrappers add wall-clock timeouts, return-size caps, and call-count
budgets around host extension functions. They are applied automatically
when extensions are registered with enforcement parameters.

Example::

    from pydantic_monty.enforcement import enforce_timeout, enforce_size


    @enforce_timeout(timeout_ms=5000)
    @enforce_size(max_return_bytes=1_000_000)
    def my_extension_fn(data: list) -> dict:
        return process(data)
"""

from __future__ import annotations

import functools
import sys
import threading
from typing import Any, Callable, TypeVar

F = TypeVar('F', bound=Callable[..., Any])


def enforce_timeout(timeout_ms: int) -> Callable[[F], F]:
    """Wraps a callable with a wall-clock timeout.

    If the function does not return within ``timeout_ms`` milliseconds,
    a ``TimeoutError`` is raised. The function continues running in its
    thread — this is a best-effort mechanism.

    Args:
        timeout_ms: Maximum wall-clock time in milliseconds.
    """

    def decorator(fn: F) -> F:
        @functools.wraps(fn)
        def wrapper(*args: Any, **kwargs: Any) -> Any:
            result: list[Any] = []
            error: list[BaseException] = []

            def target() -> None:
                try:
                    result.append(fn(*args, **kwargs))
                except BaseException as exc:
                    error.append(exc)

            thread = threading.Thread(target=target, daemon=True)
            thread.start()
            thread.join(timeout=timeout_ms / 1000.0)

            if thread.is_alive():
                raise TimeoutError(f'{fn.__name__} exceeded {timeout_ms}ms timeout')
            if error:
                raise error[0]
            return result[0]

        return wrapper  # type: ignore[return-value]

    return decorator


def enforce_size(max_return_bytes: int) -> Callable[[F], F]:
    """Wraps a callable with a return-value size cap.

    After the function returns, ``sys.getsizeof`` is used for a shallow
    size estimate. If the return value exceeds ``max_return_bytes``,
    a ``ValueError`` is raised.

    Args:
        max_return_bytes: Maximum shallow size of the return value in bytes.
    """

    def decorator(fn: F) -> F:
        @functools.wraps(fn)
        def wrapper(*args: Any, **kwargs: Any) -> Any:
            result = fn(*args, **kwargs)
            size = sys.getsizeof(result)
            if size > max_return_bytes:
                raise ValueError(f'{fn.__name__} returned {size} bytes, exceeding {max_return_bytes} byte limit')
            return result

        return wrapper  # type: ignore[return-value]

    return decorator


def enforce_call_count(max_calls: int) -> Callable[[F], F]:
    """Wraps a callable with a call-count budget.

    After ``max_calls`` invocations, subsequent calls raise ``RuntimeError``.
    The counter is shared across all threads.

    Args:
        max_calls: Maximum number of times the function may be called.
    """

    def decorator(fn: F) -> F:
        lock = threading.Lock()
        remaining = [max_calls]

        @functools.wraps(fn)
        def wrapper(*args: Any, **kwargs: Any) -> Any:
            with lock:
                if remaining[0] <= 0:
                    raise RuntimeError(f'{fn.__name__} call budget exhausted (limit: {max_calls})')
                remaining[0] -= 1
            return fn(*args, **kwargs)

        return wrapper  # type: ignore[return-value]

    return decorator
