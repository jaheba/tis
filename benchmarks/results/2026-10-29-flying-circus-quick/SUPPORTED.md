# Supported Flying Circus workloads
54 of 103 pass the unchanged manifest. This includes the internal startup check; 53 workloads were timed.

## internal

`startup`

## applications

`expense_report`, `order_cleanup`, `api_report`, `stdout_lines`

## benchmark_game

`game_nbody`, `game_spectral_norm`

## pyperformance

`perf_barnes_hut`, `perf_float`, `perf_unpack_sequence`, `perf_json_loads`

## text

`text_line_logs_small`, `text_line_logs_medium`

## memory

`memory_string_index_short`, `memory_string_index_long`

## rustpython

`rustpython_mandelbrot`, `rustpython_pystone`

## rustpython_micro

`rustpython_micro_addition_100`, `rustpython_micro_addition_300`, `rustpython_micro_addition_500`, `rustpython_micro_addition_700`, `rustpython_micro_addition_900`, `rustpython_micro_cmp`, `rustpython_micro_comprehension_dict_100`, `rustpython_micro_comprehension_dict_300`, `rustpython_micro_comprehension_dict_500`, `rustpython_micro_comprehension_dict_700`, `rustpython_micro_comprehension_dict_900`, `rustpython_micro_comprehension_list_100`, `rustpython_micro_comprehension_list_300`, `rustpython_micro_comprehension_list_500`, `rustpython_micro_comprehension_list_700`, `rustpython_micro_comprehension_list_900`, `rustpython_micro_comprehension_set_100`, `rustpython_micro_comprehension_set_300`, `rustpython_micro_comprehension_set_500`, `rustpython_micro_comprehension_set_700`, `rustpython_micro_comprehension_set_900`, `rustpython_micro_list_count_builtins_100`, `rustpython_micro_list_count_builtins_300`, `rustpython_micro_list_count_builtins_500`, `rustpython_micro_list_count_builtins_700`, `rustpython_micro_list_count_builtins_900`, `rustpython_micro_loop_append_100`, `rustpython_micro_loop_append_300`, `rustpython_micro_loop_append_500`, `rustpython_micro_loop_append_700`, `rustpython_micro_loop_append_900`, `rustpython_micro_loop_string_100`, `rustpython_micro_loop_string_300`, `rustpython_micro_loop_string_500`, `rustpython_micro_loop_string_700`, `rustpython_micro_loop_string_900`, `rustpython_micro_strings`

## Rejected or mismatched

Six stdout mismatches remain unsupported: JSON dumps and five custom-equality list-count variants.

| Workload | Result | Detail |
| --- | --- | --- |
| capacity_planning | unsupported | NotImplementedError: unsupported: expression outside core subset |
| portfolio_risk | unsupported | NotImplementedError: unsupported: expression outside core subset |
| ticket_search | unsupported | NotImplementedError: unsupported: expression outside core subset |
| game_fannkuch | unsupported | RuntimeError: type error: index must be an integer |
| game_pidigits | unsupported | NotImplementedError: unsupported: only builtin math and json imports |
| game_regex_dna | unsupported | NotImplementedError: unsupported: only builtin math and json imports |
| game_meteor_contest | unsupported | NotImplementedError: unsupported: only builtin math and json imports |
| perf_json_dumps | mismatch | stdout differs from manifest |
| text_regex_logs_small | unsupported | NotImplementedError: unsupported: only builtin math and json imports |
| text_regex_logs_medium | unsupported | NotImplementedError: unsupported: only builtin math and json imports |
| text_csv_aggregate_small | unsupported | NotImplementedError: unsupported: statement outside core subset |
| text_csv_aggregate_medium | unsupported | NotImplementedError: unsupported: statement outside core subset |
| text_csv_cleanup_small | unsupported | NotImplementedError: unsupported: statement outside core subset |
| text_csv_cleanup_medium | unsupported | NotImplementedError: unsupported: statement outside core subset |
| text_json_parse_small | unsupported | NotImplementedError: unsupported: expression outside core subset |
| text_json_parse_medium | unsupported | NotImplementedError: unsupported: expression outside core subset |
| text_json_transform_small | unsupported | NotImplementedError: unsupported: builtin keyword arguments |
| text_json_transform_medium | unsupported | NotImplementedError: unsupported: builtin keyword arguments |
| text_json_lines_small | unsupported | NotImplementedError: unsupported: statement outside core subset |
| text_json_lines_medium | unsupported | NotImplementedError: unsupported: statement outside core subset |
| memory_string_list_short | unsupported | NotImplementedError: unsupported: expression outside core subset |
| memory_string_list_long | unsupported | NotImplementedError: unsupported: expression outside core subset |
| memory_customer_records | unsupported | NotImplementedError: unsupported: expression outside core subset |
| perf_richards | unsupported | NotImplementedError: unsupported: statement outside core subset |
| rustpython_base64 | unsupported | NotImplementedError: unsupported: only builtin math and json imports |
| rustpython_decimal_factorial | unsupported | NotImplementedError: unsupported: only builtin math and json imports |
| rustpython_deepcopy | unsupported | NotImplementedError: unsupported: only builtin math and json imports |
| rustpython_deltablue | unsupported | NotImplementedError: unsupported: function decorators |
| rustpython_generators | unsupported | NotImplementedError: unsupported: only builtin math and json imports |
| rustpython_nqueens | unsupported | NotImplementedError: unsupported: expression outside core subset |
| rustpython_pickle | unsupported | NotImplementedError: unsupported: only builtin math and json imports |
| rustpython_regex_effbot | unsupported | NotImplementedError: unsupported: only builtin math and json imports |
| rustpython_unpickle | unsupported | NotImplementedError: unsupported: only builtin math and json imports |
| rustpython_micro_complex_class | unsupported | NotImplementedError: unsupported: function decorators |
| rustpython_micro_frozenset_100 | unsupported | RuntimeError: NameError: frozenset |
| rustpython_micro_frozenset_300 | unsupported | RuntimeError: NameError: frozenset |
| rustpython_micro_frozenset_500 | unsupported | RuntimeError: NameError: frozenset |
| rustpython_micro_frozenset_700 | unsupported | RuntimeError: NameError: frozenset |
| rustpython_micro_frozenset_900 | unsupported | RuntimeError: NameError: frozenset |
| rustpython_micro_list_count_custom_eq_100 | mismatch | stdout differs from manifest |
| rustpython_micro_list_count_custom_eq_300 | mismatch | stdout differs from manifest |
| rustpython_micro_list_count_custom_eq_500 | mismatch | stdout differs from manifest |
| rustpython_micro_list_count_custom_eq_700 | mismatch | stdout differs from manifest |
| rustpython_micro_list_count_custom_eq_900 | mismatch | stdout differs from manifest |
| rustpython_micro_sorted_100 | unsupported | NotImplementedError: unsupported: only builtin math and json imports |
| rustpython_micro_sorted_300 | unsupported | NotImplementedError: unsupported: only builtin math and json imports |
| rustpython_micro_sorted_500 | unsupported | NotImplementedError: unsupported: only builtin math and json imports |
| rustpython_micro_sorted_700 | unsupported | NotImplementedError: unsupported: only builtin math and json imports |
| rustpython_micro_sorted_900 | unsupported | NotImplementedError: unsupported: only builtin math and json imports |
