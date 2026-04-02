"""Thread-safe handle store for managing opaque host-side objects.

When a host extension function creates a heavy Python object (e.g. a trained
ML model or a database connection), it shouldn't serialize the object and
send it into the sandbox. Instead, it registers the object in a
:class:`HandleStore` and returns a lightweight handle dict that Monty code
can pass back in subsequent calls.

Example::

    from pydantic_monty.handles import HandleStore

    store = HandleStore()


    # In the host extension function:
    def fit(model_name: str, data: list) -> dict:
        model = train_model(model_name, data)
        return store.register(model, 'sklearn.Model')


    # Later, when Monty code passes the handle back:
    def predict(handle: dict, inputs: list) -> list:
        model = store.get(handle['handle_id'])
        return model.predict(inputs)
"""

from __future__ import annotations

import threading
from typing import Any


class HandleStore:
    """Thread-safe registry mapping integer IDs to Python objects.

    Handles are returned to Monty code as plain dicts with ``handle_id``,
    ``type_name``, and ``extension_id`` keys. The sandbox only sees the
    lightweight dict; the actual Python object stays on the host side.
    """

    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._objects: dict[int, Any] = {}
        self._next_id = 1

    def register(self, obj: Any, type_name: str, extension_id: str = '') -> dict[str, Any]:
        """Stores an object and returns a handle dict for Monty code.

        Args:
            obj: The Python object to store.
            type_name: Human-readable type name (e.g. ``"sklearn.Model"``).
            extension_id: Optional extension identifier.

        Returns:
            A dict with ``handle_id``, ``type_name``, and ``extension_id``.
        """
        with self._lock:
            handle_id = self._next_id
            self._next_id += 1
            self._objects[handle_id] = obj
        return {
            'handle_id': handle_id,
            'type_name': type_name,
            'extension_id': extension_id,
        }

    def get(self, handle_id: int) -> Any:
        """Retrieves the object associated with a handle ID.

        Args:
            handle_id: The integer ID from a handle dict.

        Returns:
            The stored Python object.

        Raises:
            KeyError: If the handle ID is not found.
        """
        with self._lock:
            return self._objects[handle_id]

    def remove(self, handle_id: int) -> Any:
        """Removes and returns the object associated with a handle ID.

        Args:
            handle_id: The integer ID from a handle dict.

        Returns:
            The stored Python object.

        Raises:
            KeyError: If the handle ID is not found.
        """
        with self._lock:
            return self._objects.pop(handle_id)

    def clear(self) -> None:
        """Removes all stored objects."""
        with self._lock:
            self._objects.clear()

    def __len__(self) -> int:
        with self._lock:
            return len(self._objects)
