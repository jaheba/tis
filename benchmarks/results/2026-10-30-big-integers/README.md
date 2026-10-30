# Arbitrary-precision integers

Baseline commit: adb81da (compact boolean lists and bytes compatibility). New integer changes remain uncommitted.
The source hashes, release build command and frozen binary hashes are in provenance.json and changed-files.json.
Local release binaries are retained under ignored binaries/.

## Implementation

Small integers keep their existing inline i64 representation and checked fast paths.
Overflow and large literals use immutable heap integers; results normalize back to inline values when possible.
JIT overflow guards reconstruct the pre-operation frame and resume generic arithmetic; prior writes are not replayed.
Profiles distinguish BigInt references from unboxed Int values. Value remains 16 bytes.

The runtime service covers arithmetic, floor division/modulo, shifts, bit operations, exact mixed float comparisons,
integer true division, hashing and numeric key equality. The same values work through JSON, int construction,
bit_length/bit_count, divmod, pow (including modular inverses), hex, oct and bin.
Float divmod and its // and % operations now share remainder-based quotient correction.

Remaining gaps include integer byte-conversion methods, Unicode decimal parsing, decimal string digit limits,
formatting and diagnostic compatibility. Ranges and native integer registers remain i64. This is not full Monty parity.

## Validation

cargo fmt --all; all-target Clippy with -D warnings; cargo test --no-fail-fast: 411 passed.
The integer fixture runs through all four tiers and both interpreter encodings.
Tests check overflowing compiled addition after prior writes, collection, dictionary key identity and root retention.
The same Python fixture passes CPython 3.15.0.

The deterministic differential script checks 642 cases per tier in bytecode and native modes against CPython.
Values match in every case, including bit-for-bit floating division. There are 38 float representation differences per tier;
those remain visible in differential/bigint_differential.json rather than being counted as exact textual matches.

## Monty fixture audit

All 711 fixtures remain visible. Bytecode and native counts match.
The audit uses Monty expectations/assertions; it does not execute reference fixtures in host CPython.
Tracebacks and exception-message mismatches remain separate from exact passes.
Baseline audit: ../2026-10-30-monty-parity/after-bytecode/results.json.

| Status | Before | After |
|---|---:|---:|
| engine_error | 4 | 4 |
| exception | 67 | 64 |
| exception_message_difference | 89 | 90 |
| intentional_divergence | 8 | 8 |
| missing_exception | 1 | 1 |
| outside_scope | 206 | 206 |
| outside_target | 1 | 1 |
| pass | 64 | 66 |
| traceback_type_difference | 42 | 40 |
| traceback_unverified | 96 | 98 |
| unsupported | 133 | 133 |

New exact passes: builtin__string_funcs.py and int__overflow_division.py. No previous exact pass regressed.
Several broad fixtures now reach their next missing feature instead of failing at integer overflow; for example,
bytes__methods.py reaches capitalize. Fixture counts are not a percentage of language features.

## Quick regression comparison

Three shuffled fresh-process follow-up samples per binary/tier; medians include compilation and process startup.
These are diagnostic timings. All outputs match. The initial two-sample data are retained in regressions.json;
its slow initial baseline sieve sample does not repeat in the follow-up and is not treated as a speedup.

| Workload | Bytecode before/after (ms) | After/before | Native before/after (ms) | After/before |
|---|---:|---:|---:|---:|
| sieve | 172.29 / 172.70 | 1.002 | 50.65 / 49.97 | 0.987 |
| game_nbody | 276.20 / 280.10 | 1.014 | 37.83 / 37.20 | 0.983 |
| barnes_hut | 2921.92 / 2878.18 | 0.985 | 2909.53 / 2947.84 | 1.013 |
| perf_unpack_sequence | 174.13 / 181.05 | 1.040 | 227.70 / 220.26 | 0.967 |

Observed follow-up differences are within 4%; these short samples do not establish statistical equivalence.

## Reproduce

```sh
python3 tools/check_monty.py --monty /path/to/monty --tis /path/to/tis --tier bytecode --output /tmp/new-bytecode-audit
python3 tools/check_monty.py --monty /path/to/monty --tis /path/to/tis --tier native --output /tmp/new-native-audit
python3.15 differential/check_bigint_rounding.py /path/to/tis
```

For timing reproduction, put the matching release binaries in binaries/before and binaries/after, then run regressions.py and followup.py.
Archived workloads are supplied. JSON command paths reflect the original temporary staging directory.
