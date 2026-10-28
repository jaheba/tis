#!/usr/bin/env python3
"""Serial, interleaved n-body measurements. Build release examples before invoking this runner."""
import argparse
import csv
import datetime
import hashlib
import json
import os
from pathlib import Path
import platform
import random
import statistics
import subprocess
import sys
import time

MODES = ['bytecode', 'lir', 'native', 'copy-patch', 'cpython']
EXPECTED = '-0.169075164\n-0.169089263\n'
TAIL = '\noffset_momentum(BODIES[DEFAULT_REFERENCE])\n'


def python_worker(source, warmups, jit_events=False):
    pypy = sys.implementation.name == 'pypy'
    events = []
    if jit_events:
        if not pypy:
            raise ValueError('JIT event hooks require PyPy')
        import pypyjit

        def compiled(info):
            events.append(dict(type=info.type, loop_no=info.loop_no,
                               greenkey=repr(info.greenkey), asm_bytes=info.asmlen))

        pypyjit.set_compile_hook(compiled, operations=False)
    backend_compile_ns = None if pypy else 0
    text = Path(source).read_text()
    setup, _ = text.rsplit(TAIL, 1)
    start = time.perf_counter_ns()
    code = compile(setup, str(source), 'exec')
    compile_ns = time.perf_counter_ns() - start
    start = time.perf_counter_ns()
    namespace = {'__name__': '__main__'}
    exec(code, namespace)
    assert namespace['DEFAULT_ITERATIONS'] == 20000
    namespace['offset_momentum'](namespace['BODIES'][namespace['DEFAULT_REFERENCE']])
    snapshot = [(items, items[:]) for body in namespace['SYSTEM'] for items in body[:2]]
    setup_ns = time.perf_counter_ns() - start
    advance, energy = namespace['advance'], namespace['report_energy']

    def sample():
        for target, initial in snapshot:
            target[:] = initial
        start = time.perf_counter_ns()
        advance(0.01, 20000)
        elapsed = time.perf_counter_ns() - start
        assert f'{energy():.9f}' == '-0.169089263'
        return elapsed

    warmup_ns, warmup_events = [], []
    for _ in range(warmups):
        warmup_ns.append(sample())
        warmup_events.append(len(events))
    print(json.dumps(dict(ready=True, compile_ns=compile_ns, setup_ns=setup_ns,
                          warmup_ns=warmup_ns, backend_compile_ns=backend_compile_ns,
                          jit_events=events if jit_events else None,
                          warmup_event_counts=warmup_events if jit_events else None)), flush=True)
    for line in sys.stdin:
        if line.strip() == 'quit':
            break
        assert line.strip() == 'run'
        before = len(events)
        elapsed = sample()
        print(json.dumps(dict(run_ns=elapsed, energy='-0.169089263', backend_compile_ns=backend_compile_ns,
                              deoptimizations=None if pypy else 0,
                              jit_new_events=events[before:] if jit_events else None)), flush=True)


def command_output(command):
    result = subprocess.run(command, capture_output=True, text=True)
    return (result.stdout + result.stderr).strip()


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def summarize(samples):
    quartiles = statistics.quantiles(samples, n=4, method='inclusive')
    return dict(median_ns=statistics.median(samples), min_ns=min(samples), max_ns=max(samples),
                q1_ns=quartiles[0], q3_ns=quartiles[2])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--run', type=Path)
    parser.add_argument('--worker', type=Path)
    parser.add_argument('--kernel', type=Path)
    parser.add_argument('--source', type=Path, default=Path(__file__).resolve().parents[1] / 'tests/fixtures/game_nbody.py')
    parser.add_argument('--python', type=Path, default=Path(sys.executable))
    parser.add_argument('--output', type=Path)
    parser.add_argument('--samples', type=int, default=10)
    parser.add_argument('--warmups', type=int, default=2)
    parser.add_argument('--cpu-label')
    parser.add_argument('--seed', type=int, default=20261028)
    parser.add_argument('--python-worker', '--cpython-worker', action='store_true', help=argparse.SUPPRESS)
    parser.add_argument('--jit-events', action='store_true', help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.python_worker:
        python_worker(args.source, args.warmups, args.jit_events)
        return
    if not all([args.run, args.worker, args.kernel, args.output]):
        parser.error('--run, --worker, --kernel and --output are required')
    if args.samples < 10 or args.warmups < 1:
        parser.error('use at least 10 samples and one warmup')
    args.output.mkdir(parents=True, exist_ok=False)
    args.run, args.worker, args.kernel, args.python = [path.resolve() for path in (args.run, args.worker, args.kernel, args.python)]
    source = args.source.resolve()
    metadata = dict(timestamp=datetime.datetime.now(datetime.timezone.utc).isoformat(),
                    platform=platform.platform(), machine=platform.machine(),
                    cpu=args.cpu_label or command_output(['sysctl', '-n', 'machdep.cpu.brand_string']),
                    power=command_output(['pmset', '-g', 'batt']),
                    rust=command_output(['rustc', '--version']),
                    python=command_output([str(args.python), '--version']),
                    python_jit=command_output([str(args.python), '-c', "import sys; j=getattr(sys,'_jit',None); print({'available':bool(j and j.is_available()),'enabled':bool(j and j.is_enabled())})"]),
                    source_sha256=digest(source), iterations=20000, samples=args.samples,
                    warmups=args.warmups, seed=args.seed,
                    binaries={str(path): digest(path) for path in (args.run, args.worker, args.kernel, args.python)},
                    environment={name: os.environ.get(name) for name in ['PYTHON_JIT', 'PYTHON_GIL', 'RUSTFLAGS']},
                    core_revision=command_output(['git', '-C', str(source.parents[2]), 'rev-parse', 'HEAD']),
                    runner_sha256=digest(__file__))
    results = dict(metadata=metadata, end_to_end=[], end_to_end_warmups=[], steady=[], readiness={}, kernel=[])
    rng = random.Random(args.seed)

    def save():
        (args.output / 'raw.json').write_text(json.dumps(results, indent=2) + '\n')

    commands = {mode: [str(args.run), '--tier', mode, str(source)] for mode in MODES[:-1]}
    commands['cpython'] = [str(args.python), str(source)]
    save()
    for round_index in range(-args.warmups, args.samples):
        order = MODES[:]
        rng.shuffle(order)
        for mode in order:
            start = time.perf_counter_ns()
            result = subprocess.run(commands[mode], capture_output=True, text=True, check=True, timeout=60)
            elapsed = time.perf_counter_ns() - start
            assert result.stdout == EXPECTED, (mode, result.stdout, result.stderr)
            assert not result.stderr, (mode, result.stderr)
            record = dict(mode=mode, round=round_index, run_ns=elapsed)
            results['end_to_end' if round_index >= 0 else 'end_to_end_warmups'].append(record)
            save()
            print(f"end-to-end {round_index + 1:2} {mode:10} {elapsed / 1e9:.6f}s", flush=True)

    workers = {}
    try:
        for mode in MODES:
            command = ([str(args.python), str(Path(__file__).resolve()), '--cpython-worker', '--source', str(source), '--warmups', str(args.warmups)]
                       if mode == 'cpython' else [str(args.worker), '--tier', mode, '--warmups', str(args.warmups)])
            process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, bufsize=1)
            workers[mode] = process
            line = process.stdout.readline()
            if not line:
                raise RuntimeError((mode, process.stderr.read()))
            readiness = json.loads(line)
            assert readiness['ready']
            results['readiness'][mode] = readiness
            save()
            print(f"worker ready {mode}: first={readiness['warmup_ns'][0]/1e9:.6f}s", flush=True)
        for round_index in range(args.samples):
            order = MODES[:]
            rng.shuffle(order)
            for mode in order:
                process = workers[mode]
                process.stdin.write('run\n')
                process.stdin.flush()
                line = process.stdout.readline()
                if not line:
                    raise RuntimeError((mode, process.stderr.read()))
                sample = json.loads(line)
                assert sample['energy'] == '-0.169089263'
                assert sample['backend_compile_ns'] == sample['deoptimizations'] == 0
                for label in ['native', 'copy-patch', 'lir']:
                    if mode == label:
                        assert sample[{'native': 'native_entries', 'copy-patch': 'copy_patch_entries', 'lir': 'lir_entries'}[label]] == 1
                results['steady'].append(dict(mode=mode, round=round_index, **sample))
                save()
                print(f"steady {round_index + 1:2} {mode:10} {sample['run_ns']/1e9:.6f}s", flush=True)
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

    for round_index in range(5):
        result = subprocess.run([str(args.kernel)], capture_output=True, text=True, check=True, timeout=60)
        for row in csv.DictReader(result.stdout.splitlines()):
            results['kernel'].append(dict(round=round_index, **row))
        save()
    summary = {section: {mode: summarize([row['run_ns'] for row in results[section] if row['mode'] == mode])
                         for mode in MODES} for section in ['end_to_end', 'steady']}
    results['summary'] = summary
    save()
    (args.output / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
    print(json.dumps(summary, indent=2))


if __name__ == '__main__':
    main()
