# Full Flying Circus before / after

Before: `e2e931d`. After: `a6ce17c`.
Both runtime sources were rebuilt with rustc 1.99.0 and the same benchmark-only worker.
Release builds use optimization level 3, no LTO and 16 codegen units.
Native enables read-only and mutation regions.

All 102 unchanged public workloads ran serially with `--quick`: three timing samples per runtime/workload/mode,
one repeated warmup, and one separate RSS sample.
Startup is measured separately.
Repeated requests use fresh VM sessions; they are not retained-VM warmed JIT timings.

These are rough comparisons. Three samples cannot establish statistical significance, especially for tiny workloads and PyPy with one warmup. Unsupported/error/mismatch cases remain visible and are excluded from paired timing aggregates.

| Runtime | One-shot ok / unsupported / failed | Repeated ok / unsupported / failed |
| --- | ---: | ---: |
| before-bytecode | 53 / 34 / 15 | 53 / 34 / 15 |
| after-bytecode | 71 / 19 / 12 | 71 / 19 / 12 |
| before-native | 53 / 34 / 15 | 53 / 34 / 15 |
| after-native | 71 / 19 / 12 | 71 / 19 / 12 |
| monty | 92 / 6 / 4 | 92 / 6 / 4 |
| cpython | 102 / 0 / 0 | 102 / 0 / 0 |
| pypy | 102 / 0 / 0 | 102 / 0 / 0 |

Ratios below 1 favor after. Geometric means use only workloads passing in both versions for the given tier/mode; each workload has equal weight. Newly supported programs have no before/after speed ratio.

| Tier | One-shot common cases / geometric mean after÷before | Repeated common cases / geometric mean after÷before | RSS common cases / geometric mean after÷before |
| --- | ---: | ---: | ---: |
| bytecode | 53 / 0.630 | 53 / 0.631 | 53 / 0.950 |
| native | 53 / 0.617 | 53 / 0.609 | 53 / 0.896 |

## Bytecode support changes

one-shot: newly passing: capacity_planning, portfolio_risk, ticket_search, perf_json_dumps, text_csv_aggregate_small, text_csv_aggregate_medium, text_csv_cleanup_small, text_csv_cleanup_medium, text_json_parse_small, text_json_parse_medium, text_json_transform_small, text_json_transform_medium, text_json_lines_small, text_json_lines_medium, memory_string_list_short, memory_string_list_long, memory_customer_records, perf_richards. Lost passing cases: none.
repeated: newly passing: capacity_planning, portfolio_risk, ticket_search, perf_json_dumps, text_csv_aggregate_small, text_csv_aggregate_medium, text_csv_cleanup_small, text_csv_cleanup_medium, text_json_parse_small, text_json_parse_medium, text_json_transform_small, text_json_transform_medium, text_json_lines_small, text_json_lines_medium, memory_string_list_short, memory_string_list_long, memory_customer_records, perf_richards. Lost passing cases: none.

## Bytecode largest repeated changes

| Workload | Before ms | After ms | After÷before |
| --- | ---: | ---: | ---: |
| rustpython_micro_strings | 237.087 | 3.958 | 0.017 |
| rustpython_micro_comprehension_set_900 | 2.110 | 0.303 | 0.143 |
| rustpython_micro_comprehension_set_700 | 1.259 | 0.307 | 0.244 |
| rustpython_micro_comprehension_set_500 | 0.815 | 0.262 | 0.321 |
| rustpython_micro_loop_string_900 | 0.343 | 0.117 | 0.340 |
| rustpython_micro_loop_string_500 | 0.219 | 0.099 | 0.452 |
| rustpython_micro_loop_string_700 | 0.264 | 0.123 | 0.467 |
| perf_json_loads | 1.922 | 0.937 | 0.487 |
| rustpython_micro_list_count_builtins_700 | 0.074 | 0.094 | 1.267 |
| rustpython_micro_list_count_builtins_900 | 0.056 | 0.058 | 1.030 |
| rustpython_micro_list_count_builtins_100 | 0.079 | 0.078 | 0.989 |
| rustpython_mandelbrot | 27.627 | 26.746 | 0.968 |
| game_spectral_norm | 296.770 | 286.866 | 0.967 |
| game_nbody | 247.003 | 237.877 | 0.963 |
| perf_barnes_hut | 2485.586 | 2358.323 | 0.949 |
| rustpython_pystone | 461.418 | 436.407 | 0.946 |

## Native support changes

one-shot: newly passing: capacity_planning, portfolio_risk, ticket_search, perf_json_dumps, text_csv_aggregate_small, text_csv_aggregate_medium, text_csv_cleanup_small, text_csv_cleanup_medium, text_json_parse_small, text_json_parse_medium, text_json_transform_small, text_json_transform_medium, text_json_lines_small, text_json_lines_medium, memory_string_list_short, memory_string_list_long, memory_customer_records, perf_richards. Lost passing cases: none.
repeated: newly passing: capacity_planning, portfolio_risk, ticket_search, perf_json_dumps, text_csv_aggregate_small, text_csv_aggregate_medium, text_csv_cleanup_small, text_csv_cleanup_medium, text_json_parse_small, text_json_parse_medium, text_json_transform_small, text_json_transform_medium, text_json_lines_small, text_json_lines_medium, memory_string_list_short, memory_string_list_long, memory_customer_records, perf_richards. Lost passing cases: none.

## Native largest repeated changes

| Workload | Before ms | After ms | After÷before |
| --- | ---: | ---: | ---: |
| rustpython_micro_strings | 243.154 | 4.447 | 0.018 |
| perf_unpack_sequence | 2298.660 | 220.054 | 0.096 |
| rustpython_micro_comprehension_set_900 | 2.053 | 0.307 | 0.149 |
| rustpython_micro_comprehension_set_700 | 1.322 | 0.304 | 0.230 |
| rustpython_micro_comprehension_set_500 | 0.819 | 0.220 | 0.269 |
| rustpython_micro_loop_string_700 | 0.295 | 0.116 | 0.392 |
| rustpython_micro_loop_string_900 | 0.352 | 0.144 | 0.410 |
| api_report | 0.694 | 0.289 | 0.416 |
| rustpython_micro_list_count_builtins_900 | 0.063 | 0.071 | 1.121 |
| rustpython_micro_list_count_builtins_500 | 0.056 | 0.058 | 1.024 |
| game_spectral_norm | 334.896 | 336.529 | 1.005 |
| perf_barnes_hut | 584.108 | 585.041 | 1.002 |
| perf_float | 226.805 | 225.566 | 0.995 |
| rustpython_micro_list_count_builtins_700 | 0.071 | 0.070 | 0.980 |
| stdout_lines | 6.338 | 6.015 | 0.949 |
| rustpython_pystone | 670.788 | 635.543 | 0.947 |

## Complete paired results

Each timing cell is one-shot / repeated after÷before. n/a means a version did not finish with correct output and all required samples. RSS is before→after MiB. Errors and compatibility details are retained in failures.json and the full report.

| Workload | BC ratio | Native ratio | BC RSS MiB | Native RSS MiB |
| --- | ---: | ---: | ---: | ---: |
| expense_report | 0.618 / 0.692 | 0.620 / 0.502 | 4.0 → 3.7 | 4.0 → 3.7 |
| order_cleanup | 0.621 / 0.659 | 0.649 / 0.506 | 4.2 → 4.1 | 4.3 → 4.1 |
| api_report | 0.646 / 0.664 | 0.657 / 0.416 | 4.1 → 4.0 | 4.1 → 4.0 |
| capacity_planning | n/a / n/a | n/a / n/a | n/a → 12.8 | n/a → 15.9 |
| portfolio_risk | n/a / n/a | n/a / n/a | n/a → 5.8 | n/a → 9.3 |
| ticket_search | n/a / n/a | n/a / n/a | n/a → 14.2 | n/a → 18.5 |
| game_fannkuch | n/a / n/a | n/a / n/a | n/a → n/a | n/a → n/a |
| game_nbody | 0.954 / 0.963 | 0.904 / 0.882 | 7.5 → 7.3 | 12.3 → 12.2 |
| game_pidigits | n/a / n/a | n/a / n/a | n/a → n/a | n/a → n/a |
| game_spectral_norm | 0.941 / 0.967 | 1.009 / 1.005 | 6.1 → 6.0 | 9.5 → 9.2 |
| game_regex_dna | n/a / n/a | n/a / n/a | n/a → n/a | n/a → n/a |
| game_meteor_contest | n/a / n/a | n/a / n/a | n/a → n/a | n/a → n/a |
| perf_barnes_hut | 0.952 / 0.949 | 0.973 / 1.002 | 10.2 → 9.8 | 22.0 → 22.8 |
| perf_float | 0.942 / 0.940 | 0.966 / 0.995 | 57.7 → 56.0 | 72.4 → 71.9 |
| perf_unpack_sequence | 0.942 / 0.928 | 0.098 / 0.096 | 29.9 → 25.5 | 804.5 → 35.5 |
| perf_json_dumps | n/a / n/a | n/a / n/a | n/a → 9.1 | n/a → 11.8 |
| perf_json_loads | 0.566 / 0.487 | 0.533 / 0.544 | 6.1 → 5.6 | 6.1 → 5.6 |
| text_line_logs_small | 0.709 / 0.628 | 0.831 / 0.636 | 4.4 → 4.2 | 4.4 → 4.2 |
| text_line_logs_medium | 0.886 / 0.839 | 0.895 / 0.882 | 17.5 → 16.0 | 23.4 → 18.7 |
| text_regex_logs_small | n/a / n/a | n/a / n/a | n/a → n/a | n/a → n/a |
| text_regex_logs_medium | n/a / n/a | n/a / n/a | n/a → n/a | n/a → n/a |
| text_csv_aggregate_small | n/a / n/a | n/a / n/a | n/a → 6.5 | n/a → 10.6 |
| text_csv_aggregate_medium | n/a / n/a | n/a / n/a | n/a → 52.0 | n/a → 56.5 |
| text_csv_cleanup_small | n/a / n/a | n/a / n/a | n/a → 6.7 | n/a → 10.8 |
| text_csv_cleanup_medium | n/a / n/a | n/a / n/a | n/a → 80.8 | n/a → 77.1 |
| text_json_parse_small | n/a / n/a | n/a / n/a | n/a → 4.1 | n/a → 4.1 |
| text_json_parse_medium | n/a / n/a | n/a / n/a | n/a → 18.5 | n/a → 23.4 |
| text_json_transform_small | n/a / n/a | n/a / n/a | n/a → 4.3 | n/a → 4.4 |
| text_json_transform_medium | n/a / n/a | n/a / n/a | n/a → 20.9 | n/a → 25.1 |
| text_json_lines_small | n/a / n/a | n/a / n/a | n/a → 4.2 | n/a → 4.2 |
| text_json_lines_medium | n/a / n/a | n/a / n/a | n/a → 14.6 | n/a → 20.0 |
| memory_string_list_short | n/a / n/a | n/a / n/a | n/a → 13.7 | n/a → 17.7 |
| memory_string_list_long | n/a / n/a | n/a / n/a | n/a → 147.8 | n/a → 193.4 |
| memory_string_index_short | 0.700 / 0.658 | 0.800 / 0.793 | 41.1 → 34.0 | 42.8 → 39.1 |
| memory_string_index_long | 0.839 / 0.768 | 0.851 / 0.796 | 52.7 → 51.3 | 55.8 → 54.3 |
| memory_customer_records | n/a / n/a | n/a / n/a | n/a → 385.3 | n/a → 385.0 |
| stdout_lines | 0.912 / 0.856 | 0.985 / 0.949 | 4.2 → 4.1 | 6.8 → 6.8 |
| perf_richards | n/a / n/a | n/a / n/a | n/a → 10.9 | n/a → 18.2 |
| rustpython_base64 | n/a / n/a | n/a / n/a | n/a → n/a | n/a → n/a |
| rustpython_decimal_factorial | n/a / n/a | n/a / n/a | n/a → n/a | n/a → n/a |
| rustpython_deepcopy | n/a / n/a | n/a / n/a | n/a → n/a | n/a → n/a |
| rustpython_deltablue | n/a / n/a | n/a / n/a | n/a → n/a | n/a → n/a |
| rustpython_generators | n/a / n/a | n/a / n/a | n/a → n/a | n/a → n/a |
| rustpython_mandelbrot | 0.969 / 0.968 | 0.914 / 0.918 | 4.8 → 4.6 | 9.3 → 8.9 |
| rustpython_nqueens | n/a / n/a | n/a / n/a | n/a → n/a | n/a → n/a |
| rustpython_pickle | n/a / n/a | n/a / n/a | n/a → n/a | n/a → n/a |
| rustpython_pystone | 0.936 / 0.946 | 0.949 / 0.947 | 8.3 → 8.4 | 13.6 → 14.0 |
| rustpython_regex_effbot | n/a / n/a | n/a / n/a | n/a → n/a | n/a → n/a |
| rustpython_unpickle | n/a / n/a | n/a / n/a | n/a → n/a | n/a → n/a |
| rustpython_micro_addition_100 | 0.654 / 0.636 | 0.812 / 0.886 | 3.4 → 3.4 | 3.5 → 3.4 |
| rustpython_micro_addition_300 | 0.650 / 0.661 | 0.749 / 0.771 | 3.4 → 3.4 | 3.5 → 3.4 |
| rustpython_micro_addition_500 | 0.769 / 0.784 | 0.742 / 0.714 | 3.5 → 3.4 | 3.5 → 3.4 |
| rustpython_micro_addition_700 | 0.750 / 0.731 | 0.843 / 0.856 | 3.5 → 3.4 | 3.5 → 3.4 |
| rustpython_micro_addition_900 | 0.764 / 0.801 | 0.905 / 0.890 | 3.5 → 3.4 | 3.5 → 3.4 |
| rustpython_micro_cmp | 0.790 / 0.775 | 0.957 / 0.940 | 3.7 → 3.5 | 6.8 → 6.8 |
| rustpython_micro_complex_class | n/a / n/a | n/a / n/a | n/a → n/a | n/a → n/a |
| rustpython_micro_comprehension_dict_100 | 0.767 / 0.659 | 0.713 / 0.750 | 3.7 → 3.6 | 3.7 → 3.6 |
| rustpython_micro_comprehension_dict_300 | 0.717 / 0.740 | 0.718 / 0.669 | 3.7 → 3.6 | 3.7 → 3.7 |
| rustpython_micro_comprehension_dict_500 | 0.711 / 0.706 | 0.656 / 0.628 | 3.8 → 3.7 | 3.8 → 3.7 |
| rustpython_micro_comprehension_dict_700 | 0.778 / 0.696 | 0.770 / 0.701 | 3.9 → 3.8 | 3.9 → 3.8 |
| rustpython_micro_comprehension_dict_900 | 0.716 / 0.685 | 0.646 / 0.657 | 3.9 → 3.8 | 3.9 → 3.8 |
| rustpython_micro_comprehension_list_100 | 0.878 / 0.713 | 0.684 / 0.760 | 3.7 → 3.6 | 3.7 → 3.6 |
| rustpython_micro_comprehension_list_300 | 0.671 / 0.763 | 0.662 / 0.647 | 3.7 → 3.6 | 3.8 → 3.6 |
| rustpython_micro_comprehension_list_500 | 0.719 / 0.688 | 0.653 / 0.719 | 3.8 → 3.6 | 3.8 → 3.6 |
| rustpython_micro_comprehension_list_700 | 0.695 / 0.672 | 0.638 / 0.690 | 3.7 → 3.6 | 3.8 → 3.6 |
| rustpython_micro_comprehension_list_900 | 0.669 / 0.621 | 0.642 / 0.678 | 3.8 → 3.6 | 3.9 → 3.6 |
| rustpython_micro_comprehension_set_100 | 0.681 / 0.675 | 0.591 / 0.682 | 3.8 → 3.6 | 3.8 → 3.6 |
| rustpython_micro_comprehension_set_300 | 0.466 / 0.522 | 0.466 / 0.494 | 4.0 → 3.7 | 4.1 → 3.7 |
| rustpython_micro_comprehension_set_500 | 0.284 / 0.321 | 0.329 / 0.269 | 4.2 → 3.7 | 4.2 → 3.8 |
| rustpython_micro_comprehension_set_700 | 0.219 / 0.244 | 0.300 / 0.230 | 4.4 → 3.8 | 4.4 → 3.9 |
| rustpython_micro_comprehension_set_900 | 0.182 / 0.143 | 0.188 / 0.149 | 4.4 → 3.8 | 4.6 → 4.0 |
| rustpython_micro_frozenset_100 | n/a / n/a | n/a / n/a | n/a → n/a | n/a → n/a |
| rustpython_micro_frozenset_300 | n/a / n/a | n/a / n/a | n/a → n/a | n/a → n/a |
| rustpython_micro_frozenset_500 | n/a / n/a | n/a / n/a | n/a → n/a | n/a → n/a |
| rustpython_micro_frozenset_700 | n/a / n/a | n/a / n/a | n/a → n/a | n/a → n/a |
| rustpython_micro_frozenset_900 | n/a / n/a | n/a / n/a | n/a → n/a | n/a → n/a |
| rustpython_micro_list_count_builtins_100 | 1.192 / 0.989 | 0.627 / 0.702 | 3.6 → 3.6 | 3.7 → 3.6 |
| rustpython_micro_list_count_builtins_300 | 0.947 / 0.649 | 0.821 / 0.761 | 3.6 → 3.6 | 3.7 → 3.6 |
| rustpython_micro_list_count_builtins_500 | 0.761 / 0.795 | 0.954 / 1.024 | 3.6 → 3.6 | 3.8 → 3.6 |
| rustpython_micro_list_count_builtins_700 | 0.575 / 1.267 | 0.726 / 0.980 | 3.7 → 3.6 | 3.9 → 3.6 |
| rustpython_micro_list_count_builtins_900 | 0.880 / 1.030 | 0.963 / 1.121 | 3.7 → 3.6 | 3.9 → 3.6 |
| rustpython_micro_list_count_custom_eq_100 | n/a / n/a | n/a / n/a | n/a → n/a | n/a → n/a |
| rustpython_micro_list_count_custom_eq_300 | n/a / n/a | n/a / n/a | n/a → n/a | n/a → n/a |
| rustpython_micro_list_count_custom_eq_500 | n/a / n/a | n/a / n/a | n/a → n/a | n/a → n/a |
| rustpython_micro_list_count_custom_eq_700 | n/a / n/a | n/a / n/a | n/a → n/a | n/a → n/a |
| rustpython_micro_list_count_custom_eq_900 | n/a / n/a | n/a / n/a | n/a → n/a | n/a → n/a |
| rustpython_micro_loop_append_100 | 0.632 / 0.671 | 0.656 / 0.704 | 3.5 → 3.4 | 3.6 → 3.4 |
| rustpython_micro_loop_append_300 | 0.741 / 0.804 | 0.791 / 0.765 | 3.5 → 3.4 | 3.6 → 3.5 |
| rustpython_micro_loop_append_500 | 0.684 / 0.734 | 0.736 / 0.767 | 3.6 → 3.4 | 3.7 → 3.5 |
| rustpython_micro_loop_append_700 | 0.684 / 0.828 | 0.721 / 0.798 | 3.6 → 3.4 | 3.8 → 3.6 |
| rustpython_micro_loop_append_900 | 0.745 / 0.738 | 0.763 / 0.788 | 3.6 → 3.4 | 3.8 → 3.6 |
| rustpython_micro_loop_string_100 | 0.607 / 0.657 | 0.659 / 0.744 | 3.4 → 3.3 | 3.5 → 3.3 |
| rustpython_micro_loop_string_300 | 0.484 / 0.534 | 0.544 / 0.560 | 3.5 → 3.3 | 3.5 → 3.3 |
| rustpython_micro_loop_string_500 | 0.545 / 0.452 | 0.477 / 0.549 | 3.5 → 3.4 | 3.6 → 3.3 |
| rustpython_micro_loop_string_700 | 0.468 / 0.467 | 0.477 / 0.392 | 3.6 → 3.3 | 3.6 → 3.3 |
| rustpython_micro_loop_string_900 | 0.359 / 0.340 | 0.371 / 0.410 | 3.6 → 3.3 | 3.6 → 3.3 |
| rustpython_micro_sorted_100 | n/a / n/a | n/a / n/a | n/a → n/a | n/a → n/a |
| rustpython_micro_sorted_300 | n/a / n/a | n/a / n/a | n/a → n/a | n/a → n/a |
| rustpython_micro_sorted_500 | n/a / n/a | n/a / n/a | n/a → n/a | n/a → n/a |
| rustpython_micro_sorted_700 | n/a / n/a | n/a / n/a | n/a → n/a | n/a → n/a |
| rustpython_micro_sorted_900 | n/a / n/a | n/a / n/a | n/a → n/a | n/a → n/a |
| rustpython_micro_strings | 0.017 / 0.017 | 0.020 / 0.018 | 4.5 → 3.5 | 6.9 → 6.1 |

## After versus reference runtimes

| Comparison | Mode | Correct common workloads | Geometric mean ratio | After faster | After within 10% |
| --- | --- | ---: | ---: | ---: | ---: |
| after-bytecode/monty | one-shot | 71 | 1.185 | 22 | 28 |
| after-bytecode/monty | repeated | 71 | 1.274 | 14 | 19 |
| after-native/cpython | one-shot | 71 | 1.874 | 14 | 15 |
| after-native/cpython | repeated | 71 | 1.796 | 18 | 19 |
| after-native/pypy | one-shot | 71 | 1.675 | 23 | 24 |
| after-native/pypy | repeated | 71 | 1.420 | 34 | 36 |

[Full seven-runtime report](quick/report.html) · [Bytecode comparison](bytecode-comparison/report.html) · [Native comparison](native-comparison/report.html) · [All failure details](failures.json) · [Build provenance](provenance.json)


## Follow-up and remaining gaps

The six-sample counter-test follow-up is in [count-followup/report.html](count-followup/report.html).
The initial 12–27% slowdowns on tiny list-count cases did not reproduce at that magnitude:
follow-up ratios were 0.74–0.92 for most cells and 1.01 for native repeated count-700.
This remains quick sampling, not a significance claim.
All original measurements remain in the full matrix; the summary does not replace them with follow-up values.

Both tiers now pass 71 workloads, with 19 unsupported and 12 failed cases.
The remaining failed cases are fannkuch's non-integer index path, missing frozenset in six workloads,
and custom equality in five list-count workloads.
No previously passing workload was lost in either timing mode.
The command exits nonzero because these visible failures remain, not because the matrix stopped early.
CPython and PyPy pass all 102; Monty passes 92.

Across the 53 common workloads, native unpacking improves from 2298.7 to 220.1 ms in repeated mode,
while Barnes–Hut is effectively unchanged at 584.1 versus 585.0 ms in this short run.
Do not interpret the aggregate gain as a uniform improvement to long-running numerical code.

## Reproduction

`provenance.json` records the exact revisions, binary hashes, compiler and build settings.
The before runtime is the exact parent commit with only the identical benchmark worker added for measurement.
The after source is committed unchanged; no implementation changes were made during this comparison.
Frozen workers are in `binaries/` and excluded from Git.
All original workload bytes and expected stdout are in `workloads/manifest.json` and the accompanying source files.
The harness snapshot includes the tested tis adapter; the original Flying Circus checkout was not edited.

`run.py` records the complete invocation and local paths.
To relocate the run, extract the harness snapshot, adjust its PYTHONPATH and the worker paths,
and choose a fresh output directory.
`pair_reports.py` generates tier-specific reports from the original samples without rerunning measurements.
New comparison evidence is left uncommitted; the requested implementation commit was made before benchmarking.
