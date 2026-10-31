# Bytecode toward Monty parity — 2026-10-31

Target: tis bytecode elapsed time no more than 10% above the frozen Monty baseline. This experiment keeps the full 120-workload public Flying Circus suite and unchanged default workload sizes. Failures and unsupported workloads remain visible.

## Full-suite result

Serial `--quick`: three timing samples, one repeated warmup, one separate RSS sample and five startup samples. Both modes include source compilation, execution and output delivery. Readiness, fixture preparation and session reset are excluded. Repeated means fresh VM sessions in a reused worker; these are not retained-code timings. Three samples support rough comparisons only.

| Runtime | One-shot pass / unsupported / failed | Repeated pass / unsupported / failed |
| --- | ---: | ---: |
| before | 88 / 17 / 15 | 88 / 17 / 15 |
| after | 88 / 17 / 15 | 88 / 17 / 15 |
| monty | 110 / 6 / 4 | 110 / 6 / 4 |

| Comparison | Mode | Common passes | Geometric mean ratio | First within 10% |
| --- | --- | ---: | ---: | ---: |
| before / monty | one-shot | 87 | 1.306 | 32 |
| before / monty | repeated | 87 | 1.425 | 21 |
| after / monty | one-shot | 87 | 1.239 | 30 |
| after / monty | repeated | 87 | 1.386 | 22 |
| after / before | one-shot | 88 | 0.949 | 83 |
| after / before | repeated | 88 | 0.972 | 73 |

Equal weighting across workloads, including many tiny microbenchmarks, makes this a suite comparison rather than an application-wide speed score. Unavailable cases are excluded only from paired ratios.

## Representative elapsed times

Each cell is one-shot / repeated milliseconds, with compilation included.

| Workload | Before | After | Monty |
| --- | ---: | ---: | ---: |
| perf_unpack_sequence | 150.651 / 151.524 | 47.065 / 45.510 | 47.902 / 47.892 |
| text_unicode_access_ascii | 15.212 / 15.618 | 4.074 / 4.139 | 2.876 / 2.844 |
| rustpython_micro_strings | 4.704 / 4.539 | 3.225 / 3.189 | 0.902 / 0.855 |
| memory_customer_records | 930.877 / 872.466 | 557.669 / 559.534 | 323.393 / 344.646 |
| ticket_search | 39.375 / 38.761 | 38.705 / 40.251 | 23.648 / 23.212 |
| memory_string_list_short | 319.073 / 316.372 | 320.593 / 326.166 | 171.830 / 174.124 |
| game_nbody | 275.090 / 274.847 | 218.363 / 218.687 | 289.938 / 296.714 |
| perf_barnes_hut | 3079.702 / 3080.884 | 3017.721 / 3008.550 | 4170.467 / 4174.817 |
| text_csv_cleanup_medium | 872.378 / 869.935 | 808.732 / 805.237 | 679.297 / 671.364 |
| text_json_parse_medium | 11.023 / 10.880 | 11.263 / 10.734 | 10.170 / 9.409 |

The target remains unmet: 1.239× Monty one-shot and 1.386× in repeated fresh sessions. Relative to the paired frozen tis control, elapsed time fell about 5.1% and 2.8%, respectively.

## Changes and isolated evidence

- ASCII indexing uses the existing cached ASCII flag and direct byte access. Host string mutation invalidates the cache. Unicode retains code-point indexing.
- Bytecode uses the non-allocating string iterator probe, with shared cursor mutation; character values fit inline. Exhaustion and custom iterators retain their original paths.
- Exact string-key dictionary literals reserve storage and build directly. Other keys retain hash/equality protocol dispatch and duplicate-key identity/order rules.
- Quiet register bytecode can batch a checked list/tuple unpack plus its adjacent reads. The internal token must have no other users. All elements are copied before further execution; no callback, GC or side exit occurs inside the batch. Feedback collection, custom iterators and length errors use the original instructions.
- Closure HIR bodies use copy-on-write storage. Captures/defaults remain per closure, code profiles remain keyed by semantic identity, and editing one body detaches it and retires cached code. Runtime legacy addition feedback can still detach a shared body; moving that feedback out of HIR is a remaining opportunity.

The first seven-sample comparison measured ASCII indexing at 15.24 → 3.96 ms and string iteration at 4.67 → 3.23 ms. CJK indexing remained about 36 ms. A later seven-sample comparison isolated HIR sharing: customer records took 880.8 → 548.3 ms (Monty 333.0 ms); ticket search, short string lists and unpacking did not show comparable gains. Dictionary construction did not establish a standalone throughput win in the broad comparison.

A five-second macOS `sample` capture of the prior candidate showed allocation/free work, HIR cloning and destruction of cloned HIR bodies during GC. It motivated the sharing experiment; sample counts are not precise CPU attribution.

## Regression follow-up

A seven-sample follow-up did not reproduce the large integer-addition and tiny string-loop regressions in the three-sample suite.
The apparent 1.94× mixed-Unicode slowdown shrank to 1.11×; ASCII Unicode-memory storage was 1.09× and emoji storage 1.02× the frozen control.
These remaining Unicode costs are not explained by the current profile and remain open.
Record-building again improved from 865.2 to 553.8 ms; unpacking from 149.2 to 45.7 ms, versus Monty's 47.6 ms.
The quick suite is noisy enough that small whole-suite gains need stronger confirmation.

## RSS

Separate one-shot peak RSS samples, whole process including startup; one sample per workload, so variability is unknown.

| Workload | Before MiB | After MiB | Monty MiB |
| --- | ---: | ---: | ---: |
| memory_customer_records | 384.5 | 182.5 | 152.0 |
| perf_unpack_sequence | 25.7 | 17.2 | 11.4 |
| memory_string_list_short | 13.7 | 13.7 | 89.1 |
| memory_unicode_mixed | 44.6 | 44.6 | 36.4 |

## Validation and limits

`cargo fmt --all`, `cargo clippy --all-targets -- -D warnings`, and all 469 tests passed. Regression coverage includes Unicode/exhaustion, cache invalidation after host string mutation, duplicate dictionary key ordering and protocol callbacks, list unpack snapshots, custom iterator effects, shared HIR storage, independent body edits and cached-code invalidation after opaque body replacement.

No workload or Monty implementation was changed. The frozen before worker includes the earlier uncommitted Cranelift Boolean emitter, which bytecode does not execute. Default release features and tis `codegen-units=1` are consistent. Monty is the cached compatible 1.4.0 worker. The full suite still exposes missing modules, builtins and methods; this experiment targets performance, not additional language coverage.

Raw evidence is outside Git: `/Users/jaheba/Library/Caches/tis-benchmarks/2026-10-31-monty-parity/`. It contains all sampling rounds, binary hashes, frozen workers, source snapshot/diff, commands, profiles and validation logs.

[Full interactive report](/Users/jaheba/Library/Caches/tis-benchmarks/2026-10-31-monty-parity/final-full/report.html)

## Complete timing table

Each cell is one-shot / repeated milliseconds.

| Workload | Before | After | Monty |
| --- | ---: | ---: | ---: |
| expense_report | 0.379 / 0.248 | 0.392 / 0.261 | 0.320 / 0.137 |
| order_cleanup | 0.550 / 0.364 | 0.521 / 0.357 | 0.459 / 0.222 |
| api_report | 0.484 / 0.345 | 0.525 / 0.342 | 0.496 / 0.257 |
| capacity_planning | 22.497 / 21.951 | 23.876 / 23.169 | 32.373 / 31.611 |
| portfolio_risk | 62.248 / 62.050 | 67.407 / 67.936 | 63.666 / 64.775 |
| ticket_search | 39.375 / 38.761 | 38.705 / 40.251 | 23.648 / 23.212 |
| game_fannkuch | 1781.366 / 1799.065 | 1834.143 / 1870.728 | 1182.637 / 1207.098 |
| game_nbody | 275.090 / 274.847 | 218.363 / 218.687 | 289.938 / 296.714 |
| game_pidigits | unsupported / unsupported | unsupported / unsupported | unsupported / unsupported |
| game_spectral_norm | 291.186 / 292.121 | 277.979 / 281.372 | 259.911 / 258.289 |
| game_regex_dna | unsupported / unsupported | unsupported / unsupported | failed / failed |
| game_meteor_contest | failed / failed | failed / failed | 350.282 / 355.503 |
| perf_barnes_hut | 3079.702 / 3080.884 | 3017.721 / 3008.550 | 4170.467 / 4174.817 |
| perf_float | 235.022 / 235.140 | 245.076 / 238.012 | 256.106 / 256.498 |
| perf_unpack_sequence | 150.651 / 151.524 | 47.065 / 45.510 | 47.902 / 47.892 |
| perf_json_dumps | 8.760 / 8.267 | 8.637 / 8.495 | 10.933 / 10.756 |
| perf_json_loads | 1.216 / 0.999 | 1.237 / 0.992 | 1.116 / 0.845 |
| text_line_logs_small | 0.505 / 0.402 | 0.502 / 0.394 | 0.433 / 0.271 |
| text_line_logs_medium | 21.928 / 23.616 | 21.394 / 22.575 | 15.294 / 15.658 |
| text_regex_logs_small | unsupported / unsupported | unsupported / unsupported | 0.862 / 0.684 |
| text_regex_logs_medium | unsupported / unsupported | unsupported / unsupported | 25.709 / 26.202 |
| text_csv_aggregate_small | 5.872 / 5.649 | 5.486 / 5.158 | 3.786 / 3.482 |
| text_csv_aggregate_medium | 447.108 / 442.249 | 407.945 / 399.211 | 335.851 / 334.494 |
| text_csv_cleanup_small | 9.857 / 9.706 | 9.188 / 8.887 | 7.185 / 6.785 |
| text_csv_cleanup_medium | 872.378 / 869.935 | 808.732 / 805.237 | 679.297 / 671.364 |
| text_json_parse_small | 0.350 / 0.281 | 0.366 / 0.270 | 0.391 / 0.262 |
| text_json_parse_medium | 11.023 / 10.880 | 11.263 / 10.734 | 10.170 / 9.409 |
| text_json_transform_small | 0.533 / 0.389 | 0.482 / 0.434 | 0.500 / 0.287 |
| text_json_transform_medium | 15.271 / 13.674 | 15.026 / 14.291 | 11.840 / 11.408 |
| text_json_lines_small | 0.544 / 0.393 | 0.555 / 0.446 | 0.602 / 0.388 |
| text_json_lines_medium | 17.850 / 18.031 | 17.743 / 18.128 | 15.333 / 14.542 |
| memory_string_list_short | 319.073 / 316.372 | 320.593 / 326.166 | 171.830 / 174.124 |
| memory_string_list_long | 356.200 / 354.541 | 362.568 / 372.222 | 245.310 / 241.911 |
| memory_string_index_short | 101.379 / 101.105 | 105.964 / 100.193 | 80.807 / 80.826 |
| memory_string_index_long | 130.617 / 134.629 | 142.856 / 138.376 | 127.352 / 119.278 |
| memory_customer_records | 930.877 / 872.466 | 557.669 / 559.534 | 323.393 / 344.646 |
| stdout_lines | 4.250 / 3.852 | 4.028 / 3.749 | 2.145 / 1.928 |
| perf_richards | 279.014 / 279.396 | 288.438 / 288.126 | 190.467 / 195.827 |
| rustpython_base64 | unsupported / unsupported | unsupported / unsupported | 1.663 / 1.564 |
| rustpython_decimal_factorial | unsupported / unsupported | unsupported / unsupported | 1.696 / 1.508 |
| rustpython_deepcopy | unsupported / unsupported | unsupported / unsupported | unsupported / unsupported |
| rustpython_deltablue | failed / failed | failed / failed | unsupported / unsupported |
| rustpython_generators | unsupported / unsupported | unsupported / unsupported | unsupported / unsupported |
| rustpython_mandelbrot | 27.354 / 27.329 | 29.217 / 29.356 | 19.196 / 19.014 |
| rustpython_nqueens | unsupported / unsupported | unsupported / unsupported | unsupported / unsupported |
| rustpython_pickle | unsupported / unsupported | unsupported / unsupported | failed / failed |
| rustpython_pystone | 476.636 / 490.657 | 500.048 / 497.911 | 315.562 / 317.089 |
| rustpython_regex_effbot | unsupported / unsupported | unsupported / unsupported | failed / failed |
| rustpython_unpickle | unsupported / unsupported | unsupported / unsupported | failed / failed |
| rustpython_micro_addition_100 | 0.127 / 0.115 | 0.116 / 0.086 | 0.132 / 0.058 |
| rustpython_micro_addition_300 | 0.154 / 0.105 | 0.150 / 0.135 | 0.143 / 0.065 |
| rustpython_micro_addition_500 | 0.192 / 0.130 | 0.171 / 0.140 | 0.143 / 0.082 |
| rustpython_micro_addition_700 | 0.198 / 0.167 | 0.191 / 0.177 | 0.182 / 0.102 |
| rustpython_micro_addition_900 | 0.220 / 0.170 | 0.226 / 0.217 | 0.176 / 0.105 |
| rustpython_micro_cmp | 2.734 / 2.514 | 2.735 / 2.713 | 1.786 / 1.759 |
| rustpython_micro_complex_class | 0.181 / 0.113 | 0.179 / 0.101 | unsupported / unsupported |
| rustpython_micro_comprehension_dict_100 | 0.201 / 0.168 | 0.198 / 0.118 | 0.166 / 0.108 |
| rustpython_micro_comprehension_dict_300 | 0.306 / 0.192 | 0.212 / 0.169 | 0.189 / 0.131 |
| rustpython_micro_comprehension_dict_500 | 0.286 / 0.239 | 0.257 / 0.208 | 0.189 / 0.141 |
| rustpython_micro_comprehension_dict_700 | 0.309 / 0.246 | 0.305 / 0.252 | 0.226 / 0.147 |
| rustpython_micro_comprehension_dict_900 | 0.369 / 0.306 | 0.349 / 0.297 | 0.230 / 0.170 |
| rustpython_micro_comprehension_list_100 | 0.239 / 0.176 | 0.160 / 0.119 | 0.137 / 0.088 |
| rustpython_micro_comprehension_list_300 | 0.207 / 0.155 | 0.281 / 0.174 | 0.153 / 0.069 |
| rustpython_micro_comprehension_list_500 | 0.228 / 0.193 | 0.219 / 0.155 | 0.172 / 0.119 |
| rustpython_micro_comprehension_list_700 | 0.251 / 0.196 | 0.239 / 0.206 | 0.179 / 0.119 |
| rustpython_micro_comprehension_list_900 | 0.294 / 0.203 | 0.261 / 0.213 | 0.183 / 0.123 |
| rustpython_micro_comprehension_set_100 | 0.186 / 0.143 | 0.181 / 0.141 | 0.176 / 0.100 |
| rustpython_micro_comprehension_set_300 | 0.254 / 0.181 | 0.228 / 0.176 | 0.162 / 0.096 |
| rustpython_micro_comprehension_set_500 | 0.280 / 0.233 | 0.285 / 0.205 | 0.172 / 0.089 |
| rustpython_micro_comprehension_set_700 | 0.321 / 0.272 | 0.321 / 0.279 | 0.199 / 0.144 |
| rustpython_micro_comprehension_set_900 | 0.388 / 0.308 | 0.374 / 0.324 | 0.214 / 0.169 |
| rustpython_micro_frozenset_100 | failed / failed | failed / failed | 0.098 / 0.060 |
| rustpython_micro_frozenset_300 | failed / failed | failed / failed | 0.083 / 0.060 |
| rustpython_micro_frozenset_500 | failed / failed | failed / failed | 0.094 / 0.072 |
| rustpython_micro_frozenset_700 | failed / failed | failed / failed | 0.088 / 0.071 |
| rustpython_micro_frozenset_900 | failed / failed | failed / failed | 0.090 / 0.074 |
| rustpython_micro_list_count_builtins_100 | 0.075 / 0.052 | 0.079 / 0.060 | 0.117 / 0.073 |
| rustpython_micro_list_count_builtins_300 | 0.078 / 0.063 | 0.083 / 0.074 | 0.104 / 0.073 |
| rustpython_micro_list_count_builtins_500 | 0.092 / 0.066 | 0.084 / 0.073 | 0.108 / 0.069 |
| rustpython_micro_list_count_builtins_700 | 0.100 / 0.069 | 0.106 / 0.077 | 0.107 / 0.062 |
| rustpython_micro_list_count_builtins_900 | 0.112 / 0.081 | 0.089 / 0.077 | 0.118 / 0.067 |
| rustpython_micro_list_count_custom_eq_100 | 0.091 / 0.082 | 0.121 / 0.075 | 0.117 / 0.079 |
| rustpython_micro_list_count_custom_eq_300 | 0.156 / 0.129 | 0.142 / 0.125 | 0.143 / 0.147 |
| rustpython_micro_list_count_custom_eq_500 | 0.190 / 0.157 | 0.190 / 0.153 | 0.200 / 0.159 |
| rustpython_micro_list_count_custom_eq_700 | 0.226 / 0.196 | 0.233 / 0.204 | 0.221 / 0.207 |
| rustpython_micro_list_count_custom_eq_900 | 0.257 / 0.252 | 0.284 / 0.250 | 0.247 / 0.208 |
| rustpython_micro_loop_append_100 | 0.144 / 0.111 | 0.133 / 0.102 | 0.161 / 0.124 |
| rustpython_micro_loop_append_300 | 0.188 / 0.138 | 0.174 / 0.167 | 0.156 / 0.077 |
| rustpython_micro_loop_append_500 | 0.210 / 0.186 | 0.228 / 0.197 | 0.181 / 0.100 |
| rustpython_micro_loop_append_700 | 0.250 / 0.201 | 0.267 / 0.236 | 0.190 / 0.129 |
| rustpython_micro_loop_append_900 | 0.291 / 0.263 | 0.312 / 0.264 | 0.215 / 0.149 |
| rustpython_micro_loop_string_100 | 0.088 / 0.061 | 0.099 / 0.086 | 0.064 / 0.073 |
| rustpython_micro_loop_string_300 | 0.117 / 0.085 | 0.132 / 0.080 | 0.073 / 0.048 |
| rustpython_micro_loop_string_500 | 0.138 / 0.128 | 0.127 / 0.124 | 0.067 / 0.045 |
| rustpython_micro_loop_string_700 | 0.162 / 0.128 | 0.140 / 0.134 | 0.070 / 0.069 |
| rustpython_micro_loop_string_900 | 0.181 / 0.152 | 0.155 / 0.145 | 0.079 / 0.057 |
| rustpython_micro_sorted_100 | unsupported / unsupported | unsupported / unsupported | 0.216 / 0.162 |
| rustpython_micro_sorted_300 | unsupported / unsupported | unsupported / unsupported | 0.420 / 0.348 |
| rustpython_micro_sorted_500 | unsupported / unsupported | unsupported / unsupported | 0.627 / 0.574 |
| rustpython_micro_sorted_700 | unsupported / unsupported | unsupported / unsupported | 0.878 / 0.775 |
| rustpython_micro_sorted_900 | unsupported / unsupported | unsupported / unsupported | 1.041 / 1.006 |
| rustpython_micro_strings | 4.704 / 4.539 | 3.225 / 3.189 | 0.902 / 0.855 |
| text_unicode_access_ascii | 15.212 / 15.618 | 4.074 / 4.139 | 2.876 / 2.844 |
| memory_unicode_ascii | 8.646 / 5.798 | 8.568 / 6.828 | 5.332 / 6.064 |
| text_unicode_access_latin1 | 25.562 / 26.133 | 25.778 / 25.881 | 25.122 / 25.076 |
| memory_unicode_latin1 | 15.570 / 11.361 | 16.575 / 12.365 | 9.106 / 8.315 |
| text_unicode_access_cjk | 35.769 / 35.709 | 35.936 / 35.994 | 35.340 / 35.200 |
| memory_unicode_cjk | 20.925 / 17.739 | 20.367 / 13.523 | 12.488 / 13.032 |
| text_unicode_access_emoji | 45.648 / 46.025 | 45.781 / 45.698 | 45.069 / 45.451 |
| memory_unicode_emoji | 19.538 / 18.249 | 19.540 / 20.268 | 14.966 / 14.843 |
| text_unicode_access_mixed | 15.262 / 15.416 | 15.383 / 15.335 | 14.101 / 13.713 |
| memory_unicode_mixed | 10.425 / 7.713 | 10.793 / 15.000 | 6.984 / 6.741 |
| agent_unicode_german_ascii_small | failed / failed | failed / failed | 0.739 / 0.509 |
| agent_unicode_german_ascii_large | failed / failed | failed / failed | 4.746 / 4.545 |
| agent_unicode_german_small | failed / failed | failed / failed | 1.103 / 0.965 |
| agent_unicode_german_large | failed / failed | failed / failed | 9.464 / 9.092 |
| agent_unicode_german_emoji_small | failed / failed | failed / failed | 1.083 / 0.832 |
| agent_unicode_german_emoji_large | failed / failed | failed / failed | 9.401 / 8.903 |
| agent_unicode_chinese_small | failed / failed | failed / failed | 0.970 / 0.798 |
| agent_unicode_chinese_large | failed / failed | failed / failed | 8.213 / 7.882 |
