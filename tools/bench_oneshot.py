#!/usr/bin/env python3
"""Measure fresh-process, no-warmup execution of the three benchmark fixtures."""
import argparse
import datetime
import json
from pathlib import Path
import platform
import random
import subprocess
import sys
import time

from bench_nbody import EXPECTED, digest, summarize
from compare_runtimes import checked_stderr, runtime_info

FIXTURES = {'nbody': 'game_nbody', 'barnes-hut': 'barnes_hut', 'ticket-search': 'ticket_search'}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--run', type=Path, required=True)
    parser.add_argument('--baseline-run',type=Path,help='also time an older tis executable in each round')
    parser.add_argument('--python', type=Path, default=Path(sys.executable))
    parser.add_argument('--pypy', type=Path)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--samples', type=int, default=3)
    parser.add_argument('--timeout', type=float, default=120)
    parser.add_argument('--benchmarks', nargs='+', choices=FIXTURES, default=list(FIXTURES))
    parser.add_argument('--tiers', nargs='+', choices=['bytecode', 'lir', 'native', 'copy-patch'],
                        default=['bytecode', 'native'])
    parser.add_argument('--cfg-inlining', action='store_true', help='enable experimental CFG inlining in candidate tis')
    args = parser.parse_args()
    if args.samples < 1 or args.timeout <= 0:
        parser.error('use at least one sample and a positive per-process timeout')
    args.output.mkdir(parents=True, exist_ok=False)
    root = Path(__file__).resolve().parents[1] / 'tests/fixtures'
    binaries = {'tis': args.run.resolve(), 'cpython': args.python.resolve()}
    if args.baseline_run:
        binaries['baseline']=args.baseline_run.resolve()
    if args.pypy:
        binaries['pypy'] = args.pypy.resolve()
    results = dict(timestamp=datetime.datetime.now(datetime.timezone.utc).isoformat(),
                   platform=platform.platform(), samples=args.samples, warmups=0,
                   timing='fresh subprocess wall time, including startup, parsing, compilation, execution and exit',
                   seed=20261028, timeout_seconds=args.timeout,
                   binaries={k: dict(path=str(v), sha256=digest(v)) for k, v in binaries.items()},
                   runtimes={k: runtime_info(v) for k, v in binaries.items() if k in ['cpython','pypy']},
                   sources={}, commands={}, raw={}, summary={})
    rng = random.Random(results['seed'])

    def save():
        (args.output / 'comparison.json').write_text(json.dumps(results, indent=2) + '\n')

    for benchmark in args.benchmarks:
        source = root / f'{FIXTURES[benchmark]}.py'
        expected = EXPECTED if benchmark == 'nbody' else source.with_suffix('.out').read_text()
        results['sources'][benchmark] = dict(path=str(source), sha256=digest(source), expected_stdout=expected)
        commands = {tier: [str(binaries['tis']), '--tier', tier, *(['--cfg-inlining'] if args.cfg_inlining else []), str(source)] for tier in args.tiers}
        if args.baseline_run:
            commands.update({f'baseline/{tier}':[str(binaries['baseline']),'--tier',tier,str(source)] for tier in args.tiers})
        commands.update({mode: [str(binary), str(source)] for mode, binary in binaries.items() if mode in ['cpython','pypy']})
        results['commands'][benchmark] = commands
        for mode in commands:
            results['raw'][f'{benchmark}/{mode}'] = []
        for iteration in range(args.samples):
            order = list(commands)
            rng.shuffle(order)
            for mode in order:
                start = time.perf_counter_ns()
                process = subprocess.run(commands[mode], capture_output=True, text=True, timeout=args.timeout)
                elapsed = time.perf_counter_ns() - start
                sample = dict(round=iteration, run_ns=elapsed, returncode=process.returncode,
                              stdout=process.stdout, stderr=process.stderr)
                results['raw'][f'{benchmark}/{mode}'].append(sample)
                save()
                if process.returncode or process.stdout != expected:
                    raise RuntimeError(f'{benchmark}/{mode} failed validation: {sample}')
                checked_stderr(process.stderr, mode == 'pypy')
            print(f'{benchmark} cold round {iteration + 1}', flush=True)
        for mode in commands:
            name = f'{benchmark}/{mode}'
            results['summary'][name] = summarize([v['run_ns'] for v in results['raw'][name]])
        save()
    print(json.dumps(results['summary'], indent=2))


if __name__ == '__main__':
    main()
