# Staged profiling, 2026-10-28

Detailed feedback now starts after hotspot detection and stops after a bounded observation window.
Guard failures record unexpected types immediately and can trigger bounded reprofiling and recompilation.

## Policy

The policy belongs to each stable HIR identity and revision; closures sharing code share its profile.
Artifact installation remains local to each VM.
A code revision resets the phase, observations and window budget.

| Phase | Work |
| --- | --- |
| Cold | Function-entry/return/error and loop-backedge counters; no detailed operand, result or call-target observations |
| Profiling | Collect types, call targets and block/edge visits, with per-site and per-window limits |
| Ready | Freeze detailed feedback; bytecode can continue or an enabled tier can compile the collected profile |
| Compiled | Freeze detailed feedback; retain execution, fast-path, guard-failure and side-exit counters |
| Retired | Keep generic execution after the automatic retry budget or a compilation failure |

Defaults remain 1,000 entries or 1,000 backedges for hotspot detection.
A hotspot opens a function-wide window, even during the first call to a long-running function.
The window lasts 128 completed calls or backedges, with up to 32 observations per site.
Completed calls allow recursive results to contribute before compilation freezes the profile.
Per-site limits also stop a polymorphic site from collecting indefinitely inside an active window.

A guard miss bypasses sampling and records the actual operand types before bytecode resumes the original operation.
Native/LIR side exits also record their reason.
A miss can open another window; observations accumulate so polymorphic operations lower generically.
At most three windows run per revision, including the initial window.
Further misses retire automatic compilation for that revision; explicit compilation remains available.
Existing adaptive bytecode addition continues its lightweight counters and reports its guard failures outside windows.

`Vm::profiling` exposes `ProfileConfig` with window, site and retry budgets.
`ProfileConfig::eager()` collects every observation for diagnostics and deliberate manual lowering experiments.
The IR demo and small compiler-comparison example use this explicit diagnostic mode.
Normal execution and the measurements below use staged profiling.
Site `executions` counts sampled executions, not lifetime instruction executions.
Function entries/returns/errors and backedges remain lifetime counters.
Argument and return types are sampled when calls enter or return during a window.

Guards and runtime helpers retain all Python semantic checks independently of profiling policy.
GC root synchronization, local-slot tracing and side-exit reconstruction are unchanged.

## Warm n-body results

Apple M1 Pro, macOS 27.0, AC power, Rust 1.99.0, default Cargo release builds.
The baseline is the exact indexed-local worker from the preceding experiment, with continuous site feedback.
Its SHA-256 matches that experiment's candidate.
Baseline core revision: `c8889abdd922186dfd627b64fb56e3c4383fe19f`.
Candidate revision: `e9c9a1eb0b56f88423164785ead5c32839a973a8`.

Each worker performs two untimed warmups and ten timed `advance(0.01, 20000)` calls.
Runtime order is shuffled per round with seed 20261028; workers run serially.
Initial body positions and velocities are restored before each call, outside timing.
Energy validation is outside timing and matches `-0.169089263` for every sample.
Every optimized sample enters its selected tier once, with no backend compilation or deoptimization during timing.
Builds, tests and other timed workloads finish before measurement begins.

| Tier | Continuous feedback | Staged profiling | Time reduction | Candidate interquartile range |
| --- | ---: | ---: | ---: | ---: |
| Bytecode | 1.598 s | 1.158 s | 27.50% | 1.155–1.161 s |
| Reference LIR | 2.131 s | 1.730 s | 18.84% | 1.725–1.751 s |
| Cranelift | 1.761 s | 1.305 s | 25.90% | 1.298–1.311 s |
| Copy-and-patch | 1.891 s | 1.393 s | 26.34% | 1.388–1.404 s |

The candidate retains one observation window and 5,016 sampled site executions for `advance` in every tier.
After warmup its phase is `Ready` in bytecode and `Compiled` in the other tiers.
This retains useful specialization metadata while avoiding continuous recording during steady execution.
Bytecode remains faster than either native backend; callback/root costs and boxed local helpers remain.
The measurements cover one workload on one machine and do not attribute time to individual internal functions.

`comparison.json` contains all samples, warmups, executable hashes, phase/window information and timing summaries.
Source and fixture licensing are retained in `../2026-10-28-nbody/`.

## Validation

All 102 release tests pass, including ten new profiling-policy tests.
The complete debug suite passed before the final call-target test and formatter change; affected debug tests were repeated
after those changes, including all ten policy tests.
Formatting and Clippy with warnings denied pass.

The new tests cover cold metadata absence, delayed compilation, frozen profiles, per-site limits, polymorphism,
bounded call targets, first-call loop OSR, guard-triggered recompilation without duplicated writes,
retry exhaustion, bytecode guard misses, code revisions and profiles shared across VMs.
Backend tests use short explicit windows; n-body acceptance uses the default 128-tick window.
Tests specifically inspecting complete type observations use the eager diagnostic policy.

## Reproduce

Save the baseline worker before rebuilding the candidate, then run from the repository root:

```sh
cargo build --release --example nbody_bench
python3 tools/compare_nbody.py \
  --baseline /path/to/continuous-feedback-nbody-bench \
  --candidate target/release/examples/nbody_bench \
  --output /tmp/tis-staged-profiling-results
```

Use a new output directory and keep builds/tests separate from timing.
