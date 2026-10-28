#!/usr/bin/env python3
"""Summarize macOS sample call trees using exclusive and inclusive sample counts."""
import argparse
from collections import Counter
import json
import gzip
from pathlib import Path
import re
import subprocess

NODE = re.compile(r'^([ +!:\t|]+)(\d+) (.+)$')


def parse(text):
    graph = text.split('Call graph:\n', 1)[1].split('\nTotal number in stack', 1)[0]
    roots, stack = [], []
    for line in graph.splitlines():
        match = NODE.match(line)
        if not match:
            continue
        prefix, count, frame = match.groups()
        name = frame.split('  (in ', 1)[0]
        if name.startswith('Thread_'):
            name = '<thread>'
        node = dict(name=name, count=int(count), children=[])
        depth = len(prefix)
        while stack and stack[-1][0] >= depth:
            stack.pop()
        if stack:
            stack[-1][1]['children'].append(node)
        else:
            roots.append(node)
        stack.append((depth, node))
    if not roots:
        raise ValueError('empty sample call tree')
    return roots


def summarize(roots):
    exclusive, inclusive = Counter(), Counter()
    paths = Counter()
    total = sum(node['count'] for node in roots)

    def visit(node, ancestors):
        path = (*ancestors, node['name'])
        own = node['count'] - sum(child['count'] for child in node['children'])
        if own < 0:
            raise ValueError(f'children exceed parent samples: {node["name"]}')
        exclusive[node['name']] += own
        paths[path] += own
        for name in set(path):
            inclusive[name] += own
        for child in node['children']:
            visit(child, path)

    for root in roots:
        visit(root, ())
    assert sum(exclusive.values()) == total
    return dict(total_samples=total,
                functions=[dict(name=name, exclusive=count, exclusive_percent=100 * count / total,
                                inclusive=inclusive[name], inclusive_percent=100 * inclusive[name] / total)
                           for name, count in exclusive.most_common()],
                paths=[dict(stack=list(path), samples=count, percent=100 * count / total)
                       for path, count in paths.most_common() if count])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('profiles', type=Path, nargs='+')
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--demangler', type=Path,
                        help='optional line-oriented Rust symbol demangler, such as rustfilt')
    args = parser.parse_args()
    texts = []
    for path in args.profiles:
        if path.suffix == '.gz':
            with gzip.open(path, 'rt') as stream:
                texts.append(stream.read())
        else:
            texts.append(path.read_text())
    if args.demangler:
        symbols = sorted(set(re.findall(r'\b_R[A-Za-z0-9_]+', '\n'.join(texts))))
        result = subprocess.run([str(args.demangler)], input='\n'.join(symbols) + '\n',
                                text=True, capture_output=True, check=True)
        names = dict(zip(symbols, result.stdout.splitlines(), strict=True))
        texts = [re.sub(r'\b_R[A-Za-z0-9_]+', lambda m: names[m[0]], text) for text in texts]
    summaries = []
    all_roots = []
    for path, text in zip(args.profiles, texts):
        roots = parse(text)
        all_roots.extend(roots)
        summaries.append(dict(path=path.name, **summarize(roots)))
    args.output.write_text(json.dumps(dict(profiles=summaries, combined=summarize(all_roots)), indent=2) + '\n')


if __name__ == '__main__':
    main()
