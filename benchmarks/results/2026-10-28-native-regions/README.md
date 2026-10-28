# Complete native sequence-loop regions

Implementation: `b7b1823`, with entry-safety and empty-cache follow-up `fd6f370`.
Baseline runtime: `3b77e8b`. Apple M1 Pro, macOS ARM64, rustc 1.99.0, release builds.
Same 20,000-step nbody fixture, expected energy `-0.169089263`. Raw JSON retains binary hashes and runtime metadata.

## Paired fresh-process comparison

Ten serial, shuffled samples per binary/tier, each in a new process after two warmups. No builds, tests or CPU
sampling ran during timed comparisons.

| Tier | Before | After | Time change |
| --- | ---: | ---: | ---: |
| Cranelift native | 154.54 ms | 39.52 ms | -74.4%, 3.91x speedup |
| Copy-and-patch | 186.35 ms | 184.47 ms | -1.0% |

Region execution currently accelerates Cranelift. Copy-and-patch keeps the ordinary LIR operations; it shares
constant negation folding and runtime bridge code. Its small timing change is not evidence of a region speedup.
Every timed native call had one native entry, zero compilation/deoptimization/feedback probes, and correct energy.

## Fresh runtime comparison

Ten cold process samples and ten steady samples after ten warmups. CPython 3.15.0 has no available JIT in this
build; PyPy is 7.3.16 / Python 3.10.14. Timings from this protocol should be compared within this table.

| Runtime | Steady kernel | Cold process, end to end |
| --- | ---: | ---: |
| tis native | 39.82 ms | 55.61 ms |
| CPython | 77.14 ms | 102.41 ms |
| PyPy | 7.36 ms | 42.16 ms |
| tis copy-and-patch | 194.34 ms | 197.47 ms |
| tis bytecode | 301.59 ms | 327.70 ms |
| tis reference LIR | 468.60 ms | 476.73 ms |

Native is 1.94x faster than CPython and 5.41x slower than PyPy on the steady kernel. The former implementation
was roughly 21x slower than PyPy. This closes part of the gap; it does not establish PyPy-level performance.
The different copy-and-patch medians across protocols remain visible in the raw measurements.

## What now stays native

The loop planner recognizes two-block sequence loops with an invariant iterator, an internal next/is-done test,
a body backedge, and an explicit allowlist of nonallocating operations. The verifier checks region imports,
entry snapshots, edge direction and reconstructed guard schemas. Function and OSR entry cannot jump directly
into a region body. Unsupported loops keep the ordinary execution machinery.

Entry prepares C-layout heap descriptors and tagged snapshots of read-only list/tuple storage. Float buffers stay
in their original allocation, with stale boxed snapshots cleared before native writes. Iterator index/exhaustion
field pointers are captured from stable heap payloads and renewed for the invariant iterator at entry. Native next
updates real iterator progress. Range values use checked 128-bit intermediate arithmetic.

For invariant object paths, a schema validates all source items, exact unpack lengths, bounds and scalar types before
any iteration executes. Validation is cached while shapes and identities remain stable. Numerical failures retain
in-loop guards. Changing object phis retain their per-iteration guards. An entry miss resumes before next; an exit
later in the body preserves stores and consumed items exactly. The final warmed LIR has two regions with 33 and
14 hoisted constraints. Float power remains a pure math call, not a VM callback.

Allocating, mutating or opaque helpers invalidate cached views. Current builtin iteration preserves sequence storage;
other reads renew float views before native writes. Every collection advances a heap epoch that invalidates cached
pointers and schemas. Caches are not GC roots. Preparation uses reusable scratch storage and currently falls back
when the arena has more than 65,536 slots. Cranelift's speed optimization pipeline is enabled.

The outer loop, iterator allocation and region entry still call runtime services. The new machinery does not yet
virtualize those iterators, eliminate all boxed state, inline Python functions or optimize arbitrary CFG regions.

## Runtime work per native nbody call

| Counter | Before | After |
| --- | ---: | ---: |
| Generic unpack batches | 700,000 | 0 |
| Ordinary float-view callbacks | 1,000,000 | 0 |
| Root synchronizations | 400,003 | 60,003 |
| Native region entries | 0 | 40,000 |
| Completed native region iterations | 0 | 300,000 |
| Region guard failures | — | 0 |
| Runtime callbacks, final measured count | — | 240,013 |

The 300,000 iterations comprise 200,000 pair updates and 100,000 position updates. Ordinary inner iterations make
no VM callbacks. Remaining callbacks are at region boundaries and in the outer execution machinery.
Backedge counters are accumulated in native code and published in bulk, preserving hotspot counts.

The benchmark worker now snapshots tier counters immediately around `advance`. During long profiles, energy
validation can itself become hot and compile outside the timer; its compilation and entries must not be attributed
to the measured kernel. Energy is still checked after every call.

## Profiles and validation

Two ten-second native profiles validated 216 and 207 successive calls, including correct energy and zero kernel
compilation or deoptimization. Sampling is intrusive; sampled durations are not the benchmark timings above.

| Region | Final sampled share |
| --- | ---: |
| General VM callback, inclusive | 22.2–23.1% |
| Region preparation callback, inclusive | 7.8–8.4% |
| Generated-code addresses, exclusive | 63.5–64.6% |
| libm pow, exclusive | 3.4–3.8% |

The preceding typed-unpacking profiles spent about 84% inside general/view callbacks. Most time is now in generated
code. Boxed register/phi traffic and the remaining outer-loop machinery are the next profiling targets. Raw profiles,
metadata, demangled summaries and warmed LIR are retained alongside these measurements.

All 172 tests passed in debug and release, cargo fmt passed, and clippy all targets passed with warnings denied.
New tests cover partial numerical effects, iterator progress after failure, changing shapes across calls, collection
between regions and during iterator allocation, exact bulk backedge counts, eager feedback, sticky exhaustion,
signed ranges, changing object phis, guard-metadata rejection, invalid native entry and twelve deterministic aliased
graph comparisons against bytecode with bit-for-bit float checks. Copied Monty arena/free-list internals were unchanged.
