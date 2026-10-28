#!/usr/bin/env python3
"""Extract optimized advance traces and compare static operations with warmed tis LIR."""
import argparse
from collections import Counter
import json
from pathlib import Path
import re

SECTION = re.compile(r'^\[[^\]]+\] \{(jit-log-opt-(?:loop|bridge))\n(.*?)^\[[^\]]+\] \1\}', re.M | re.S)
OP = re.compile(r'(?:(?:[pif]\d+) = )?([a-z][a-z0-9_]*)\(')


def inspect_trace(text):
    lines = [re.sub(r'^\+\d+: ', '', line) for line in text.splitlines()]
    labels = [i for i, line in enumerate(lines) if line.startswith('label(')]
    jumps = [line for line in lines if line.startswith('jump(')]
    token = lambda line: re.search(r'descr=TargetToken\((\d+)\)', line)
    has_backedge = bool(labels and jumps and token(lines[labels[-1]]) and token(jumps[-1])
                        and token(lines[labels[-1]])[1] == token(jumps[-1])[1])
    body = lines[labels[-1]:] if has_backedge else lines[1:]
    operations = []
    for line in body:
        match = OP.match(line)
        if match and match[1] != 'debug_merge_point':
            operations.append((match[1], line))
    counts = Counter(op for op, _ in operations)
    return dict(header=lines[0], has_internal_backedge=has_backedge,
                static_operations=dict(sorted(counts.items())),
                calls=[line for op, line in operations if op.startswith(('call_', 'cond_call'))],
                allocations=[line for op, line in operations if op.startswith('new') or 'malloc' in op],
                float_array_reads=sum(op == 'getarrayitem_gc_f' and '<ArrayF 8>' in line for op, line in operations),
                float_array_writes=sum(op == 'setarrayitem_gc' and '<ArrayF 8>' in line for op, line in operations),
                loop_carried_float_arguments=len(re.findall(r'\bf\d+\b', lines[labels[-1]])) if has_backedge else None,
                body='\n'.join(line for _, line in operations) + '\n')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--log', type=Path, required=True)
    parser.add_argument('--tis-lir', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    traces = []
    for index, match in enumerate(SECTION.finditer(args.log.read_text())):
        text = match[2]
        if 'advance;' not in text:
            continue
        trace = inspect_trace(text)
        name = f'pypy-advance-{index + 1}'
        (args.output / (name + '.txt')).write_text(text)
        (args.output / (name + '-body.txt')).write_text(trace.pop('body'))
        traces.append(dict(path=name + '.txt', **trace))
    if not traces:
        raise ValueError('no advance traces found')
    lir = args.tis_lir.read_text()
    blocks = []
    for match in re.finditer(r'^(b\d+)\([^\n]*\):\n(.*?)(?=^b\d+\(|^exit\d+:|\Z)', lir, re.M | re.S):
        counts = Counter()
        for line in match[2].splitlines():
            operation = re.match(r'  r\d+:\w+ = (.*?) ; %', line)
            if operation:
                rhs = operation[1]
                if rhs.startswith('call '):
                    name = rhs.split(' [', 1)[0]
                else:
                    name = re.match(r'\w+', rhs)[0]
                counts[name] += 1
        blocks.append(dict(block=match[1], static_operations=dict(sorted(counts.items()))))
    (args.output / 'summary.json').write_text(json.dumps(dict(pypy=traces, tis=blocks), indent=2) + '\n')
    print(json.dumps(dict(pypy=[{k:v for k,v in t.items() if k != 'allocations'} for t in traces], tis=blocks), indent=2))


if __name__ == '__main__':
    main()
