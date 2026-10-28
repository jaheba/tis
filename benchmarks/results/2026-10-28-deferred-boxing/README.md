# Deferred native float boxing

Implementation: `c8dccc9`. Baseline runtime: `d2a8168`.
Apple M1 Pro, macOS ARM64, release builds, 20,000-step nbody. Expected energy: `-0.169089263`.

## Result

No demonstrated native speedup. Ten fresh-process samples per binary, each warmed twice before the measured call:

| Native | Median | Range |
| --- | ---: | ---: |
| Before | 160.74 ms | 159.80–172.20 ms |
| After | 161.10 ms | 159.81–162.64 ms |

The after median is 0.2% slower. Earlier exploratory runs were also approximately flat. A final interleaved
steady-process comparison suggested a 6.4% improvement (171.92 to 160.88 ms), but its baseline process stayed
near 172 ms throughout. Fresh baseline processes reproduced both the 160 and 172 ms ranges. That improvement
is not robust across processes; code placement is a possible explanation, not an established cause.

The final steady-process measurements for other tiers were bytecode 298.58 to 299.34 ms, reference LIR
480.92 to 440.26 ms and copy-and-patch 195.58 to 195.42 ms. Reference LIR execution was not optimized by this
change; do not attribute its timing difference to deferred native boxing.

Both benchmark modes retain binary hashes, readiness responses and raw samples. `tools/compare_nbody.py
--fresh-processes` starts, warms and closes a worker for each sample; normal mode retains warmed workers.
No builds or CPU sampling ran during timed comparisons.

## Implementation

LIR dumps expose 30 float boxing recipes for warmed `advance`. Eligible `BoxFloat` results remain in Cranelift
SSA variables, with tagged values materialized for helper arguments, outgoing edges, returns and side exits.
Read-only callback failures materialize snapshots on their failure branch. Native GC root states omit proven
deferred floats while original reconstruction states remain intact. Recipes imported through OSR retain tagged
storage; eager feedback artifacts retain immediate boxing. Reopened feedback windows materialize for observation.
Copy-and-patch continues to use its existing boxed register image.

This change does not extend float-view lifetimes across blocks, callbacks or loop backedges.

## CPU profiles

Two ten-second warmed samples before and after. Every sampled call validated energy, native entry and absence
of compilation/deoptimization. Raw sample output, metadata and demangled summaries are retained.

| Region | Before | After |
| --- | ---: | ---: |
| Runtime callback, inclusive | 85.0–85.9% | 85.2–85.8% |
| Context step, exclusive | 43.0–43.1% | 43.3–43.9% |
| Root synchronization, exclusive | 6.8% | 6.4–6.7% |

These samples identify runtime bridge overhead as the dominant remaining cost; they do not resolve individual
machine instructions inside generated code. The final timed native runs still executed 1,200,000 unpack batches,
500,000 float-view acquisitions and 400,003 root synchronizations. All had zero float-view failures, deoptimizations
or compilation during the timed call. The next performance experiment should remove generic unpacking and loop
helpers from native hot loops.

Validation: 151 tests passed in debug and release, cargo fmt, and clippy all targets with warnings denied.
New tests cover reconstruction after a store followed by a guard failure, arguments to allocating helpers,
returns, and deferred floats alongside live heap references through automatic collection in a native loop.
