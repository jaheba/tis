#!/usr/bin/env python3
"""Check unchanged Flying Circus workloads against their recorded stdout."""

import argparse
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import subprocess
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--suite', type=Path, required=True,
                        help='Flying Circus workloads directory containing manifest.json')
    parser.add_argument('--run', type=Path, required=True)
    parser.add_argument('--tiers', nargs='+', default=['bytecode'],
                        choices=['bytecode', 'lir', 'native', 'copy-patch'])
    parser.add_argument('--benchmark', action='append', help='Exact manifest name; repeatable')
    parser.add_argument('--timeout', type=float, default=30)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()

    if args.timeout <= 0:
        parser.error('--timeout must be positive')

    suite = args.suite.resolve()
    binary = args.run.resolve()
    manifest_path = suite / 'manifest.json'
    manifest = json.loads(manifest_path.read_text())
    selected = args.benchmark or list(manifest)
    if unknown := set(selected) - manifest.keys():
        parser.error(f'unknown benchmarks: {sorted(unknown)}')

    # Keep existing reports intact, and save after each request for interrupted runs.
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.open('x'):
        pass

    report = {
        'timestamp': datetime.now(timezone.utc).isoformat(),
        'binary': str(binary),
        'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
        'manifest': str(manifest_path),
        'manifest_sha256': hashlib.sha256(manifest_path.read_bytes()).hexdigest(),
        'timeout_seconds': args.timeout,
        'results': [],
    }
    for name in selected:
        entry = manifest[name]
        source = (suite / entry['file']).resolve()
        for tier in args.tiers:
            command = [str(binary), '--tier', tier, str(source)]
            started = time.monotonic()
            record = dict(name=name, tier=tier, command=command,
                          source_sha256=hashlib.sha256(source.read_bytes()).hexdigest())
            try:
                result = subprocess.run(command, capture_output=True, text=True,
                                        timeout=args.timeout)
                status = ('error' if result.returncode else
                          'mismatch' if result.stdout != entry['stdout'] else
                          'stderr' if result.stderr else 'pass')
                record.update(status=status, returncode=result.returncode,
                              stdout=result.stdout, stderr=result.stderr)
            except subprocess.TimeoutExpired as error:
                # TimeoutExpired carries bytes even when text mode was requested.
                record.update(status='timeout', stdout=(error.stdout or b'').decode(),
                              stderr=(error.stderr or b'').decode())
            record['seconds'] = time.monotonic() - started
            record['expected_stdout'] = entry['stdout']
            report['results'].append(record)
            args.output.write_text(json.dumps(report, indent=2) + '\n')
            detail = record['stderr'].strip().splitlines()
            print(f"{name}/{tier}: {record['status']}" +
                  (f' ({detail[0]})' if detail else ''), flush=True)

    passed = sum(row['status'] == 'pass' for row in report['results'])
    print(f"{passed}/{len(report['results'])} passed; {args.output}")
    return 0 if passed == len(report['results']) else 1


if __name__ == '__main__':
    raise SystemExit(main())
