# Barnes–Hut runtime fast paths

Implementation: `ef22a35`. Barnes/ticket baseline: `a5bf5d0`. Nbody regression baseline: `22dfc7a`.
Apple M1 Pro, macOS 27, ARM64 release, Rust 1.99.0. Fixture and numerical algorithm unchanged.
Barnes–Hut still uses 200 particles, 100 steps and theta 0.5, with final energy `3386.616186`.

## Before / after, ten samples each

| Protocol | Before native | After native | Time reduction |
| --- | ---: | ---: | ---: |
| Warmed function calls | 11.037 s | 6.425 s | 41.8% |
| Fresh processes, zero warmups | 11.036 s | 6.466 s | 41.4% |

The native speedup is 1.72x warmed and 1.71x one-shot. Runs are serial and interleaved in seeded
random order. No builds, tests or profiling overlap timing. All outputs validate against the existing
CPython golden output. One-shot timings include startup, parsing, compilation, execution and exit.
The paired one-shot comparison also measures CPython at 659.907 ms; this remains substantially faster.

Warm workers get two warmup calls. Some timed calls still perform incremental compilation, retained
in the measurements: baseline maximum 6.225 ms, candidate maximum 6.980 ms. All warmed deoptimizations
are zero. This is the same staged-profile protocol as the starting baseline, not a claim of zero
compilation or maximum steady-state throughput. JSON retains warmup times, raw samples, quartiles,
min/max, binary hashes, and runtime/validation counters. One-shot JSON retains exact stdout/stderr.

## What changed

- Simple positional calls whose locals all use frame slots bind directly into those slots and reuse
  the lexical parent. Captured names, global/nonlocal declarations, explicit namespace access and
  general argument binding retain environment handling. Eligibility derives from HIR and is cached
  alongside immutable bytecode. Other simple calls avoid the temporary binding map and empty kwargs dict.
- Existing instance-field reads have a nonallocating Cranelift probe. Misses, methods, other receiver
  types and active observation windows use the complete runtime path. Successful probes need no
  root synchronization or GC checkpoint. Attribute writes cannot collect under the current object model,
  which has no descriptors or Python setters, so their LIR effects no longer request a safepoint.
- Direct class members and no-base misses avoid allocating traversal vectors and visited sets.
  Inherited lookup keeps its cycle checks.
- Native entry reuses cleared slot buffers. Active recursive frames own separate buffers. Inactive
  buffers are not roots and do not keep Python objects alive; their capacity is retained at the native
  invocation high-water mark. This trades retained scratch capacity for fewer allocator calls.

No changes were made to the copied heap storage internals or benchmark source.

Median warmed counters: root synchronizations fall from 53,668,761 to 18,694,458 (65.2% reduction),
with 34,452,153 successful field probes. Total C gateway calls increase from 110,108,679 to 114,797,125,
because failed probes then use the general helper. A cheap field probe and a full allocating helper
have different costs; total callback count alone is no longer an adequate performance proxy.

## Regression checks

| Warmed median | Before | After |
| --- | ---: | ---: |
| Nbody native, 20k steps | 11.580 ms | 11.550 ms |
| Nbody bytecode | 299.544 ms | 299.552 ms |
| Ticket search native | 113.564 ms | 109.787 ms |
| Ticket search bytecode | 93.692 ms | 90.514 ms |

Ten serial interleaved samples for each pair, unchanged inputs and output checks.
Nbody shows no meaningful change. Ticket search is about 3% faster in this run, a smaller change with
visible sample overlap. The nbody baseline is its frozen earlier worker; math/sorting support was added
between that source and the Barnes starting baseline, without changes to the nbody kernel.

## Profiles and remaining work

Before profiles sample the first eight seconds of fresh native and bytecode processes, two passes each.
After profiles sample the first four seconds of fresh native processes, two passes. Profiling runs are
separate from timing; different sample windows cover different amounts of simulation progress, so their
percentages are diagnostic rather than a directly matched cost decomposition. Every profiled run validates
its final energy. Raw reports are compressed and native summaries retained.

After samples still place about 16–17% of exclusive CPU samples in native Context::step, about 5.6–5.7%
in the native entry bridge, 4.5–4.9% in root synchronization and 4.7–5.0% in a BTreeMap lookup symbol,
plus roughly 5.3–5.6% in string comparisons. These separate symbols must not be treated as independent
causal speedup estimates. Remaining targets are direct object-field layouts, compiled method calls,
less boxed dispatch and cheaper frame entry. This change does not approach PyPy's performance yet.

## Validation and reproduction

188 tests pass in both debug and release; all-target Clippy with warnings denied is clean.
Coverage includes field aliases, missing fields and method fallback, collection after field replacement,
recursive buffer reuse, positional defaults, separate returned closure captures, scope errors, all four
execution tiers and the three language benchmarks.

```sh
cargo build --release --examples
python3 tools/compare_nbody.py --benchmark barnes-hut --baseline /path/to/old-bench \
  --candidate target/release/examples/bench --tiers native --output /tmp/barnes-warm
python3 tools/bench_oneshot.py --run target/release/examples/run --baseline-run /path/to/old-run \
  --python /path/to/python3 --benchmarks barnes-hut --tiers native --output /tmp/barnes-cold
```
