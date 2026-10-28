#!/usr/bin/env python3
"""Compare complete application requests using Flying Circus workers and a tis adapter."""
import argparse
import datetime
import json
from pathlib import Path
import platform
import random
import subprocess
import time

from flying_circus.workers import MontyWorker, CPythonWorker
from flying_circus.runtimes import resolve
from flying_circus.bench import validate
from bench_nbody import EXPECTED, digest, summarize

FIXTURES = {'nbody': 'game_nbody', 'barnes-hut': 'barnes_hut', 'ticket-search': 'ticket_search'}


class TisWorker:
    def __init__(self, binary, tier, source):
        self.process = subprocess.Popen([str(binary), tier, str(source)], stdin=subprocess.PIPE,
                                        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, bufsize=1)
        assert json.loads(self.process.stdout.readline()) == {'ready': True}

    def reset(self):
        self.process.stdin.write('reset\n')
        self.process.stdin.flush()
        assert json.loads(self.process.stdout.readline()) == {'reset': True}

    def run(self):
        self.process.stdin.write('run\n')
        self.process.stdin.flush()
        line = self.process.stdout.readline()
        if not line:
            raise RuntimeError(self.process.stderr.read())
        result = json.loads(line)
        return result['stdout'], result['stderr']

    def close(self):
        if self.process.poll() is None:
            self.process.stdin.write('quit\n')
            self.process.stdin.flush()
        self.process.wait(timeout=30)
        error = self.process.stderr.read()
        if self.process.returncode or error:
            raise RuntimeError(error)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--monty', type=Path, required=True)
    p.add_argument('--monty-revision', required=True)
    p.add_argument('--tis-revision',required=True)
    p.add_argument('--tis-run', type=Path, required=True)
    p.add_argument('--tis-worker', type=Path, required=True)
    p.add_argument('--python', type=Path, required=True)
    p.add_argument('--output', type=Path, required=True)
    p.add_argument('--samples', type=int, default=10)
    p.add_argument('--warmups', type=int, default=2)
    args = p.parse_args()
    if args.samples < 10 or args.warmups < 2:
        p.error('use at least ten samples and two repeated-worker warmups')
    args.output.mkdir(parents=True, exist_ok=False)
    binaries = {k: getattr(args, k).resolve() for k in ['monty', 'tis_run', 'tis_worker', 'python']}
    monty_info = resolve('monty=' + str(binaries['monty']))
    monty_info['binary'] = str(monty_info['binary'])
    data = dict(timestamp=datetime.datetime.now(datetime.timezone.utc).isoformat(), platform=platform.platform(),
                samples=args.samples, warmups=args.warmups, seed=20261028,
                tis_revision=args.tis_revision, monty_revision=args.monty_revision, monty_info=monty_info,
                binaries={k: dict(path=str(v), sha256=digest(v)) for k, v in binaries.items()},
                sources={}, raw={}, warmup_raw={}, summary={})
    rng = random.Random(data['seed'])
    root = Path(__file__).resolve().parents[1] / 'tests/fixtures'

    def save():
        (args.output / 'comparison.json').write_text(json.dumps(data, indent=2) + '\n')

    for benchmark, fixture in FIXTURES.items():
        source = root / f'{fixture}.py'
        code = source.read_text()
        expected = EXPECTED if benchmark == 'nbody' else source.with_suffix('.out').read_text()
        data['sources'][benchmark] = dict(path=str(source), sha256=digest(source), expected=expected)
        commands = {'tis-native': [str(binaries['tis_run']), '--tier', 'native', str(source)],
                    'tis-bytecode': [str(binaries['tis_run']), '--tier', 'bytecode', str(source)],
                    'monty': [str(binaries['monty']), str(source)],
                    'cpython': [str(binaries['python']), str(source)]}
        for mode in commands:
            data['raw'][f'{benchmark}/oneshot/{mode}'] = []
        for iteration in range(args.samples):
            order = list(commands)
            rng.shuffle(order)
            for mode in order:
                start = time.perf_counter_ns()
                result = subprocess.run(commands[mode], capture_output=True, text=True, timeout=120)
                elapsed = time.perf_counter_ns() - start
                validate(result, expected, mode == 'monty' and monty_info['legacy_cli_summary'])
                data['raw'][f'{benchmark}/oneshot/{mode}'].append(dict(run_ns=elapsed,
                    stdout=result.stdout, stderr=result.stderr, returncode=result.returncode))
                save()
            print(f'{benchmark} one-shot {iteration + 1}', flush=True)
        workers = {}
        try:
            workers['tis-native'] = TisWorker(binaries['tis_worker'], 'native', source)
            workers['tis-bytecode'] = TisWorker(binaries['tis_worker'], 'bytecode', source)
            workers['monty'] = MontyWorker(binaries['monty'], 120)
            workers['cpython'] = CPythonWorker(binaries['python'], 120)
            workers['monty'].ready()
            workers['cpython'].ready()
            for mode in workers:
                data['raw'][f'{benchmark}/repeated/{mode}'] = []
                data['warmup_raw'][f'{benchmark}/{mode}'] = []
            for iteration in range(args.warmups + args.samples):
                order = list(workers)
                rng.shuffle(order)
                for mode in order:
                    worker = workers[mode]
                    if mode.startswith('tis-'):
                        worker.reset()
                    else:
                        worker.configure(source.name)
                    start = time.perf_counter_ns()
                    stdout, stderr = worker.run() if mode.startswith('tis-') else worker.run(code, source.name)
                    elapsed = time.perf_counter_ns() - start
                    assert stdout == expected and not stderr, (mode, stdout, stderr)
                    if mode == 'monty':
                        worker.reset()
                    row = dict(run_ns=elapsed, stdout=stdout, stderr=stderr)
                    if iteration < args.warmups:
                        data['warmup_raw'][f'{benchmark}/{mode}'].append(row)
                    else:
                        data['raw'][f'{benchmark}/repeated/{mode}'].append(row)
                    save()
                print(f'{benchmark} repeated {iteration + 1 - args.warmups}', flush=True)
        finally:
            for worker in workers.values():
                worker.close()
        data['summary'] = {k: summarize([r['run_ns'] for r in rows]) for k, rows in data['raw'].items() if rows}
        save()
    print(json.dumps(data['summary'], indent=2))


if __name__ == '__main__':
    main()
