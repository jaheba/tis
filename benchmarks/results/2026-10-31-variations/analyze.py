"""Compare variant/control medians, normalized by CPython's matching cost change."""
import argparse
import json
from pathlib import Path

parser = argparse.ArgumentParser()
parser.add_argument('results', type=Path)
args = parser.parse_args()
for mode in ['one-shot', 'repeated']:
    rows = {tier: json.loads((args.results / mode / (tier + '.json')).read_text())['benchmarks']
            for tier in ['bytecode', 'cnp', 'cranelift', 'cpython']}
    print(mode)
    print('| Workload | Tier | Control ms | Variant ms | Raw ratio | CPython-normalized ratio |')
    print('| --- | --- | ---: | ---: | ---: | ---: |')
    for family in ['game_nbody', 'sieve', 'perf_unpack_sequence']:
        control, variant = family + '__canonical', family + '__structure'
        python_ratio = rows['cpython'][variant]['median_wall_seconds'] / rows['cpython'][control]['median_wall_seconds']
        for tier in ['bytecode', 'cnp', 'cranelift']:
            old, new = rows[tier][control], rows[tier][variant]
            assert old['status'] == new['status'] == 'ok'
            old, new = old['median_wall_seconds'], new['median_wall_seconds']
            ratio = new / old
            print(f'| {family} | {tier} | {old*1000:.3f} | {new*1000:.3f} | {ratio:.3f} | {ratio/python_ratio:.3f} |')
