#!/usr/bin/env python3
"""Compare cold and warmed nbody execution across tis, CPython and PyPy."""
import argparse
import datetime
import json
import os
from pathlib import Path
import platform
import random
import subprocess
import sys
import time

from bench_nbody import EXPECTED, digest, summarize

PYPY_WARNING = 'Warning: cannot find your CPU L2 cache size with sysctl()'


def checked_stderr(text, pypy):
    lines = [line for line in text.splitlines() if line.strip()]
    if any(not pypy or line != PYPY_WARNING for line in lines):
        raise RuntimeError(text)
    return lines


def runtime_info(binary):
    code = """import sys, platform, json
j=getattr(sys,'_jit',None)
info=dict(version=sys.version, implementation=sys.implementation.name, machine=platform.machine(),
          cpython_jit_available=bool(j and j.is_available()), cpython_jit_enabled=bool(j and j.is_enabled()))
if sys.implementation.name == 'pypy':
    import pypyjit
    info['jit_defaults']=pypyjit.defaults
print(json.dumps(info))
"""
    result = subprocess.run([str(binary), '-c', code], capture_output=True, text=True, check=True)
    info = json.loads(result.stdout)
    info['stderr'] = checked_stderr(result.stderr, info['implementation'] == 'pypy')
    return info


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--run', type=Path, required=True)
    parser.add_argument('--worker', type=Path, required=True)
    parser.add_argument('--python', type=Path, required=True)
    parser.add_argument('--pypy', type=Path, required=True)
    parser.add_argument('--source', type=Path, default=Path(__file__).resolve().parents[1] / 'tests/fixtures/game_nbody.py')
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--samples', type=int, default=10)
    parser.add_argument('--warmups', type=int, default=10)
    parser.add_argument('--seed', type=int, default=20261028)
    parser.add_argument('--tiers', nargs='+', choices=['bytecode', 'lir', 'native', 'copy-patch'],
                        default=['bytecode', 'lir', 'native', 'copy-patch'])
    args = parser.parse_args()
    if args.samples < 10 or args.warmups < 2:
        parser.error('use at least ten samples and two warmup calls')
    if os.environ.get('PYPYLOG') or os.environ.get('PYPY_DISABLE_JIT'):
        parser.error('timed comparisons require PyPy JIT enabled and logging disabled')
    for name in ['run', 'worker', 'python', 'pypy', 'source', 'output']:
        setattr(args, name, getattr(args, name).resolve())
    args.output.mkdir(parents=True, exist_ok=False)
    modes = [*args.tiers, 'cpython', 'pypy']
    python_worker = Path(__file__).with_name('bench_nbody.py').resolve()
    metadata = dict(timestamp=datetime.datetime.now(datetime.timezone.utc).isoformat(),
                    platform=platform.platform(), samples=args.samples, warmups=args.warmups, seed=args.seed,
                    source_sha256=digest(args.source), runner_sha256=digest(__file__),
                    binaries={name: dict(path=str(getattr(args, name)), sha256=digest(getattr(args, name)))
                              for name in ['run', 'worker', 'python', 'pypy']},
                    runtimes={name: runtime_info(binary) for name, binary in
                              [('cpython', args.python), ('pypy', args.pypy)]},
                    environment={key: os.environ.get(key) for key in
                                 ['PYPYLOG', 'PYPY_DISABLE_JIT', 'PYPY_GC_NURSERY', 'PYTHON_JIT', 'PYTHON_GIL']})
    assert metadata['runtimes']['pypy']['implementation'] == 'pypy'
    assert metadata['runtimes']['cpython']['implementation'] == 'cpython'
    data = dict(metadata=metadata, end_to_end=[], readiness={}, steady=[], worker_stderr={})
    rng = random.Random(args.seed)

    def save():
        (args.output / 'comparison.json').write_text(json.dumps(data, indent=2) + '\n')

    # Each cold sample starts a fresh process and includes source compilation and JIT warmup.
    commands = {tier: [str(args.run), '--tier', tier, str(args.source)] for tier in args.tiers}
    commands.update(cpython=[str(args.python), str(args.source)], pypy=[str(args.pypy), str(args.source)])
    for round_index in range(args.samples):
        order = modes[:]
        rng.shuffle(order)
        for mode in order:
            start = time.perf_counter_ns()
            result = subprocess.run(commands[mode], capture_output=True, text=True, check=True, timeout=60)
            elapsed = time.perf_counter_ns() - start
            assert result.stdout == EXPECTED
            warnings = checked_stderr(result.stderr, mode == 'pypy')
            data['end_to_end'].append(dict(mode=mode, round=round_index, run_ns=elapsed, stderr=warnings))
            save()
        print(f'cold round {round_index + 1}', flush=True)

    workers = {}
    try:
        for mode in modes:
            if mode in ['cpython', 'pypy']:
                binary = args.python if mode == 'cpython' else args.pypy
                command = [str(binary), str(python_worker), '--python-worker', '--source', str(args.source),
                           '--warmups', str(args.warmups)]
            else:
                command = [str(args.worker), '--tier', mode, '--warmups', str(args.warmups)]
            process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                       stderr=subprocess.PIPE, text=True, bufsize=1)
            workers[mode] = process
            line = process.stdout.readline()
            if not line:
                raise RuntimeError((mode, process.stderr.read()))
            readiness = json.loads(line)
            assert readiness['ready'] and len(readiness['warmup_ns']) == args.warmups
            data['readiness'][mode] = readiness
            save()
            print(f'ready {mode}', flush=True)
        for round_index in range(args.samples):
            order = modes[:]
            rng.shuffle(order)
            for mode in order:
                process = workers[mode]
                process.stdin.write('run\n')
                process.stdin.flush()
                line = process.stdout.readline()
                if not line:
                    raise RuntimeError((mode, process.stderr.read()))
                row = json.loads(line)
                assert row['energy'] == '-0.169089263'
                if mode != 'pypy':
                    assert row['backend_compile_ns'] == row['deoptimizations'] == 0
                entry = {'native': 'native_entries', 'copy-patch': 'copy_patch_entries', 'lir': 'lir_entries'}.get(mode)
                if entry:
                    assert row[entry] == 1
                data['steady'].append(dict(mode=mode, round=round_index, **row))
                save()
            print(f'warm round {round_index + 1}', flush=True)
    finally:
        for process in workers.values():
            if process.poll() is None:
                process.stdin.write('quit\n')
                process.stdin.flush()
        for mode, process in workers.items():
            process.wait(timeout=30)
            stderr = process.stderr.read()
            if process.returncode:
                raise RuntimeError((mode, process.returncode, stderr))
            data['worker_stderr'][mode] = checked_stderr(stderr, mode == 'pypy')
    data['summary'] = {section: {mode: summarize([row['run_ns'] for row in data[section] if row['mode'] == mode])
                                 for mode in modes} for section in ['end_to_end', 'steady']}
    save()
    print(json.dumps(data['summary'], indent=2))


if __name__ == '__main__':
    main()
