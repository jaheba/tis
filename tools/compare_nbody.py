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
    parser.add_argument('--samples', type=int, default=3)
    parser.add_argument('--warmups', type=int, default=1)
    parser.add_argument('--fresh-processes', action='store_true',
                        help='warm a new worker for every sample to expose process layout variation')
    parser.add_argument('--benchmark',choices=['barnes-hut','ticket-search'],help='compare generic bench workers instead of nbody')
    parser.add_argument('--seed', type=int, default=20261028)
    parser.add_argument('--tiers', nargs='+', choices=['bytecode', 'lir', 'native', 'copy-patch'],
                        default=['bytecode', 'lir', 'native', 'copy-patch'])
    parser.add_argument('--baseline-bytecode', choices=['stack', 'register'])
    parser.add_argument('--candidate-bytecode', choices=['stack', 'register'])
    parser.add_argument('--baseline-numeric', choices=['disabled', 'adaptive'])
    parser.add_argument('--candidate-numeric', choices=['disabled', 'adaptive'])
    parser.add_argument('--baseline-mutation-regions', action='store_true')
    parser.add_argument('--candidate-mutation-regions', action='store_true')
    parser.add_argument('--baseline-readonly-regions', action='store_true')
    parser.add_argument('--candidate-readonly-regions', action='store_true')
    parser.add_argument('--baseline-cfg-inlining', action='store_true')
    parser.add_argument('--candidate-cfg-inlining', action='store_true')
    args = parser.parse_args()
    if (args.baseline_cfg_inlining or args.candidate_cfg_inlining or args.baseline_readonly_regions or args.candidate_readonly_regions or args.baseline_mutation_regions or args.candidate_mutation_regions) and not args.benchmark:
        parser.error('native region and CFG inlining experiments require --benchmark')
    if args.samples < 1 or args.warmups < 1:
        parser.error('use at least one sample and one warmup')
    args.output.mkdir(parents=True, exist_ok=False)
    binaries = {name: path.resolve() for name, path in
                [('baseline', args.baseline), ('candidate', args.candidate)]}
    results = dict(timestamp=datetime.datetime.now(datetime.timezone.utc).isoformat(),
                   platform=platform.platform(), samples=args.samples, warmups=args.warmups, seed=args.seed, tiers=args.tiers,
                   binaries={name: dict(path=str(path), sha256=digest(path))
                             for name, path in binaries.items()},
                   bytecode_modes={'baseline': args.baseline_bytecode, 'candidate': args.candidate_bytecode},
                   numeric_modes={'baseline': args.baseline_numeric, 'candidate': args.candidate_numeric},
                   mutation_regions={'baseline':args.baseline_mutation_regions,'candidate':args.candidate_mutation_regions},
                   readonly_regions={'baseline':args.baseline_readonly_regions,'candidate':args.candidate_readonly_regions},
                   cfg_inlining={'baseline':args.baseline_cfg_inlining,'candidate':args.candidate_cfg_inlining},
                   benchmark=args.benchmark or 'nbody',
                   fresh_processes=args.fresh_processes, readiness={}, samples_raw=[])
    workers = {}

    def save():
        (args.output / 'comparison.json').write_text(json.dumps(results, indent=2) + '\n')

    def start(label):
        name, mode = label.split('/')
        command = [str(binaries[name]), *([args.benchmark] if args.benchmark else []), '--tier', mode, '--warmups', str(args.warmups)]
        if results['mutation_regions'][name]:
            command += ['--mutation-regions']
        if results['readonly_regions'][name]:
            command += ['--readonly-regions']
        if results['cfg_inlining'][name]:
            command += ['--cfg-inlining']
        encoding = results['bytecode_modes'][name]
        if encoding:
            command += ['--bytecode', encoding]
        numeric = results['numeric_modes'][name]
        if numeric:
            command += ['--numeric', numeric]
        process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                   stderr=subprocess.PIPE, text=True, bufsize=1)
        workers[label] = process
        line = process.stdout.readline()
        if not line:
            raise RuntimeError((label, process.stderr.read()))
        readiness = json.loads(line)
        assert readiness['ready']
        if args.fresh_processes:
            results['readiness'].setdefault(label, []).append(readiness)
        else:
            results['readiness'][label] = readiness
        save()
        return process

    def stop(process):
        if process.poll() is None:
            process.stdin.write('quit\n')
            process.stdin.flush()
        process.wait(timeout=30)
        error = process.stderr.read()
        if process.returncode or error:
            raise RuntimeError((process.returncode, error))

    try:
        for name in binaries:
            for mode in args.tiers:
                label = f'{name}/{mode}'
                workers[label] = None
                if not args.fresh_processes:
                    start(label)
                    print(f'ready {label}', flush=True)
        rng = random.Random(args.seed)
        for round_index in range(args.samples):
            order = list(workers)
            rng.shuffle(order)
            for label in order:
                process = start(label) if args.fresh_processes else workers[label]
                process.stdin.write('run\n')
                process.stdin.flush()
                line = process.stdout.readline()
                if not line:
                    raise RuntimeError((label, process.stderr.read()))
                sample = json.loads(line)
                if args.benchmark:
                    assert sample['validated']
                else:
                    assert sample['energy'] == '-0.169089263'
                    assert sample['backend_compile_ns'] == sample['deoptimizations'] == 0
                mode = label.split('/')[1]
                entry = {'lir': 'lir_entries', 'native': 'native_entries',
                         'copy-patch': 'copy_patch_entries'}.get(mode)
                if entry and not args.benchmark:
                    assert sample[entry] == 1
                results['samples_raw'].append(dict(label=label, round=round_index, **sample))
                if args.fresh_processes:
                    stop(process)
                    workers[label] = None
                save()
                print(f'{round_index + 1:2} {label:22} {sample["run_ns"] / 1e9:.6f}s', flush=True)
    finally:
        for process in workers.values():
            if process is not None:
                stop(process)
    results['summary'] = {
        label: summarize([row['run_ns'] for row in results['samples_raw'] if row['label'] == label])
        for label in workers}
    save()
    print(json.dumps(results['summary'], indent=2))


if __name__ == '__main__':
    main()
