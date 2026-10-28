# Unpack batches and scalar truth, 2026-10-28

Source: `22c40859bb9d54b40e3c514a83dc2b1ad0c5996d`.
Baseline: frozen worker from `10cc685` (native feedback gates and effect-based safepoints).

Native nbody time falls by 28.6% (1.400× faster), and copy-and-patch by 27.9% (1.387× faster).
Bytecode and reference LIR are approximately unchanged. This measures this kernel, not general Python performance.

| Tier | Before median (s) | After median (s) | Speedup |
| --- | ---: | ---: | ---: |
| bytecode | 0.292386 | 0.294633 | 0.992× |
| lir | 0.468665 | 0.471283 | 0.994× |
| native | 0.307252 | 0.219503 | 1.400× |
| copy-patch | 0.334128 | 0.240956 | 1.387× |

The runner uses two warmups per worker, then ten serial, randomly interleaved samples per label/tier.
Every sample restores identical body coordinates and velocities, times only the 20,000-step `advance`,
and checks energy `-0.169089263` outside the timer. No tests, builds or other benchmark workers ran concurrently.
All timed samples reported zero backend compilation and deoptimization.
Each optimized native/copy-and-patch sample recorded 1,200,000 unpack batches, zero truth fallbacks, zero detailed-feedback
probes and 400,003 root synchronizations. The root-synchronization count is unchanged from the preceding implementation.
Raw worker hashes, readiness, samples, counters and quartiles are retained in `comparison.json`.
Platform: Apple M1 Pro / macOS 27.0 ARM64, Rust 1.99.0, default release profile.

A separate fresh CPython 3.15.0 control has median 0.076987s over ten samples after two warmups.
Its experimental JIT is unavailable and disabled. It uses the same restore/advance/energy worker contract.
The native median is approximately 2.85× that CPython median, and copy-and-patch approximately 3.13×.
The CPython control ran separately after the interleaved comparison; those ratios are approximate, not paired measurements.
Full readiness, environment and timing data are in `cpython.json`.

## Execution

The LIR verifier already requires every `SequenceUnpack` view to be followed immediately by its ordered `SequenceGet`
element reads, with no intervening mutation or Python execution and no other uses of the view.
Both native generators now replace that verified group with one `UNPACK_BATCH` callback in staged artifacts.
One heap kind/length check precedes all writes, and a single scoped heap read fills the group's boxed register destinations.
This replaces the guard callback plus N element callbacks with one callback and avoids repeated heap validation.
The batch cannot allocate, collect, mutate the source or call Python.
It writes the same register image consumed by later safepoints and deoptimization, so immediate VM-stack publication is unnecessary.
No raw heap-storage pointers are exposed to generated code.

A kind or length mismatch exits at the original unpack guard before writing destinations.
Assignments and earlier effects retain their existing order and are not replayed on fallback.
Both generators retain all original instruction locations even for elided element code, preserving feedback identity.
Eager artifacts use individual reads. If a recursive guard failure reopens a window in an already active staged artifact,
the batch event follows the original per-element observation path.

Scalar truth for None, bool, integer and float is emitted directly in Cranelift and a new copy-and-patch truth stencil.
Float truth ignores only the sign bit, making both zeros false while retaining true NaNs, infinities and subnormals.
Heap references, unbound values and invalid boxed tags use the original runtime path and its error behavior.
The stencil library now has 22 entries; build-time relocation and tail-continuation checks remain enforced.
Existing heap-access and GC safety boundaries are unchanged.

## Follow-up CPU profile

Two warmed ten-second captures at 1 ms contain 16,013 samples.
The optimized debug-symbol build has the same runtime source as the benchmark worker.
Every completed advance around the captures preserved energy and reported zero compilation/deoptimization.
Raw sample reports, validated responses and demangled summaries are retained.
These profiled timings include sampling overhead and are not substitutes for the benchmark comparison.

The callback boundary remains 88.8% inclusive samples. Its own wrapper is 7.4% exclusive and the now separately visible
`Context::step` body is 38.4% exclusive; root synchronization is 5.0%, heap handle reads 7.1%, and slot conversions 7.5%.
Anonymous generated-code frames are 7.8% exclusive. Percentages are relative to a smaller total after the speedup.
The compiler outlined `Context::step` in this build, so comparing only the wrapper's self percentage with the previous profile
would falsely attribute inlining changes to a runtime improvement.
The next useful fusion candidate is iterator advancement plus its separate exhaustion check.
Direct sequence reads outside verified unpack groups still use the existing callbacks.

## Validation

All 140 tests pass in debug and release, with all-target clippy and formatting passing.
Focused tests cover scalar truth including NaNs and signed zero, heap truth fallbacks, batched reference results surviving
source mutation and nested collections, guard failures without replaying prior effects, eager artifact rebuilding and
per-element observations, and reopened staged windows inside active batched artifacts.
Existing sequence-error, partial-assignment, GC, side-exit, panic-boundary, tail-threading and nbody acceptance tests pass.

Reproduce the comparison with `tools/compare_nbody.py` and frozen before/after `nbody_bench` workers.
Use `tools/profile_nbody.py --tiers native` with a release debug-symbol worker to reproduce the CPU samples.
