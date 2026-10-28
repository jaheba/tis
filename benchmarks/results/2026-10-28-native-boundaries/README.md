# Native runtime boundaries, 2026-10-28

Feedback gate source: `f3a7088`.
Final effects/safepoint source: `10cc6857b3198be8346a2688d1dc7c599bffc344`.
The frozen baseline is the numeric-specialization worker (`9ca3888`; later commits added profiling tools and evidence).

The final paired comparison reduces native nbody time by 20.0%, copy-and-patch by 18.6%, and reference LIR by 17.1%.
Bytecode is approximately unchanged. This measures the 20,000-step nbody application, not general Python throughput.

| Tier | Before median (s) | Final median (s) | Speedup |
| --- | ---: | ---: | ---: |
| bytecode | 0.292195 | 0.290526 | 1.006× |
| lir | 0.554535 | 0.459952 | 1.206× |
| native | 0.385001 | 0.308115 | 1.250× |
| copy-patch | 0.424901 | 0.345873 | 1.228× |

Two incremental comparisons isolate the work:

- `feedback-gates.json`: cached feedback gates reduce native time from 0.383236s to 0.352912s,
  and copy-and-patch from 0.426579s to 0.398023s.
- `helper-effects.json`: final effects/safepoints versus the cached-gate worker reduce native time
  from 0.358683s to 0.310478s, and copy-and-patch from 0.395454s to 0.337610s.

Those comparisons ran separately; do not combine their medians into a single paired speedup.
The final table uses `comparison.json`, which compares baseline and final workers in the same interleaved run.
Copy-and-patch medians vary between the incremental and overall comparisons; both raw sets are retained.

Each comparison uses two warmups per worker and ten serial, randomly interleaved samples per label/tier.
Every sample restores the same body coordinates and velocities, times only `advance`, and checks energy `-0.169089263`
outside the timer. All timed samples report zero backend compilation and deoptimization.
Final native and copy-and-patch samples each report zero feedback probes and 400,003 root synchronizations.
No tests or builds ran during timed comparisons. Platform: Apple M1 Pro / macOS 27.0 ARM64, Rust 1.99.0,
default Cargo release profile. JSON files retain binary hashes, readiness, raw samples and quartiles.

## Execution changes

Native contexts cache the detailed-feedback gate, refreshing it after block probes and reentrant helpers.
Frozen callbacks avoid observer calls and unnecessary result-profile borrows.
Eager execution continues sampling, and a recursive guard failure can reopen the active parent's feedback window.

`Helper::effects()` now distinguishes Python heap allocation, Python calls and observable writes.
Rust scratch allocations do not trigger tracing GC. Exact non-allocating helpers retain complete deoptimization states
but have empty GC maps, skip root synchronization and do not run a collection checkpoint afterwards.
Unknown operations and generic iteration/call/attribute/format helpers retain conservative effects.
The verifier rejects understated effects and checks GC maps against the declared effects.
Both native backends share the new runtime behavior; reference LIR uses the same safepoint policy.

Allocating or calling helpers still publish complete live roots before executing and collect only after publishing results.
A fully rooted entry checkpoint is retained because argument binding and environment setup can allocate even when the body
cannot allocate. Without that entry check, repeated host calls to a scalar function could accumulate setup garbage.
Deoptimization reconstruction still uses the complete state, independently of each operation's GC map.

## Counter placement control

The first overall diagnostic (`layout-regression.json`) showed a 4% bytecode slowdown, despite unchanged bytecode execution.
Moving the new counters to the end of `TierStats` preserves the existing hot numeric-counter offsets.
`layout-control.json` then measured a 0.291401s candidate median, and the final overall comparison also has no bytecode regression.
This demonstrates sensitivity to statistics placement; it does not establish a specific microarchitectural cause.

## Profile after the changes

Two warmed ten-second native captures at 1 ms contain 16,141 samples in total.
The optimized debug-symbol build has the same runtime source as the final benchmark worker.
Every completed advance around capture reported zero compilation/deoptimization and preserved the expected energy.
Raw sample files, validated responses and demangled summaries are retained alongside the benchmark results.

Compared with `../2026-10-28-cpu-profiles`, exclusive native root-synchronization samples fall from 19.8% to 3.7%.
No samples remain in `observe_site_with`, consistent with zero measured observer calls.
Callback self samples are now 51.3% and callback-inclusive samples 88.8%.
The rising self percentage is relative to a smaller total and also contains inlined context work;
it is not evidence that callback glue became slower in absolute time.
Anonymous generated-code frames account for 7.9% exclusive samples; they do not isolate arithmetic instruction cost.
The next measured target is callback frequency: non-allocating sequence reads, truth and loop mechanics.
Further removing root checks is less promising after this change.

## Validation

All 135 tests pass in debug and release; formatting and all-target clippy pass.
Boundary regressions cover stable/eager feedback, recursive guard failure while a parent native artifact is active,
GC-map verification, references surviving mutation and later collections, and bounded setup garbage after 10,000
host calls to an allocation-free compiled body. Existing GC, side-exit, panic-boundary and nbody acceptance tests also pass.

Reproduce the overall benchmark with `tools/compare_nbody.py` using frozen before/after release workers.
Use `tools/profile_nbody.py --tiers native` with a debug-symbol release worker to reproduce the follow-up profiles.
