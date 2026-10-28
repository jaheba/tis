# Typed float-list unpacking and narrow view callbacks

Implementation: `3b77e8b`. Baseline runtime: `c8dccc9`.
Apple M1 Pro, macOS ARM64, release builds, 20,000-step nbody. Expected energy: `-0.169089263`.
Raw measurements include binary hashes, readiness responses, counters and individual samples.

## Fresh-process measurements

Ten serial, shuffled samples per binary and tier. Each sample starts a new worker and warms it twice before
measuring one call. Builds, tests and CPU sampling were stopped during timing.

| Tier | Before | After | Time change |
| --- | ---: | ---: | ---: |
| Native | 161.39 ms | 154.50 ms | -4.3% |
| Copy-and-patch | 196.15 ms | 186.62 ms | -4.9% |
| Reference LIR, separate fresh-process run | 433.00 ms | 498.07 ms | +15.0% |

The reference LIR regression is reproducible across fresh processes. This implementation targets native execution;
reference LIR remains the checked execution path, and its performance needs further work. These measurements do
not isolate instruction expansion, interpreter dispatch and code layout as causes of the reference regression.

A separate steady-process comparison, ten serial interleaved samples after two warmups:

| Tier | Before | After |
| --- | ---: | ---: |
| Bytecode | 299.50 ms | 299.41 ms |
| Reference LIR | 435.24 ms | 493.40 ms |
| Native | 161.54 ms | 154.67 ms |
| Copy-and-patch | 194.92 ms | 186.73 ms |

Every timed native/copy-and-patch sample had correct energy, one backend entry and zero compilation,
deoptimizations, float-view failures or feedback probes.

## Lowering and runtime changes

A profiled, fixed-length list unpack qualifies when its element reads form one eager contiguous bundle and all
observed element results are floats. LIR emits an exact-length `FloatListView`, a boxed source alias for HIR
snapshot reconstruction, and direct `FloatListLoad` operations. All loads precede assignment-target mutation.
Wrong lengths and incompatible storage exit before unpacking. Source evaluation and earlier side effects are
not repeated. Mixed lists, tuples, iterators and starred unpacking keep their existing execution paths.

Float-view acquisition uses a narrow runtime gateway with panic containment. It cannot call Python, collect
or change frames, so it does not snapshot frame bookkeeping. View lifetimes remain block-local and end before
invalidating callbacks. The copied heap storage modules were unchanged. Copy-and-patch adds one copy/feedback
stencil, bringing the checked library to 25 bodies; relocation and tail-continuation checks still pass.

LIR also retains original HIR observation operands. Reopened feedback branches materialize only the current
site's inputs and result. Materializing every float recipe in each optional branch caused a native regression
in an intermediate implementation by extending unrelated value lifetimes; that implementation was corrected
before the final comparison. Both changes are included in the reported result.

Per timed nbody call:

| Counter | Before | After |
| --- | ---: | ---: |
| Generic unpack batches | 1,200,000 | 700,000 |
| Float-view acquisitions | 500,000 | 1,000,000 |
| Root synchronizations | 400,003 | 400,003 |

This replaces 500,000 general unpack callbacks with narrow view acquisitions plus direct loads. The total
number of runtime crossings does not decrease; all view acquisitions now use the narrow gateway.

## Remaining CPU costs

Two ten-second warmed native profiles validate every worker response. Demangled summaries, raw compressed
samples and metadata are retained. Compare with the preceding deferred-boxing after-profiles for the baseline.

| Region | Before | After |
| --- | ---: | ---: |
| General callback, inclusive | 85.2–85.8% | 76.8–76.9% |
| Narrow float-view callback, inclusive | included above | 6.9–7.6% |
| Context step, exclusive | 43.3–43.9% | 38.2–39.3% |

Callbacks still dominate. Remaining tuple unpacking, iteration and block bookkeeping are the next targets;
reducing their actual work or eliminating crossings is needed for a larger improvement.

Validation: 158 tests pass in debug and release, cargo fmt, separate stencil rustfmt, and clippy all targets
with warnings denied. Added cases cover exact-length failures before assignment, mixed storage and tuple
fallbacks, nested unpacking, aliasing targets, source evaluation exactly once, required length checks in the
verifier, and precise observations when a recursive guard failure reopens a staged feedback window.
