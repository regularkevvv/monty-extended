"""End-to-end test for the datatools native extension.

Builds the extension `.dylib`/`.so`, loads it into Monty via the `library_path`
key, and exercises every exported function from sandboxed Python code.
"""

import platform
import subprocess
import sys
from pathlib import Path

# -- Locate the built shared library ------------------------------------------

EXTENSION_DIR = Path(__file__).parent
TARGET_DIR = EXTENSION_DIR / 'target' / 'release'

if platform.system() == 'Darwin':
    LIB_NAME = 'libmonty_ext_datatools.dylib'
elif platform.system() == 'Linux':
    LIB_NAME = 'libmonty_ext_datatools.so'
else:
    raise RuntimeError(f'unsupported platform: {platform.system()}')

LIB_PATH = TARGET_DIR / LIB_NAME

if not LIB_PATH.exists():
    print(f'Building native extension at {EXTENSION_DIR}...')
    subprocess.check_call(['cargo', 'build', '--release'], cwd=EXTENSION_DIR)

assert LIB_PATH.exists(), f'shared library not found at {LIB_PATH}'

# -- Import pydantic_monty ----------------------------------------------------

from pydantic_monty import Monty


def make_monty(code):
    """Create a Monty instance with the datatools native extension loaded."""
    return Monty(code, extensions=[{'library_path': str(LIB_PATH)}])


def test_parse_csv_and_row_count():
    """Basic CSV parsing and row counting."""
    code = """\
import datatools

csv_text = 'name,age,score\\nAlice,30,95\\nBob,25,87\\nCharlie,35,92'
df = datatools.parse_csv(csv_text)
datatools.row_count(df)
"""
    result = make_monty(code).run()
    assert result == 3, f'expected 3, got {result}'
    print('  PASS: parse_csv + row_count')


def test_columns():
    """Retrieve column names."""
    code = """\
import datatools

csv_text = 'name,age,score\\nAlice,30,95'
df = datatools.parse_csv(csv_text)
datatools.columns(df)
"""
    result = make_monty(code).run()
    assert result == ['name', 'age', 'score'], f'expected column list, got {result}'
    print('  PASS: columns')


def test_head():
    """Get first N rows as list of dicts."""
    code = """\
import datatools

csv_text = 'name,age\\nAlice,30\\nBob,25\\nCharlie,35'
df = datatools.parse_csv(csv_text)
datatools.head(df, 2)
"""
    result = make_monty(code).run()
    assert len(result) == 2, f'expected 2 rows, got {len(result)}'
    assert result[0]['name'] == 'Alice', f'expected Alice, got {result[0]}'
    assert result[1]['age'] == 25.0, f'expected 25.0, got {result[1]["age"]}'
    print('  PASS: head')


def test_column_sum():
    """Sum a numeric column."""
    code = """\
import datatools

csv_text = 'x,y\\n10,1\\n20,2\\n30,3'
df = datatools.parse_csv(csv_text)
datatools.column_sum(df, 'x')
"""
    result = make_monty(code).run()
    assert result == 60.0, f'expected 60.0, got {result}'
    print('  PASS: column_sum')


def test_column_mean():
    """Mean of a numeric column."""
    code = """\
import datatools

csv_text = 'val\\n10\\n20\\n30'
df = datatools.parse_csv(csv_text)
datatools.column_mean(df, 'val')
"""
    result = make_monty(code).run()
    assert result == 20.0, f'expected 20.0, got {result}'
    print('  PASS: column_mean')


def test_filter_gt():
    """Filter rows where column > threshold, then count."""
    code = """\
import datatools

csv_text = 'name,score\\nAlice,95\\nBob,60\\nCharlie,85\\nDave,70'
df = datatools.parse_csv(csv_text)
high = datatools.filter_gt(df, 'score', 80)
datatools.row_count(high)
"""
    result = make_monty(code).run()
    assert result == 2, f'expected 2 rows after filter, got {result}'
    print('  PASS: filter_gt')


def test_chained_operations():
    """Chain multiple operations: parse, filter, head, aggregate."""
    code = """\
import datatools

csv_text = 'product,price,qty\\nApple,1.5,100\\nBanana,0.75,200\\nCherry,3.0,50\\nDate,5.0,25\\nElderberry,8.0,10'
df = datatools.parse_csv(csv_text)

expensive = datatools.filter_gt(df, 'price', 2.0)

total = datatools.row_count(df)
exp_count = datatools.row_count(expensive)
cols = datatools.columns(expensive)
rows = datatools.head(expensive)
mean_price = datatools.column_mean(expensive, 'price')

{
    'total': total,
    'expensive_count': exp_count,
    'columns': cols,
    'rows': rows,
    'mean_price': mean_price,
}
"""
    result = make_monty(code).run()

    assert result['total'] == 5, f'total: {result["total"]}'
    assert result['expensive_count'] == 3, f'expensive_count: {result["expensive_count"]}'
    assert result['columns'] == ['product', 'price', 'qty'], f'columns: {result["columns"]}'
    assert len(result['rows']) == 3, f'rows count: {len(result["rows"])}'

    # Mean of 3.0, 5.0, 8.0 = 16/3
    mean = result['mean_price']
    assert abs(mean - 16.0 / 3.0) < 0.001, f'mean_price: {mean}'

    print('  PASS: chained_operations')


def test_extension_skills():
    """Verify that extension_skills() returns the skill text."""
    code = 'x = 1'
    m = make_monty(code)
    skills = m.extension_skills()
    assert 'datatools' in skills, f'expected "datatools" in skills, got: {skills[:100]}'
    assert 'parse_csv' in skills, f'expected "parse_csv" in skills'
    print('  PASS: extension_skills')


def test_multiple_dataframes():
    """Create multiple DataFrames and operate on them independently."""
    code = """\
import datatools

csv1 = 'a,b\\n1,2\\n3,4'
csv2 = 'x,y,z\\n10,20,30\\n40,50,60\\n70,80,90'

df1 = datatools.parse_csv(csv1)
df2 = datatools.parse_csv(csv2)

{
    'df1_rows': datatools.row_count(df1),
    'df2_rows': datatools.row_count(df2),
    'df1_cols': datatools.columns(df1),
    'df2_cols': datatools.columns(df2),
    'sum_a': datatools.column_sum(df1, 'a'),
    'sum_z': datatools.column_sum(df2, 'z'),
}
"""
    result = make_monty(code).run()
    assert result['df1_rows'] == 2
    assert result['df2_rows'] == 3
    assert result['df1_cols'] == ['a', 'b']
    assert result['df2_cols'] == ['x', 'y', 'z']
    assert result['sum_a'] == 4.0
    assert result['sum_z'] == 180.0
    print('  PASS: multiple_dataframes')


if __name__ == '__main__':
    print(f'Python {sys.version}')
    print(f'Library: {LIB_PATH}')
    print()
    print('Running native extension E2E tests...')
    print()

    test_parse_csv_and_row_count()
    test_columns()
    test_head()
    test_column_sum()
    test_column_mean()
    test_filter_gt()
    test_chained_operations()
    test_extension_skills()
    test_multiple_dataframes()

    print()
    print('All tests passed!')
