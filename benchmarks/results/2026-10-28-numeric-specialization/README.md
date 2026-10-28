# Numeric specialization, 2026-10-28

Source: `9ca3888c958a5d9b0379b8dd1dcde9a2be225be7`.
Baseline: frozen worker from the preceding register-bytecode implementation (`57ba478`, runtime code `f95d169`).

Feedback-driven register arithmetic reduces nbody interpreter time by 21.0%, or 1.265× faster.
The same-binary switch comparison confirms a 20.1% reduction independently of compiler layout changes.

| Tier | Previous median (s) | Adaptive median (s) | Speedup |
| --- | ---: | ---: | ---: |
| bytecode | 0.368452 | 0.291249 | 1.265× |
| lir | 0.545424 | 0.541489 | 1.007× |
| native | 0.381104 | 0.383823 | 0.993× |
| copy-patch | 0.438967 | 0.425548 | 1.032× |

`comparison.json` contains ten serial, randomly interleaved warm samples for every tier and label.
`control.json` compares the same candidate binary in bytecode mode with `--numeric disabled` and `--numeric adaptive`:
medians are 0.364642s and 0.291458s respectively (1.251× faster).
Both comparisons use two warmups per worker, restore the same body coordinates and velocities before every sample,
and time only `advance` with 20,000 iterations. Energy `-0.169089263` is validated outside the timer.
No tests or builds ran during timing. Platform: Apple M1 Pro, macOS 27.0 ARM64, Rust 1.99.0, default release profile.
Raw files retain worker hashes, readiness, counters, individual samples and quartiles.

All timed samples had zero backend compilation and deoptimization.
Each adaptive bytecode sample executed 5,400,000 numeric fast paths with zero type misses and arithmetic fallbacks.
Warmup installed 30 numeric plans across the VM. Detailed advance feedback remained bounded at 2,912 samples in one window,
matching the previous worker. Successful numeric execution increments VM counters without borrowing HIR feedback per operation.

The native and LIR tiers show no material improvement; the copy-and-patch difference is small and is not evidence that
numeric register specialization speeds up compiled code. Those tiers bypass the register fast paths in measured runs.
The interpreter is now faster than the current compiled tiers on this workload. This does not establish general throughput
or identify which remaining native helper, boxing or root-maintenance cost dominates.

## Execution

Completed staged profiling windows select exact monomorphic integer/float signatures with at least eight observations.
The original instruction buffer stays immutable; a per-instruction `Cell` cache holds each guarded numeric plan.
`Numeric` register instructions retain direct operands and a mapping to the original runtime operation for fallback.
Plans share floating-point arithmetic with the LIR/native/copy-and-patch scalar module.

Integer add/subtract/multiply retain overflow checks and comparisons use exact integer ordering.
Float and mixed numeric arithmetic retain result types, zero checks and power-domain behavior.
Mixed integer/float comparisons and integer power stay generic. Booleans fail exact integer guards and use generic semantics.
A type miss clears the site's plan before executing the original operation, records unexpected kinds and opens the existing
bounded profiling window. Arithmetic failures execute the original operation to produce its existing error.
Neither fallback restarts the function or repeats preceding effects. Code revision invalidation discards the caches.
Eager diagnostic profiling keeps generic execution and existing detailed feedback behavior.

`Vm.numeric = NumericMode::Disabled` and `--numeric disabled` select generic execution.
`TierStats.numeric_*` exposes installed plans, fast executions, type misses and arithmetic fallbacks.
Register disassembly includes installed plan annotations for warmed code.

Validation: all 129 tests pass in debug and release; formatting and all-target clippy pass.
Numeric tests compare all supported operators with generic execution, including NaNs, infinities, signed zero, overflow,
container inplace mutation, precise comparisons, error recovery, encoding switches, tier transitions and HIR invalidation.

Reproduce the controlled comparison using one release worker:

```sh
python3 tools/compare_nbody.py \
  --baseline /absolute/path/to/nbody_bench \
  --candidate /absolute/path/to/nbody_bench \
  --baseline-numeric disabled --candidate-numeric adaptive \
  --tiers bytecode --output /tmp/tis-numeric-control
```
