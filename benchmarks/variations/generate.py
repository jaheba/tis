"""Generate standalone, deterministic workloads and record equivalent-source groups."""
import argparse
import hashlib
import json
from pathlib import Path
import random
import subprocess
import sys
import textwrap

def generate(output, seed, reference=sys.executable):
    output.mkdir(parents=True, exist_ok=False)
    cases = {}

    def add(family, variant, source, description, equivalent=True):
        name = family + '__' + variant
        source = textwrap.dedent(source).strip() + '\n'
        path = output / (name + '.py')
        path.write_text(source)
        result = subprocess.run([reference, str(path)],
                                capture_output=True, text=True, check=True)
        cases[name] = dict(family=family, variant=variant, description=description,
                           equivalent=equivalent, seed=seed, stdout=result.stdout,
                           sha256=hashlib.sha256(source.encode()).hexdigest())

    fill = '''
def fill(limit):
    flags = [True] * (limit + 1)
    stop = limit + 1
    BODY
    return flags.count(False), flags[0], flags[-1]
def bench():
    result = None
    for repeat in range(3):
        result = fill(300003)
    return result
# --- benchmark ---
print(bench())
'''
    loops = {
        'range': 'for cursor in range(1, stop, 3):\n        flags[cursor] = False',
        'counter': 'cursor = 1\n    while cursor < stop:\n        flags[cursor] = False\n        cursor += 3',
        'expression': 'cursor = 1\n    while cursor < limit + 1:\n        flags[cursor] = False\n        cursor += 3',
        'reversed': 'cursor = 1\n    while stop > cursor:\n        flags[cursor] = False\n        cursor += 3',
    }
    for variant, body in loops.items():
        add('fill', variant, fill.replace('BODY', body), variant + ' induction form')
    add('fill', 'data', fill.replace('BODY', loops['counter']).replace('300003', '390017')
        .replace('cursor', 'position_83').replace('+= 3', '+= 7'),
        'new size, stride and binding names', False)

    numeric = '''
def norm(x, y, z):
    return x*x + y*y + z*z
points = [[(i % 37) * 0.125, (i % 19) * 0.25, (i % 13) * 0.5] for i in range(800)]
def bench():
    total = 0.0
    for repeat in range(25):
        for point in points:
            x, y, z = point
            total += EXPR
    return int(total * 16)
# --- benchmark ---
print(bench())
'''
    add('numeric', 'inline', numeric.replace('EXPR', 'x*x + y*y + z*z'), 'inline squared norm')
    add('numeric', 'helper', numeric.replace('EXPR', 'norm(x, y, z)'), 'extract pure arithmetic helper')
    add('numeric', 'tuples', numeric.replace('EXPR', 'x*x + y*y + z*z').replace(
        '[[(', '[((').replace('0.5] for', '0.5) for'), 'tuple records instead of list records')
    add('numeric', 'data', numeric.replace('EXPR', 'norm(x, y, z)').replace('800', '1031')
        .replace('% 37', '% 41').replace('norm', 'squared_length_83'),
        'new record count and values; renamed helper', False)

    rng = random.Random(seed)
    words = ['key_' + str(rng.randrange(97)) for _ in range(2000)]
    dictionary = '''
words = WORDS
def bump(counts, word):
    counts[word] = counts.get(word, 0) + 1
def bench():
    counts = {}
    for repeat in range(15):
        for word in words:
            UPDATE
    total = 0
    for key in sorted(counts):
        total += len(key) * counts[key]
    return len(counts), total, sum(counts.values())
# --- benchmark ---
print(bench())
'''
    updates = {'get': 'counts[word] = counts.get(word, 0) + 1',
               'membership': 'if word in counts:\n                counts[word] += 1\n            else:\n                counts[word] = 1',
               'helper': 'bump(counts, word)'}
    for variant, update in updates.items():
        add('histogram', variant, dictionary.replace('WORDS', repr(words)).replace('UPDATE', update),
            variant + ' dictionary update')
    words = ['键🙂_' + word[4:] for word in reversed(words)]
    add('histogram', 'unicode', dictionary.replace('WORDS', repr(words)).replace('UPDATE', updates['get']),
        'Unicode keys; reverse logical input order', False)

    objects = '''
class Point:
    def __init__(self, x, y):
        self.x = x
        self.y = y
    def norm(self):
        return self.x*self.x + self.y*self.y
def norm(point):
    return point.x*point.x + point.y*point.y
points = [Point((i % 29) * 0.25, (i % 17) * 0.5) for i in range(600)]
def bench():
    total = 0.0
    for repeat in range(25):
        for point in points:
            total += EXPR
    return int(total * 16)
# --- benchmark ---
print(bench())
'''
    add('objects', 'fields', objects.replace('EXPR', 'point.x*point.x + point.y*point.y'), 'direct fields')
    add('objects', 'helper', objects.replace('EXPR', 'norm(point)'), 'helper receiving object')
    add('objects', 'method', objects.replace('EXPR', 'point.norm()'), 'bound method')
    add('objects', 'layout', objects.replace('EXPR', 'point.x*point.x + point.y*point.y')
        .replace('self.x = x\n        self.y = y', 'self.y = y\n        self.x = x')
        .replace('.x', '.horizontal').replace('.y', '.vertical').replace('Point', 'Vector83'),
        'renamed class/fields; reversed constructor stores')

    for family in {case['family'] for case in cases.values()}:
        outputs = {case['stdout'] for case in cases.values() if case['family'] == family and case['equivalent']}
        assert len(outputs) == 1, (family, outputs)
    (output / 'manifest.json').write_text(json.dumps(cases, indent=2, ensure_ascii=False))
    print(f'{len(cases)} workloads in {output}')


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--seed', type=int, default=1807)
    parser.add_argument('--python', default=sys.executable)
    args = parser.parse_args()
    generate(args.output, args.seed, args.python)
