# Bytecode builtin and profiling fast paths

Base: `3f01b92` (native sieve optimization). Candidate is uncommitted.
Release builds on macOS ARM64, Rust 1.99.0. Frozen executable hashes and changed-source hashes are in `provenance.json`.

The original sieve sample showed dispatch, heap access, repeated loop admission/profile checks and iterator handling dominating execution. The candidate adds guarded exact-sequence reads/stores, cached immutable range parameters, quiet iterator-header fusion, VM/window-specific admission memoization and frame-local backedge batching. Counters are merged before new admission and on return/error. Ordinary field access skips redundant polls only while descriptor/hook guards hold. Custom protocols, allocating fallbacks and feedback reopening retain the full runtime path.

## One-shot comparison

Milliseconds, median of three serial samples per runtime/workload, shuffled runtime order. Fresh processes include startup, parsing, compilation, execution, output and exit; no workload warmup. Executable/file caches were primed. Monty's CLI diagnostic stderr is included. Every output matches CPython exactly. These are quick samples, not stable performance guarantees; small differences and startup-dominated workloads are noisy. Barnes native enables the same readonly/mutation-region flags in both builds.

| Workload | Before bytecode | After bytecode | Change | Monty | CPython | After native |
|---|---:|---:|---:|---:|---:|---:|
| sieve | 369.71 | 171.82 | -53.5% | 170.40 | 76.53 | 50.41 |
| nbody | 281.24 | 263.43 | -6.3% | 300.21 | 100.95 | 35.36 |
| barnes-hut | 2778.57 | 2781.03 | +0.1% | 4163.46 | 643.47 | 648.15 |
| ticket-search | 42.30 | 41.74 | -1.3% | 30.01 | 33.34 | 88.70 |
| bisect-search | 24.36 | 22.99 | -5.6% | 39.12 | 41.81 | 30.70 |
| bisect-insort | 8.80 | 9.46 | +7.6% | 12.00 | 30.42 | 13.41 |
| perf_unpack_sequence | 180.78 | 164.86 | -8.8% | 54.40 | 41.46 | 215.02 |
| perf_json_loads | 5.28 | 5.09 | -3.6% | 6.67 | 27.36 | 4.83 |
| perf_json_dumps | 13.18 | 13.08 | -0.7% | 17.41 | 36.70 | 15.66 |
| text_json_parse_small | 4.04 | 4.69 | +16.1% | 6.60 | 26.21 | 4.51 |
| text_json_transform_small | 4.70 | 4.22 | -10.2% | 6.27 | 25.34 | 4.54 |
| text_json_lines_small | 4.34 | 4.58 | +5.6% | 6.63 | 25.98 | 5.10 |
| text_csv_aggregate_small | 10.15 | 9.92 | -2.3% | 10.24 | 26.05 | 14.34 |
| text_csv_cleanup_small | 13.52 | 13.87 | +2.6% | 13.29 | 26.02 | 18.07 |

## Retained-VM sieve

One untimed warmup and three serial timed calls. Definitions/setup excluded; each call includes fresh sieve allocation, marking, scanning and output capture. Native compilation from the warmup is excluded and kept separately in `warmed.json`.

- before-bytecode: 372.14 ms
- before-native: 43.78 ms
- after-bytecode: 165.58 ms
- after-native: 41.23 ms
- cpython: 49.94 ms
- pypy: 15.18 ms

## Follow-up

Same fresh-process boundaries and exact-output verification. Five samples for the four longer workloads; nine samples for startup-dominated bisect insertion, JSON parsing/lines and CSV cleanup. Their initial apparent regressions did not repeat.

| Workload | Before bytecode | After bytecode | Change |
|---|---:|---:|---:|
| sieve | 370.67 | 171.01 | -53.9% |
| barnes-hut | 2784.34 | 2786.24 | +0.1% |
| perf_unpack_sequence | 180.94 | 166.74 | -7.8% |
| ticket-search | 41.31 | 41.23 | -0.2% |
| bisect-insort | 9.14 | 9.09 | -0.6% |
| text_json_parse_small | 3.72 | 3.76 | +1.0% |
| text_json_lines_small | 3.87 | 3.86 | -0.4% |
| text_csv_cleanup_small | 13.49 | 13.17 | -2.4% |

## Validation and reproduction

`cargo fmt --all`, `cargo clippy --all-targets --offline --locked -- -D warnings`, and `cargo test --offline --locked` pass (397 tests). Regression coverage includes later hot loops, shared code across VMs, numeric/tier changes, detailed feedback reopening through recursive truth callbacks, iterator cursor behavior and GC roots. The 14-workload matrix also checks native execution and the sieve stack interpreter. No workload outputs are precomputed by tis.

Build with `cargo build --release --offline --locked --bin tis --example bench --example run --example circus`. Place the candidate release examples into `binaries/after-bench` and `binaries/after-run`, and the CLI into `binaries/tis`. Baseline binaries are frozen from the preceding sieve report. Run `TIS_SOURCE_ROOT=/Users/jaheba/p/tis python3 measure.py` serially, then `python3 followup.py` after updating its captured paths if moving this directory. Raw commands, outputs, compile costs, timing samples and failures are retained in the JSON files. Executable files are locally retained and gitignored. Intermediate experiment results are preserved in `first-pass/`, `unpack-pass/`, `field-pass/` and `store-pass/`.

This is a targeted bytecode improvement, not parity with CPython or PyPy. In particular, unpacking and ticket search still lag Monty. See the follow-up table for residual small regressions rather than extrapolating the sieve result.
