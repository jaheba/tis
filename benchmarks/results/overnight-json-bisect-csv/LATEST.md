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

[Full results and validation](FINAL.md)
