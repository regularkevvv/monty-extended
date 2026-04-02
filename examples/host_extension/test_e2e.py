"""End-to-end tests for host-backed extensions.

Exercises challenging scenarios: stateful handles, error propagation,
enforcement wrappers, kwargs, nested data, multiple extensions, and
combining native + host extensions in the same sandbox.
"""

from __future__ import annotations

import platform
import sys
from pathlib import Path
from typing import Any

from pydantic_monty import HandleStore, Monty, MontyModule

# ---------------------------------------------------------------------------
# 1. Stateful extension: key-value store with transactions
# ---------------------------------------------------------------------------

kv_store_data: dict[str, Any] = {}
kv_tx_log: list[dict[str, Any]] = []

kv = MontyModule(
    'kv',
    skill='# kv -- transactional key-value store',
    version='0.1.0',
)


@kv.function()
def put(key: str, value: Any) -> None:
    kv_store_data[key] = value
    kv_tx_log.append({'op': 'put', 'key': key})


@kv.function()
def get(key: str) -> Any:
    if key not in kv_store_data:
        raise KeyError(f'key not found: {key!r}')
    return kv_store_data[key]


@kv.function()
def delete(key: str) -> bool:
    if key in kv_store_data:
        del kv_store_data[key]
        kv_tx_log.append({'op': 'delete', 'key': key})
        return True
    return False


@kv.function()
def keys() -> list[str]:
    return sorted(kv_store_data.keys())


@kv.function()
def tx_count() -> int:
    return len(kv_tx_log)


# ---------------------------------------------------------------------------
# 2. Extension with handles: graph builder
# ---------------------------------------------------------------------------

graph_store = HandleStore()

graph = MontyModule(
    'graph',
    skill='# graph -- directed graph builder with handles',
    version='0.1.0',
)


class Graph:
    def __init__(self) -> None:
        self.nodes: set[str] = set()
        self.edges: list[tuple[str, str, float]] = []

    def add_node(self, name: str) -> None:
        self.nodes.add(name)

    def add_edge(self, src: str, dst: str, weight: float) -> None:
        self.nodes.add(src)
        self.nodes.add(dst)
        self.edges.append((src, dst, weight))

    def neighbors(self, node: str) -> list[dict[str, Any]]:
        return [
            {'node': dst, 'weight': w}
            for src, dst, w in self.edges
            if src == node
        ]

    def shortest_path_weight(self, start: str, end: str) -> float | None:
        """Dijkstra's shortest path — real computation in the host."""
        import heapq

        dist: dict[str, float] = {n: float('inf') for n in self.nodes}
        dist[start] = 0.0
        heap = [(0.0, start)]

        while heap:
            d, u = heapq.heappop(heap)
            if u == end:
                return d
            if d > dist[u]:
                continue
            for src, dst, w in self.edges:
                if src == u and dist[u] + w < dist[dst]:
                    dist[dst] = dist[u] + w
                    heapq.heappush(heap, (dist[dst], dst))

        return None


@graph.function()
def create() -> dict[str, Any]:
    return graph_store.register(Graph(), 'graph.Graph', extension_id='graph')


@graph.function()
def add_node(g: dict[str, Any], name: str) -> None:
    graph_store.get(g['handle_id']).add_node(name)


@graph.function()
def add_edge(g: dict[str, Any], src: str, dst: str, weight: float) -> None:
    graph_store.get(g['handle_id']).add_edge(src, dst, weight)


@graph.function()
def node_count(g: dict[str, Any]) -> int:
    return len(graph_store.get(g['handle_id']).nodes)


@graph.function()
def edge_count(g: dict[str, Any]) -> int:
    return len(graph_store.get(g['handle_id']).edges)


@graph.function()
def neighbors(g: dict[str, Any], node: str) -> list[dict[str, Any]]:
    return graph_store.get(g['handle_id']).neighbors(node)


@graph.function()
def shortest_path(g: dict[str, Any], start: str, end: str) -> float:
    result = graph_store.get(g['handle_id']).shortest_path_weight(start, end)
    if result is None:
        raise ValueError(f'no path from {start!r} to {end!r}')
    return result


# ---------------------------------------------------------------------------
# 3. Extension with enforcement limits
# ---------------------------------------------------------------------------

limited = MontyModule('limited', skill='# limited -- enforcement test')


@limited.function(max_calls=3)
def call_limited(x: int) -> int:
    return x * 2


@limited.function(max_return_bytes=100)
def big_return(n: int) -> list[int]:
    return list(range(n))


# ---------------------------------------------------------------------------
# 4. Extension that processes complex nested data
# ---------------------------------------------------------------------------

transformer = MontyModule('transformer', skill='# transformer -- data transforms')


@transformer.function()
def flatten(nested: list[Any]) -> list[Any]:
    """Recursively flatten a nested list."""
    result: list[Any] = []
    for item in nested:
        if isinstance(item, list):
            result.extend(flatten(item))
        else:
            result.append(item)
    return result


@transformer.function()
def group_by(records: list[dict[str, Any]], key: str) -> dict[str, list[dict[str, Any]]]:
    """Group a list of dicts by a key field."""
    groups: dict[str, list[dict[str, Any]]] = {}
    for record in records:
        k = str(record[key])
        groups.setdefault(k, []).append(record)
    return groups


@transformer.function()
def aggregate(records: list[dict[str, Any]], field: str, op: str) -> float:
    """Aggregate a numeric field: sum, mean, min, max."""
    values = [r[field] for r in records]
    if op == 'sum':
        return sum(values)
    if op == 'mean':
        return sum(values) / len(values)
    if op == 'min':
        return min(values)
    if op == 'max':
        return max(values)
    raise ValueError(f'unknown op: {op!r}')


# ===========================================================================
# Tests
# ===========================================================================


def test_stateful_kv_store():
    """Sandbox code drives a stateful key-value store across multiple calls."""
    kv_store_data.clear()
    kv_tx_log.clear()

    code = """\
import kv

kv.put('name', 'Alice')
kv.put('age', 30)
kv.put('scores', [95, 87, 92])

name = kv.get('name')
age = kv.get('age')
scores = kv.get('scores')

kv.put('age', 31)
kv.delete('scores')

{
    'name': name,
    'age': age,
    'scores': scores,
    'final_keys': kv.keys(),
    'tx_count': kv.tx_count(),
}
"""
    result = Monty(code, extensions=[kv.to_extension_dict()]).run()

    assert result['name'] == 'Alice', f'name: {result["name"]}'
    assert result['age'] == 30, f'age: {result["age"]}'
    assert result['scores'] == [95, 87, 92], f'scores: {result["scores"]}'
    assert result['final_keys'] == ['age', 'name'], f'keys: {result["final_keys"]}'
    # put x3 + put(age again) + delete = 5 transactions
    assert result['tx_count'] == 5, f'tx_count: {result["tx_count"]}'
    print('  PASS: stateful_kv_store')


def test_error_propagation():
    """Host exceptions propagate correctly into sandbox try/except."""
    kv_store_data.clear()
    kv_tx_log.clear()

    code = """\
import kv

errors = []

try:
    kv.get('nonexistent')
except KeyError as e:
    errors.append(str(e))

kv.put('x', 42)
try:
    kv.get('y')
except KeyError as e:
    errors.append(str(e))

got_x = kv.get('x')

{
    'errors': errors,
    'got_x': got_x,
}
"""
    result = Monty(code, extensions=[kv.to_extension_dict()]).run()

    assert len(result['errors']) == 2, f'expected 2 errors, got {result["errors"]}'
    assert 'nonexistent' in result['errors'][0], f'error[0]: {result["errors"][0]}'
    assert 'y' in result['errors'][1], f'error[1]: {result["errors"][1]}'
    assert result['got_x'] == 42
    print('  PASS: error_propagation')


def test_graph_handles():
    """Build a graph with handles, run Dijkstra in the host."""
    graph_store.clear()

    code = """\
import graph

g = graph.create()

graph.add_edge(g, 'A', 'B', 1.0)
graph.add_edge(g, 'B', 'C', 2.0)
graph.add_edge(g, 'A', 'C', 10.0)
graph.add_edge(g, 'C', 'D', 1.0)
graph.add_edge(g, 'B', 'D', 5.0)

nc = graph.node_count(g)
ec = graph.edge_count(g)
nb = graph.neighbors(g, 'A')

# shortest A->D: A->B(1) + B->C(2) + C->D(1) = 4.0
sp = graph.shortest_path(g, 'A', 'D')

{
    'nodes': nc,
    'edges': ec,
    'a_neighbors': nb,
    'shortest_a_d': sp,
}
"""
    result = Monty(code, extensions=[graph.to_extension_dict()]).run()

    assert result['nodes'] == 4, f'nodes: {result["nodes"]}'
    assert result['edges'] == 5, f'edges: {result["edges"]}'

    nb = result['a_neighbors']
    assert len(nb) == 2, f'neighbors: {nb}'
    assert nb[0]['node'] == 'B'
    assert nb[0]['weight'] == 1.0
    assert nb[1]['node'] == 'C'
    assert nb[1]['weight'] == 10.0

    assert result['shortest_a_d'] == 4.0, f'shortest: {result["shortest_a_d"]}'
    print('  PASS: graph_handles')


def test_multiple_handles():
    """Multiple independent graph handles coexist."""
    graph_store.clear()

    code = """\
import graph

g1 = graph.create()
g2 = graph.create()

graph.add_edge(g1, 'X', 'Y', 1.0)
graph.add_edge(g2, 'A', 'B', 2.0)
graph.add_edge(g2, 'B', 'C', 3.0)

{
    'g1_nodes': graph.node_count(g1),
    'g1_edges': graph.edge_count(g1),
    'g2_nodes': graph.node_count(g2),
    'g2_edges': graph.edge_count(g2),
}
"""
    result = Monty(code, extensions=[graph.to_extension_dict()]).run()

    assert result['g1_nodes'] == 2
    assert result['g1_edges'] == 1
    assert result['g2_nodes'] == 3
    assert result['g2_edges'] == 2
    print('  PASS: multiple_handles')


def test_call_count_enforcement():
    """Call count budget is enforced — 4th call fails."""
    code = """\
import limited

results = []
results.append(limited.call_limited(1))
results.append(limited.call_limited(2))
results.append(limited.call_limited(3))

error = None
try:
    limited.call_limited(4)
except RuntimeError as e:
    error = str(e)

{
    'results': results,
    'error': error,
}
"""
    result = Monty(code, extensions=[limited.to_extension_dict()]).run()

    assert result['results'] == [2, 4, 6], f'results: {result["results"]}'
    assert result['error'] is not None, 'expected error on 4th call'
    assert 'budget exhausted' in result['error'], f'error: {result["error"]}'
    print('  PASS: call_count_enforcement')


def test_return_size_enforcement():
    """Return size cap triggers ValueError on oversized return."""
    code = """\
import limited

small = limited.big_return(3)

error = None
try:
    limited.big_return(10000)
except ValueError as e:
    error = str(e)

{
    'small': small,
    'error': error,
}
"""
    result = Monty(code, extensions=[limited.to_extension_dict()]).run()

    assert result['small'] == [0, 1, 2], f'small: {result["small"]}'
    assert result['error'] is not None, 'expected size error'
    assert 'byte limit' in result['error'], f'error: {result["error"]}'
    print('  PASS: return_size_enforcement')


def test_nested_data_transforms():
    """Complex nested data round-trips through the host correctly."""
    code = """\
import transformer

nested = [1, [2, 3], [4, [5, 6]], [[7, [8]], 9]]
flat = transformer.flatten(nested)

records = [
    {'dept': 'eng', 'name': 'Alice', 'salary': 120000},
    {'dept': 'eng', 'name': 'Bob', 'salary': 110000},
    {'dept': 'sales', 'name': 'Carol', 'salary': 95000},
    {'dept': 'sales', 'name': 'Dave', 'salary': 105000},
    {'dept': 'eng', 'name': 'Eve', 'salary': 130000},
]

grouped = transformer.group_by(records, 'dept')
eng_mean = transformer.aggregate(grouped['eng'], 'salary', 'mean')
sales_max = transformer.aggregate(grouped['sales'], 'salary', 'max')

{
    'flat': flat,
    'eng_count': len(grouped['eng']),
    'sales_count': len(grouped['sales']),
    'eng_mean': eng_mean,
    'sales_max': sales_max,
}
"""
    result = Monty(code, extensions=[transformer.to_extension_dict()]).run()

    assert result['flat'] == [1, 2, 3, 4, 5, 6, 7, 8, 9], f'flat: {result["flat"]}'
    assert result['eng_count'] == 3
    assert result['sales_count'] == 2
    assert result['eng_mean'] == 120000.0, f'eng_mean: {result["eng_mean"]}'
    assert result['sales_max'] == 105000.0, f'sales_max: {result["sales_max"]}'
    print('  PASS: nested_data_transforms')


def test_multiple_extensions_together():
    """Two host extensions used together in the same sandbox."""
    kv_store_data.clear()
    kv_tx_log.clear()
    graph_store.clear()

    code = """\
import kv
import graph

g = graph.create()
graph.add_edge(g, 'start', 'mid', 3.0)
graph.add_edge(g, 'mid', 'end', 4.0)

dist = graph.shortest_path(g, 'start', 'end')

kv.put('shortest_distance', dist)
kv.put('node_count', graph.node_count(g))

{
    'distance': kv.get('shortest_distance'),
    'nodes': kv.get('node_count'),
}
"""
    result = Monty(
        code,
        extensions=[kv.to_extension_dict(), graph.to_extension_dict()],
    ).run()

    assert result['distance'] == 7.0, f'distance: {result["distance"]}'
    assert result['nodes'] == 3, f'nodes: {result["nodes"]}'
    print('  PASS: multiple_extensions_together')


def test_host_and_native_together():
    """Combine a native extension (datatools) with a host extension (transformer)."""
    ext_dir = Path(__file__).parent.parent / 'native_extension' / 'target' / 'release'

    if platform.system() == 'Darwin':
        lib = ext_dir / 'libmonty_ext_datatools.dylib'
    elif platform.system() == 'Linux':
        lib = ext_dir / 'libmonty_ext_datatools.so'
    else:
        print('  SKIP: host_and_native_together (unsupported platform)')
        return

    if not lib.exists():
        print('  SKIP: host_and_native_together (native extension not built)')
        return

    code = """\
import datatools
import transformer

csv_text = 'name,score\\nAlice,95\\nBob,60\\nCharlie,85\\nDave,70\\nEve,90'
df = datatools.parse_csv(csv_text)

rows = datatools.head(df, 100)

scores = []
for row in rows:
    scores.append(row['score'])

mean = transformer.aggregate(
    [{'v': s} for s in scores],
    'v',
    'mean',
)

high = datatools.filter_gt(df, 'score', 80)
high_count = datatools.row_count(high)

{
    'total_rows': datatools.row_count(df),
    'mean_score': mean,
    'high_scorers': high_count,
}
"""
    result = Monty(
        code,
        extensions=[
            {'library_path': str(lib)},
            transformer.to_extension_dict(),
        ],
    ).run()

    assert result['total_rows'] == 5
    assert result['mean_score'] == 80.0, f'mean: {result["mean_score"]}'
    assert result['high_scorers'] == 3
    print('  PASS: host_and_native_together')


def test_sandbox_loop_with_host_calls():
    """Sandbox loop that makes host calls on each iteration."""
    kv_store_data.clear()
    kv_tx_log.clear()

    code = """\
import kv

for i in range(10):
    kv.put('item_' + str(i), i * i)

total = 0
for i in range(10):
    total = total + kv.get('item_' + str(i))

{
    'total': total,
    'key_count': len(kv.keys()),
    'tx_count': kv.tx_count(),
}
"""
    result = Monty(code, extensions=[kv.to_extension_dict()]).run()

    expected_total = sum(i * i for i in range(10))  # 0+1+4+9+16+25+36+49+64+81 = 285
    assert result['total'] == expected_total, f'total: {result["total"]}'
    assert result['key_count'] == 10
    assert result['tx_count'] == 10  # 10 puts
    print('  PASS: sandbox_loop_with_host_calls')


def test_extension_skills_multiple():
    """Skills from multiple extensions are concatenated."""
    code = 'x = 1'
    m = Monty(
        code,
        extensions=[kv.to_extension_dict(), graph.to_extension_dict()],
    )
    skills = m.extension_skills()
    assert 'kv' in skills
    assert 'graph' in skills
    print('  PASS: extension_skills_multiple')


if __name__ == '__main__':
    print(f'Python {sys.version}')
    print()
    print('Running host extension E2E tests...')
    print()

    test_stateful_kv_store()
    test_error_propagation()
    test_graph_handles()
    test_multiple_handles()
    test_call_count_enforcement()
    test_return_size_enforcement()
    test_nested_data_transforms()
    test_multiple_extensions_together()
    test_host_and_native_together()
    test_sandbox_loop_with_host_calls()
    test_extension_skills_multiple()

    print()
    print('All tests passed!')
