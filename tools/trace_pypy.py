#!/usr/bin/env python3
"""Capture PyPy optimized traces and compile events separately from performance measurements."""
import argparse
import json
import os
from pathlib import Path
import subprocess

from compare_runtimes import checked_stderr, runtime_info
from bench_nbody import digest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--pypy', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--source', type=Path, default=Path(__file__).resolve().parents[1] / 'tests/fixtures/game_nbody.py')
    parser.add_argument('--warmups', type=int, default=10)
    parser.add_argument('--samples', type=int, default=10)
    args = parser.parse_args()
    if min(args.warmups, args.samples) < 1:
        parser.error('warmups and samples must be positive')
    args.pypy, args.source, args.output = [p.resolve() for p in (args.pypy, args.source, args.output)]
    args.output.mkdir(parents=True, exist_ok=False)
    env = os.environ.copy()
    env['PYPYLOG'] = f'jit-log-opt,jit-backend-counts,jit-summary:{args.output / "pypy.log"}'
    command = [str(args.pypy), str(Path(__file__).with_name('bench_nbody.py').resolve()),
               '--python-worker', '--jit-events', '--source', str(args.source), '--warmups', str(args.warmups)]
    process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, text=True, env=env, bufsize=1)
    responses = []
    try:
        line = process.stdout.readline()
        if not line:
            raise RuntimeError(process.stderr.read())
        readiness = json.loads(line)
        assert readiness['ready'] and readiness['jit_events']
        for _ in range(args.samples):
            process.stdin.write('run\n')
            process.stdin.flush()
            line = process.stdout.readline()
            if not line:
                raise RuntimeError(process.stderr.read())
            row = json.loads(line)
            assert row['energy'] == '-0.169089263'
            responses.append(row)
    finally:
        if process.poll() is None:
            process.stdin.write('quit\n')
            process.stdin.flush()
        process.wait(timeout=30)
    stderr = process.stderr.read()
    if process.returncode:
        raise RuntimeError((process.returncode, stderr))
    checked_stderr(stderr, True)
    result = dict(runtime=runtime_info(args.pypy), binary_sha256=digest(args.pypy),
                  source_sha256=digest(args.source), command=command, pypylog=env['PYPYLOG'],
                  stderr=stderr, readiness=readiness, responses=responses)
    (args.output / 'events.json').write_text(json.dumps(result, indent=2) + '\n')
    print('warmup event counts:', readiness['warmup_event_counts'])
    print('post-warmup events:', sum(len(row['jit_new_events']) for row in responses))


if __name__ == '__main__':
    main()
