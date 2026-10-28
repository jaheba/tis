#!/usr/bin/env python3
"""Sample warmed nbody workers on macOS, retaining profiles and validated worker responses."""
import argparse
import datetime
import json
import platform
import subprocess
from pathlib import Path

from bench_nbody import digest


def profile(worker, tier, duration, interval, output):
    process = subprocess.Popen([str(worker), '--tier', tier, '--warmups', '2'],
                               stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, text=True, bufsize=1)
    sampler = None
    try:
        readiness = json.loads(process.stdout.readline())
        assert readiness['ready']
        command = ['/usr/bin/sample', str(process.pid), str(duration), str(interval),
                   '-file', str(output)]
        sampler = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        samples = []
        while sampler.poll() is None:
            process.stdin.write('run\n')
            process.stdin.flush()
            line = process.stdout.readline()
            if not line:
                raise RuntimeError(process.stderr.read())
            response = json.loads(line)
            assert response['energy'] == '-0.169089263'
            assert response['backend_compile_ns'] == response['deoptimizations'] == 0
            entry = {'native': 'native_entries', 'copy-patch': 'copy_patch_entries',
                     'lir': 'lir_entries'}.get(tier)
            if entry:
                assert response[entry] == 1
            samples.append(response)
        stdout, stderr = sampler.communicate(timeout=30)
        if sampler.returncode:
            raise RuntimeError(f'sample failed: {stdout}\n{stderr}')
        return dict(tier=tier, readiness=readiness, responses=samples,
                    command=command, sampler_stdout=stdout, sampler_stderr=stderr)
    finally:
        if sampler is not None and sampler.poll() is None:
            sampler.terminate()
            sampler.wait(timeout=30)
        if process.poll() is None:
            process.stdin.write('quit\n')
            process.stdin.flush()
        process.wait(timeout=30)
        stderr = process.stderr.read()
        if process.returncode or stderr:
            raise RuntimeError(f'worker failed ({process.returncode}): {stderr}')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--worker', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--tiers', nargs='+', choices=['bytecode', 'lir', 'native', 'copy-patch'],
                        default=['bytecode', 'native'])
    parser.add_argument('--duration', type=int, default=10)
    parser.add_argument('--interval-ms', type=int, default=1)
    parser.add_argument('--repeat', type=int, default=2)
    args = parser.parse_args()
    if min(args.duration, args.interval_ms, args.repeat) < 1:
        parser.error('duration, interval and repeat must be positive')
    args.worker = args.worker.resolve()
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=False)
    metadata = dict(timestamp=datetime.datetime.now(datetime.timezone.utc).isoformat(),
                    platform=platform.platform(), worker=str(args.worker), sha256=digest(args.worker),
                    duration=args.duration, interval_ms=args.interval_ms, repeat=args.repeat, profiles=[])
    # Alternate tiers between passes; workers always finish before the next profile starts.
    for repetition in range(args.repeat):
        for tier in args.tiers:
            path = args.output / f'{tier}-{repetition + 1}.sample.txt'
            result = profile(args.worker, tier, args.duration, args.interval_ms, path)
            metadata['profiles'].append(dict(path=path.name, **result))
            (args.output / 'metadata.json').write_text(json.dumps(metadata, indent=2) + '\n')
            print(f'profiled {tier} pass {repetition + 1}: {len(result["responses"])} validated runs', flush=True)


if __name__ == '__main__':
    main()
