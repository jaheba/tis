from pathlib import Path
import json,statistics
b=Path(__file__).resolve().parent;p=json.loads((b/'photo.json').read_text());r=json.loads((b/'regressions.json').read_text());f=json.loads((b/'followup.json').read_text()) if (b/'followup.json').exists() else {}
s='''# Captured locals in eager comprehensions

The exact photographed sieve, unchanged at 30,000,000, is saved as sieve_photo.py and tests/fixtures/sieve_comprehension.py. Every runtime prints 1857859. The before executable includes the preceding uncommitted bytecode fast paths. Binary and source hashes, base revision and compiler versions are in provenance.json.

## Exact 30-million sieve

Three serial fresh-process samples per runtime, randomized order; table shows medians. No workload warmup. Executables were primed with --version to exclude first-launch OS verification. Startup, parsing, HIR lowering, native compilation, execution, output and exit are included. Both tis builds use the same --stats flag. RSS comes from wait4 on macOS ARM64 and is reported in MiB. All runtime programs are identical; tis executes its own release binary.

| Runtime | Elapsed (ms) | Peak RSS (MiB) |
|---|---:|---:|
'''
for k,t in p['median_ms'].items():s+=f"| {k} | {t:.2f} | {p['median_rss_mib'][k]:.1f} |\n"
s+='\n## Runtime evidence\n\nThe comprehension previously captured is_prime, keeping marking and scanning accesses in environment lookups. Eligible eager comprehensions now promote private target variables to SSA before their blocks are spliced into the enclosing HIR. The outer list remains an ordinary local. Temporary SSA values stay within the comprehension graph instead of increasing caller frame slots and phi arguments on unrelated loops.\n\n| Counter | Before native | After native |\n|---|---:|---:|\n'
for k in ['native_runtime_calls','name_cache_hits','native_root_syncs','deoptimizations','compilation_errors','native_compile_ns']:
 v=[statistics.median(x['stats'][k] for x in p['samples'][label]) for label in ['before-native','after-native']];s+=f'| {k} | {v[0]:,.0f} | {v[1]:,.0f} |\n'
s+='''
Before/after HIR dumps are saved alongside the timings. The final HIR has ordinary local slots i, is_prime, j and n; no comprehension target slots or environment loads of is_prime remain.

Inlining requires a potential enclosing-local capture benefit. Noncapturing comprehensions keep smaller separately compiled functions to limit cold native compilation cost. Escaping callables, suspended generators, frame-sensitive operations, possibly unbound targets, module/class scopes and exception-control fragments retain the existing separate-frame implementation. Calls that can mutate an outer captured cell still read its live environment; no captured value is snapshotted.

## Existing-workload regression check

Three paired samples in each tier, identical source and exact expected stdout from the preceding CPython-verified suite. Every output passes. Native Barnes-Hut uses matching read-only/mutation-region flags in both builds. These are quick timings; small differences, especially on startup-dominated workloads, need confirmation.

| Workload | Bytecode before (ms) | After (ms) | Native before (ms) | After (ms) |
|---|---:|---:|---:|---:|
'''
for n,d in r.items():
 m=d['median_ms'];s+=f"| {n} | {m['before-bytecode']:.2f} | {m['after-bytecode']:.2f} | {m['before-native']:.2f} | {m['after-native']:.2f} |\n"
s+='\n## Follow-up\n\nFive additional native samples of the exact photo sieve, seven samples for ticket search in both tiers, and nine samples for two small JSON cases. Fresh processes, matched commands, exact stdout checks.\n\n| Workload | Runtime | Median (ms) |\n|---|---|---:|\n'
for n,d in f.items():
 for k,t in d['median_ms'].items():s+=f'| {n} | {k} | {t:.2f} |\n'
s+='''
## Validation and reproduction

Formatting, all-target Clippy with warnings denied, and the full Cargo suite pass: 402 tests. The five new regression tests exercise all four execution tiers with register and stack bytecode, local/target shadowing, first-iterable order, nested list/set/dict comprehensions, live nonlocal rebinding, GC pressure, errors, and escaping lambda cells. Scope assertions also pass independently in CPython (scope_reference.py).

Build with cargo build --release --offline --locked --bin tis --example ir --example bench --example run. Run the CLI with --tier native tests/fixtures/sieve_comprehension.py, or --tier bytecode for the interpreter. The CLI defaults to bytecode. Before/after binaries are frozen locally under binaries/ and gitignored. measure.py and followup.py contain the exact serial commands and sampling procedure; copied workloads and raw outputs are retained. The measurement script's previous-suite path resolves expected outputs and workload snapshots in the repository. Intermediate experiments are under frame-slot-pass/ and all-comprehensions-pass/; their timings are not final results.

PyPy remains faster on this sieve. No parity claim is made from these quick samples, and native compilation costs are included rather than reported as warmed execution.
'''
(b/'README.md').write_text(s)
