# Borrowed string runtime and adaptive Unicode offsets — 2026-10-31

Adapted the string-access approach from [Monty PR #1195](https://github.com/pydantic/monty/pull/1195), inspected at head `09cb7bf75d03109c63974f4af992b1d1c212099c`. The upstream license remains included at `src/heap/LICENSE-MONTY`. This change keeps the existing `Object::String(String)` representation and eight-byte generation slots.

## Behavior and implementation

- Added bound string `find`, `rfind`, `index`, `rindex` and `count`, with optional start/end positions. Searches return character positions, not byte offsets. Empty needles, huge bounds, `None`, negative bounds and user `__index__` conversions preserve Python behavior.
- Extended `startswith` and `endswith` to bounded and tuple affix forms, including lazy tuple type checking.
- Removed full receiver clones from ordinary string method dispatch and encoding. Non-reentrant operations borrow text through `&Heap`; custom join iteration and search-bound conversion finish before taking that borrow. Reentrant bounds conversion roots the receiver and arguments across Python and collection.
- Unicode indexing, search and contiguous slices share derived character/byte offsets. Endpoints avoid cursor allocation. A cursor tracks nearby reads; distant traversals build stride-32 checkpoints only after scanning 32 times the UTF-8 byte length. Strings no longer than 64 bytes retain bounded scans. Compact offsets use u32, with usize offsets for larger strings.
- Per-string cache state is lazy in a side table keyed by checked heap identity and generation. Host mutable access invalidates it before exposing the object; GC prunes it. Length-only access uses a fixed per-heap recent-length cache and existing indexed-string state, without allocating an entry for each cold string.
- Upper/lower conversion processes useful ASCII runs in bulk. Dense Unicode retains the standard conversion loop; lowercase involving capital sigma retains full-string contextual conversion. This does not add casefold support.

## Full-suite coverage and timing

The unchanged 120 public workloads ran serially with `--quick`: three timing samples, one repeated warmup, five startup samples and one separate RSS sample. Both request modes include compilation, execution and output delivery, excluding readiness, fixture preparation and reset. Repeated requests create fresh VM sessions; these are not retained-JIT timings.

| Runtime | One-shot pass / unsupported / failed | Repeated pass / unsupported / failed |
| --- | ---: | ---: |
| before | 88 / 17 / 15 | 88 / 17 / 15 |
| after | 96 / 17 / 7 | 96 / 17 / 7 |
| monty | 110 / 6 / 4 | 110 / 6 / 4 |
| cpython | 120 / 0 / 0 | 120 / 0 / 0 |

All eight agent Unicode workloads now pass, raising tis coverage from 88 to 96. The remaining seven runtime failures are six missing-frozenset workloads and the existing class-base case. Unsupported cases and reference-runtime failures remain visible.

| Comparison | Mode | Common passes | Geometric mean elapsed ratio |
| --- | --- | ---: | ---: |
| after / before | one-shot | 88 | 0.939 |
| after / before | repeated | 88 | 0.929 |
| before / monty | one-shot | 87 | 1.241 |
| before / monty | repeated | 87 | 1.370 |
| after / monty | one-shot | 95 | 1.134 |
| after / monty | repeated | 95 | 1.232 |
| after / cpython | one-shot | 96 | 1.727 |
| after / cpython | repeated | 96 | 1.712 |

Ratios below 1 favor the first runtime. Before/Monty and after/Monty use different intersections because eight workloads became supported. The paired after/before comparison across 88 workloads measures the speed change directly. Equal weighting includes many tiny microbenchmarks. Three samples support rough estimates, not statistical rankings. Monty parity remains unmet.

## Seven-sample confirmation

A focused confirmation ran 20 unchanged default workloads plus seven standalone controls, serially with seven timing samples, one warmup and one RSS sample. Default sizes and expected outputs were unchanged; custom expected outputs were captured with CPython. Each table entry is repeated fresh-session milliseconds, including compilation.

| Workload | Before BC | After BC | After CnP | After Cranelift | Monty | CPython |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| text_unicode_access_ascii | 3.934 | 4.027 | 6.134 | 8.146 | 2.644 | 2.479 |
| text_unicode_access_latin1 | 25.622 | 5.677 | 7.818 | 9.772 | 24.958 | 2.556 |
| text_unicode_access_cjk | 35.785 | 5.718 | 7.934 | 9.895 | 35.064 | 3.011 |
| text_unicode_access_emoji | 46.018 | 5.813 | 7.969 | 9.996 | 44.774 | 3.095 |
| text_unicode_access_mixed | 15.127 | 5.536 | 7.740 | 9.740 | 13.733 | 2.479 |
| short_unicode_access | 5.064 | 4.989 | 7.500 | 8.772 | 3.922 | 3.169 |
| prefix_mixed | 2.005 | 0.809 | 1.237 | 2.625 | 33.936 | 0.392 |
| slice_unicode | 3.388 | 0.914 | 0.908 | 0.917 | 3.444 | 0.342 |
| length_unicode | 18.119 | 1.495 | 1.955 | 3.118 | 17.412 | 1.033 |
| case_mixed | 16.107 | 16.408 | 16.533 | 16.514 | 11.677 | 12.549 |
| case_ascii_runs | 20.270 | 4.835 | 4.809 | 4.731 | 24.055 | 20.288 |
| agent_unicode_german_large | failed | 5.241 | 5.593 | 5.399 | 9.035 | 6.213 |
| agent_unicode_german_emoji_large | failed | 5.242 | 5.426 | 5.477 | 8.842 | 7.085 |
| agent_unicode_chinese_large | failed | 9.253 | 9.432 | 9.224 | 7.915 | 4.303 |
| game_nbody | 218.084 | 216.996 | 43.285 | 30.807 | 290.563 | 78.017 |
| perf_unpack_sequence | 45.780 | 45.076 | 152.121 | 152.771 | 47.579 | 17.466 |

All 27 confirmation workloads pass in each final tis tier and CPython; the before worker fails nine and Monty fails the reentrancy/semantics control. CnP and Cranelift use the shared runtime services; this experiment adds no direct string-loop stencils or native UTF-8 lowering. The native tiers still pay runtime crossings and compilation costs.

## Rejected policies and limits

The first cache version allocated state for every Unicode length and probed the cache table on every heap mutation. In its three-sample round, Latin-1 retained-string time grew from 14.0 to 24.4 ms. The final version allocates index state on demand and invalidates only actual Unicode string mutations. Measurements from different rounds have different allocator histories; their absolute times should not be compared as one paired experiment.

The initial offset cache also slowed a 36-byte Unicode indexing control from 5.12 to 6.00 ms. The bounded-scan path returned it near baseline in confirmation: 5.06 versus 4.99 ms. Short strings and one-off length queries should not pay for large-string indexing machinery.

Mixed text containing only short ASCII fragments did not benefit from run conversion: the final control took about 16.4 ms versus 16.1 ms before. The bounded selector deliberately leaves that case on the standard decoder. Long ASCII-run conversion improved about 4.2×. Chinese agent workloads still trail Monty and CPython.

The full three-sample suite showed CJK/emoji memory regressions that did not recur in the seven-sample confirmation. Mixed-Unicode memory was about 4% slower there; that size of difference remains uncertain. RSS has only one sample per workload. No claim of eliminated memory variability or overall Monty parity is supported.

| Retained-string control | Before ms | After ms | Before peak MiB | After peak MiB |
| --- | ---: | ---: | ---: | ---: |
| memory_unicode_ascii | 6.813 | 6.689 | 44.7 | 44.6 |
| memory_unicode_latin1 | 11.149 | 10.937 | 83.8 | 83.8 |
| memory_unicode_cjk | 12.971 | 12.504 | 115.3 | 115.3 |
| memory_unicode_emoji | 13.414 | 13.153 | 84.1 | 84.2 |
| memory_unicode_mixed | 8.281 | 8.590 | 44.6 | 44.6 |

## Validation and reproduction

`cargo fmt --all`, all-target Clippy with warnings denied, and all 475 tests passed. New tests cover random/inverse Unicode offsets, adaptive checkpoint admission, short/sequential/endpoint access, mutable-host invalidation including a forgotten guard, GC slot reuse, case expansions and contextual sigma. A Python semantics fixture runs under all four tiers and both bytecode encodings; Flying Circus also validates it against CPython. Bounds callbacks reenter the same string and allocate enough objects to trigger collection.

No Monty implementation or original Flying Circus checkout was modified. The frozen before worker is the preceding validated tis parity candidate. Default release features and tis `codegen-units=1` are consistent. The Monty reference is cached 1.4.0, predating PR #1195; these are not measurements against the PR build. CPython is 3.15.0. The installed tis CLI matches the final source. No commits or pushes were made.

All raw JSON, logs, frozen binaries, workload snapshots, commands, source hashes and source archives are outside Git at `/Users/jaheba/Library/Caches/tis-benchmarks/2026-10-31-string-runtime/`. `reproduce.py` reruns the full suite with the archived workers and copied adapter. The reports record timing boundaries, runtime flags and binary hashes.

[Full report](/Users/jaheba/Library/Caches/tis-benchmarks/2026-10-31-string-runtime/full/report.html) · [Confirmation report](/Users/jaheba/Library/Caches/tis-benchmarks/2026-10-31-string-runtime/confirm/report.html)

## All full-suite timing results

Each cell is one-shot / repeated milliseconds.

| Workload | Before BC | After BC | Monty | CPython |
| --- | ---: | ---: | ---: | ---: |
| expense_report | 0.427 / 0.257 | 0.384 / 0.245 | 0.385 / 0.161 | 1.090 / 0.272 |
| order_cleanup | 0.560 / 0.404 | 0.544 / 0.374 | 0.406 / 0.204 | 1.247 / 0.272 |
| api_report | 0.502 / 0.326 | 0.502 / 0.277 | 0.484 / 0.198 | 1.112 / 0.344 |
| capacity_planning | 23.588 / 22.775 | 23.915 / 22.971 | 32.173 / 31.801 | 9.696 / 8.778 |
| portfolio_risk | 67.635 / 66.694 | 68.034 / 67.027 | 63.867 / 64.388 | 52.020 / 51.034 |
| ticket_search | 37.758 / 40.137 | 40.615 / 38.135 | 23.786 / 22.399 | 9.635 / 6.915 |
| game_fannkuch | 1842.412 / 1836.630 | 1873.425 / 1895.464 | 1184.213 / 1196.632 | 268.753 / 270.181 |
| game_nbody | 220.950 / 218.918 | 217.668 / 215.642 | 289.969 / 288.718 | 79.186 / 77.521 |
| game_pidigits | unsupported / unsupported | unsupported / unsupported | unsupported / unsupported | 282.738 / 285.168 |
| game_spectral_norm | 279.367 / 283.254 | 280.181 / 281.432 | 258.167 / 258.356 | 66.407 / 65.907 |
| game_regex_dna | unsupported / unsupported | unsupported / unsupported | failed / failed | 249.549 / 246.860 |
| game_meteor_contest | failed / failed | failed / failed | 345.068 / 348.322 | 83.709 / 81.930 |
| perf_barnes_hut | 2989.677 / 3024.926 | 3017.836 / 3001.114 | 4167.509 / 4169.050 | 624.792 / 623.023 |
| perf_float | 250.783 / 236.986 | 240.069 / 236.027 | 257.424 / 253.077 | 50.786 / 48.102 |
| perf_unpack_sequence | 45.990 / 46.363 | 46.380 / 45.630 | 48.127 / 47.416 | 18.339 / 17.614 |
| perf_json_dumps | 8.733 / 8.527 | 8.825 / 8.311 | 10.918 / 10.563 | 10.749 / 9.993 |
| perf_json_loads | 1.212 / 0.971 | 1.281 / 1.033 | 1.154 / 0.859 | 1.928 / 1.053 |
| text_line_logs_small | 0.496 / 0.411 | 0.488 / 0.407 | 0.390 / 0.287 | 0.350 / 0.302 |
| text_line_logs_medium | 21.067 / 22.240 | 20.570 / 24.000 | 15.193 / 14.889 | 7.893 / 7.597 |
| text_regex_logs_small | unsupported / unsupported | unsupported / unsupported | 0.866 / 0.622 | 0.487 / 0.385 |
| text_regex_logs_medium | unsupported / unsupported | unsupported / unsupported | 26.107 / 26.672 | 15.742 / 15.301 |
| text_csv_aggregate_small | 5.634 / 5.264 | 5.769 / 5.388 | 3.725 / 3.727 | 1.432 / 1.351 |
| text_csv_aggregate_medium | 417.761 / 414.533 | 407.285 / 410.774 | 335.916 / 334.614 | 81.055 / 80.758 |
| text_csv_cleanup_small | 9.304 / 9.075 | 9.472 / 8.931 | 6.995 / 6.917 | 2.124 / 2.105 |
| text_csv_cleanup_medium | 830.393 / 787.873 | 792.485 / 794.161 | 676.140 / 669.046 | 154.429 / 152.491 |
| text_json_parse_small | 0.337 / 0.265 | 0.332 / 0.271 | 0.388 / 0.277 | 0.255 / 0.223 |
| text_json_parse_medium | 11.364 / 11.698 | 11.253 / 11.239 | 10.341 / 9.357 | 8.903 / 9.136 |
| text_json_transform_small | 0.493 / 0.393 | 0.485 / 0.350 | 0.534 / 0.364 | 0.369 / 0.357 |
| text_json_transform_medium | 15.385 / 15.136 | 14.967 / 14.391 | 12.270 / 10.919 | 9.774 / 8.932 |
| text_json_lines_small | 0.524 / 0.408 | 0.515 / 0.422 | 0.539 / 0.375 | 0.426 / 0.412 |
| text_json_lines_medium | 18.307 / 18.883 | 17.986 / 17.993 | 15.054 / 14.629 | 14.021 / 14.120 |
| memory_string_list_short | 324.387 / 323.054 | 332.589 / 334.216 | 171.946 / 166.637 | 91.496 / 86.171 |
| memory_string_list_long | 374.299 / 366.922 | 374.139 / 365.513 | 244.509 / 238.796 | 94.030 / 90.100 |
| memory_string_index_short | 105.723 / 102.824 | 108.939 / 105.527 | 85.514 / 80.449 | 63.181 / 68.059 |
| memory_string_index_long | 137.342 / 136.267 | 144.765 / 144.968 | 132.813 / 124.084 | 78.635 / 71.208 |
| memory_customer_records | 551.655 / 574.476 | 574.756 / 562.611 | 351.887 / 337.801 | 155.548 / 153.669 |
| stdout_lines | 4.117 / 3.750 | 4.121 / 3.998 | 2.168 / 1.918 | 4.103 / 3.170 |
| perf_richards | 286.477 / 287.364 | 286.969 / 297.497 | 190.232 / 192.910 | 30.492 / 29.537 |
| rustpython_base64 | unsupported / unsupported | unsupported / unsupported | 1.686 / 1.587 | 5.921 / 0.857 |
| rustpython_decimal_factorial | unsupported / unsupported | unsupported / unsupported | 1.705 / 1.538 | 6.087 / 1.074 |
| rustpython_deepcopy | unsupported / unsupported | unsupported / unsupported | unsupported / unsupported | 7.595 / 2.740 |
| rustpython_deltablue | failed / failed | failed / failed | unsupported / unsupported | 5.856 / 5.073 |
| rustpython_generators | unsupported / unsupported | unsupported / unsupported | unsupported / unsupported | 76.382 / 76.624 |
| rustpython_mandelbrot | 29.262 / 29.285 | 29.047 / 29.124 | 19.199 / 19.242 | 17.603 / 15.911 |
| rustpython_nqueens | unsupported / unsupported | unsupported / unsupported | unsupported / unsupported | 58.066 / 56.121 |
| rustpython_pickle | unsupported / unsupported | unsupported / unsupported | failed / failed | 12.360 / 9.975 |
| rustpython_pystone | 496.460 / 506.197 | 506.033 / 518.112 | 317.140 / 315.544 | 62.483 / 62.243 |
| rustpython_regex_effbot | unsupported / unsupported | unsupported / unsupported | failed / failed | 24.576 / 23.372 |
| rustpython_unpickle | unsupported / unsupported | unsupported / unsupported | failed / failed | 8.696 / 2.842 |
| rustpython_micro_addition_100 | 0.122 / 0.085 | 0.124 / 0.086 | 0.133 / 0.088 | 0.102 / 0.126 |
| rustpython_micro_addition_300 | 0.142 / 0.126 | 0.145 / 0.107 | 0.144 / 0.097 | 0.107 / 0.102 |
| rustpython_micro_addition_500 | 0.168 / 0.135 | 0.172 / 0.179 | 0.158 / 0.101 | 0.148 / 0.131 |
| rustpython_micro_addition_700 | 0.204 / 0.162 | 0.198 / 0.180 | 0.148 / 0.101 | 0.153 / 0.153 |
| rustpython_micro_addition_900 | 0.239 / 0.194 | 0.222 / 0.207 | 0.163 / 0.126 | 0.168 / 0.161 |
| rustpython_micro_cmp | 2.737 / 2.683 | 2.847 / 2.761 | 1.755 / 1.794 | 1.496 / 1.457 |
| rustpython_micro_complex_class | 0.175 / 0.109 | 0.175 / 0.114 | unsupported / unsupported | 0.187 / 0.166 |
| rustpython_micro_comprehension_dict_100 | 0.201 / 0.132 | 0.192 / 0.145 | 0.176 / 0.098 | 0.119 / 0.145 |
| rustpython_micro_comprehension_dict_300 | 0.212 / 0.175 | 0.218 / 0.165 | 0.190 / 0.120 | 0.120 / 0.140 |
| rustpython_micro_comprehension_dict_500 | 0.282 / 0.201 | 0.271 / 0.220 | 0.203 / 0.138 | 0.152 / 0.135 |
| rustpython_micro_comprehension_dict_700 | 0.314 / 0.265 | 0.299 / 0.240 | 0.234 / 0.145 | 0.160 / 0.160 |
| rustpython_micro_comprehension_dict_900 | 0.348 / 0.307 | 0.352 / 0.274 | 0.247 / 0.159 | 0.190 / 0.158 |
| rustpython_micro_comprehension_list_100 | 0.180 / 0.100 | 0.174 / 0.153 | 0.130 / 0.059 | 0.133 / 0.105 |
| rustpython_micro_comprehension_list_300 | 0.214 / 0.149 | 0.212 / 0.162 | 0.161 / 0.110 | 0.130 / 0.118 |
| rustpython_micro_comprehension_list_500 | 0.224 / 0.183 | 0.246 / 0.167 | 0.172 / 0.124 | 0.149 / 0.128 |
| rustpython_micro_comprehension_list_700 | 0.242 / 0.197 | 0.246 / 0.207 | 0.176 / 0.101 | 0.124 / 0.120 |
| rustpython_micro_comprehension_list_900 | 0.272 / 0.223 | 0.276 / 0.213 | 0.182 / 0.121 | 0.139 / 0.137 |
| rustpython_micro_comprehension_set_100 | 0.182 / 0.130 | 0.181 / 0.119 | 0.146 / 0.099 | 0.122 / 0.128 |
| rustpython_micro_comprehension_set_300 | 0.230 / 0.174 | 0.239 / 0.196 | 0.190 / 0.115 | 0.139 / 0.106 |
| rustpython_micro_comprehension_set_500 | 0.275 / 0.239 | 0.279 / 0.216 | 0.205 / 0.140 | 0.126 / 0.129 |
| rustpython_micro_comprehension_set_700 | 0.313 / 0.263 | 0.329 / 0.268 | 0.199 / 0.143 | 0.119 / 0.111 |
| rustpython_micro_comprehension_set_900 | 0.382 / 0.322 | 0.404 / 0.311 | 0.212 / 0.151 | 0.126 / 0.136 |
| rustpython_micro_frozenset_100 | failed / failed | failed / failed | 0.100 / 0.078 | 0.102 / 0.112 |
| rustpython_micro_frozenset_300 | failed / failed | failed / failed | 0.096 / 0.076 | 0.100 / 0.079 |
| rustpython_micro_frozenset_500 | failed / failed | failed / failed | 0.104 / 0.098 | 0.112 / 0.074 |
| rustpython_micro_frozenset_700 | failed / failed | failed / failed | 0.093 / 0.107 | 0.094 / 0.105 |
| rustpython_micro_frozenset_900 | failed / failed | failed / failed | 0.104 / 0.062 | 0.084 / 0.078 |
| rustpython_micro_list_count_builtins_100 | 0.073 / 0.061 | 0.084 / 0.061 | 0.102 / 0.072 | 0.082 / 0.084 |
| rustpython_micro_list_count_builtins_300 | 0.096 / 0.077 | 0.090 / 0.064 | 0.096 / 0.071 | 0.107 / 0.086 |
| rustpython_micro_list_count_builtins_500 | 0.090 / 0.060 | 0.093 / 0.071 | 0.102 / 0.080 | 0.100 / 0.100 |
| rustpython_micro_list_count_builtins_700 | 0.096 / 0.066 | 0.100 / 0.074 | 0.100 / 0.057 | 0.103 / 0.084 |
| rustpython_micro_list_count_builtins_900 | 0.095 / 0.091 | 0.094 / 0.072 | 0.116 / 0.075 | 0.098 / 0.095 |
| rustpython_micro_list_count_custom_eq_100 | 0.117 / 0.135 | 0.111 / 0.091 | 0.117 / 0.078 | 0.097 / 0.076 |
| rustpython_micro_list_count_custom_eq_300 | 0.163 / 0.113 | 0.148 / 0.120 | 0.153 / 0.130 | 0.118 / 0.082 |
| rustpython_micro_list_count_custom_eq_500 | 0.194 / 0.158 | 0.198 / 0.160 | 0.195 / 0.150 | 0.098 / 0.084 |
| rustpython_micro_list_count_custom_eq_700 | 0.236 / 0.202 | 0.234 / 0.207 | 0.224 / 0.199 | 0.110 / 0.087 |
| rustpython_micro_list_count_custom_eq_900 | 0.265 / 0.235 | 0.278 / 0.256 | 0.257 / 0.224 | 0.135 / 0.095 |
| rustpython_micro_loop_append_100 | 0.142 / 0.096 | 0.136 / 0.106 | 0.148 / 0.090 | 0.116 / 0.126 |
| rustpython_micro_loop_append_300 | 0.180 / 0.145 | 0.183 / 0.139 | 0.163 / 0.102 | 0.128 / 0.155 |
| rustpython_micro_loop_append_500 | 0.203 / 0.192 | 0.222 / 0.186 | 0.174 / 0.102 | 0.160 / 0.133 |
| rustpython_micro_loop_append_700 | 0.244 / 0.215 | 0.267 / 0.224 | 0.205 / 0.138 | 0.174 / 0.143 |
| rustpython_micro_loop_append_900 | 0.273 / 0.248 | 0.293 / 0.247 | 0.212 / 0.156 | 0.184 / 0.161 |
| rustpython_micro_loop_string_100 | 0.088 / 0.084 | 0.098 / 0.086 | 0.065 / 0.043 | 0.077 / 0.070 |
| rustpython_micro_loop_string_300 | 0.111 / 0.096 | 0.104 / 0.094 | 0.064 / 0.054 | 0.077 / 0.074 |
| rustpython_micro_loop_string_500 | 0.129 / 0.102 | 0.128 / 0.098 | 0.069 / 0.045 | 0.080 / 0.071 |
| rustpython_micro_loop_string_700 | 0.137 / 0.127 | 0.141 / 0.114 | 0.072 / 0.062 | 0.085 / 0.079 |
| rustpython_micro_loop_string_900 | 0.157 / 0.142 | 0.156 / 0.146 | 0.066 / 0.048 | 0.086 / 0.102 |
| rustpython_micro_sorted_100 | unsupported / unsupported | unsupported / unsupported | 0.218 / 0.202 | 0.236 / 0.182 |
| rustpython_micro_sorted_300 | unsupported / unsupported | unsupported / unsupported | 0.421 / 0.389 | 0.332 / 0.294 |
| rustpython_micro_sorted_500 | unsupported / unsupported | unsupported / unsupported | 0.634 / 0.571 | 0.458 / 0.474 |
| rustpython_micro_sorted_700 | unsupported / unsupported | unsupported / unsupported | 0.832 / 0.778 | 0.563 / 0.592 |
| rustpython_micro_sorted_900 | unsupported / unsupported | unsupported / unsupported | 1.027 / 1.021 | 0.769 / 0.689 |
| rustpython_micro_strings | 3.220 / 3.239 | 3.234 / 3.313 | 0.964 / 0.861 | 0.822 / 0.783 |
| text_unicode_access_ascii | 4.027 / 4.096 | 4.107 / 3.931 | 2.925 / 2.591 | 3.325 / 2.290 |
| memory_unicode_ascii | 8.906 / 6.595 | 9.078 / 7.120 | 5.279 / 5.168 | 5.142 / 2.028 |
| text_unicode_access_latin1 | 25.754 / 25.524 | 5.771 / 5.720 | 25.021 / 24.792 | 3.268 / 2.302 |
| memory_unicode_latin1 | 16.414 / 16.566 | 16.516 / 16.447 | 9.444 / 10.711 | 5.286 / 3.673 |
| text_unicode_access_cjk | 36.072 / 36.057 | 5.819 / 5.713 | 35.257 / 35.074 | 3.771 / 2.734 |
| memory_unicode_cjk | 20.953 / 13.262 | 24.961 / 26.552 | 12.745 / 13.403 | 7.858 / 7.590 |
| text_unicode_access_emoji | 45.608 / 46.047 | 6.000 / 5.686 | 45.291 / 44.934 | 3.594 / 2.889 |
| memory_unicode_emoji | 19.760 / 30.148 | 22.533 / 12.959 | 14.599 / 16.967 | 12.635 / 12.933 |
| text_unicode_access_mixed | 15.529 / 15.413 | 5.732 / 5.585 | 13.827 / 13.732 | 3.249 / 2.173 |
| memory_unicode_mixed | 11.309 / 9.725 | 10.809 / 10.245 | 6.531 / 7.058 | 13.160 / 6.614 |
| agent_unicode_german_ascii_small | failed / failed | 0.735 / 0.632 | 0.774 / 0.562 | 0.557 / 0.607 |
| agent_unicode_german_ascii_large | failed / failed | 4.525 / 4.209 | 4.616 / 4.252 | 4.298 / 4.138 |
| agent_unicode_german_small | failed / failed | 0.820 / 0.694 | 1.037 / 0.824 | 0.752 / 0.758 |
| agent_unicode_german_large | failed / failed | 5.774 / 5.530 | 9.489 / 9.130 | 6.939 / 6.447 |
| agent_unicode_german_emoji_small | failed / failed | 0.822 / 0.712 | 1.043 / 0.873 | 0.772 / 0.785 |
| agent_unicode_german_emoji_large | failed / failed | 6.072 / 5.527 | 9.256 / 9.316 | 7.053 / 7.282 |
| agent_unicode_chinese_small | failed / failed | 1.061 / 0.915 | 1.029 / 0.781 | 0.642 / 0.701 |
| agent_unicode_chinese_large | failed / failed | 9.670 / 9.498 | 8.125 / 8.242 | 4.455 / 4.649 |
