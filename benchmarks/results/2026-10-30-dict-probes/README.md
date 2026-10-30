# Builtin dictionary probe reduction

Baseline: 0354d7b plus the direct HIR slice-read change recorded in
`../2026-10-30-fused-slices/README.md`. Candidate changes only runtime mapping
lookup/update paths. Release/default features on macOS arm64, native readonly and
mutation region flags enabled.

Builtin-only mapping reads borrow string storage or convert other keys once.
They previously constructed a key just to test whether the protocol path was
needed, then converted it again for lookup. Builtin-only dictionary writes now
use insertion's existing probe instead of searching first and probing again.
Builtin key conversion also bypasses unnecessary Python protocol dispatch.

Mappings containing custom keys retain the existing hash/equality path, including
mutation retries. Set insertion still checks membership to retain the original
value. Dictionary insertion preserves original equal-key identity and order.
No cross-instruction lookup cache or speculative mutation assumptions are added.

## Measurement

Unchanged Game k-nucleotide, FASTA n=20,000. Flying Circus `--quick -x1`, three
samples per mode, one repeated warmup and one RSS sample. Compilation, execution
and output included; worker startup and fixture preparation excluded. Repeated
requests use fresh VMs rather than retained warmed JIT code. Exact stdout is
validated against CPython. These short runs are approximate.

| Runtime | One-shot ms | Repeated ms | Peak RSS MiB |
| --- | ---: | ---: | ---: |
| tis bytecode | 222 | 220 | 23.6 |
| tis native | 321 | 322 | 28.5 |
| CPython 3.15 | 74.0 | 77.7 | 34.4 |
| PyPy 7.3.16 | 54.6 | 52.7 | 82.4 |

Five-sample alternating CLI comparison, including launch and fixture setup:
bytecode 277 → 232 ms (16% less), native 384 → 339 ms (12% less).
A separate three-sample comparison agrees: 278 → 231 ms and 387 → 341 ms.
Native remains slower than bytecode and both Python baselines.

Runtime calls remain 2,662,469 and root syncs 2,621,447 in both native builds.
Neither run records compilation errors or deoptimizations. This change reduces
runtime helper work rather than crossing the native/runtime boundary less often.

## Controls and validation

Three-sample CLI controls, before → after in milliseconds:

| Workload | Bytecode | Native |
| --- | ---: | ---: |
| sieve | 171.8 → 169.3 | 50.2 → 51.4 |
| game_nbody | 278.0 → 274.6 | 37.3 → 36.8 |
| barnes_hut | 2833.0 → 2833.7 | 668.6 → 673.3 |
| perf_unpack_sequence | 168.8 → 171.3 | 216.4 → 217.6 |

Outputs match before/after. Control variations are below 3%; quick samples cannot
rule out small regressions.

Formatting, all-target Clippy with `-D warnings`, and 418 tests pass. The new
regression runs all four tiers with both bytecode encodings. It checks numeric
key equivalence, insertion order, original equal-key identity, long string and
tuple keys, mixed custom/builtin keys, hash/equality callback counts, and duplicate
set insertion. The same Python test passes in CPython.

Raw samples, standalone benchmark source, frozen binaries, hashes, validation
logs, adapter source and reproduction scripts are archived outside Git at
`~/Library/Caches/tis-benchmarks/2026-10-30-dict-probes.tar.gz`.
`final-followup.py` runs the alternating comparison; `compare.py` runs controls;
`run_quick.py` records the Flying Circus invocation. Build with
`cargo build --release --locked --bin tis --example circus`, default features.
