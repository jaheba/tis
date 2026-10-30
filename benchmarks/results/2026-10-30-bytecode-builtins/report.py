from pathlib import Path
import json,hashlib,re
b=Path(__file__).resolve().parent;root=Path('/private/tmp/tis-bytecode-opt');o=json.loads((b/'oneshot.json').read_text());w=json.loads((b/'warmed.json').read_text());f=json.loads((b/'followup.json').read_text())
p=b/'provenance.json';d=json.loads(p.read_text());d['source_sha256']={n:hashlib.sha256((root/n).read_bytes()).hexdigest() for n in json.loads((b/'changed-files.json').read_text())};d['binary_sha256']={x.name:hashlib.sha256(x.read_bytes()).hexdigest() for x in (b/'binaries').iterdir() if x.name!='.gitignore'};p.write_text(json.dumps(d,indent=2)+'\n')
s='''# Bytecode builtin and profiling fast paths

Base: `3f01b92` (native sieve optimization). Candidate is uncommitted.
Release builds on macOS ARM64, Rust 1.99.0. Frozen executable hashes and changed-source hashes are in `provenance.json`.

The original sieve sample showed dispatch, heap access, repeated loop admission/profile checks and iterator handling dominating execution. The candidate adds guarded exact-sequence reads/stores, cached immutable range parameters, quiet iterator-header fusion, VM/window-specific admission memoization and frame-local backedge batching. Counters are merged before new admission and on return/error. Ordinary field access skips redundant polls only while descriptor/hook guards hold. Custom protocols, allocating fallbacks and feedback reopening retain the full runtime path.

## One-shot comparison

Milliseconds, median of three serial samples per runtime/workload, shuffled runtime order. Fresh processes include startup, parsing, compilation, execution, output and exit; no workload warmup. Executable/file caches were primed. Monty's CLI diagnostic stderr is included. Every output matches CPython exactly. These are quick samples, not stable performance guarantees; small differences and startup-dominated workloads are noisy. Barnes native enables the same readonly/mutation-region flags in both builds.

| Workload | Before bytecode | After bytecode | Change | Monty | CPython | After native |
|---|---:|---:|---:|---:|---:|---:|
'''
for n,x in o.items():
 m=x['median_ms'];a=m['after-bytecode'];z=m['before-bytecode'];s+=f"| {n} | {z:.2f} | {a:.2f} | {(a/z-1)*100:+.1f}% | {m.get('monty',float('nan')):.2f} | {m['cpython']:.2f} | {m['after-native']:.2f} |\n"
s+='\n## Retained-VM sieve\n\nOne untimed warmup and three serial timed calls. Definitions/setup excluded; each call includes fresh sieve allocation, marking, scanning and output capture. Native compilation from the warmup is excluded and kept separately in `warmed.json`.\n\n'
for n,t in w['median_ms'].items():s+=f'- {n}: {t:.2f} ms\n'
s+='\n## Five-sample follow-up\n\nSame fresh-process boundaries and exact-output verification.\n\n| Workload | Before bytecode | After bytecode | Change |\n|---|---:|---:|---:|\n'
for n,x in f.items():
 m=x['median_ms'];s+=f"| {n} | {m['before-bytecode']:.2f} | {m['after-bytecode']:.2f} | {(m['after-bytecode']/m['before-bytecode']-1)*100:+.1f}% |\n"
s+='''
## Validation and reproduction

`cargo fmt --all`, `cargo clippy --all-targets --offline --locked -- -D warnings`, and `cargo test --offline --locked` pass (397 tests). Regression coverage includes later hot loops, shared code across VMs, numeric/tier changes, detailed feedback reopening through recursive truth callbacks, iterator cursor behavior and GC roots. The 14-workload matrix also checks native execution and the sieve stack interpreter. No workload outputs are precomputed by tis.

Build with `cargo build --release --offline --locked --bin tis --example bench --example run --example circus`. Place the candidate release examples into `binaries/after-bench` and `binaries/after-run`, and the CLI into `binaries/tis`. Baseline binaries are frozen from the preceding sieve report. Run `TIS_SOURCE_ROOT=/Users/jaheba/p/tis python3 measure.py` serially, then `python3 followup.py` after updating its captured paths if moving this directory. Raw commands, outputs, compile costs, timing samples and failures are retained in the JSON files. Executable files are locally retained and gitignored. Intermediate experiment results are preserved in `first-pass/`, `unpack-pass/`, `field-pass/` and `store-pass/`.

This is a targeted bytecode improvement, not parity with CPython or PyPy. In particular, unpacking and ticket search still lag Monty. See the follow-up table for residual small regressions rather than extrapolating the sieve result.
'''
(b/'README.md').write_text(s)
