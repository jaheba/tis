"""Rerun the exact suite from its external evidence archive (Python 3.12+)."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tarfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--archive', type=Path, default=Path.home() / 'Library/Caches/tis-benchmarks/2026-10-30-benchmark-game.tar.gz')
parser.add_argument('--output', type=Path, required=True, help='New directory outside the repository')
args = parser.parse_args()
args.output.mkdir(parents=True, exist_ok=False)
root = args.output.resolve()
with tarfile.open(args.archive) as archive:
    archive.extractall(root, filter='data')
with tarfile.open(root / 'harness-snapshot.tar.gz') as archive:
    archive.extractall(root, filter='data')
old = '/private/tmp/tis-benchmark-game'
command = [arg.replace(old, str(root)) for arg in json.loads((root / 'command.json').read_text())]
command[command.index('--output') + 1] = str(root / 'rerun')
env = os.environ.copy()
env['PYTHONPATH'] = str(root / 'harness/src')
raise SystemExit(subprocess.run(command, env=env, cwd=root / 'harness').returncode)
