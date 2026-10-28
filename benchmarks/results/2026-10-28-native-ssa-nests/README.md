# Tagged SSA and closed native loop nests

Source commits: `9f30582` (tagged SSA and diagnostics), `f05f2b6` (closed nests).
Baseline worker: `fd6f370`. Apple M1 Pro, macOS 27, Rust 1.99.0, ARM64 release builds.
All runs use the unchanged game_nbody fixture, 20,000 steps, and validate energy `-0.169089263`.

## Timings

| Comparison | Median |
| --- | ---: |
| Previous native, fresh warmed worker per sample | 39.716 ms |
| Current native, fresh warmed worker per sample | 14.421 ms |
| Current native, steady process | 14.516 ms |
| CPython 3.15.0, steady process | 76.482 ms |
| PyPy 7.3.16 / Python 3.10.14, steady process | 7.206 ms |

The native before/after improvement is 2.75x. Current native is 5.27x faster than CPython
and 2.01x slower than PyPy on this workload. These results do not establish broader Python performance.
Ten serial interleaved samples per comparison; no builds, tests or profiling overlap timed runs.
Before/after uses a fresh worker with two warmup calls for each sample. Runtime comparison retains
10 cold end-to-end runs separately from 10 steady samples after 10 warmups. CPython JIT is unavailable;
PyPy JIT is enabled. Binary hashes, warmup times, raw samples and runtime versions are in the JSON files.

Cold end-to-end medians: tis 46.614 ms, CPython 102.885 ms, PyPy 39.991 ms.
Native compilation and the later simple-region-to-nest upgrade remain visible in warmup/readiness data.
Timed steady calls compile nothing and deoptimize zero times.

## Execution and remaining costs

Each measured call enters one prepared nest, completes 320,000 backedges, makes 11 runtime callbacks
and synchronizes roots twice. Previous native execution made 240,013 callbacks and 60,003 root synchronizations.
Private inner iterators are reused; iterator progress and backedge counts stay in SSA until publication.
The numerical kernel preserves float storage aliases and Python error ordering through explicit exits.

Two separate ten-second sampling passes attribute 88.29% / 88.31% of exclusive samples to anonymous
frames, predominantly generated code, and 10.21% / 10.07% to `pow` (plus about 0.6% in its linkage stub).
Sampling includes reset and energy validation between advances; profiled times are not benchmark results.
Next targets are redundant descriptor loads, register pressure, stable path hoisting and cold exit size.

The captured whole function grows from 10,188 to 29,552 machine bytes. Its allocated local stack
reservation grows from 656 to 1,088 bytes, excluding saved registers. Static whole-function size includes
cold reconstruction paths and is not a hot-instruction count. Compiler IR and allocated assembly are
compressed alongside the LIR; binary images contain process-specific relocated addresses and are omitted.

Validation: 180 tests pass in release; full debug suite plus the final expanded region suite pass.
Clippy with all targets and warnings denied is clean. Tests cover partial writes, external outer iterator
progress on failure, shared inner iterator exhaustion, empty outer loops with invalid inner sources,
metadata tampering, collections, aliases and the retained simple-region path.

## Reproduction

```sh
cargo build --release --examples
python3 tools/compare_nbody.py --baseline /path/to/old-worker --candidate target/release/examples/nbody_bench \
  --tiers native --fresh-processes --output /tmp/nests-before-after
python3 tools/compare_runtimes.py --run target/release/examples/run --worker target/release/examples/nbody_bench \
  --python /path/to/python3 --pypy /path/to/pypy3 --tiers native --output /tmp/nests-runtimes
python3 tools/profile_nbody.py --worker target/release/examples/nbody_bench --tiers native --output /tmp/nests-samples
```
