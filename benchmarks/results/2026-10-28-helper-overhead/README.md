# Profiling n-body and removing helper overhead

The first native performance improvement comes from reducing helper overhead.
Arithmetic specialization already happens, but most surrounding operations still use the runtime.

## Evidence

A separate macOS `sample` capture of the baseline copy-and-patch tier collected 1,365 main-thread samples.
Its collapsed leaf counts include 204 in `observe_site`, 174 in the native callback and 156 in `sync_lir_roots`.
Allocator and helper-argument collection frames also appear among the leading leaf functions.
These are sample counts from a short, intrusive capture, not exact percentages of workload elapsed time.
Generated stencils lack symbols in this capture.
The complete capture is in `copy-patch.sample.txt.gz`.
A separate capture after the timed comparison is in `candidate-copy-patch.sample.txt.gz`.
Profile updates and callback execution remain its leading leaf functions; it is not an allocation-free execution path.

```sh
target/release/examples/run --tier copy-patch tests/fixtures/game_nbody.py &
task_pid=$!
sample "$task_pid" 2 1 -file copy-patch.sample.txt
wait "$task_pid"
```

`advance.lir.txt` is the first function dump from the existing `hotspots` example after running the fixture.
That diagnostic example uses 16-entry/128-backedge thresholds; timing workers use the default 1,000/1,000 thresholds.
The dump contains 48 name-load helper sites, 22 name-store helper sites and 30 specialized floating binary operations.
These are static counts, not execution frequencies.
For example, `dx = x1 - x2` becomes two name-load helpers, guards/unboxing, a floating subtraction, boxing and a store helper.
The arithmetic is native, but surrounding name access and bookkeeping still require callbacks.

```sh
cargo run --release --example hotspots -- tests/fixtures/game_nbody.py
```

## Change

Native helper arguments now reuse a vector owned by each native invocation instead of allocating one per helper call.
Recursive invocations own separate scratch vectors.
The existing transient roots still protect arguments while a helper executes.

Root-map membership is checked by LIR verification at installation and by native compilation.
The immutable maps no longer need a quadratic membership scan on every helper invocation in release builds.
A debug assertion retains the check in debug builds.
Root synchronization itself, profiling, speculative guards and deoptimization behavior remain enabled.

The change affects both native backends; moving validation also benefits the reference LIR interpreter.
All 86 tests pass, including GC during nested calls, recursive native execution, invalid root-map verification,
deoptimization and n-body acceptance in all four tiers.
The root-map regression also checks rejection by `install_lir` in release mode, where the per-helper assertion is absent.
Formatting and Clippy with warnings denied pass.

## Measurement

Apple M1 Pro, macOS 27.0, default Cargo release profile, Rust 1.99.0.
The baseline binary is the exact worker used in the preceding n-body report; its SHA-256 matches that report.
Baseline core revision is `12e5234a97a0619797f03a807c3bc1a1f5d5af3e`.
Candidate revision is `87980ab4ade219f2d933bdca59c1411168f183d2`.
`comparison.json` retains executable hashes, warmups, individual samples, validation data and quartiles.

Each worker performs two untimed warmups, then ten timed `advance(0.01, 20000)` calls.
Initial body positions and velocities are restored before each call, outside the timer.
Energy validation is also outside the timer.
Workers run serially in shuffled round order with seed 20261028.
Only one worker is active at a time; the others wait on stdin.
Every optimized timed call enters the requested tier once, with no backend compilation or deoptimization.
Bytecode is included as a control.
Builds, tests and sampling finish before timed runs begin.

| Tier | Baseline median | Candidate median | Time reduction | Candidate interquartile range |
| --- | ---: | ---: | ---: | ---: |
| Bytecode control | 2.265 s | 2.262 s | 0.15% | 2.258–2.266 s |
| Reference LIR | 2.565 s | 2.507 s | 2.27% | 2.504–2.517 s |
| Cranelift | 2.649 s | 2.166 s | 18.21% | 2.163–2.176 s |
| Copy-and-patch | 2.772 s | 2.346 s | 15.38% | 2.340–2.349 s |

Cranelift now takes about 4.2% less time than candidate bytecode on this workload.
Copy-and-patch takes about 3.7% more time than candidate bytecode.
The bytecode control moved by only 0.15%; that small difference is not evidence of a bytecode optimization.

This experiment measures the two cleanup changes together, so it does not attribute a separate speedup to each one.
It does not measure fresh-process latency.

## Next representation change

The LIR still shows a larger issue: ordinary locals live in string-keyed environments, and scalar results are boxed
and often guarded again at their next use.
The next architectural experiment should promote uncaptured function locals to HIR values with block parameters
at control-flow joins, while retaining explicit cells/environments for captured, global and nonlocal names.
That gives both bytecode and LIR fewer name operations and lets LIR retain scalar intermediates across arithmetic.
It must preserve unbound-local errors, closure mutation and side-exit reconstruction of live locals.

## Reproduce the comparison

Save a baseline worker before rebuilding the candidate:

```sh
cargo build --release --example nbody_bench
cp target/release/examples/nbody_bench /tmp/baseline-nbody-bench
# Apply the candidate change, then rebuild.
cargo build --release --example nbody_bench
python3 tools/compare_nbody.py \
  --baseline /tmp/baseline-nbody-bench \
  --candidate target/release/examples/nbody_bench \
  --output /tmp/tis-nbody-comparison
```

Use a new output directory.
Source and fixture licensing are retained in the preceding `2026-10-28-nbody` result directory.
