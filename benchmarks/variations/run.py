"""Run Flying Circus fresh sessions and a separate retained-session comparison."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import select
import shutil
import subprocess
import time

PYTHON = '/Users/jaheba/.local/share/uv/tools/flying-circus/bin/python'
CACHE = Path.home() / 'Library/Caches/tis-benchmarks'
HARNESS = CACHE / '2026-10-31-all120/harness/src'
FROZEN = CACHE / '2026-10-31-counter-loops/worker'
TIERS = [('bytecode', 'bytecode'), ('cnp', 'copy-patch'), ('cranelift', 'native')]


def fresh(root):
    command = [PYTHON, '-m', 'flying_circus', '--quick', '--samples', '3', '--warmups', '3',
               '--memory-samples', '0', '--startup-samples', '2', '-x1', '-v',
               '--output', str(root / 'fresh'), '--format', 'html', '--format', 'json']
    for path in sorted((root / 'programs').glob('*.py')):
        command += ['--benchmark', str(path)]
    for label, tier in TIERS:
        command += ['-r', label + '=' + str(root / 'worker'),
                    '--runtime-arg', label + '=--tier', '--runtime-arg', label + '=' + tier]
        if label != 'bytecode':
            for flag in ['--readonly-regions', '--mutation-regions']:
                command += ['--runtime-arg', label + '=' + flag]
    command += ['-r', 'cpython=/opt/homebrew/bin/python3', '-r', 'pypy=/opt/homebrew/bin/pypy3']
    env = os.environ.copy()
    env['PYTHONPATH'] = str(HARNESS)
    (root / 'fresh-command.json').write_text(json.dumps(command, indent=2))
    with (root / 'fresh.log').open('w') as log:
        result = subprocess.run(command, env=env, stdout=log, stderr=subprocess.STDOUT)
    (root / 'fresh-exit.json').write_text(json.dumps({'exit': result.returncode}))
    return result.returncode


def retained(root, warmups=20, samples=10):
    (root / 'retained-config.json').write_text(json.dumps(dict(warmups=warmups, samples=samples)))
    manifest = json.loads((root / 'programs/manifest.json').read_text())
    commands = {label: [str(root / 'worker'), '--tier', tier, '--worker'] for label, tier in TIERS}
    for label in ['cnp', 'cranelift']:
        commands[label][1:1] = ['--readonly-regions', '--mutation-regions']
    for label, binary in [('cpython', '/opt/homebrew/bin/python3'), ('pypy', '/opt/homebrew/bin/pypy3')]:
        commands[label] = [binary, '-u', str(HARNESS / 'flying_circus/cpython_worker.py')]
    results = {}
    for label, command in commands.items():
        rows = results[label] = {}
        for name, case in manifest.items():
            with (root / 'retained.stderr').open('a') as errors:
                worker = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                          stderr=errors, text=True, bufsize=1)
                def response():
                    if not select.select([worker.stdout], [], [], 60)[0]:
                        raise TimeoutError('worker response')
                    line = worker.stdout.readline()
                    if not line:
                        raise RuntimeError('worker exited')
                    return json.loads(line)

                def request(code):
                    worker.stdin.write(json.dumps({'kind': 'run', 'code': code, 'filename': name}) + '\n')
                    worker.stdin.flush()
                    return response()

                try:
                    assert response().get('ready')
                    source = (root / 'programs' / (name + '.py')).read_text()
                    setup, invocation = source.split('# --- benchmark ---')
                    start = time.perf_counter()
                    prepared = request(setup)
                    setup_seconds = time.perf_counter() - start
                    if not prepared.get('ok'):
                        raise RuntimeError(prepared)
                    warm, measured = [], []
                    for index in range(warmups + samples):
                        start = time.perf_counter()
                        result = request(invocation)
                        seconds = time.perf_counter() - start
                        if not result.get('ok') or result.get('stdout') != case['stdout']:
                            raise RuntimeError(result)
                        (warm if index < warmups else measured).append(seconds)
                    rows[name] = dict(status='ok', setup_seconds=setup_seconds,
                                      warmup_seconds=warm, sample_seconds=measured)
                except Exception as error:
                    rows[name] = dict(status='error', error=str(error))
                finally:
                    worker.stdin.close()
                    try:
                        worker.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        worker.kill()
                        worker.wait()
            print('retained', label, name, rows[name]['status'], flush=True)
            (root / 'retained.json').write_text(json.dumps(results, indent=2))
    return results


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('root', type=Path)
    parser.add_argument('--worker', type=Path, default=FROZEN)
    parser.add_argument('--skip-fresh', action='store_true')
    parser.add_argument('--retained-warmups', type=int, default=20)
    parser.add_argument('--retained-samples', type=int, default=10)
    args = parser.parse_args()
    root = args.root.resolve()
    shutil.copy2(args.worker, root / 'worker')
    provenance = {}
    for label, path in [('tis', root / 'worker'), ('cpython', Path('/opt/homebrew/bin/python3').resolve()),
                        ('pypy', Path('/opt/homebrew/bin/pypy3').resolve())]:
        version = subprocess.run([str(path), '--version'], capture_output=True, text=True)
        provenance[label] = dict(path=str(path), sha256=hashlib.sha256(path.read_bytes()).hexdigest(),
                                 version=version.stdout.strip(), stderr=version.stderr.strip())
    (root / 'provenance.json').write_text(json.dumps(provenance, indent=2))
    if not args.skip_fresh:
        print('fresh exit', fresh(root), flush=True)
    retained(root, args.retained_warmups, args.retained_samples)
