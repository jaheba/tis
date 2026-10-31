# Builtin method acquisition and exact iterator probes

Dictionary histograms spent substantial time publishing roots for builtin method acquisition and sequence advancement.
Exact builtin method reads now allocate one fresh bound object inside the closed field gateway, after checking the
one-object collection budget. They never call Python or collect. Attribute protocol mode, unknown receivers/names
and pressure retain the original callback. A separate counter records builtin-method hits without inflating field hits.

The LIR selector previously inferred string iteration from a string result even when the source was a list of strings.
Only the iterator source profile now selects StringNext. Cranelift's range-cursor miss also tries the existing closed
list/tuple advance and dedicated UTF-8 string advance before publishing roots. CnP's fallback supports the same cases.
These are runtime/type fixes; benchmark programs and outputs are unchanged.

## Retained custom comparison

Milliseconds, ten samples after twenty warmups in the same VM/function objects. Wrapper compilation and worker
transport remain included. Baseline is cf49fec. First table compares its frozen preceding run with the final candidate;
the paired confirmation below checks the affected workloads again in one experiment. Both native tiers enable
readonly and mutation regions. Release binaries use the same target directory and build configuration.

| Workload | CnP before → after | Cranelift before → after | CPython after | PyPy after |
|---|---:|---:|---:|---:|
| fill__range | 0.442 → 0.436 | 0.372 → 0.394 | 10.976 | 5.778 |
| fill__counter | 0.438 → 0.440 | 0.372 → 0.389 | 13.058 | 5.746 |
| fill__expression | 0.429 → 0.450 | 0.380 → 0.386 | 15.038 | 5.680 |
| fill__reversed | 0.428 → 0.438 | 0.401 → 0.396 | 13.065 | 5.757 |
| fill__data | 0.443 → 0.438 | 0.384 → 0.390 | 14.430 | 6.196 |
| numeric__inline | 0.747 → 0.729 | 0.245 → 0.251 | 0.979 | 0.117 |
| numeric__helper | 0.844 → 0.837 | 0.276 → 0.266 | 1.284 | 0.286 |
| numeric__tuples | 0.704 → 0.694 | 0.284 → 0.270 | 0.983 | 0.119 |
| numeric__data | 1.069 → 1.054 | 0.345 → 0.336 | 1.620 | 0.338 |
| histogram__get | 12.683 → 6.477 | 12.838 → 7.305 | 1.294 | 0.679 |
| histogram__membership | 11.376 → 7.215 | 10.336 → 7.967 | 1.399 | 0.870 |
| histogram__helper | 19.702 → 13.009 | 18.530 → 14.240 | 1.826 | 0.974 |
| histogram__unicode | 13.450 → 6.733 | 13.041 → 7.603 | 1.234 | 0.630 |
| objects__fields | 0.598 → 0.601 | 2.709 → 1.228 | 0.610 | 0.111 |
| objects__helper | 1.231 → 1.230 | 3.346 → 1.915 | 0.861 | 0.193 |
| objects__method | 3.861 → 3.902 | 5.953 → 4.632 | 0.905 | 0.107 |
| objects__layout | 0.613 → 0.591 | 2.756 → 1.236 | 0.603 | 0.111 |

The histogram gains do not establish CPython/PyPy parity. The get variant still takes about 5–6× CPython time and
10× PyPy time. Exact lookups, updates, boxed-value traffic and helper frames remain. General user-method dispatch
was not fused in this experiment; its Cranelift gain comes from the common iterator fallback.

## Paired confirmation

Twenty warmups and ten retained samples per workload/runtime, old then new, serially. Milliseconds.

| Workload | CnP before → after | Cranelift before → after |
|---|---:|---:|
| histogram__get | 12.870 → 6.447 | 12.672 → 7.363 |
| histogram__membership | 11.549 → 7.271 | 10.398 → 7.645 |
| histogram__helper | 19.122 → 13.090 | 18.671 → 14.371 |
| histogram__unicode | 13.349 → 6.641 | 13.046 → 7.582 |

## Fresh sessions and regression controls

Custom fresh one-shot and repeated modes each have three samples per workload/runtime. Fixture setup, worker startup
and reset are excluded; execution, profiling and backend compilation are included. Repeated mode retains the worker,
not application functions/VMs. All 255 custom case/runtime/mode comparisons have exact expected output.

| Mode / workload | CnP before → after | Cranelift before → after |
|---|---:|---:|
| one-shot / histogram__get | 13.038 → 7.016 | 16.444 → 11.077 |
| one-shot / histogram__membership | 11.945 → 7.779 | 14.364 → 11.637 |
| one-shot / histogram__helper | 19.448 → 13.788 | 23.072 → 19.256 |
| one-shot / histogram__unicode | 13.715 → 7.326 | 16.375 → 11.423 |
| repeated / histogram__get | 12.891 → 7.109 | 15.746 → 10.716 |
| repeated / histogram__membership | 11.507 → 7.606 | 13.682 → 11.284 |
| repeated / histogram__helper | 19.283 → 13.537 | 22.559 → 18.841 |
| repeated / histogram__unicode | 13.696 → 7.223 | 16.030 → 10.898 |

Paired repeated fresh-session controls, three samples, milliseconds:

| Workload | CnP before → after | Cranelift before → after |
|---|---:|---:|
| game_nbody__canonical | 43.627 → 43.718 | 31.866 → 31.432 |
| game_nbody__structure | 48.905 → 49.184 | 34.475 → 33.821 |
| perf_barnes_hut__canonical | 3272.292 → 3339.135 | 679.993 → 674.925 |
| perf_unpack_sequence__canonical | 155.174 → 157.734 | 154.071 → 154.718 |
| sieve__canonical | 3.758 → 3.822 | 8.556 → 8.700 |
| sieve__structure | 3.655 → 3.763 | 8.454 → 8.273 |

The CnP controls vary up to about 3% and native controls about 2%. Quick samples do not prove equivalence or
attribute these small differences. n-body and the preceding numeric-helper win remain near their controls.

## Diagnostics and failed experiment

For one get-histogram execution, Cranelift runtime crossings fall 173,902 → 116,166 and root synchronizations
58,169 → 334. CnP crossings fall 173,804 → 87,149 and root synchronizations 58,071 → 318.
Both record 28,866 closed builtin-method acquisitions. Bound objects remain distinct, so this does not eliminate
allocation itself. The current global attribute-protocol guard is conservative even for exact builtin receivers.

Removing the false string-result inference first improved CnP but regressed Cranelift: its range miss lacked the
closed sequence fallback. Adding that fallback exposed an existing helper contract problem. try_sequence_next also
accepted strings through get_builtin, mixing UTF-8 byte cursors with element indices and raising Index at exhaustion.
The full string_iteration test caught it before installation. The helper now explicitly accepts only lists/tuples;
strings use try_string_next. The string regression checks observable results and low root traffic, rather than requiring
a particular LIR helper name. Tests retain Unicode, empty strings, source-type changes, GC and sticky exhaustion.

Compilation and code sizes below are single CLI diagnostics, not repeated latency measurements. No RSS samples were
collected. Fresh-session tables include compilation cost rather than assuming warmup removed it.

| Backend | Compile ms before → after | Code bytes before → after |
|---|---:|---:|
| copy-patch | 0.125 → 0.113 | 10996 → 11980 |
| native | 8.394 → 9.288 | 6316 → 6668 |

Validation: fmt, all-target Clippy with warnings denied, 496 Rust tests, 255 custom comparisons and 120/120 exact
Flying Circus outputs in each native tier. New tests cover fresh/escaped builtin method identity under pressure,
descriptor fallback, missing attributes, and Unicode strings yielded by lists without per-item root synchronization.

## Reproduction and evidence

Raw results, binaries, source state, hashes, logs and intermediate experiments live outside Git:
`~/Library/Caches/tis-benchmarks/2026-10-31-dictionary-paths/`.

`verified/fresh-command.json` and `verified/controls-command.json` record exact timing commands; `validate.py` runs
Clippy, tests, release build, custom samples, controls and full correctness sequentially. The `programs` directory
holds the unchanged 17 workloads. The old binary is cf49fec; provenance and source-state files identify hashes/flags.
`diagnostics.json` records separate CLI counters. `pilots` preserves the method-only and incomplete iterator results.
Use the archived worker with `benchmarks/variations/run.py OUTPUT --worker WORKER`; use `controls.py` for paired
regression controls and `verify120.py` for exact full-suite outputs. Local absolute paths may need updating.
