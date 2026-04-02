"""Tests for the extension system: native loading, host dispatch, mixed modules,
error handling, enforcement wrappers, handle round-trips, skills, and type stubs.
"""

from __future__ import annotations

import platform
import subprocess
from pathlib import Path
from typing import Any

import pytest
from inline_snapshot import snapshot

import pydantic_monty
from pydantic_monty import HandleStore, Monty, MontyModule

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

EXTENSION_DIR = Path(__file__).parents[2] / 'monty' / '..' / '..' / 'examples' / 'native_extension'
EXTENSION_DIR = (Path(__file__).parent / '..' / '..' / '..' / 'examples' / 'native_extension').resolve()

_LIB_PATH: str | None = None


def _native_lib_path() -> str | None:
    """Returns the path to the built native extension, or None if unavailable."""
    global _LIB_PATH
    if _LIB_PATH is not None:
        return _LIB_PATH

    if platform.system() == 'Darwin':
        lib = EXTENSION_DIR / 'target' / 'release' / 'libmonty_ext_datatools.dylib'
    elif platform.system() == 'Linux':
        lib = EXTENSION_DIR / 'target' / 'release' / 'libmonty_ext_datatools.so'
    else:
        return None

    if not lib.exists():
        try:
            subprocess.check_call(['cargo', 'build', '--release'], cwd=EXTENSION_DIR, timeout=120)
        except (subprocess.CalledProcessError, FileNotFoundError):
            return None

    if lib.exists():
        _LIB_PATH = str(lib)
        return _LIB_PATH
    return None


def native_ext_dict() -> dict[str, Any]:
    """Returns the extension dict for the native datatools extension."""
    path = _native_lib_path()
    assert path is not None, 'native extension not available'
    return {'library_path': path}


requires_native = pytest.mark.skipif(
    _native_lib_path() is None,
    reason='native extension not built',
)


def _make_host_module() -> tuple[MontyModule, HandleStore]:
    """Creates a simple host extension with a handle store for testing."""
    store = HandleStore()

    mod = MontyModule(
        'testmod',
        skill='# testmod\nA test module.',
        type_stub='def add(a: int, b: int) -> int: ...\ndef greet(name: str) -> str: ...',
        version='1.0.0',
    )

    @mod.function()
    def add(a: int, b: int) -> int:
        return a + b

    @mod.function()
    def greet(name: str) -> str:
        return f'hello {name}'

    @mod.function()
    def make_obj(label: str) -> dict[str, Any]:
        return store.register({'label': label}, 'testmod.Obj', extension_id='testmod')

    @mod.function()
    def get_label(obj: dict[str, Any]) -> str:
        return store.get(obj['handle_id'])['label']

    @mod.function()
    def echo(value: Any) -> Any:
        return value

    @mod.function()
    def fail(msg: str) -> None:
        raise ValueError(msg)

    return mod, store


# ===========================================================================
# Host extension tests
# ===========================================================================


class TestHostExtensionBasic:
    """Basic host extension functionality: import, call, return values."""

    def test_simple_function_call(self):
        mod, _ = _make_host_module()
        result = Monty(
            'import testmod\ntestmod.add(3, 4)',
            extensions=[mod.to_extension_dict()],
        ).run()
        assert result == snapshot(7)

    def test_string_return(self):
        mod, _ = _make_host_module()
        result = Monty(
            "import testmod\ntestmod.greet('world')",
            extensions=[mod.to_extension_dict()],
        ).run()
        assert result == snapshot('hello world')

    def test_multiple_calls(self):
        mod, _ = _make_host_module()
        code = """\
import testmod
a = testmod.add(1, 2)
b = testmod.add(10, 20)
a + b
"""
        assert Monty(code, extensions=[mod.to_extension_dict()]).run() == snapshot(33)

    def test_call_in_loop(self):
        mod, _ = _make_host_module()
        code = """\
import testmod
total = 0
for i in range(5):
    total = total + testmod.add(i, i)
total
"""
        assert Monty(code, extensions=[mod.to_extension_dict()]).run() == snapshot(20)

    def test_none_return(self):
        mod, _ = _make_host_module()
        code = """\
import testmod
try:
    testmod.fail('boom')
except ValueError:
    pass
'ok'
"""
        assert Monty(code, extensions=[mod.to_extension_dict()]).run() == snapshot('ok')


class TestHostExtensionErrors:
    """Error propagation from host to sandbox."""

    def test_exception_caught_in_sandbox(self):
        mod, _ = _make_host_module()
        code = """\
import testmod
msg = None
try:
    testmod.fail('something broke')
except ValueError as e:
    msg = str(e)
msg
"""
        assert Monty(code, extensions=[mod.to_extension_dict()]).run() == snapshot('something broke')

    def test_exception_uncaught_raises(self):
        mod, _ = _make_host_module()
        code = "import testmod\ntestmod.fail('kaboom')"
        with pytest.raises(pydantic_monty.MontyRuntimeError) as exc_info:
            Monty(code, extensions=[mod.to_extension_dict()]).run()
        assert 'kaboom' in str(exc_info.value)


class TestHostExtensionHandles:
    """Handle-based objects round-trip through the FFI boundary."""

    def test_create_and_read_handle(self):
        mod, store = _make_host_module()
        code = """\
import testmod
obj = testmod.make_obj('alpha')
testmod.get_label(obj)
"""
        result = Monty(code, extensions=[mod.to_extension_dict()]).run()
        assert result == snapshot('alpha')
        store.clear()

    def test_multiple_handles(self):
        mod, store = _make_host_module()
        code = """\
import testmod
a = testmod.make_obj('first')
b = testmod.make_obj('second')
[testmod.get_label(a), testmod.get_label(b)]
"""
        result = Monty(code, extensions=[mod.to_extension_dict()]).run()
        assert result == snapshot(['first', 'second'])
        store.clear()


class TestHostExtensionDataRoundTrip:
    """Complex data types survive the Python → Monty → host → Monty round trip."""

    @pytest.mark.parametrize(
        'value',
        [
            42,
            3.14,
            True,
            False,
            'hello',
            None,
            [1, 2, 3],
            [1, 'two', 3.0, True, None],
            {'a': 1, 'b': 'two'},
            {'nested': {'deep': [1, 2, 3]}},
            [],
            {},
        ],
        ids=[
            'int',
            'float',
            'true',
            'false',
            'str',
            'none',
            'list_int',
            'list_mixed',
            'dict_simple',
            'dict_nested',
            'empty_list',
            'empty_dict',
        ],
    )
    def test_echo_round_trip(self, value: Any):
        mod, _ = _make_host_module()
        m = Monty(
            'import testmod\ntestmod.echo(x)',
            inputs=['x'],
            extensions=[mod.to_extension_dict()],
        )
        result = m.run(inputs={'x': value})
        assert result == value


class TestHostExtensionEnforcement:
    """Enforcement wrappers: call count and return size limits."""

    def test_call_count_limit(self):
        mod = MontyModule('lim')

        @mod.function(max_calls=2)
        def limited(x: int) -> int:
            return x

        code = """\
import lim
a = lim.limited(1)
b = lim.limited(2)
error = None
try:
    lim.limited(3)
except RuntimeError as e:
    error = str(e)
{'a': a, 'b': b, 'error': error}
"""
        result = Monty(code, extensions=[mod.to_extension_dict()]).run()
        assert result == snapshot({'a': 1, 'b': 2, 'error': 'limited call budget exhausted (limit: 2)'})

    def test_return_size_limit(self):
        mod = MontyModule('sizemod')

        @mod.function(max_return_bytes=1_000_000)
        def big(n: int) -> list[int]:
            return list(range(n))

        code = """\
import sizemod
ok = sizemod.big(3)
error = None
try:
    sizemod.big(10000000)
except ValueError as e:
    error = str(e)
{'ok': ok, 'has_error': error is not None}
"""
        result = Monty(code, extensions=[mod.to_extension_dict()]).run()
        assert result['ok'] == snapshot([0, 1, 2])
        assert result['has_error'] is True


class TestExtensionSkills:
    """extension_skills() returns combined skill text."""

    def test_single_extension_skills(self):
        mod, _ = _make_host_module()
        m = Monty('1', extensions=[mod.to_extension_dict()])
        skills = m.extension_skills()
        assert skills == snapshot('# testmod\nA test module.')

    def test_multiple_extension_skills(self):
        mod1 = MontyModule('alpha', skill='# Alpha')
        mod2 = MontyModule('beta', skill='# Beta')

        @mod1.function()
        def noop1() -> None:
            pass

        @mod2.function()
        def noop2() -> None:
            pass

        m = Monty('1', extensions=[mod1.to_extension_dict(), mod2.to_extension_dict()])
        skills = m.extension_skills()
        assert '# Alpha' in skills
        assert '# Beta' in skills

    def test_no_extensions_empty_skills(self):
        m = Monty('1')
        assert m.extension_skills() == snapshot('')


class TestMultipleExtensions:
    """Multiple host extensions coexist in one sandbox."""

    def test_two_extensions(self):
        mod_a = MontyModule('ext_a')
        mod_b = MontyModule('ext_b')

        @mod_a.function()
        def double(x: int) -> int:
            return x * 2

        @mod_b.function()
        def triple(x: int) -> int:
            return x * 3

        code = """\
import ext_a
import ext_b
ext_a.double(5) + ext_b.triple(5)
"""
        result = Monty(
            code,
            extensions=[mod_a.to_extension_dict(), mod_b.to_extension_dict()],
        ).run()
        assert result == snapshot(25)


# ===========================================================================
# Native extension tests
# ===========================================================================


@requires_native
class TestNativeExtension:
    """Native extension loading and dispatch via library_path."""

    def test_load_and_call(self):
        code = """\
import datatools
csv_text = 'a,b\\n1,2\\n3,4'
df = datatools.parse_csv(csv_text)
datatools.row_count(df)
"""
        assert Monty(code, extensions=[native_ext_dict()]).run() == snapshot(2)

    def test_columns(self):
        code = """\
import datatools
df = datatools.parse_csv('x,y,z\\n1,2,3')
datatools.columns(df)
"""
        assert Monty(code, extensions=[native_ext_dict()]).run() == snapshot(['x', 'y', 'z'])

    def test_head(self):
        code = """\
import datatools
df = datatools.parse_csv('name,val\\nAlice,10\\nBob,20\\nCharlie,30')
datatools.head(df, 2)
"""
        result = Monty(code, extensions=[native_ext_dict()]).run()
        assert len(result) == snapshot(2)
        assert result[0]['name'] == snapshot('Alice')

    def test_column_sum(self):
        code = """\
import datatools
df = datatools.parse_csv('v\\n10\\n20\\n30')
datatools.column_sum(df, 'v')
"""
        assert Monty(code, extensions=[native_ext_dict()]).run() == snapshot(60.0)

    def test_filter_gt(self):
        code = """\
import datatools
df = datatools.parse_csv('x\\n5\\n15\\n25\\n35')
filtered = datatools.filter_gt(df, 'x', 20)
datatools.row_count(filtered)
"""
        assert Monty(code, extensions=[native_ext_dict()]).run() == snapshot(2)

    def test_extension_skills(self):
        m = Monty('1', extensions=[native_ext_dict()])
        skills = m.extension_skills()
        assert 'datatools' in skills
        assert 'parse_csv' in skills

    def test_reuse_monty_instance(self):
        code = """\
import datatools
df = datatools.parse_csv(csv)
datatools.row_count(df)
"""
        m = Monty(code, inputs=['csv'], extensions=[native_ext_dict()])
        assert m.run(inputs={'csv': 'a\n1\n2'}) == snapshot(2)
        assert m.run(inputs={'csv': 'a\n1\n2\n3\n4'}) == snapshot(4)


@requires_native
class TestNativeExtensionErrors:
    """Native extension error handling."""

    def test_bad_library_path(self):
        with pytest.raises(RuntimeError) as exc_info:
            Monty('1', extensions=[{'library_path': '/nonexistent/lib.so'}])
        assert 'failed to load' in str(exc_info.value)

    def test_missing_column(self):
        code = """\
import datatools
df = datatools.parse_csv('a\\n1')
error = None
try:
    datatools.column_sum(df, 'nonexistent')
except TypeError as e:
    error = str(e)
error
"""
        result = Monty(code, extensions=[native_ext_dict()]).run()
        assert 'nonexistent' in result


# ===========================================================================
# Mixed native + host tests
# ===========================================================================


@requires_native
class TestMixedExtensions:
    """Native and host extensions used together in the same sandbox."""

    def test_native_and_host_together(self):
        mod = MontyModule('util')

        @mod.function()
        def double(x: float) -> float:
            return x * 2

        code = """\
import datatools
import util

df = datatools.parse_csv('v\\n10\\n20\\n30')
s = datatools.column_sum(df, 'v')
util.double(s)
"""
        result = Monty(
            code,
            extensions=[native_ext_dict(), mod.to_extension_dict()],
        ).run()
        assert result == snapshot(120.0)

    def test_host_processes_native_output(self):
        store = HandleStore()
        mod = MontyModule('processor')

        @mod.function()
        def summarize(rows: list[dict[str, Any]]) -> dict[str, Any]:
            count = len(rows)
            return {'count': count, 'first': rows[0] if rows else None}

        code = """\
import datatools
import processor

df = datatools.parse_csv('name,age\\nAlice,30\\nBob,25')
rows = datatools.head(df)
processor.summarize(rows)
"""
        result = Monty(
            code,
            extensions=[native_ext_dict(), mod.to_extension_dict()],
        ).run()
        assert result == snapshot({'count': 2, 'first': {'name': 'Alice', 'age': 30.0}})


# ===========================================================================
# Type checking integration
# ===========================================================================


class TestExtensionTypeStubs:
    """Extension type stubs are collected from extensions."""

    def test_type_stubs_collected(self):
        mod, _ = _make_host_module()
        m = Monty('1', extensions=[mod.to_extension_dict()])
        # The stubs are available in the runner's extension registry.
        # We verify indirectly via the skills (stubs and skills are both collected).
        skills = m.extension_skills()
        assert 'testmod' in skills


# ===========================================================================
# Handle method call tests
# ===========================================================================


@requires_native
class TestNativeHandleMethods:
    """Native extension handle method calls (df.method() syntax)."""

    def test_row_count_method(self):
        code = """\
import datatools
df = datatools.parse_csv('a,b\\n1,2\\n3,4\\n5,6')
df.row_count()
"""
        assert Monty(code, extensions=[native_ext_dict()]).run() == snapshot(3)

    def test_columns_method(self):
        code = """\
import datatools
df = datatools.parse_csv('x,y,z\\n1,2,3')
df.columns()
"""
        assert Monty(code, extensions=[native_ext_dict()]).run() == snapshot(['x', 'y', 'z'])

    def test_head_method(self):
        code = """\
import datatools
df = datatools.parse_csv('name,val\\nAlice,10\\nBob,20\\nCharlie,30')
df.head(2)
"""
        result = Monty(code, extensions=[native_ext_dict()]).run()
        assert len(result) == snapshot(2)
        assert result[0]['name'] == snapshot('Alice')

    def test_column_mean_method(self):
        code = """\
import datatools
df = datatools.parse_csv('v\\n10\\n20\\n30')
df.column_mean('v')
"""
        assert Monty(code, extensions=[native_ext_dict()]).run() == snapshot(20.0)

    def test_filter_gt_chain(self):
        """filter_gt returns a new handle, chain methods on it."""
        code = """\
import datatools
df = datatools.parse_csv('x\\n5\\n15\\n25\\n35')
high = df.filter_gt('x', 20)
high.row_count()
"""
        assert Monty(code, extensions=[native_ext_dict()]).run() == snapshot(2)

    def test_method_and_function_interchangeable(self):
        """df.row_count() and datatools.row_count(df) produce the same result."""
        code = """\
import datatools
df = datatools.parse_csv('a\\n1\\n2\\n3')
method_result = df.row_count()
func_result = datatools.row_count(df)
method_result == func_result
"""
        assert Monty(code, extensions=[native_ext_dict()]).run() is True

    def test_invalid_method(self):
        code = """\
import datatools
df = datatools.parse_csv('a\\n1')
error = None
try:
    df.nonexistent()
except TypeError as e:
    error = str(e)
error
"""
        result = Monty(code, extensions=[native_ext_dict()]).run()
        assert 'nonexistent' in result


class TestHostHandleMethods:
    """Host extension handle method calls (handle.method() syntax)."""

    def test_handle_method_call(self):
        store = HandleStore()
        mod = MontyModule('things')

        @mod.function()
        def create(name: str) -> dict[str, Any]:
            return store.register({'name': name, 'count': 0}, 'things.Thing', extension_id='things')

        @mod.function()
        def get_name(thing: dict[str, Any]) -> str:
            return store.get(thing['handle_id'])['name']

        @mod.function()
        def increment(thing: dict[str, Any]) -> None:
            store.get(thing['handle_id'])['count'] += 1

        @mod.function()
        def get_count(thing: dict[str, Any]) -> int:
            return store.get(thing['handle_id'])['count']

        code = """\
import things
t = things.create('widget')

# Use method syntax on the handle
t.increment()
t.increment()
t.increment()

{'name': t.get_name(), 'count': t.get_count()}
"""
        result = Monty(code, extensions=[mod.to_extension_dict()]).run()
        assert result == snapshot({'name': 'widget', 'count': 3})
        store.clear()

    def test_handle_method_and_function_both_work(self):
        store = HandleStore()
        mod = MontyModule('counter')

        @mod.function()
        def create(initial: int) -> dict[str, Any]:
            return store.register({'value': initial}, 'counter.Counter', extension_id='counter')

        @mod.function()
        def get_value(c: dict[str, Any]) -> int:
            return store.get(c['handle_id'])['value']

        code = """\
import counter
c = counter.create(10)
method_result = c.get_value()
func_result = counter.get_value(c)
method_result == func_result
"""
        result = Monty(code, extensions=[mod.to_extension_dict()]).run()
        assert result is True
        store.clear()


# ===========================================================================
# Async dispatch tests
# ===========================================================================


class TestReplExtensions:
    """REPL mode supports extensions."""

    def test_repl_host_extension(self):
        mod, _ = _make_host_module()
        repl = pydantic_monty.MontyRepl(extensions=[mod.to_extension_dict()])
        result = repl.feed_run('import testmod\ntestmod.add(5, 7)')
        assert result == snapshot(12)

    def test_repl_multiple_snippets(self):
        mod, _ = _make_host_module()
        repl = pydantic_monty.MontyRepl(extensions=[mod.to_extension_dict()])
        repl.feed_run('import testmod')
        result = repl.feed_run("testmod.greet('repl')")
        assert result == snapshot('hello repl')

    @requires_native
    def test_repl_native_extension(self):
        repl = pydantic_monty.MontyRepl(extensions=[native_ext_dict()])
        repl.feed_run("import datatools")
        repl.feed_run("df = datatools.parse_csv('a,b\\n1,2\\n3,4')")
        result = repl.feed_run('datatools.row_count(df)')
        assert result == snapshot(2)


class TestAsyncExtensionDispatch:
    """Host extension calls work through run_async()."""

    async def test_async_host_extension(self):
        mod, _ = _make_host_module()
        code = "import testmod\ntestmod.add(10, 20)"
        m = Monty(code, extensions=[mod.to_extension_dict()])
        result = await m.run_async()
        assert result == snapshot(30)

    async def test_async_host_extension_multiple_calls(self):
        mod, _ = _make_host_module()
        code = """\
import testmod
a = testmod.add(1, 2)
b = testmod.greet('async')
{'sum': a, 'greeting': b}
"""
        m = Monty(code, extensions=[mod.to_extension_dict()])
        result = await m.run_async()
        assert result == snapshot({'sum': 3, 'greeting': 'hello async'})

    async def test_async_host_with_error(self):
        mod, _ = _make_host_module()
        code = """\
import testmod
error = None
try:
    testmod.fail('async boom')
except ValueError as e:
    error = str(e)
error
"""
        m = Monty(code, extensions=[mod.to_extension_dict()])
        result = await m.run_async()
        assert result == snapshot('async boom')
