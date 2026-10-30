#!/usr/bin/env python3
"""Run the object-heavy benchmarks serially across tis, CPython and PyPy."""
import argparse
import datetime
import json
from pathlib import Path
import platform
import random
import subprocess
import sys
import time

from bench_nbody import digest, summarize
from compare_runtimes import runtime_info, checked_stderr

FIXTURES = {'barnes-hut': ('barnes_hut', "\nprint(f'", 'bench_quadtree_nbody', (1, 200, 100, 0.5)),
            'ticket-search': ('ticket_search', '\nsearch()', 'search', ()),
            'sieve': ('sieve', '\nbench_sieve(LIMIT)', 'bench_sieve', (1_000_000,))}


def python_worker(benchmark, warmups):
    import contextlib
    import io
    fixture, tail, function, arguments = FIXTURES[benchmark]
    root = Path(__file__).resolve().parents[1] / 'tests/fixtures'
    source = (root / f'{fixture}.py').read_text().rsplit(tail, 1)[0]
    expected = (root / f'{fixture}.out').read_text()
    namespace = {'__name__': '__main__'}
    exec(compile(source, str(root / f'{fixture}.py'), 'exec'), namespace)

    def sample():
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            start = time.perf_counter_ns()
            value = namespace[function](*arguments)
            elapsed = time.perf_counter_ns() - start
        text = f'{value:.6f}\n' if benchmark == 'barnes-hut' else output.getvalue()
        assert text == expected, (text, expected)
        return dict(run_ns=elapsed, validated=True, backend_compile_ns=None)

    timings = [sample()['run_ns'] for _ in range(warmups)]
    print(json.dumps(dict(ready=True, warmup_ns=timings)), flush=True)
    for line in sys.stdin:
        if line.strip() == 'quit':
            break
        assert line.strip() == 'run'
        print(json.dumps(sample()), flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--worker', type=Path)
    parser.add_argument('--python', type=Path, default=Path(sys.executable))
    parser.add_argument('--pypy', type=Path)
    parser.add_argument('--output', type=Path)
    parser.add_argument('--samples', type=int, default=3)
    parser.add_argument('--warmups', type=int, default=1)
    parser.add_argument('--benchmarks', nargs='+', choices=FIXTURES, default=list(FIXTURES))
    parser.add_argument('--tiers', nargs='+', choices=['bytecode', 'lir', 'native', 'copy-patch'],
                        default=['bytecode', 'native'])
    parser.add_argument('--mutation-regions', action='store_true')
    parser.add_argument('--readonly-regions', action='store_true')
    parser.add_argument('--python-worker', choices=FIXTURES)
    args = parser.parse_args()
    if args.python_worker:
        python_worker(args.python_worker, args.warmups)
        return
    if not args.worker or not args.output or args.samples < 1 or args.warmups < 1:
        parser.error('provide --worker and --output, at least one sample and one warmup')
    args.output.mkdir(parents=True, exist_ok=False)
    binaries = {'tis': args.worker.resolve(), 'cpython': args.python.resolve()}
    if args.pypy:
        binaries['pypy'] = args.pypy.resolve()
    results = dict(timestamp=datetime.datetime.now(datetime.timezone.utc).isoformat(),
                   platform=platform.platform(), samples=args.samples, warmups=args.warmups, readonly_regions=args.readonly_regions, mutation_regions=args.mutation_regions,
                   binaries={k: dict(path=str(v), sha256=digest(v)) for k, v in binaries.items()},
                   seed=20261028,
                   runner_sha256=digest(Path(__file__).resolve()),
                   parameters={b: FIXTURES[b][3] for b in args.benchmarks},
                   timing='reused-process function invocation; setup, formatting and validation excluded',
                   worker_warnings={},
                   runtimes={k: runtime_info(v) for k,v in binaries.items() if k!='tis'},
                   source_hashes={b: digest(Path(__file__).resolve().parents[1] / 'tests/fixtures' /
                                           f'{FIXTURES[b][0]}.py') for b in args.benchmarks},
                   readiness={}, raw={}, summary={})
    rng = random.Random(20261028)
    for benchmark in args.benchmarks:
        modes = [*args.tiers, 'cpython', *(['pypy'] if args.pypy else [])]
        workers = {}
        errors = {}
        try:
            for mode in modes:
                if mode in args.tiers:
                    command = [str(binaries['tis']), benchmark, '--tier', mode,
                               '--warmups', str(args.warmups)]
                    if args.mutation_regions:
                        command += ['--mutation-regions']
                    if args.readonly_regions:
                        command += ['--readonly-regions']
                else:
                    command = [str(binaries[mode]), str(Path(__file__).resolve()),
                               '--python-worker', benchmark, '--warmups', str(args.warmups)]
                stderr = (args.output / f'{benchmark}-{mode}.stderr').open('w+')
                errors[mode] = stderr
                p = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                     stderr=stderr, text=True, bufsize=1)
                workers[mode] = p
                line = p.stdout.readline()
                if not line:
                    raise RuntimeError(f'{benchmark}/{mode} failed; see stderr file')
                results['readiness'][f'{benchmark}/{mode}'] = json.loads(line)
                assert results['readiness'][f'{benchmark}/{mode}']['ready']
                results['raw'][f'{benchmark}/{mode}'] = []
                print(f'ready {benchmark}/{mode}', flush=True)
            for iteration in range(args.samples):
                rng.shuffle(modes)
                for mode in modes:
                    p = workers[mode]
                    p.stdin.write('run\n')
                    p.stdin.flush()
                    line = p.stdout.readline()
                    if not line:
                        raise RuntimeError(f'{benchmark}/{mode} failed; see stderr file')
                    sample = json.loads(line)
                    assert sample['validated']
                    results['raw'][f'{benchmark}/{mode}'].append(sample)
                print(f'{benchmark} round {iteration + 1}', flush=True)
            for mode in modes:
                name = f'{benchmark}/{mode}'
                results['summary'][name] = summarize([v['run_ns'] for v in results['raw'][name]])
        finally:
            for mode, p in workers.items():
                if p.poll() is None:
                    p.stdin.write('quit\n')
                    p.stdin.flush()
                p.wait(timeout=30)
                errors[mode].close()
            (args.output / 'comparison.json').write_text(json.dumps(results, indent=2) + '\n')
        for mode, p in workers.items():
            text = (args.output / f'{benchmark}-{mode}.stderr').read_text()
            if p.returncode:
                raise RuntimeError(f'{benchmark}/{mode} exited {p.returncode}: {text}')
            # This PyPy build emits a known cache-size warning at startup; preserve it while
            # rejecting every unexpected diagnostic, matching the other comparison drivers.
            results['worker_warnings'][f'{benchmark}/{mode}'] = checked_stderr(text, mode == 'pypy')
        (args.output / 'comparison.json').write_text(json.dumps(results, indent=2) + '\n')
    print(json.dumps(results['summary'], indent=2))


if __name__ == '__main__':
    main()
