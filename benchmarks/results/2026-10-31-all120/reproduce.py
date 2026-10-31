"""Run the unchanged full suite with the archived tis worker adapter."""
import argparse
import os
from pathlib import Path
import subprocess

parser = argparse.ArgumentParser()
parser.add_argument('--binary', type=Path, required=True)
parser.add_argument('--harness', type=Path, required=True)
parser.add_argument('--harness-python', type=Path, required=True)
parser.add_argument('--cpython', type=Path, required=True)
parser.add_argument('--output', type=Path, required=True)
args = parser.parse_args()
command = [str(args.harness_python.resolve()), '-m', 'flying_circus', '--quick', '-v', '-x1',
           '--output', str(args.output.resolve()), '--format', 'html', '--format', 'markdown', '--format', 'json']
for label, tier in [('tis-bytecode', 'bytecode'), ('tis-lir', 'lir'),
                    ('tis-cranelift', 'native'), ('tis-cnp', 'copy-patch')]:
    command += ['-r', f'{label}={args.binary.resolve()}', '--runtime-arg', f'{label}=--tier',
                '--runtime-arg', f'{label}={tier}']
command += ['-r', f'cpython={args.cpython.resolve()}']
environment = os.environ.copy()
environment['PYTHONPATH'] = str(args.harness.resolve() / 'src')
subprocess.run(command, cwd=args.harness.resolve(), env=environment, check=True)
