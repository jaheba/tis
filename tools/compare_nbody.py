#!/usr/bin/env python3
"""Compare two built nbody_bench workers with serial, interleaved warm samples."""
import argparse
import datetime
import json
import platform
import random
import subprocess
from pathlib import Path

from bench_nbody import digest, summarize


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--baseline', type=Path, required=True)
    parser.add_argument('--candidate', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--samples', type=int, default=10)
    parser.add_argument('--seed', type=int, default=20261028)
    args = parser.parse_args()
    if args.samples < 10:
        parser.error('use at least 10 samples')
    args.output.mkdir(parents=True, exist_ok=False)
    binaries = {name: path.resolve() for name, path in
                [('baseline', args.baseline), ('candidate', args.candidate)]}
    results = dict(timestamp=datetime.datetime.now(datetime.timezone.utc).isoformat(),
                   platform=platform.platform(), samples=args.samples, warmups=2, seed=args.seed,
                   binaries={name: dict(path=str(path), sha256=digest(path))
                             for name, path in binaries.items()},
                   readiness={}, samples_raw=[])
    workers = {}

    def save():
        (args.output / 'comparison.json').write_text(json.dumps(results, indent=2) + '\n')

    try:
        for name, binary in binaries.items():
            for mode in ['bytecode', 'lir', 'native', 'copy-patch']:
                label = f'{name}/{mode}'
                process = subprocess.Popen([str(binary), '--tier', mode, '--warmups', '2'],
                                           stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                           stderr=subprocess.PIPE, text=True, bufsize=1)
                workers[label] = process
                line = process.stdout.readline()
                if not line:
                    raise RuntimeError((label, process.stderr.read()))
                readiness = json.loads(line)
                assert readiness['ready']
                results['readiness'][label] = readiness
                save()
                print(f'ready {label}', flush=True)
        rng = random.Random(args.seed)
        for round_index in range(args.samples):
            order = list(workers)
            rng.shuffle(order)
            for label in order:
                process = workers[label]
                process.stdin.write('run\n')
                process.stdin.flush()
                line = process.stdout.readline()
                if not line:
                    raise RuntimeError((label, process.stderr.read()))
                sample = json.loads(line)
                assert sample['energy'] == '-0.169089263'
                assert sample['backend_compile_ns'] == sample['deoptimizations'] == 0
                mode = label.split('/')[1]
                entry = {'lir': 'lir_entries', 'native': 'native_entries',
                         'copy-patch': 'copy_patch_entries'}.get(mode)
                if entry:
                    assert sample[entry] == 1
                results['samples_raw'].append(dict(label=label, round=round_index, **sample))
                save()
                print(f'{round_index + 1:2} {label:22} {sample["run_ns"] / 1e9:.6f}s', flush=True)
    finally:
        for process in workers.values():
            if process.poll() is None:
                process.stdin.write('quit\n')
                process.stdin.flush()
        for process in workers.values():
            process.wait(timeout=30)
            error = process.stderr.read()
            if process.returncode or error:
                raise RuntimeError((process.returncode, error))
    results['summary'] = {
        label: summarize([row['run_ns'] for row in results['samples_raw'] if row['label'] == label])
        for label in workers}
    save()
    print(json.dumps(results['summary'], indent=2))


if __name__ == '__main__':
    main()
