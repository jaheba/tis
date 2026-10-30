# Captured locals in eager comprehensions

The exact photographed sieve, unchanged at 30,000,000, is saved as sieve_photo.py and tests/fixtures/sieve_comprehension.py. Every runtime prints 1857859. The before executable includes the preceding uncommitted bytecode fast paths. Binary and source hashes, base revision and compiler versions are in provenance.json.

## Exact 30-million sieve

Three serial fresh-process samples per runtime, randomized order; table shows medians. No workload warmup. Executables were primed with --version to exclude first-launch OS verification. Startup, parsing, HIR lowering, native compilation, execution, output and exit are included. Both tis builds use the same --stats flag. RSS comes from wait4 on macOS ARM64 and is reported in MiB. All runtime programs are identical; tis executes its own release binary.

| Runtime | Elapsed (ms) | Peak RSS (MiB) |
|---|---:|---:|
| before-bytecode | 6526.07 | 493.0 |
| before-native | 2661.60 | 496.9 |
| after-bytecode | 4660.02 | 492.9 |
| after-native | 1266.60 | 497.7 |
| cpython | 2962.33 | 316.7 |
| pypy | 848.49 | 620.4 |

## Runtime evidence

The comprehension previously captured is_prime, keeping marking and scanning accesses in environment lookups. Eligible eager comprehensions now promote private target variables to SSA before their blocks are spliced into the enclosing HIR. The outer list remains an ordinary local. Temporary SSA values stay within the comprehension graph instead of increasing caller frame slots and phi arguments on unrelated loops.

| Counter | Before native | After native |
|---|---:|---:|
| native_runtime_calls | 102,513,181 | 1,862,925 |
| name_cache_hits | 100,653,410 | 721 |
| native_root_syncs | 3,618 | 3,617 |
| deoptimizations | 0 | 0 |
| compilation_errors | 0 | 0 |
| native_compile_ns | 5,013,750 | 4,214,542 |

Before/after HIR dumps are saved alongside the timings. The final HIR has ordinary local slots i, is_prime, j and n; no comprehension target slots or environment loads of is_prime remain.

Inlining requires a potential enclosing-local capture benefit. Noncapturing comprehensions keep smaller separately compiled functions to limit cold native compilation cost. Escaping callables, suspended generators, frame-sensitive operations, possibly unbound targets, module/class scopes and exception-control fragments retain the existing separate-frame implementation. Calls that can mutate an outer captured cell still read its live environment; no captured value is snapshotted.

## Existing-workload regression check

Three paired samples in each tier, identical source and exact expected stdout from the preceding CPython-verified suite. Every output passes. Native Barnes-Hut uses matching read-only/mutation-region flags in both builds. These are quick timings; small differences, especially on startup-dominated workloads, need confirmation.

| Workload | Bytecode before (ms) | After (ms) | Native before (ms) | After (ms) |
|---|---:|---:|---:|---:|
| sieve | 172.72 | 170.17 | 50.38 | 49.57 |
| nbody | 264.55 | 264.79 | 35.85 | 35.72 |
| barnes-hut | 2774.10 | 2786.41 | 650.27 | 655.16 |
| ticket-search | 41.59 | 42.06 | 87.26 | 88.78 |
| bisect-search | 22.81 | 22.75 | 30.19 | 30.41 |
| bisect-insort | 9.46 | 8.84 | 12.95 | 13.14 |
| perf_unpack_sequence | 167.13 | 166.54 | 214.07 | 215.77 |
| perf_json_loads | 5.07 | 5.41 | 4.96 | 4.92 |
| perf_json_dumps | 12.76 | 12.72 | 15.57 | 15.81 |
| text_json_parse_small | 4.18 | 3.83 | 4.01 | 3.95 |
| text_json_transform_small | 4.24 | 4.13 | 4.00 | 4.12 |
| text_json_lines_small | 4.27 | 4.55 | 4.18 | 3.89 |
| text_csv_aggregate_small | 9.87 | 9.86 | 14.26 | 14.19 |
| text_csv_cleanup_small | 13.37 | 13.44 | 17.07 | 16.96 |

## Follow-up

Five additional native samples of the exact photo sieve, seven samples for ticket search in both tiers, and nine samples for two small JSON cases. Fresh processes, matched commands, exact stdout checks.

| Workload | Runtime | Median (ms) |
|---|---|---:|
| photo | before-native | 2666.68 |
| photo | after-native | 1271.05 |
| ticket-search | before-bytecode | 42.55 |
| ticket-search | before-native | 87.17 |
| ticket-search | after-bytecode | 42.25 |
| ticket-search | after-native | 88.38 |
| text_json_transform_small | before-bytecode | 4.13 |
| text_json_transform_small | before-native | 3.87 |
| text_json_transform_small | after-bytecode | 3.90 |
| text_json_transform_small | after-native | 4.05 |
| text_json_parse_small | before-bytecode | 3.71 |
| text_json_parse_small | before-native | 3.72 |
| text_json_parse_small | after-bytecode | 3.83 |
| text_json_parse_small | after-native | 3.78 |

The follow-up native ticket-search result is about 1.4% slower (87.17 to 88.38 ms). N-body, Barnes-Hut and unpacking remain within about 1% in the quick main comparison. Startup-dominated JSON cases retain small mixed differences; no general speedup is claimed for them.

## Validation and reproduction

Formatting, all-target Clippy with warnings denied, and the full Cargo suite pass: 402 tests. The five new regression tests exercise all four execution tiers with register and stack bytecode, local/target shadowing, first-iterable order, nested list/set/dict comprehensions, live nonlocal rebinding, GC pressure, errors, and escaping lambda cells. Scope assertions also pass independently in CPython (scope_reference.py).

Build with cargo build --release --offline --locked --bin tis --example ir --example bench --example run. Run the CLI with --tier native tests/fixtures/sieve_comprehension.py, or --tier bytecode for the interpreter. The CLI defaults to bytecode. Before/after binaries are frozen locally under binaries/ and gitignored. measure.py and followup.py contain the exact serial commands and sampling procedure; copied workloads and raw outputs are retained. The measurement script's previous-suite path resolves expected outputs and workload snapshots in the repository. Intermediate experiments are under frame-slot-pass/ and all-comprehensions-pass/; their timings are not final results.

PyPy remains faster on this sieve. No parity claim is made from these quick samples, and native compilation costs are included rather than reported as warmed execution.
