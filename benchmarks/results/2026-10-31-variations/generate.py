"""Generate deterministic workload variants without changing the upstream checkout."""
import argparse
import ast
import builtins
import copy
import csv
import hashlib
import io
import json
from pathlib import Path
import random

FAMILIES = ['game_nbody', 'perf_barnes_hut', 'ticket_search', 'perf_unpack_sequence',
            'text_json_transform_medium', 'text_csv_cleanup_medium', 'agent_unicode_chinese_small',
            'rustpython_micro_sorted_300', 'rustpython_base64', 'rustpython_generators']
MARKER = '# --- benchmark ---'


def rename_map(tree, seed=707):
    bound, protected = set(), set(dir(builtins))
    for node in ast.walk(tree):
        if isinstance(node, ast.Attribute):
            protected.add(node.attr)
        if isinstance(node, ast.keyword) and node.arg:
            protected.add(node.arg)
        if isinstance(node, ast.Name) and isinstance(node.ctx, ast.Store):
            bound.add(node.id)
        if isinstance(node, ast.arg):
            bound.add(node.arg)
        if isinstance(node, (ast.FunctionDef, ast.ClassDef)):
            bound.add(node.name)
        if isinstance(node, (ast.Import, ast.ImportFrom)):
            bound.update(alias.asname or alias.name.split('.')[0] for alias in node.names)
    return {name: 'v' + hashlib.sha256((str(seed) + ':' + name).encode()).hexdigest()[:9]
            for name in sorted(bound - protected) if not name.startswith('__')}


class Rename(ast.NodeTransformer):
    def __init__(self, names):
        self.names = names

    def visit_Name(self, node):
        node.id = self.names.get(node.id, node.id)
        return node

    def visit_arg(self, node):
        node.arg = self.names.get(node.arg, node.arg)
        return self.generic_visit(node)

    def visit_FunctionDef(self, node):
        node.name = self.names.get(node.name, node.name)
        return self.generic_visit(node)

    visit_ClassDef = visit_FunctionDef

    def visit_Import(self, node):
        for alias in node.names:
            old = alias.asname or alias.name.split('.')[0]
            if old in self.names:
                alias.asname = self.names[old]
        return node

    visit_ImportFrom = visit_Import

    def visit_Global(self, node):
        node.names = [self.names.get(name, name) for name in node.names]
        return node

    visit_Nonlocal = visit_Global


def data_variant(family, source, seed=707):
    if family == 'game_nbody':
        assert source.count('advance(0.01, DEFAULT_ITERATIONS)') == 1
        step=0.0065+(seed%5)*0.0005
        return source.replace('advance(0.01, DEFAULT_ITERATIONS)', f'advance({step:g}, DEFAULT_ITERATIONS)'), f'time step 0.01 -> {step:g}; same iterations'
    if family == 'perf_barnes_hut':
        theta=0.55+(seed%5)*0.05
        return source.replace('DEFAULT_THETA = 0.5', f'DEFAULT_THETA = {theta:g}'), f'theta 0.5 -> {theta:g}; same particles and iterations'
    if family == 'ticket_search':
        count=1700+seed%90
        return source.replace('TICKETS = 1600', f'TICKETS = {count}'), f'ticket count 1600 -> {count}'
    if family == 'perf_unpack_sequence':
        assert source.count('range(10)') == 2
        start=94+seed%100
        return source.replace('range(10)', f'range({start}, {start+10})'), 'ten different integer values; identical unpack count'
    if family == 'rustpython_micro_sorted_300':
        return source.replace('seed(0)', f'seed({seed})'), f'random seed 0 -> {seed}; same item count'
    if family == 'rustpython_base64':
        return source.replace('random.Random(12345)', f'random.Random({seed})'), f'random seed 12345 -> {seed}; same byte counts'
    if family == 'rustpython_generators':
        count=110000+seed%11
        return source.replace('range(100000)', f'range({count})'), f'tree nodes 100000 -> {count}'
    tree = ast.parse(source)
    assignment = next(node for node in tree.body if isinstance(node, ast.Assign)
                      and any(isinstance(target, ast.Name) and target.id == 'TEXT' for target in node.targets))
    text = ast.literal_eval(assignment.value)
    rng = random.Random(seed)
    if family == 'text_csv_cleanup_medium':
        rows = list(csv.reader(io.StringIO(text)))
        header, rows = rows[0], rows[1:]
        rng.shuffle(rows)
        for index, row in enumerate(rows):
            if row[1].strip() and index % 3 == 0:
                row[1] = row[1].replace('Zoë', '東京')
        output = io.StringIO(newline='')
        csv.writer(output, lineterminator='\n').writerows([header, *rows])
        changed = output.getvalue()
        description = 'shuffle complete CSV records; vary Unicode names; retain quoting/newlines'
    else:
        value = json.loads(text)
        key = 'records' if family == 'text_json_transform_medium' else 'results'
        rng.shuffle(value[key])
        if key == 'records':
            for index, row in enumerate(value[key]):
                row['amount_cents'] = (row['amount_cents'] + index * 13) % 20000
        changed = json.dumps(value, ensure_ascii=False, separators=(',', ':'))
        description = 'shuffle JSON records; vary amounts' if key == 'records' else 'shuffle Chinese search records; same text and record count'
    lines = source.splitlines(keepends=True)
    lines[assignment.lineno - 1:assignment.end_lineno] = ['TEXT = ' + repr(changed) + '\n']
    return ''.join(lines), description


def render(source, transformer=None):
    # Keep fixture preparation outside request timing in every version.
    parts = source.split(MARKER)
    assert len(parts) <= 2
    rendered = []
    for part in parts:
        tree = ast.parse(part)
        if transformer:
            tree = transformer.visit(tree)
        rendered.append(ast.unparse(ast.fix_missing_locations(tree)) + '\n')
    return ('\n' + MARKER + '\n').join(rendered)


def structural(family, source):
    tree = ast.parse(source)
    if family == 'perf_unpack_sequence':
        function = next(n for n in tree.body if isinstance(n, ast.FunctionDef) and n.name == 'do_unpacking')
        loop = next(n for n in function.body if isinstance(n, ast.For))
        assert len(loop.body) == 400 and all(isinstance(n, ast.Assign) for n in loop.body)
        loop.body = [ast.For(target=ast.Name(id='repeat_unpack', ctx=ast.Store()),
                            iter=ast.Call(func=ast.Name(id='range', ctx=ast.Load()), args=[ast.Constant(400)], keywords=[]),
                            body=[copy.deepcopy(loop.body[0])], orelse=[])]
        return ast.unparse(ast.fix_missing_locations(tree)) + '\n', '400 repeated statements -> 400-iteration inner loop; identical unpack count/output'
    class Helper(ast.NodeTransformer):
        count = 0
        def visit_BinOp(self, node):
            if ast.dump(node, include_attributes=False) == ast.dump(ast.parse('dx * dx + dy * dy + dz * dz', mode='eval').body, include_attributes=False):
                self.count += 1
                return ast.Call(func=ast.Name(id='distance_squared', ctx=ast.Load()),
                                args=[ast.Name(id=n, ctx=ast.Load()) for n in ['dx','dy','dz']], keywords=[])
            return self.generic_visit(node)
    helper = Helper()
    tree = helper.visit(tree)
    assert helper.count > 0
    tree.body.insert(0, ast.parse('def distance_squared(x, y, z):\n    return x*x + y*y + z*z\n').body[0])
    return ast.unparse(ast.fix_missing_locations(tree)) + '\n', f'extract squared-distance expression into a helper ({helper.count} call sites); identical output'


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--workloads', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--seed', type=int, default=707)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    manifest = {}
    for family in FAMILIES:
        source = (args.workloads / (family + '.py')).read_text()
        changed, description = data_variant(family, source, args.seed)
        variants = {'original': (source, 'verbatim upstream control'),
                    'canonical': (render(source), 'AST round-trip control; timing boundary retained'),
                    'renamed': (render(source, Rename(rename_map(ast.parse(source), args.seed))), 'deterministic binding renaming; attributes/keyword parameters retained'),
                    'data': (render(changed), description)}
        if family in ['game_nbody','perf_unpack_sequence']:
            variants['structure'] = structural(family, source)
        for variation, (code, description) in variants.items():
            name = family + '__' + variation
            path = args.output / (name + '.py')
            path.write_text(code)
            manifest[name] = {'family': family, 'variation': variation, 'description': description, 'seed': args.seed,
                              'sha256': hashlib.sha256(code.encode()).hexdigest(), 'source_bytes': len(code.encode()),
                              'original_sha256': hashlib.sha256(source.encode()).hexdigest()}
    (args.output / 'variations.json').write_text(json.dumps(manifest, indent=2))
    print(f'Generated {len(manifest)} programs across {len(FAMILIES)} families')


if __name__ == '__main__':
    main()
