# SSA locals and native safepoints, 2026-10-28

Uncaptured function locals now become SSA values before bytecode generation.
The final change reduces n-body execution time in every tier and makes Cranelift faster than the bytecode interpreter
on this workload.

## Representation

Straight-line local loads and stores become direct value references.
Branches and loops pass current local values through block parameters, with parallel edge transfers.
Unchanged loop-carried values and single-predecessor joins are collapsed.
Erased values are compacted before publication so bytecode/register buffers do not retain holes for removed local operations.
Captured names remain in shared environments; class/module namespaces and global/nonlocal behavior are unchanged.

An internal `Unbound` marker represents missing bindings across joins.
Definite-assignment analysis removes checks for proven bound reads; other reads retain `CheckLocal` and raise the original
`UnboundLocalError`.
The marker is distinct from Python `None`.
It is not accepted as a Python function argument and is rejected by representation, truth and dictionary-key operations.

Every HIR program point retains its local-binding map.
Liveness and LIR verification include these bindings, so a heap object stored only in a promoted local remains a GC root.
Side exits reconstruct locals from SSA register locations, including which bindings are unbound.
Prologue bindings that have not yet loaded their parameter can still use their initial frame slot.
Mutable slot stores cannot be inserted into already promoted HIR.

Within a block, typed LIR reuses primitive registers for previously guarded values and arithmetic results.
For `dx = a - b; product = dx * dx; return product + a`, three floating operations need only two float guards and two unboxes.
The primitive `dx` flows directly into multiplication.
Boxed shadow values remain in the native register image for reconstruction; this is not fully deferred boxing.
Reuse across helpers is safe because scalar SSA values are immutable.

## Safepoint execution

Detailed profiling reads actual backend registers instead of requiring every scalar to be copied into the VM root stack.
Safepoints publish heap references to the collector; scalars remain in register images for reconstruction.
Only helpers can produce boxed heap references in the current LIR.
Other boxed results are constants or boxed primitive arithmetic, so staged native code omits their finish callbacks.
Block-entry and helper callbacks retain root publication and collection checkpoints.

Eager diagnostic mode keeps scalar-result callbacks and their complete type/fast-path observations.
Switching an existing VM to eager mode recompiles an artifact that omitted those callbacks.
Active code remains owned through recursive calls and retirement.
Default native code keeps tier-entry, backedge, guard and side-exit counts without per-scalar finish callbacks.

## Warm n-body results

Apple M1 Pro, macOS 27.0, AC power, Rust 1.99.0, default Cargo release profile.
Baseline core revision: `e9c9a1eb0b56f88423164785ead5c32839a973a8`.
Its executable is the exact candidate retained from the staged-profiling experiment; SHA-256 hashes match.
Final candidate revision: `2f729ae14e1803408bba6ab56a57c7b8584c63c4`.

Each runtime performs two warmups and ten timed `advance(0.01, 20000)` calls.
Workers run serially in shuffled round order, seed 20261028.
Scene restoration, energy validation, source compilation and process startup are outside the timer.
All samples produce the expected energy; optimized samples each enter their tier once with no compilation or
deoptimization during timing.
Builds and tests finish before timed samples begin.

| Tier | Staged slot baseline | Final SSA candidate | Time reduction | Candidate interquartile range |
| --- | ---: | ---: | ---: | ---: |
| Bytecode | 1.177 s | 1.079 s | 8.31% | 1.070–1.082 s |
| Reference LIR | 1.804 s | 1.461 s | 19.01% | 1.447–1.475 s |
| Cranelift | 1.309 s | 0.957 s | 26.88% | 0.949–0.967 s |
| Copy-and-patch | 1.454 s | 1.068 s | 26.57% | 1.061–1.091 s |

Cranelift takes about 11% less time than candidate bytecode.
Copy-and-patch and bytecode are close, with overlapping interquartile ranges.
These numbers measure the combined SSA, primitive-reuse and root/callback changes, not an isolated attribution to each one.
They cover one workload on one machine.
SSA and reconstruction-map construction add frontend compilation work, which these warm timings exclude.
Raw readiness records retain setup and compilation observations, but those single observations are not latency distributions.

`comparison.json` retains all samples, hashes, warmups and summaries.
The candidate retains one profiling window and 2,912 sampled site executions for `advance`, versus 5,016 in the baseline.
The phase remains `Ready` for bytecode and `Compiled` for the optimized tiers after warmup.

An earlier controlled implementation retained full scalar root-stack publication and all primitive finish callbacks.
It improved bytecode but regressed native execution: Cranelift measured 1.391 s versus a 1.278 s baseline, and copy-and-patch
1.534 s versus 1.374 s in that separate run.
`full-scalar-publication-comparison.json` retains that experiment.
The final comparison uses its own contemporaneous baseline and includes the safepoint changes described above.

## Inspect and reproduce

`chain.py`, `slot-chain.hir.txt` and `ssa-chain.hir.txt` show the same source before and after promotion.
`advance.lir.txt` is the first n-body dump from the `hotspots` example, using its diagnostic 16-entry/128-backedge thresholds.
It contains four prologue parameter loads, no local stores, 23 float guards and 30 floating binary operations.
It also retains 30 boxed arithmetic shadows.
These are static counts, not execution frequencies.

```sh
cargo build --release --examples
target/release/examples/ir --source chain.py --function chain --format text --no-promote
target/release/examples/ir --source chain.py --function chain --format text
python3 tools/compare_nbody.py \
  --baseline /path/to/staged-profile-nbody-bench \
  --candidate target/release/examples/nbody_bench \
  --output /tmp/tis-ssa-results
```

From the repository root, use this result directory's path for `chain.py`.
Use a fresh output directory and keep builds/tests separate from timing.
`CompileOptions { promote_locals: false }` also retains slot-based HIR for Rust experiments.
The n-body fixture and its license remain in `../2026-10-28-nbody/`.

## Validation

All 110 tests pass in debug and release builds; formatting and Clippy with warnings denied pass.
After adding Python `None` to the detached reconstruction case, the SSA tests were repeated in both profiles.
New tests cover scalar reuse, joins and parallel swaps, `break`/`continue`, zero-iteration loops, unbound locals,
bound/unbound/None reconstruction, missing local roots, allocation during helper calls, type deoptimization and eager-mode
artifact rebuilding.
Differential tests compare promoted and unpromoted HIR output/errors across all four tiers for nested control flow,
partial assignments and aliased containers.
