"""Rerun k-nucleotide outside Git using cached inputs and the Flying Circus adapter (Python 3.12+)."""

import argparse
import os
import shutil
import subprocess
import tarfile
from pathlib import Path

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument(
    "--archive",
    type=Path,
    default=Path.home() / "Library/Caches/tis-benchmarks/2026-10-30-sorted-slices.tar.gz",
)
parser.add_argument("--worker", type=Path, help="Optional freshly built target/release/examples/circus")
parser.add_argument("--output", type=Path, required=True, help="New directory outside the repository")
args = parser.parse_args()
root = args.output.resolve()
repository = next((p for p in Path(__file__).resolve().parents if (p / ".git").exists()), None)
if repository and root.is_relative_to(repository):
    parser.error("raw results must be written outside the repository")
cpython, pypy = shutil.which("python3"), shutil.which("pypy3")
if not cpython or not pypy:
    parser.error("python3 and pypy3 must be on PATH")
root.mkdir(parents=True, exist_ok=False)
with tarfile.open(args.archive) as archive:
    archive.extractall(root, filter="data")
with tarfile.open(root / "harness-snapshot.tar.gz") as archive:
    archive.extractall(root, filter="data")
worker = str(args.worker.resolve()) if args.worker else str(root / "binaries/after-worker")
python = Path.home() / ".local/share/uv/tools/flying-circus/bin/python"
command = [
    str(python),
    "-m",
    "flying_circus",
    "--quick",
    "-x1",
    "--benchmark",
    str(root / "game_knucleotide.py"),
    "-r",
    "tis-bytecode=" + worker,
    "-r",
    "tis-native=" + worker,
    "-r",
    "cpython=" + cpython,
    "-r",
    "pypy=" + pypy,
    "--runtime-arg",
    "tis-bytecode=--tier",
    "--runtime-arg",
    "tis-bytecode=bytecode",
    "--runtime-arg",
    "tis-native=--tier",
    "--runtime-arg",
    "tis-native=native",
    "--runtime-arg",
    "tis-native=--readonly-regions",
    "--runtime-arg",
    "tis-native=--mutation-regions",
    "--format",
    "html",
    "--format",
    "markdown",
    "--format",
    "json",
    "--output",
    str(root / "rerun"),
]
env = os.environ.copy()
env["PYTHONPATH"] = str(root / "harness/src")
raise SystemExit(subprocess.run(command, env=env, cwd=root / "harness").returncode)
