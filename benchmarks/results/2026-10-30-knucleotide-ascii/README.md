# K-nucleotide ASCII metadata

Baseline: da61666. Candidate: that revision plus cached ASCII metadata in
`heap.rs` and `execution.rs`. Release profile, default features, offline locked
build, macOS arm64. No benchmark source changes.

The slice fast path repeatedly called `is_ascii()` on the entire source string.
K-nucleotide takes about 700,000 short slices of a 100,000-character string,
turning this check into tens of gigabytes of redundant reads. Record ASCII at
allocation, invalidate before mutable host access, and reset on heap-slot reuse.
The cache fits in existing generation-table padding; object and native ABI layouts
are unchanged. ASCII `len` also uses this metadata.

## Measurements

Flying Circus `--quick -x1`: three samples per mode, one repeated warmup,
one RSS sample. Worker readiness, startup and fixture setup excluded; request
compilation, execution and output included. Repeated requests use fresh VMs,
not retained warmed JIT code. Output checked exactly against CPython.

| Runtime | One-shot ms | Repeated ms | Peak RSS MiB |
| --- | ---: | ---: | ---: |
| tis bytecode | 322 | 321 | 23.6 |
| tis native | 456 | 455 | 28.7 |
| CPython 3.15 | 81.4 | 77.4 | 31.0 |
| PyPy 7.3.16 | 58.2 | 55.9 | 84.4 |

Paired CLI runs include process launch and fixture setup, unlike the worker table.
Three samples each: bytecode 923 → 326 ms (2.83×), native 1083 → 470 ms (2.31×).
Quick sampling is approximate, not a statistical significance claim.

The remaining native profile is led by runtime helper dispatch and root syncing,
followed by heap lookup, slot conversion, string hashing and allocation. Temporary
slice objects and repeated dictionary probes are candidates for further work.
Native still loses to bytecode, CPython and PyPy on this workload.

## Validation and reproduction

`cargo fmt --all`, all-target Clippy with `-D warnings`, and 416 tests pass.
Three regressions cover mutable string access (including forgotten guards),
foreign/stale handles and slot reuse, and replacing a non-string with a string.

Raw samples, source snapshots, binaries and profiles are archived externally under
`~/Library/Caches/tis-benchmarks/2026-10-30-knucleotide-ascii.tar.gz`.
The archive includes the unchanged standalone workload, CLI comparison script,
Flying Circus command/adapter references, hashes and validation logs. Use the
archived `compare.py` with frozen binaries for the paired CLI comparison, or
`run_quick.py` with the Flying Circus adapter from the prior Game experiment.

Three-sample CLI controls (before → after, milliseconds):

| Workload | Bytecode | Native |
| --- | ---: | ---: |
| sieve | 174.9 → 172.0 | 50.4 → 49.8 |
| game_nbody | 282.7 → 291.1 | 38.1 → 38.2 |
| barnes_hut | 2936.8 → 2917.4 | 680.0 → 689.8 |
| perf_unpack_sequence | 178.2 → 175.7 | 221.8 → 221.2 |

Outputs match before/after. Variations are within about 3%; this short run cannot establish absence of small regressions.
