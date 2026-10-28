import json
import subprocess
import sys
from pathlib import Path

root = Path('/Users/jaheba/p/tis')
sys.path.insert(0, str(root / 'tools'))
from bench_nbody import digest, summarize

command = [sys.executable, str(root / 'tools/bench_nbody.py'), '--cpython-worker',
           '--source', str(root / 'tests/fixtures/game_nbody.py'), '--warmups', '2']
worker = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                          stderr=subprocess.PIPE, text=True, bufsize=1)
ready = json.loads(worker.stdout.readline())
samples = []
for _ in range(10):
    worker.stdin.write('run\n')
    worker.stdin.flush()
    sample = json.loads(worker.stdout.readline())
    assert sample['energy'] == '-0.169089263'
    samples.append(sample)
worker.stdin.write('quit\n')
worker.stdin.flush()
worker.wait(timeout=10)
assert worker.returncode == 0 and not worker.stderr.read()
jit = getattr(sys, '_jit', None)
result = dict(python=sys.version, executable=sys.executable, sha256=digest(sys.executable),
              jit_available=bool(jit and jit.is_available()), jit_enabled=bool(jit and jit.is_enabled()),
              command=command, readiness=ready, samples=samples,
              summary=summarize([sample['run_ns'] for sample in samples]),
              note='Separate control run after the tier comparison; identical scene restoration and energy checks.')
Path('/private/tmp/tis-profile/sequence-cpython.json').write_text(json.dumps(result, indent=2) + '\n')
print(json.dumps(result['summary'], indent=2))
