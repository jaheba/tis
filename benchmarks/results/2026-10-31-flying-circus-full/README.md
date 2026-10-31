# Full Flying Circus — 2026-10-31

All 120 public workloads from the current Flying Circus checkout ran at their default full sizes, serially with `--quick`. The internal protocol workload is excluded by the harness. This includes 18 Unicode workloads absent from the earlier 102-workload run. No runtime changes were made for this measurement.

Three timing samples, one repeated warmup, five startup samples and one separate RSS sample were collected. Both timing modes include compilation, execution and output delivery; worker readiness, fixture preparation and reset are excluded. Repeated requests reuse a process but create a fresh VM session. These are not retained-VM warmed JIT measurements. Three samples and one PyPy warmup give rough comparisons, not statistically established rankings.

## Coverage

| Runtime | One-shot pass / unsupported / failed | Repeated pass / unsupported / failed |
| --- | ---: | ---: |
| tis-bytecode | 88 / 17 / 15 | 88 / 17 / 15 |
| tis-cnp | 88 / 17 / 15 | 88 / 17 / 15 |
| tis-cranelift | 88 / 17 / 15 | 88 / 17 / 15 |
| monty | 110 / 6 / 4 | 110 / 6 / 4 |
| cpython | 120 / 0 / 0 | 120 / 0 / 0 |
| pypy | 120 / 0 / 0 | 120 / 0 / 0 |

Failures remain failures, including missing builtins or methods reported as runtime errors. Unsupported and failed cases are excluded from paired timing aggregates. Successful samples are checked against the workload expected output.

## Paired timing comparisons

Ratios below 1 favor the first runtime. Each geometric mean uses only workloads passing in both runtimes in that mode; each workload has equal weight. Large microbenchmark and compilation effects mean this is not an application-wide speed score.

| Comparison | Mode | Common passes | Geometric mean ratio | First faster |
| --- | --- | ---: | ---: | ---: |
| tis-bytecode / monty | one-shot | 87 | 1.307 | 21 |
| tis-bytecode / monty | repeated | 87 | 1.379 | 14 |
| tis-cnp / tis-cranelift | one-shot | 88 | 0.999 | 49 |
| tis-cnp / tis-cranelift | repeated | 88 | 0.999 | 48 |
| tis-cnp / cpython | one-shot | 88 | 2.171 | 12 |
| tis-cnp / cpython | repeated | 88 | 2.105 | 17 |
| tis-cranelift / cpython | one-shot | 88 | 2.174 | 9 |
| tis-cranelift / cpython | repeated | 88 | 2.106 | 17 |
| tis-cranelift / pypy | one-shot | 88 | 1.944 | 17 |
| tis-cranelift / pypy | repeated | 88 | 1.569 | 41 |

## Selected workloads

Each cell is one-shot / repeated milliseconds. Compilation is included in both numbers.

| Workload | tis-bytecode | tis-cnp | tis-cranelift | monty | cpython | pypy |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| game_nbody | 286.401 / 282.136 | 44.990 / 43.802 | 32.861 / 31.734 | 297.187 / 290.464 | 80.061 / 80.108 | 14.522 / 14.556 |
| perf_barnes_hut | 3113.573 / 3139.845 | 3048.879 / 3003.653 | 677.518 / 662.741 | 4289.928 / 4160.044 | 640.863 / 634.741 | 340.400 / 411.459 |
| game_spectral_norm | 295.611 / 303.392 | 448.648 / 437.573 | 436.525 / 423.951 | 264.864 / 267.109 | 68.543 / 66.320 | 19.151 / 18.974 |
| text_json_parse_medium | 11.310 / 11.169 | 11.963 / 11.026 | 11.577 / 11.536 | 10.960 / 9.609 | 9.142 / 8.991 | 6.624 / 6.781 |
| text_csv_cleanup_medium | 892.141 / 896.300 | 1454.519 / 1423.454 | 789.537 / 755.124 | 686.581 / 691.873 | 156.482 / 156.537 | 150.624 / 135.629 |
| ticket_search | 40.169 / 38.470 | 61.346 / 58.239 | 80.271 / 81.459 | 23.916 / 22.723 | 11.024 / 6.703 | 14.885 / 14.808 |

## Startup and RSS

Startup is empty-command launch through exit, separate from request timing. RSS is whole-process peak including startup, measured in separate one-shot runs. The table gives the median of successful workload peak RSS values; it is not peak memory for a single combined suite.

| Runtime | Startup ms | Median workload peak RSS MiB |
| --- | ---: | ---: |
| tis-bytecode | 4.351 | 3.8 |
| tis-cnp | 3.969 | 4.1 |
| tis-cranelift | 4.125 | 4.2 |
| monty | 6.671 | 10.4 |
| cpython | 25.153 | 15.7 |
| pypy | 27.934 | 33.4 |

## Coverage gaps

The following table shows one-shot gaps. Full per-mode errors and raw samples remain in the external report.

| Workload | tis bytecode | tis CnP | tis Cranelift | Monty |
| --- | --- | --- | --- | --- |
| game_pidigits | unsupported: NotImplementedError: unsupported: only builtin math, json and bisect imports | unsupported: NotImplementedError: unsupported: only builtin math, json and bisect imports | unsupported: NotImplementedError: unsupported: only builtin math, json and bisect imports | unsupported: NotImplementedError: The monty syntax parser does not yet support yield expressions |
| game_regex_dna | unsupported: NotImplementedError: unsupported: only builtin math, json and bisect imports | unsupported: NotImplementedError: unsupported: only builtin math, json and bisect imports | unsupported: NotImplementedError: unsupported: only builtin math, json and bisect imports | failed: ['Expected complete, received function_call: function_call {\n  function_name: "bytearray"\n  values {\n  }\n  position {\n    filename: "<python-input-0>"\n    start: 3604\n    end: 3615\n  }\n}\ntotal_execution_micros: 923\nmax_suspensions: 1000\nfeed_execution_micros: 923\n'] |
| game_meteor_contest | failed: ['RuntimeError: NameError: frozenset'] | failed: ['RuntimeError: NameError: frozenset'] | failed: ['RuntimeError: NameError: frozenset'] | ok |
| text_regex_logs_small | unsupported: NotImplementedError: unsupported: only builtin math, json and bisect imports | unsupported: NotImplementedError: unsupported: only builtin math, json and bisect imports | unsupported: NotImplementedError: unsupported: only builtin math, json and bisect imports | ok |
| text_regex_logs_medium | unsupported: NotImplementedError: unsupported: only builtin math, json and bisect imports | unsupported: NotImplementedError: unsupported: only builtin math, json and bisect imports | unsupported: NotImplementedError: unsupported: only builtin math, json and bisect imports | ok |
| rustpython_base64 | unsupported: NotImplementedError: unsupported: only builtin math, json and bisect imports | unsupported: NotImplementedError: unsupported: only builtin math, json and bisect imports | unsupported: NotImplementedError: unsupported: only builtin math, json and bisect imports | ok |
| rustpython_decimal_factorial | unsupported: NotImplementedError: unsupported: only builtin math, json and bisect imports | unsupported: NotImplementedError: unsupported: only builtin math, json and bisect imports | unsupported: NotImplementedError: unsupported: only builtin math, json and bisect imports | ok |
| rustpython_deepcopy | unsupported: NotImplementedError: unsupported: only builtin math, json and bisect imports | unsupported: NotImplementedError: unsupported: only builtin math, json and bisect imports | unsupported: NotImplementedError: unsupported: only builtin math, json and bisect imports | unsupported: NotImplementedError: The monty syntax parser does not yet support class inheritance and metaclasses |
| rustpython_deltablue | failed: ['RuntimeError: type error: base must be class'] | failed: ['RuntimeError: type error: base must be class'] | failed: ['RuntimeError: type error: base must be class'] | unsupported: NotImplementedError: The monty syntax parser does not yet support class inheritance and metaclasses |
| rustpython_generators | unsupported: NotImplementedError: unsupported: only builtin math, json and bisect imports | unsupported: NotImplementedError: unsupported: only builtin math, json and bisect imports | unsupported: NotImplementedError: unsupported: only builtin math, json and bisect imports | unsupported: NotImplementedError: The monty syntax parser does not yet support yield from expressions |
| rustpython_nqueens | unsupported: NotImplementedError: unsupported: expression outside core subset | unsupported: NotImplementedError: unsupported: expression outside core subset | unsupported: NotImplementedError: unsupported: expression outside core subset | unsupported: NotImplementedError: The monty syntax parser does not yet support yield expressions |
| rustpython_pickle | unsupported: NotImplementedError: unsupported: only builtin math, json and bisect imports | unsupported: NotImplementedError: unsupported: only builtin math, json and bisect imports | unsupported: NotImplementedError: unsupported: only builtin math, json and bisect imports | failed: ['Expected complete, received os_call: os_call {\n  position {\n    filename: "<python-input-0>"\n    start: 154\n    end: 167\n  }\n}\ntotal_execution_micros: 24\nmax_suspensions: 1000\nfeed_execution_micros: 24\n'] |
| rustpython_regex_effbot | unsupported: NotImplementedError: unsupported: only builtin math, json and bisect imports | unsupported: NotImplementedError: unsupported: only builtin math, json and bisect imports | unsupported: NotImplementedError: unsupported: only builtin math, json and bisect imports | failed: ["AttributeError: 'function' object has no attribute 'data' and no __dict__ for setting new attributes"] |
| rustpython_unpickle | unsupported: NotImplementedError: unsupported: only builtin math, json and bisect imports | unsupported: NotImplementedError: unsupported: only builtin math, json and bisect imports | unsupported: NotImplementedError: unsupported: only builtin math, json and bisect imports | failed: ['Expected complete, received os_call: os_call {\n  position {\n    filename: "<python-input-0>"\n    start: 156\n    end: 169\n  }\n}\ntotal_execution_micros: 17\nmax_suspensions: 1000\nfeed_execution_micros: 17\n'] |
| rustpython_micro_complex_class | ok | ok | ok | unsupported: NotImplementedError: The monty syntax parser does not yet support method decorators (classmethod/staticmethod/property) |
| rustpython_micro_frozenset_100 | failed: ['RuntimeError: NameError: frozenset'] | failed: ['RuntimeError: NameError: frozenset'] | failed: ['RuntimeError: NameError: frozenset'] | ok |
| rustpython_micro_frozenset_300 | failed: ['RuntimeError: NameError: frozenset'] | failed: ['RuntimeError: NameError: frozenset'] | failed: ['RuntimeError: NameError: frozenset'] | ok |
| rustpython_micro_frozenset_500 | failed: ['RuntimeError: NameError: frozenset'] | failed: ['RuntimeError: NameError: frozenset'] | failed: ['RuntimeError: NameError: frozenset'] | ok |
| rustpython_micro_frozenset_700 | failed: ['RuntimeError: NameError: frozenset'] | failed: ['RuntimeError: NameError: frozenset'] | failed: ['RuntimeError: NameError: frozenset'] | ok |
| rustpython_micro_frozenset_900 | failed: ['RuntimeError: NameError: frozenset'] | failed: ['RuntimeError: NameError: frozenset'] | failed: ['RuntimeError: NameError: frozenset'] | ok |
| rustpython_micro_sorted_100 | unsupported: NotImplementedError: unsupported: only builtin math, json and bisect imports | unsupported: NotImplementedError: unsupported: only builtin math, json and bisect imports | unsupported: NotImplementedError: unsupported: only builtin math, json and bisect imports | ok |
| rustpython_micro_sorted_300 | unsupported: NotImplementedError: unsupported: only builtin math, json and bisect imports | unsupported: NotImplementedError: unsupported: only builtin math, json and bisect imports | unsupported: NotImplementedError: unsupported: only builtin math, json and bisect imports | ok |
| rustpython_micro_sorted_500 | unsupported: NotImplementedError: unsupported: only builtin math, json and bisect imports | unsupported: NotImplementedError: unsupported: only builtin math, json and bisect imports | unsupported: NotImplementedError: unsupported: only builtin math, json and bisect imports | ok |
| rustpython_micro_sorted_700 | unsupported: NotImplementedError: unsupported: only builtin math, json and bisect imports | unsupported: NotImplementedError: unsupported: only builtin math, json and bisect imports | unsupported: NotImplementedError: unsupported: only builtin math, json and bisect imports | ok |
| rustpython_micro_sorted_900 | unsupported: NotImplementedError: unsupported: only builtin math, json and bisect imports | unsupported: NotImplementedError: unsupported: only builtin math, json and bisect imports | unsupported: NotImplementedError: unsupported: only builtin math, json and bisect imports | ok |
| agent_unicode_german_ascii_small | failed: ['RuntimeError: AttributeError: find'] | failed: ['RuntimeError: AttributeError: find'] | failed: ['RuntimeError: AttributeError: find'] | ok |
| agent_unicode_german_ascii_large | failed: ['RuntimeError: AttributeError: find'] | failed: ['RuntimeError: AttributeError: find'] | failed: ['RuntimeError: AttributeError: find'] | ok |
| agent_unicode_german_small | failed: ['RuntimeError: AttributeError: find'] | failed: ['RuntimeError: AttributeError: find'] | failed: ['RuntimeError: AttributeError: find'] | ok |
| agent_unicode_german_large | failed: ['RuntimeError: AttributeError: find'] | failed: ['RuntimeError: AttributeError: find'] | failed: ['RuntimeError: AttributeError: find'] | ok |
| agent_unicode_german_emoji_small | failed: ['RuntimeError: AttributeError: find'] | failed: ['RuntimeError: AttributeError: find'] | failed: ['RuntimeError: AttributeError: find'] | ok |
| agent_unicode_german_emoji_large | failed: ['RuntimeError: AttributeError: find'] | failed: ['RuntimeError: AttributeError: find'] | failed: ['RuntimeError: AttributeError: find'] | ok |
| agent_unicode_chinese_small | failed: ['RuntimeError: AttributeError: find'] | failed: ['RuntimeError: AttributeError: find'] | failed: ['RuntimeError: AttributeError: find'] | ok |
| agent_unicode_chinese_large | failed: ['RuntimeError: AttributeError: find'] | failed: ['RuntimeError: AttributeError: find'] | failed: ['RuntimeError: AttributeError: find'] | ok |

## Reproduction and provenance

The frozen worker uses current tis HEAD `d0f1aab` plus the uncommitted, validated Cranelift Boolean-region emitter and shared tests. Default release features; `codegen-units=1` for tis. The existing Rust Circus worker executes Python through tis, with no Python delegation. Native tiers enable read-only and mutation regions.

References: cached Monty 1.4.0 (`tis-comparison-0f243e6a-20261028`), CPython 3.15.0 and PyPy 7.3.16 (Python 3.10.14). Their version and platform differences are part of this comparison.

Raw evidence and runnable copied harness are stored outside Git at `/Users/jaheba/Library/Caches/tis-benchmarks/2026-10-31-flying-circus-full/`. It contains `run.py` (exact command), `provenance.json` (binary hashes and compiler flags), source/workload hashes, the frozen tis/Monty workers and full HTML/Markdown/JSON reports. CPython/PyPy paths and hashes are recorded; their installations were not copied.

[Full interactive report](/Users/jaheba/Library/Caches/tis-benchmarks/2026-10-31-flying-circus-full/results/report.html)

## All timing results

Each cell is one-shot / repeated milliseconds; non-passing status remains visible.

| Workload | tis-bytecode | tis-cnp | tis-cranelift | monty | cpython | pypy |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| expense_report | 0.422 / 0.336 | 0.356 / 0.236 | 0.375 / 0.240 | 0.329 / 0.191 | 1.232 / 0.331 | 0.320 / 0.254 |
| order_cleanup | 0.543 / 0.408 | 0.575 / 0.379 | 0.610 / 0.341 | 0.489 / 0.230 | 1.365 / 0.354 | 0.342 / 0.495 |
| api_report | 0.495 / 0.359 | 0.502 / 0.364 | 0.530 / 0.342 | 0.511 / 0.217 | 1.271 / 0.295 | 0.353 / 0.476 |
| capacity_planning | 23.688 / 22.390 | 41.038 / 40.063 | 34.180 / 32.357 | 33.282 / 32.612 | 9.971 / 9.079 | 3.791 / 3.763 |
| portfolio_risk | 64.376 / 63.117 | 113.030 / 110.665 | 113.003 / 110.809 | 65.534 / 63.337 | 52.741 / 51.424 | 10.013 / 10.424 |
| ticket_search | 40.169 / 38.470 | 61.346 / 58.239 | 80.271 / 81.459 | 23.916 / 22.723 | 11.024 / 6.703 | 14.885 / 14.808 |
| game_fannkuch | 1822.024 / 1815.061 | 2710.201 / 2750.989 | 2559.639 / 2498.476 | 1212.854 / 1216.672 | 275.810 / 266.666 | 85.775 / 84.901 |
| game_nbody | 286.401 / 282.136 | 44.990 / 43.802 | 32.861 / 31.734 | 297.187 / 290.464 | 80.061 / 80.108 | 14.522 / 14.556 |
| game_pidigits | unsupported / unsupported | unsupported / unsupported | unsupported / unsupported | unsupported / unsupported | 289.748 / 287.414 | 136.603 / 133.048 |
| game_spectral_norm | 295.611 / 303.392 | 448.648 / 437.573 | 436.525 / 423.951 | 264.864 / 267.109 | 68.543 / 66.320 | 19.151 / 18.974 |
| game_regex_dna | unsupported / unsupported | unsupported / unsupported | unsupported / unsupported | failed / failed | 258.044 / 252.746 | 183.881 / 143.776 |
| game_meteor_contest | failed / failed | failed / failed | failed / failed | 354.489 / 348.303 | 83.134 / 81.314 | 75.818 / 70.246 |
| perf_barnes_hut | 3113.573 / 3139.845 | 3048.879 / 3003.653 | 677.518 / 662.741 | 4289.928 / 4160.044 | 640.863 / 634.741 | 340.400 / 411.459 |
| perf_float | 247.185 / 240.907 | 278.558 / 271.901 | 311.617 / 304.572 | 265.711 / 254.869 | 52.696 / 50.264 | 16.339 / 16.446 |
| perf_unpack_sequence | 153.475 / 150.255 | 206.897 / 208.619 | 209.792 / 204.725 | 49.137 / 49.341 | 18.872 / 17.852 | 47.590 / 47.469 |
| perf_json_dumps | 8.942 / 8.398 | 9.380 / 8.774 | 12.403 / 11.305 | 11.326 / 10.712 | 11.161 / 10.321 | 34.440 / 17.116 |
| perf_json_loads | 1.309 / 1.084 | 1.229 / 1.050 | 1.214 / 1.237 | 1.154 / 0.917 | 2.263 / 1.226 | 1.030 / 1.074 |
| text_line_logs_small | 0.535 / 0.431 | 0.545 / 0.465 | 0.518 / 0.453 | 0.439 / 0.320 | 0.292 / 0.322 | 0.967 / 0.437 |
| text_line_logs_medium | 22.680 / 22.918 | 31.641 / 31.732 | 33.167 / 33.660 | 15.415 / 16.246 | 8.358 / 8.036 | 15.309 / 31.998 |
| text_regex_logs_small | unsupported / unsupported | unsupported / unsupported | unsupported / unsupported | 0.885 / 0.792 | 0.537 / 0.480 | 1.952 / 0.741 |
| text_regex_logs_medium | unsupported / unsupported | unsupported / unsupported | unsupported / unsupported | 26.145 / 28.800 | 15.930 / 15.341 | 23.710 / 21.391 |
| text_csv_aggregate_small | 6.187 / 6.003 | 9.222 / 8.848 | 11.276 / 10.896 | 3.911 / 3.507 | 1.406 / 1.467 | 6.709 / 6.230 |
| text_csv_aggregate_medium | 466.154 / 447.555 | 743.818 / 736.007 | 358.283 / 347.672 | 344.919 / 334.967 | 80.519 / 81.133 | 67.322 / 56.259 |
| text_csv_cleanup_small | 10.529 / 10.064 | 15.329 / 15.553 | 14.461 / 13.225 | 7.427 / 7.137 | 2.231 / 2.194 | 11.345 / 10.652 |
| text_csv_cleanup_medium | 892.141 / 896.300 | 1454.519 / 1423.454 | 789.537 / 755.124 | 686.581 / 691.873 | 156.482 / 156.537 | 150.624 / 135.629 |
| text_json_parse_small | 0.359 / 0.272 | 0.391 / 0.279 | 0.371 / 0.293 | 0.462 / 0.252 | 0.262 / 0.314 | 0.250 / 0.373 |
| text_json_parse_medium | 11.310 / 11.169 | 11.963 / 11.026 | 11.577 / 11.536 | 10.960 / 9.609 | 9.142 / 8.991 | 6.624 / 6.781 |
| text_json_transform_small | 0.535 / 0.401 | 0.507 / 0.436 | 0.533 / 0.481 | 0.618 / 0.372 | 0.344 / 0.404 | 0.363 / 0.547 |
| text_json_transform_medium | 15.341 / 14.963 | 16.761 / 16.027 | 25.162 / 24.336 | 12.117 / 11.438 | 9.660 / 9.401 | 10.969 / 10.962 |
| text_json_lines_small | 0.513 / 0.375 | 0.526 / 0.430 | 0.518 / 0.406 | 0.556 / 0.351 | 0.428 / 0.421 | 0.867 / 0.598 |
| text_json_lines_medium | 17.612 / 18.468 | 22.744 / 23.058 | 28.693 / 29.511 | 15.261 / 14.644 | 15.220 / 15.425 | 15.906 / 23.134 |
| memory_string_list_short | 319.880 / 317.787 | 544.510 / 539.828 | 533.955 / 532.590 | 180.460 / 171.873 | 91.811 / 89.671 | 58.015 / 67.112 |
| memory_string_list_long | 373.957 / 364.475 | 659.716 / 626.335 | 648.416 / 650.101 | 243.275 / 255.874 | 97.355 / 95.313 | 77.007 / 82.833 |
| memory_string_index_short | 104.583 / 101.480 | 182.189 / 190.018 | 179.731 / 183.210 | 83.793 / 81.802 | 61.606 / 66.110 | 79.484 / 65.559 |
| memory_string_index_long | 135.907 / 137.238 | 211.059 / 217.100 | 214.315 / 207.922 | 127.506 / 124.918 | 76.128 / 77.118 | 83.061 / 66.264 |
| memory_customer_records | 919.910 / 940.749 | 1180.150 / 1230.107 | 1236.074 / 1196.206 | 325.370 / 370.500 | 150.777 / 172.843 | 134.958 / 118.880 |
| stdout_lines | 4.160 / 4.281 | 4.843 / 4.399 | 5.959 / 5.190 | 2.205 / 1.964 | 4.395 / 3.274 | 5.057 / 5.262 |
| perf_richards | 288.334 / 291.121 | 430.611 / 418.181 | 443.577 / 435.702 | 193.934 / 198.245 | 31.203 / 30.102 | 51.322 / 51.029 |
| rustpython_base64 | unsupported / unsupported | unsupported / unsupported | unsupported / unsupported | 1.840 / 1.803 | 5.252 / 0.965 | 23.837 / 9.425 |
| rustpython_decimal_factorial | unsupported / unsupported | unsupported / unsupported | unsupported / unsupported | 1.796 / 1.633 | 5.537 / 1.140 | 90.794 / 23.133 |
| rustpython_deepcopy | unsupported / unsupported | unsupported / unsupported | unsupported / unsupported | unsupported / unsupported | 5.091 / 2.788 | 50.582 / 21.272 |
| rustpython_deltablue | failed / failed | failed / failed | failed / failed | unsupported / unsupported | 6.014 / 5.379 | 14.421 / 14.833 |
| rustpython_generators | unsupported / unsupported | unsupported / unsupported | unsupported / unsupported | unsupported / unsupported | 77.918 / 75.651 | 163.785 / 175.412 |
| rustpython_mandelbrot | 28.413 / 27.709 | 28.713 / 28.803 | 29.542 / 28.945 | 19.612 / 18.942 | 17.465 / 17.959 | 16.614 / 15.785 |
| rustpython_nqueens | unsupported / unsupported | unsupported / unsupported | unsupported / unsupported | unsupported / unsupported | 59.473 / 58.132 | 42.845 / 41.110 |
| rustpython_pickle | unsupported / unsupported | unsupported / unsupported | unsupported / unsupported | failed / failed | 15.560 / 10.077 | 43.687 / 15.167 |
| rustpython_pystone | 500.291 / 492.834 | 839.796 / 816.002 | 721.541 / 717.598 | 327.755 / 322.577 | 64.207 / 62.774 | 42.834 / 44.299 |
| rustpython_regex_effbot | unsupported / unsupported | unsupported / unsupported | unsupported / unsupported | failed / failed | 25.243 / 23.942 | 21.093 / 16.886 |
| rustpython_unpickle | unsupported / unsupported | unsupported / unsupported | unsupported / unsupported | failed / failed | 7.164 / 3.020 | 20.604 / 8.563 |
| rustpython_micro_addition_100 | 0.130 / 0.088 | 0.112 / 0.096 | 0.140 / 0.087 | 0.123 / 0.097 | 0.108 / 0.127 | 0.243 / 0.175 |
| rustpython_micro_addition_300 | 0.154 / 0.101 | 0.158 / 0.108 | 0.154 / 0.107 | 0.156 / 0.093 | 0.129 / 0.125 | 0.181 / 0.177 |
| rustpython_micro_addition_500 | 0.179 / 0.156 | 0.158 / 0.144 | 0.179 / 0.130 | 0.182 / 0.111 | 0.137 / 0.137 | 0.226 / 0.212 |
| rustpython_micro_addition_700 | 0.204 / 0.171 | 0.208 / 0.188 | 0.210 / 0.150 | 0.160 / 0.114 | 0.161 / 0.150 | 0.266 / 0.276 |
| rustpython_micro_addition_900 | 0.241 / 0.187 | 0.213 / 0.172 | 0.222 / 0.166 | 0.201 / 0.096 | 0.147 / 0.151 | 0.245 / 0.260 |
| rustpython_micro_cmp | 2.722 / 2.547 | 5.519 / 5.593 | 7.324 / 6.986 | 1.825 / 1.730 | 1.699 / 1.510 | 3.470 / 3.639 |
| rustpython_micro_complex_class | 0.196 / 0.195 | 0.186 / 0.106 | 0.195 / 0.124 | unsupported / unsupported | 0.182 / 0.183 | 0.199 / 0.273 |
| rustpython_micro_comprehension_dict_100 | 0.208 / 0.169 | 0.198 / 0.157 | 0.206 / 0.184 | 0.182 / 0.106 | 0.162 / 0.146 | 0.170 / 0.171 |
| rustpython_micro_comprehension_dict_300 | 0.240 / 0.191 | 0.254 / 0.166 | 0.237 / 0.205 | 0.237 / 0.134 | 0.141 / 0.171 | 0.188 / 0.210 |
| rustpython_micro_comprehension_dict_500 | 0.282 / 0.234 | 0.286 / 0.257 | 0.296 / 0.216 | 0.250 / 0.165 | 0.155 / 0.120 | 0.208 / 0.230 |
| rustpython_micro_comprehension_dict_700 | 0.328 / 0.299 | 0.332 / 0.275 | 0.307 / 0.269 | 0.220 / 0.171 | 0.180 / 0.164 | 0.200 / 0.201 |
| rustpython_micro_comprehension_dict_900 | 0.439 / 0.299 | 0.362 / 0.266 | 0.373 / 0.299 | 0.247 / 0.166 | 0.158 / 0.220 | 0.233 / 0.231 |
| rustpython_micro_comprehension_list_100 | 0.219 / 0.116 | 0.207 / 0.144 | 0.225 / 0.143 | 0.153 / 0.111 | 0.130 / 0.146 | 0.165 / 0.236 |
| rustpython_micro_comprehension_list_300 | 0.212 / 0.145 | 0.275 / 0.147 | 0.224 / 0.159 | 0.162 / 0.127 | 0.175 / 0.127 | 0.168 / 0.223 |
| rustpython_micro_comprehension_list_500 | 0.236 / 0.183 | 0.250 / 0.158 | 0.274 / 0.172 | 0.177 / 0.132 | 0.143 / 0.138 | 0.159 / 0.178 |
| rustpython_micro_comprehension_list_700 | 0.304 / 0.224 | 0.265 / 0.216 | 0.300 / 0.213 | 0.190 / 0.209 | 0.199 / 0.129 | 0.184 / 0.173 |
| rustpython_micro_comprehension_list_900 | 0.280 / 0.250 | 0.275 / 0.224 | 0.271 / 0.221 | 0.197 / 0.137 | 0.216 / 0.151 | 0.194 / 0.224 |
| rustpython_micro_comprehension_set_100 | 0.206 / 0.147 | 0.217 / 0.152 | 0.196 / 0.146 | 0.177 / 0.095 | 0.160 / 0.137 | 0.198 / 0.164 |
| rustpython_micro_comprehension_set_300 | 0.279 / 0.197 | 0.274 / 0.215 | 0.304 / 0.177 | 0.182 / 0.127 | 0.165 / 0.155 | 0.191 / 0.186 |
| rustpython_micro_comprehension_set_500 | 0.294 / 0.229 | 0.344 / 0.281 | 0.345 / 0.266 | 0.188 / 0.119 | 0.141 / 0.119 | 0.271 / 0.174 |
| rustpython_micro_comprehension_set_700 | 0.431 / 0.305 | 0.406 / 0.316 | 0.391 / 0.338 | 0.227 / 0.193 | 0.187 / 0.136 | 0.259 / 0.192 |
| rustpython_micro_comprehension_set_900 | 0.461 / 0.322 | 0.488 / 0.367 | 0.442 / 0.353 | 0.242 / 0.182 | 0.186 / 0.152 | 0.234 / 0.335 |
| rustpython_micro_frozenset_100 | failed / failed | failed / failed | failed / failed | 0.091 / 0.069 | 0.084 / 0.071 | 0.161 / 0.145 |
| rustpython_micro_frozenset_300 | failed / failed | failed / failed | failed / failed | 0.097 / 0.062 | 0.107 / 0.082 | 0.137 / 0.144 |
| rustpython_micro_frozenset_500 | failed / failed | failed / failed | failed / failed | 0.087 / 0.079 | 0.087 / 0.119 | 0.109 / 0.137 |
| rustpython_micro_frozenset_700 | failed / failed | failed / failed | failed / failed | 0.126 / 0.068 | 0.111 / 0.117 | 0.118 / 0.127 |
| rustpython_micro_frozenset_900 | failed / failed | failed / failed | failed / failed | 0.107 / 0.088 | 0.096 / 0.101 | 0.139 / 0.119 |
| rustpython_micro_list_count_builtins_100 | 0.092 / 0.074 | 0.094 / 0.064 | 0.091 / 0.061 | 0.099 / 0.075 | 0.107 / 0.084 | 0.166 / 0.113 |
| rustpython_micro_list_count_builtins_300 | 0.094 / 0.081 | 0.089 / 0.063 | 0.127 / 0.079 | 0.109 / 0.071 | 0.131 / 0.087 | 0.110 / 0.085 |
| rustpython_micro_list_count_builtins_500 | 0.095 / 0.062 | 0.096 / 0.063 | 0.108 / 0.062 | 0.119 / 0.082 | 0.185 / 0.088 | 0.113 / 0.133 |
| rustpython_micro_list_count_builtins_700 | 0.122 / 0.075 | 0.102 / 0.079 | 0.095 / 0.103 | 0.098 / 0.090 | 0.090 / 0.088 | 0.127 / 0.119 |
| rustpython_micro_list_count_builtins_900 | 0.109 / 0.091 | 0.125 / 0.091 | 0.107 / 0.072 | 0.124 / 0.082 | 0.090 / 0.091 | 0.125 / 0.082 |
| rustpython_micro_list_count_custom_eq_100 | 0.096 / 0.099 | 0.132 / 0.074 | 0.114 / 0.101 | 0.126 / 0.093 | 0.111 / 0.101 | 0.114 / 0.214 |
| rustpython_micro_list_count_custom_eq_300 | 0.150 / 0.115 | 0.173 / 0.118 | 0.176 / 0.130 | 0.162 / 0.127 | 0.088 / 0.098 | 0.130 / 0.507 |
| rustpython_micro_list_count_custom_eq_500 | 0.203 / 0.164 | 0.206 / 0.166 | 0.196 / 0.175 | 0.188 / 0.170 | 0.106 / 0.113 | 0.165 / 0.613 |
| rustpython_micro_list_count_custom_eq_700 | 0.262 / 0.220 | 0.228 / 0.192 | 0.228 / 0.194 | 0.227 / 0.212 | 0.126 / 0.121 | 0.189 / 0.754 |
| rustpython_micro_list_count_custom_eq_900 | 0.269 / 0.249 | 0.278 / 0.251 | 0.283 / 0.265 | 0.269 / 0.237 | 0.121 / 0.134 | 0.207 / 0.865 |
| rustpython_micro_loop_append_100 | 0.144 / 0.117 | 0.158 / 0.125 | 0.156 / 0.131 | 0.173 / 0.116 | 0.166 / 0.115 | 0.191 / 0.189 |
| rustpython_micro_loop_append_300 | 0.201 / 0.137 | 0.195 / 0.146 | 0.198 / 0.185 | 0.173 / 0.126 | 0.147 / 0.133 | 0.204 / 0.215 |
| rustpython_micro_loop_append_500 | 0.231 / 0.161 | 0.331 / 0.192 | 0.267 / 0.211 | 0.234 / 0.133 | 0.177 / 0.113 | 0.232 / 0.267 |
| rustpython_micro_loop_append_700 | 0.271 / 0.220 | 0.372 / 0.255 | 0.309 / 0.260 | 0.207 / 0.166 | 0.139 / 0.159 | 0.297 / 0.271 |
| rustpython_micro_loop_append_900 | 0.323 / 0.268 | 0.367 / 0.314 | 0.354 / 0.310 | 0.228 / 0.152 | 0.146 / 0.163 | 0.352 / 0.288 |
| rustpython_micro_loop_string_100 | 0.104 / 0.077 | 0.098 / 0.071 | 0.118 / 0.073 | 0.061 / 0.051 | 0.098 / 0.068 | 0.116 / 0.109 |
| rustpython_micro_loop_string_300 | 0.112 / 0.104 | 0.121 / 0.099 | 0.128 / 0.096 | 0.081 / 0.045 | 0.100 / 0.099 | 0.217 / 0.122 |
| rustpython_micro_loop_string_500 | 0.136 / 0.123 | 0.146 / 0.129 | 0.145 / 0.110 | 0.068 / 0.055 | 0.104 / 0.108 | 0.144 / 0.136 |
| rustpython_micro_loop_string_700 | 0.163 / 0.143 | 0.171 / 0.141 | 0.169 / 0.160 | 0.076 / 0.058 | 0.127 / 0.108 | 0.138 / 0.169 |
| rustpython_micro_loop_string_900 | 0.198 / 0.161 | 0.191 / 0.166 | 0.185 / 0.150 | 0.120 / 0.077 | 0.090 / 0.087 | 0.176 / 0.157 |
| rustpython_micro_sorted_100 | unsupported / unsupported | unsupported / unsupported | unsupported / unsupported | 0.220 / 0.197 | 0.267 / 0.211 | 0.295 / 1.180 |
| rustpython_micro_sorted_300 | unsupported / unsupported | unsupported / unsupported | unsupported / unsupported | 0.456 / 0.400 | 0.361 / 0.313 | 2.123 / 1.033 |
| rustpython_micro_sorted_500 | unsupported / unsupported | unsupported / unsupported | unsupported / unsupported | 0.657 / 0.603 | 0.472 / 0.467 | 2.489 / 1.131 |
| rustpython_micro_sorted_700 | unsupported / unsupported | unsupported / unsupported | unsupported / unsupported | 0.842 / 0.791 | 0.609 / 0.547 | 2.647 / 1.465 |
| rustpython_micro_sorted_900 | unsupported / unsupported | unsupported / unsupported | unsupported / unsupported | 1.061 / 1.090 | 0.737 / 0.668 | 2.118 / 1.527 |
| rustpython_micro_strings | 4.825 / 4.582 | 5.552 / 5.434 | 4.882 / 4.553 | 0.959 / 0.887 | 0.871 / 0.741 | 1.766 / 1.678 |
| text_unicode_access_ascii | 16.043 / 15.253 | 19.632 / 19.319 | 21.572 / 21.031 | 2.911 / 2.870 | 3.399 / 2.281 | 3.766 / 3.830 |
| memory_unicode_ascii | 9.012 / 9.622 | 12.357 / 11.913 | 14.954 / 15.447 | 5.283 / 5.839 | 5.341 / 7.322 | 8.765 / 10.928 |
| text_unicode_access_latin1 | 25.922 / 25.868 | 30.076 / 30.454 | 32.768 / 31.456 | 25.588 / 24.844 | 3.485 / 2.332 | 4.012 / 4.065 |
| memory_unicode_latin1 | 25.400 / 24.377 | 18.996 / 18.794 | 21.538 / 29.065 | 9.509 / 11.796 | 4.967 / 5.636 | 13.139 / 12.316 |
| text_unicode_access_cjk | 36.676 / 35.616 | 40.756 / 40.071 | 43.237 / 41.342 | 36.452 / 35.819 | 3.782 / 2.865 | 4.444 / 4.171 |
| memory_unicode_cjk | 34.268 / 36.995 | 24.052 / 34.317 | 32.177 / 35.796 | 11.629 / 20.899 | 12.436 / 7.097 | 17.089 / 16.683 |
| text_unicode_access_emoji | 46.536 / 47.224 | 50.940 / 50.378 | 52.810 / 52.637 | 46.767 / 45.733 | 3.728 / 2.839 | 4.037 / 4.491 |
| memory_unicode_emoji | 19.597 / 24.906 | 22.778 / 31.435 | 28.671 / 33.965 | 15.721 / 27.803 | 14.234 / 18.627 | 20.722 / 26.651 |
| text_unicode_access_mixed | 15.854 / 15.288 | 19.318 / 19.862 | 21.893 / 20.744 | 14.343 / 13.671 | 3.250 / 2.244 | 3.740 / 3.903 |
| memory_unicode_mixed | 10.043 / 11.582 | 12.461 / 17.404 | 17.052 / 19.940 | 7.089 / 9.262 | 12.820 / 28.147 | 6.928 / 7.052 |
| agent_unicode_german_ascii_small | failed / failed | failed / failed | failed / failed | 0.767 / 0.611 | 0.606 / 0.603 | 0.782 / 0.954 |
| agent_unicode_german_ascii_large | failed / failed | failed / failed | failed / failed | 4.823 / 4.885 | 4.401 / 4.355 | 5.952 / 6.590 |
| agent_unicode_german_small | failed / failed | failed / failed | failed / failed | 1.139 / 0.921 | 0.827 / 0.834 | 1.496 / 1.731 |
| agent_unicode_german_large | failed / failed | failed / failed | failed / failed | 9.808 / 9.315 | 6.979 / 6.736 | 14.834 / 14.998 |
| agent_unicode_german_emoji_small | failed / failed | failed / failed | failed / failed | 1.077 / 0.966 | 0.818 / 0.903 | 1.731 / 1.697 |
| agent_unicode_german_emoji_large | failed / failed | failed / failed | failed / failed | 9.743 / 9.294 | 7.242 / 7.589 | 14.283 / 14.218 |
| agent_unicode_chinese_small | failed / failed | failed / failed | failed / failed | 1.021 / 0.847 | 0.750 / 0.638 | 1.575 / 1.399 |
| agent_unicode_chinese_large | failed / failed | failed / failed | failed / failed | 9.077 / 7.931 | 4.583 / 4.656 | 12.145 / 12.186 |
