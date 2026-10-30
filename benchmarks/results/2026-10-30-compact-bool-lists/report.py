from pathlib import Path
import json,statistics
b=Path(__file__).resolve().parent;p=json.loads((b/'photo.json').read_text());r=json.loads((b/'regressions.json').read_text());f=json.loads((b/'followup.json').read_text());before=json.loads((b/'frozen-before-phases.json').read_text());after=json.loads((b/'after-phases.json').read_text());refs=json.loads((b/'before-phases.json').read_text())['samples']
s='''# Compact boolean lists after sieve phase profiling

Base revision: 38291ae. The unchanged photographed sieve at 30,000,000 is sieve_photo.py; all runs print 1857859. Source and executable hashes are recorded in provenance.json. Builds and tests were completed before serial benchmark sampling; no interpreter execution is delegated to Python.

## Phase diagnosis

The split allocate/mark/collect program is diagnostic: it changes function/JIT boundaries, and is not the end-to-end performance baseline. Each sample uses a fresh VM/process, with definitions and startup excluded from phase timers; first-call native compilation is included. Rust host roots keep the flags alive between calls. Three samples, median milliseconds. Original pre-change observations are retained in before-phases.json; the baseline phase driver was subsequently rebuilt from byte-identical base source and frozen for reproduction.

| Runtime | Allocate | Mark | Collect |
|---|---:|---:|---:|
'''
for name,rows in [('tis before',before),('tis after',after),('CPython',refs['cpython']),('PyPy',refs['pypy'])]:s+='| '+name+' | '+' | '.join(f'{statistics.median(x[k] for x in rows)/1e6:.2f}' for k in ['allocate_ns','mark_ns','collect_ns'])+' |\n'
s+='''
Marking was the dominant gap. Collection was already close to PyPy despite roughly 1.86 million append callbacks. The change therefore targets flag storage and memory traffic.

## Unchanged end-to-end program

Three paired fresh-process samples per runtime in randomized order, median elapsed milliseconds and peak RSS MiB from wait4 on macOS ARM64. Startup, parsing, HIR/native compilation, execution, stdout capture and exit are included. No workload warmup; executable verification/file caches were primed with --version. Both tis variants use identical --stats flags. The before executable is the prior installed CLI; all four runtimes execute identical full-size source.

| Runtime | Elapsed (ms) | RSS (MiB) |
|---|---:|---:|
'''
for name,t in p['median_ms'].items():s+=f"| {name} | {t:.2f} | {p['median_rss_mib'][name]:.1f} |\n"
s+='''
Homogeneous boolean lists can retain one byte per element. Primitive repetition copies the compact buffer directly, avoiding a temporary array of 16-byte Values. Native byte loads/stores guard identity, generation, list storage and bounds. Non-boolean stores use the existing helper before converting to boxed values, preserving bool/int distinctions and arbitrary references. GC scans boxed lists but skips primitive buffers and their boxed snapshots.

A shared boxed snapshot revokes direct native boolean writes via an interior Cell token. Only an exclusive mutation clears the snapshot and republishes a writable pointer. The byte-storage pointer shares the native sequence address field, but kind 8 is decoded only as bytes; it is never dereferenced as a Value. Existing mutation/GC invalidation, cached-view restrictions and generation checks remain in place.

A first candidate regressed unpacking. Assembly showed that merging a Bool constructor with whole-Value reads made LLVM copy enum/padding fields individually. The final getter isolates that constructor behind a non-inlined helper, restoring 16-byte vector copies for ordinary values. Intermediate measurements and assembly are in scalarized-getter-pass/ and probe-getter-assembly.txt.

## Existing workload checks

Three serial paired samples per tier, exact stdout verified against the preceding CPython-validated snapshots. Native Barnes-Hut uses matching readonly/mutation-region flags. Small differences, especially process startup cases, are noisy.

| Workload | Bytecode before | After | Native before | After |
|---|---:|---:|---:|---:|
'''
for name,d in r.items():
 m=d['median_ms'];s+=f"| {name} | {m['before-bytecode']:.2f} | {m['after-bytecode']:.2f} | {m['before-native']:.2f} | {m['after-native']:.2f} |\n"
s+='\n## Follow-up\n\nFive native sieve/PyPy samples, seven unpacking/n-body samples and nine small JSON samples. All output checks pass.\n\n| Workload | Runtime | Median ms |\n|---|---|---:|\n'
for name,d in f.items():
 for key,t in d['median_ms'].items():s+=f'| {name} | {key} | {t:.2f} |\n'
s+='''
The large unpacking regression is fixed; the final follow-up still has about 2.7% higher bytecode unpacking time and about 1% higher n-body time. These costs are retained in the evidence rather than summarized as universal speedups. PyPy remains faster: its one-shot time varied around 0.79–0.87 seconds here, compared with roughly 0.97 seconds for tis native.

## Validation and reproduction

Formatting, all-target Clippy with warnings denied, and the full suite pass (405 tests). New coverage checks compact repetition/GC without materialization, bool identity, mixed integer/reference writes, aliasing, negative/bool indices, mutation, native stores, and shared snapshot invalidation both with and without intervening GC. The native gateway/provenance review is documented above; copied heap storage modules were not modified.

Build with cargo build --release --offline --locked --bin tis --example sieve_phases --example bench --example run. Run tis --tier native tests/fixtures/sieve_comprehension.py for the real workload. Run the sieve_phases example with this directory's phases.py for diagnostic JSON. Frozen before/after CLI and phase executables are locally retained under binaries/ and gitignored. measure.py and followup.py replay the paired workload samples; python_phases.py measures the same diagnostic phases in CPython/PyPy. Raw commands, outputs, RSS and native compile/stat counters are retained in JSON files, with validation logs under validation/.
'''
(b/'README.md').write_text(s)
