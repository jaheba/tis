# Flying Circus quick comparison

The current unchanged manifest has 103 workloads; tis passes 54, including the internal startup check.
The remaining results are 43 compile/runtime errors and six stdout mismatches, unchanged from the earlier audit.
All supported native runs reported zero compilation errors in the separate compatibility audit.
[Complete support list and failure details](SUPPORTED.md).

[Interactive report](quick/report.html) · [Full timing/RSS tables](quick/report.md) · [Raw results](quick/report.json)

## Run

The official Flying Circus `--quick` preset ran serially on all 53 supported non-internal workloads:
three timing samples per workload/runtime/mode, one repeated warmup, five startup samples and one RSS sample.
Both one-shot and repeated modes completed successfully across tis bytecode, tis native, CPython and PyPy.
That is 424 successful timing cells and 1,272 timed requests, with exact stdout validation.
All copied workload bytes and captured reference stdout match the original manifest.
The final run took 148.3 seconds, excluding report generation.

The machine was an Apple M1 Pro running macOS arm64.
Native tis enabled read-only and buffered mutation regions; bytecode used the normal interpreter.
Core source was commit `e2e931d`; the only new Rust code is the benchmark worker in `examples/circus.rs`.
Runtime versions were CPython 3.15.0 and PyPy 7.3.16 / Python 3.10.14.
The harness's Python was 3.14.2, with its existing dependencies.
No timing/RSS samples in the final run overlapped builds or tests.
An earlier exploratory run was excluded because a Clippy check overlapped one RSS sample.

Request timings include source compilation, JIT compilation, execution and output delivery.
Worker startup, fresh-session setup/reset and any marked fixture setup are excluded.
Repeated mode reuses a process but resets the application session each time; tis deliberately drops its VM and JIT artifacts.
These are not the retained-VM warmed function timings used in the earlier Barnes–Hut experiments.
Quick mode's three samples and one PyPy warmup do not establish stable performance or statistical significance.

## Repeated request medians

All times are milliseconds. The full report includes every workload and one-shot timings.

| Workload | tis bytecode | tis native | CPython | PyPy |
| --- | ---: | ---: | ---: | ---: |
| expense_report | 0.299 | 0.323 | 0.242 | 0.286 |
| order_cleanup | 0.562 | 0.530 | 0.240 | 0.305 |
| api_report | 0.507 | 0.471 | 0.252 | 0.345 |
| game_nbody | 246.464 | 33.780 | 77.647 | 14.200 |
| game_spectral_norm | 298.401 | 332.661 | 65.389 | 17.723 |
| perf_barnes_hut | 2496.334 | 587.464 | 625.559 | 365.832 |
| perf_float | 178.296 | 226.538 | 48.149 | 14.796 |
| perf_unpack_sequence | 192.881 | 2262.977 | 17.500 | 73.679 |
| perf_json_loads | 1.797 | 1.638 | 1.061 | 1.099 |
| text_line_logs_small | 0.508 | 0.501 | 0.296 | 0.442 |
| text_line_logs_medium | 21.931 | 28.588 | 8.101 | 14.732 |
| memory_string_index_short | 144.405 | 219.136 | 62.406 | 66.652 |
| memory_string_index_long | 155.777 | 234.313 | 75.757 | 77.899 |
| stdout_lines | 4.452 | 5.792 | 3.436 | 3.847 |
| rustpython_mandelbrot | 27.362 | 28.798 | 17.077 | 16.422 |
| rustpython_pystone | 462.463 | 673.004 | 61.486 | 41.456 |
| rustpython_micro_addition_900 | 0.247 | 0.280 | 0.123 | 0.280 |
| rustpython_micro_comprehension_list_900 | 0.318 | 0.334 | 0.108 | 0.199 |
| rustpython_micro_comprehension_dict_900 | 0.359 | 0.361 | 0.119 | 0.173 |
| rustpython_micro_comprehension_set_900 | 1.926 | 1.847 | 0.120 | 0.166 |
| rustpython_micro_list_count_builtins_900 | 0.059 | 0.049 | 0.086 | 0.086 |
| rustpython_micro_loop_append_900 | 0.318 | 0.324 | 0.129 | 0.299 |
| rustpython_micro_loop_string_900 | 0.317 | 0.318 | 0.073 | 0.146 |
| rustpython_micro_strings | 237.447 | 238.806 | 0.744 | 1.382 |

## What it shows

Native tis beats CPython on 6 of 53 repeated workloads: nbody, Barnes–Hut and four list-count variants.
It beats bytecode on 17; bytecode is faster on the other 36 in these samples.
The median native/CPython ratio across the selected workloads is 1.93×; the geometric mean is 2.47×.
There are 37 related microbenchmark variants, so this is not a balanced score across independent applications.
One-shot native tis beats CPython on 10 of 53 workloads; cold import costs differ from reused-process requests.

Nbody is a strong result: 33.8 ms native versus CPython's 77.6 ms, while PyPy takes 14.2 ms.
Barnes–Hut remains slightly ahead of CPython in this request-inclusive measurement, but the rest of the suite exposes
much broader weaknesses than the three bespoke benchmarks did.

The unpacking case takes 2.263 seconds natively versus 193 ms in bytecode and 17.5 ms in CPython.
Its native peak RSS is 943 MiB, versus 28.7 MiB in bytecode and 19.7 MiB in CPython.
A separate diagnostic reports 1.949 seconds in backend compilation and 2,263,392 bytes of generated code.
The source repeats a ten-item unpack 400 times inside a loop.
This calls for a compilation/code-growth budget and investigation of large-function IR/code expansion, rather than
assuming its timing is simply slow native execution.

The string microbenchmark only iterates over 50,000 ASCII characters, yet takes about 239 ms in both tis tiers versus
0.744 ms in CPython.
Code inspection finds that `next_inner` recomputes source length each step and delegates indexing to `Heap::get_item`;
string indexing recounts characters and uses `chars().nth(position)`.
That repeatedly rescans the string, producing quadratic iteration rather than a linear walk.
Fixing that shared runtime path should benefit both interpreter and JIT modes.

Not every native regression is explained by compilation: separate diagnostics for spectral norm and float report only
18.4 ms and 8.7 ms of backend compilation, respectively, alongside millions of runtime callbacks.
Those cases still need execution-path profiling.
Diagnostic CLI timings are separate from the measured worker requests and are not substituted into the tables.

The most concrete next targets are bounded compilation for oversized functions, linear string iteration, and then
profiling the numeric/object workloads that remain callback-heavy.
No interpreter or JIT optimization was made in this benchmarking task.

## Adapter and provenance

The local Flying Circus checkout did not recognize tis.
A temporary harness copy adds an honest `tis` runtime kind and a JSON worker using the Rust executable directly.
There is no CPython delegation, Monty-protocol impersonation, workload rewriting or reduced input size.
The original Flying Circus checkout was left unchanged.
The worker supports `--version`, empty `-c` startup, script execution, and JSON `--worker` requests.
Each configure/reset creates a new VM; fixture and measured requests share that VM within one session.
The adapter uses the same request/readiness/stdout validation and RSS machinery as the other runtimes.

The original harness tests plus the adapter integration tests pass (91 tests).
The Rust worker's fixture/reset/Unicode test passes, as do formatting and example Clippy with warnings denied.
`tis-harness-adapter.patch`, `harness-source-sha256.json`, `measured-circus.rs`, `command.json`, and source snapshots retain
what was measured.
The compatibility audit and targeted diagnostic stats are separate from the timed requests.
Temporary paths in raw metadata record actual run locations; the copied snapshots here retain the workload sources.
No commits or pushes were made for this task.
