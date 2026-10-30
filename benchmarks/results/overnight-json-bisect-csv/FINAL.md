# Overnight results — 30 October 2026

The overall performance goals were **not achieved**. All 14 full-size selected programs run unchanged with exact CPython stdout. Results use the final validated candidate20; no commits or pushes.

Serial Flying Circus `--quick`, overridden to 12 timing samples per runtime/workload/mode, 1 separate RSS sample and 1 repeated warmup. One-shot means first request in a fresh worker; repeated means fresh VM/session in a reused worker. Both include frontend, tier compilation, execution and output delivery, exclude process startup/reset. Ratios are medians; borderline results remain noisy.

| Workload | BC/Monty one / repeated | Native-mode/CPython one / repeated | RSS BC / native MiB |
| --- | ---: | ---: | ---: |
| bisect_insort | 0.87 / 0.87 | 1.12 / 1.34 | 4.2 / 7.8 |
| bisect_search | 0.63 / 0.63 | 1.52 / 1.58 | 4.3 / 8.4 |
| perf_json_dumps | 0.64 / 0.62 | 0.78 / 0.80 | 9.1 / 11.9 |
| perf_json_loads | 1.00 / 1.00 | 0.58 / 0.73 | 5.6 / 5.7 |
| text_csv_aggregate_medium | 1.15 / 1.15 | 3.84 / 3.86 | 52.1 / 56.6 |
| text_csv_aggregate_small | 1.39 / 1.40 | 7.37 / 7.05 | 6.3 / 10.6 |
| text_csv_cleanup_medium | 1.14 / 1.14 | 4.12 / 4.05 | 80.5 / 77.0 |
| text_csv_cleanup_small | 1.27 / 1.25 | 6.14 / 6.06 | 6.5 / 11.0 |
| text_json_lines_medium | 1.00 / 1.03 | 1.64 / 1.62 | 14.5 / 17.6 |
| text_json_lines_small | 0.83 / 1.00 | 1.09 / 0.91 | 4.2 / 4.3 |
| text_json_parse_medium | 0.99 / 1.03 | 1.17 / 1.12 | 18.6 / 22.7 |
| text_json_parse_small | 0.87 / 1.03 | 1.41 / 1.06 | 4.1 / 4.1 |
| text_json_transform_medium | 0.89 / 0.96 | 1.78 / 1.65 | 19.5 / 23.7 |
| text_json_transform_small | 0.99 / 1.19 | 1.32 / 0.99 | 4.3 / 4.3 |

At the measured medians, 9/14 workloads meet bytecode ≤1.10×Monty in both modes; 2/14 meet native-mode <CPython in both modes. These are per-workload goals, not an averaged score. RSS is a single whole-process peak sample. Full reports include all four runtimes’ absolute elapsed time and RSS.

## Compilation and retained VM measurements

The following retained-VM results are a separate boundary: one compiled Program/VM, globals and caches retained, first execution + 3 warmups + 3 timed executions, exact output checked for all 7 executions. Timings capture output but exclude delivery/frontend. They must not be compared directly to fresh-session CPython. Compile costs are diagnostic runs rather than values subtracted from elapsed time.

| Workload | Frontend ms | Bytecode compile ms | First native compile ms | Retained BC / native execution ms | Native entries in diagnostic |
| --- | ---: | ---: | ---: | ---: | ---: |
| bisect_insort | 0.133 | 0.034 | 2.533 | 6.458 / 5.315 | 1 |
| bisect_search | 0.188 | 0.049 | 3.278 | 19.842 / 23.834 | 2 |
| perf_json_dumps | 0.252 | 0.063 | 1.289 | 6.308 / 6.972 | 1 |
| perf_json_loads | 0.321 | 0.103 | 0.000 | 0.441 / 0.446 | 0 |
| text_csv_aggregate_medium | 2.112 | 0.223 | 10.427 | 392.239 / 303.786 | 3 |
| text_csv_aggregate_small | 1.267 | 0.104 | 5.631 | 3.705 / 2.864 | 1 |
| text_csv_cleanup_medium | 2.576 | 0.271 | 12.145 | 778.403 / 637.658 | 32833 |
| text_csv_cleanup_small | 1.350 | 0.134 | 5.680 | 7.351 / 5.906 | 2 |
| text_json_lines_medium | 0.684 | 0.182 | 4.618 | 14.690 / 20.522 | 5388 |
| text_json_lines_small | 0.156 | 0.062 | 0.000 | 0.171 / 0.165 | 0 |
| text_json_parse_medium | 0.587 | 0.160 | 0.000 | 10.162 / 11.232 | 0 |
| text_json_parse_small | 0.137 | 0.039 | 0.000 | 0.124 / 0.121 | 0 |
| text_json_transform_medium | 0.679 | 0.184 | 4.016 | 9.877 / 11.792 | 1278 |
| text_json_transform_small | 0.238 | 0.065 | 0.000 | 0.118 / 0.117 | 0 |

Native mode includes runtime kernels and interpreter fallback. In particular JSON loads/parse can have zero native entries/compilation; their gains do not establish machine-code speed.

## Changes and validation

- Added minimal real bisect, lazy generator expressions, raise/basic exceptions and restricted try/except support needed by the unchanged workloads. Multi-clause/async generators, general yield, multi-statement try bodies, finally and except-as remain unsupported; this is not a complete Python stdlib.
- Direct jiter-to-VM JSON decoding, compatible dumps options, owned compact string dictionary keys and hashed sets remove intermediary allocations/quadratic lookups. Borrowed dictionary lookups retain keyed hashes.
- HIR remains central: exact-list append proofs shared across tiers, guarded straight-line generator expression kernels, lazy reusable function bytecode, cached internal try bodies, immediate inline string constants/equality, cheaper dominance/liveness analysis. Profiling windows, GC roots, fallback and deoptimization remain explicit.
- Native inline-string comparisons, IsDone and guarded string iteration reduce runtime/root synchronization. Generator runtime-kernel wins are labeled separately from generated native code.
- Rejected the final dead-constant-elimination experiment after measured JSON regressions; it is not installed. Original Monty and Flying Circus implementations were not edited.

[Full final report](final20/report.html) · [Quick comparison](candidate20/report.html) · [Progress, profiling and rejected experiments](PROGRESS.md) · [Frozen binaries and build provenance](candidate20-build-provenance.json) · [Complete final source hashes](candidate20-all-source-sha256.json)

Validation: cargo fmt, all-target Clippy with warnings denied, and 376 passing tests. All 14 outputs matched CPython; retained-VM 28 tier/workload rows passed. Fresh-process n-body, Barnes–Hut and unpacking matched pre-overnight output in both tiers. Short single-run regression timings below are directional, not significance claims.

| Regression | Tier | Before / final ms |
| --- | --- | ---: |
| game_nbody | bytecode | 273.0 / 243.0 |
| game_nbody | native | 44.3 / 38.1 |
| perf_barnes_hut | bytecode | 2501.6 / 2364.1 |
| perf_barnes_hut | native | 604.0 / 584.0 |
| perf_unpack_sequence | bytecode | 204.8 / 185.8 |
| perf_unpack_sequence | native | 246.4 / 227.8 |

Remaining bottlenecks: CSV still makes millions of generic runtime calls and GC-root synchronizations; native startup compilation dominates small inputs. Bisect still pays Python-loop/tier overhead around fast Rust searches. JSON transforms/lines retain callback and object traffic. Future work should target these measured costs before claiming parity with CPython/PyPy.

## Final profiling findings

Fresh diagnostic CSV aggregate medium executes 4,471,128 runtime helpers and 695,394 root synchronizations; cleanup medium executes 9,521,799 helpers and 1,604,592 root synchronizations. Quiet native blocks already avoid callbacks, so removing a presumed callback on every block would not address the actual implementation.

One-second retained-VM CPU samples confirm the remaining costs. Bytecode aggregate has 312 top-of-stack samples in execute and 74 in record_block_and_phase, followed by heap access/GC/iteration. Native cleanup has 79 in Context::step, 57 in collection, 33 in Heap::get, 32 in GC child tracing, 28 in UTF8 validation, 27 in guarded_helper_callback and 24 in Slot::value. These are leaf sample counts, not percentages or exhaustive native attribution; unsymbolized generated code is also present. Profiles and commands are in candidate20-profile/ and overnight_profile20.py. Initial sandboxed sampling failed; the authorized unsandboxed retry succeeded for both processes.

The next substantive work should reduce native helper/root traffic and repeated Slot-to-Value string validation, and batch frozen bytecode block bookkeeping while retaining observation windows and exact guard/deopt state. Small CSV inputs also spend about 5.6ms compiling native code: tier policy/cheaper initial native compilation matters separately from steady execution. No speculative last-minute code changes were installed from these observations.

Final source and installed CLI hashes were verified; the installed CLI exactly matches binaries/tis-candidate20. 376 tests, all-target Clippy, formatting and diff whitespace checks pass. All 14 unchanged workloads and all 28 retained tier/workload checks pass. Targets remain incomplete: 9/14 bytecode and 2/14 native-mode workloads satisfy both timing modes at measured medians. The 12-sample follow-up and 1 RSS sample are preserved without selecting only favorable measurements. No commits or pushes.
