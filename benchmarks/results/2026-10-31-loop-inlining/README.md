# Loop-entry guards for inlined arithmetic helpers

The shared LIR region proof admits positional, unbound helpers composed of float arguments, constants and addition,
subtraction or multiplication. Both backends validate the actual callable identity, revision and arity at region entry.
Cranelift emits the expression inside the region. Copy-and-patch uses copied arithmetic stencils and skips repeated
preflight callbacks while the proof remains active. CnP also reads argument operands directly instead of copying them
to scratch. It does not invoke Cranelift.

## Retained execution

Milliseconds, medians of ten calls after twenty warmups in the same VM/function objects. Wrapper compilation and worker
transport remain included. Baseline is ec666f5, frozen before edits; programs are unchanged. Quick samples.

| Workload | CnP before | CnP after | Cranelift before | Cranelift after | CPython after | PyPy after |
|---|---:|---:|---:|---:|---:|---:|
| fill__range | 0.448 | 0.442 | 0.397 | 0.372 | 10.970 | 5.728 |
| fill__counter | 0.455 | 0.438 | 0.383 | 0.372 | 13.035 | 5.652 |
| fill__expression | 0.455 | 0.429 | 0.394 | 0.380 | 15.053 | 5.672 |
| fill__reversed | 0.464 | 0.428 | 0.391 | 0.401 | 13.434 | 5.686 |
| fill__data | 0.434 | 0.443 | 0.402 | 0.384 | 14.341 | 5.990 |
| numeric__inline | 0.745 | 0.747 | 0.244 | 0.245 | 0.985 | 0.117 |
| numeric__helper | 1.765 | 0.844 | 4.511 | 0.276 | 1.289 | 0.277 |
| numeric__tuples | 0.685 | 0.704 | 0.277 | 0.284 | 0.981 | 0.127 |
| numeric__data | 2.368 | 1.069 | 5.764 | 0.345 | 1.680 | 0.351 |
| histogram__get | 12.355 | 12.683 | 12.371 | 12.838 | 1.219 | 0.725 |
| histogram__membership | 11.203 | 11.376 | 10.155 | 10.336 | 1.375 | 0.892 |
| histogram__helper | 18.604 | 19.702 | 18.380 | 18.530 | 1.842 | 1.035 |
| histogram__unicode | 13.021 | 13.450 | 12.710 | 13.041 | 1.210 | 0.676 |
| objects__fields | 0.597 | 0.598 | 2.591 | 2.709 | 0.603 | 0.105 |
| objects__helper | 1.237 | 1.231 | 3.343 | 3.346 | 0.858 | 0.192 |
| objects__method | 3.830 | 3.861 | 5.880 | 5.953 | 0.877 | 0.118 |
| objects__layout | 0.605 | 0.613 | 2.598 | 2.756 | 0.607 | 0.136 |

## Fresh execution

Each request uses a fresh session. Fixture setup, process startup and reset are outside the execution boundary;
source execution, profiling and backend compilation are included. Three samples per runtime/workload in each mode.
Repeated mode reuses the worker process, not the VM or compiled function. These are not warmed JIT numbers.

| Mode / workload | CnP before → after | Cranelift before → after |
|---|---:|---:|
| one-shot / numeric__inline | 1.540 → 1.524 | 4.016 → 4.056 |
| one-shot / numeric__helper | 2.834 → 1.909 | 8.228 → 4.541 |
| one-shot / numeric__data | 3.326 → 2.106 | 8.877 → 4.625 |
| repeated / numeric__inline | 1.395 → 1.402 | 3.535 → 3.495 |
| repeated / numeric__helper | 2.651 → 1.800 | 7.482 → 4.095 |
| repeated / numeric__data | 3.085 → 1.952 | 8.676 → 3.850 |

## Paired regression controls

Repeated fresh-session milliseconds, three samples; old and new binaries in the same serial schedule.
One-shot results and all raw samples are archived. Small differences are not evidence of statistical equivalence.

| Workload | CnP before → after | Cranelift before → after |
|---|---:|---:|
| game_nbody__canonical | 43.437 → 43.558 | 31.373 → 32.132 |
| game_nbody__structure | 54.799 → 49.494 | 123.369 → 34.710 |
| perf_barnes_hut__canonical | 3226.235 → 3223.208 | 665.157 → 675.127 |
| perf_unpack_sequence__canonical | 151.537 → 154.076 | 152.450 → 153.460 |
| sieve__canonical | 3.661 → 3.836 | 8.184 → 8.359 |
| sieve__structure | 3.748 → 3.767 | 8.114 → 8.166 |

The original n-body control is about 2.4% slower with Cranelift in the final comparison. Its generated code size and
runtime counters match the baseline in a separate diagnostic. The initial implementation was about 20% slower:
it scanned the loop IR at every entry, even without helpers. Cached, verifier-checked site lists and skipping
binding preparation for ordinary regions removed most of that regression. The remaining small differences need
longer samples before attribution. Canonical sieve CnP varies about 5% at a roughly 4 ms boundary.

## Guards, costs and coverage

Callbacks invalidate the CnP active proof before Python execution or collection. A sticky attempt prevents repeated
schema scans after a miss within one loop. Loop exit resets the attempt. Missing names fail preparation without
raising early: empty loops and preceding observable effects keep their original behavior. Imported scalar phis need
an inductive exact-type proof. Derived call/name site metadata is checked by the LIR verifier.

In a single separate numeric-helper diagnostic, CnP runtime crossings fall from 38,596 to 76; Cranelift crossings
fall from 76,387 to 99 and root synchronizations from 18,922 to 26. CnP code grows 10,056 → 10,532 bytes;
Cranelift code grows 5,160 → 5,600 bytes. Reported backend compile time is 0.209 → 0.144 ms for CnP and
7.640 → 5.385 ms for Cranelift. These individual diagnostics do not establish a compile-latency improvement.
RSS was not sampled in this experiment.

Bound methods, field expressions, closures, defaults, keywords, non-float arguments and effectful helpers retain
the original path. Outer nest fusion around inline-helper child regions is conservatively disabled: the inner-entry
callable proof does not yet justify an outer-entry lifetime. Method acquisition and dictionary update costs remain.

Validation: fmt, separate stencil rustfmt, all-target Clippy with warnings denied, 494 Rust tests, all 255 custom
case/runtime/mode comparisons, and all 120 Flying Circus cases in each native backend. Regression tests cover
rebinding between entries, overloaded multiplication with collection and rebinding, empty loops with missing names,
recursion-error ordering, and malformed LIR guard metadata/signatures.

## Reproduction and evidence

Raw artifacts, frozen worker/CLI, hashes, pending source diff, logs, diagnostics, programs and earlier failed experiment:
`~/Library/Caches/tis-benchmarks/2026-10-31-loop-inlining/`.

The archive includes `verified/fresh-command.json`, `verified/controls-command.json`, `verified/source-state.json`,
`verified/provenance.json`, `verified/validation.json` and the exact drivers. Timing commands run serially after builds.
Use `benchmarks/variations/run.py OUTPUT --worker FROZEN_WORKER` with the archived programs and harness;
`controls.py` compares the frozen old and new workers. `verify120.py` checks exact outputs against the archived manifest.
The `scan-experiment` directory preserves the source before cached-site repair; `cached-controls` records its first check.
