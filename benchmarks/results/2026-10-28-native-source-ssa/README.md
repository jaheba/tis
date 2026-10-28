# Native sequence views and iterator-source SSA

Implementation: `22dfc7a`. Baseline: `f05f2b6` (same executable as the preceding measurement report).
Apple M1 Pro, macOS 27, ARM64 release build, Rust 1.99.0. Unchanged game_nbody, 20,000 steps.

Float-list views now retain their data pointer, length and observation flag in SSA, including callback-created
views. Nest entry also loads each invariant builtin iterator source's kind, length, data/start and range step
into SSA. Subsequent Next operations avoid descriptor resolution. Item-root schema constraints can prove that
a returned item is not the VM's Done sentinel, allowing IsDone to reuse the iterator exhaustion flag.
Unconstrained items retain the sentinel check. Float elements are still loaded at their point of use;
mutations through aliases remain visible. Shapes and source storage remain fixed only within verified nests.

## Measurements

| Ten fresh workers, two warmups each | Median | Min–max |
| --- | ---: | ---: |
| Before | 14.323 ms | 14.196–14.620 ms |
| After | 11.592 ms | 11.422–12.748 ms |

That is a 19.1% time reduction, or 1.24x speedup. In the separate ten-sample steady-process comparison:

| Runtime | Median | Min–max |
| --- | ---: | ---: |
| tis native | 12.414 ms | 11.546–12.663 ms |
| CPython 3.15.0 | 76.600 ms | 76.059–77.389 ms |
| PyPy 7.3.16 / Python 3.10.14 | 7.199 ms | 7.148–7.641 ms |

Current tis is 1.72x slower than PyPy and 6.17x faster than CPython in that steady comparison.
Process-to-process native variation is visible; the fresh-worker and steady-process medians are kept separate.
Cold end-to-end medians: tis 44.659 ms, CPython 101.784 ms, PyPy 39.890 ms.
All runs are serial and interleaved, with no concurrent builds, tests or profiling. Energy validates as
`-0.169089263`; timed calls have zero compilation and deoptimization. Runtime calls remain 11, root syncs 2,
and native backedges 320,000 per advance. JSON retains binary hashes, raw samples, warmups and runtime details.

Staged experiments, each with its own interleaved baseline, are retained as diagnostics:
float views alone 14.321→12.852 ms; views plus cached sources 14.361→11.738 ms;
reusing the proven loop condition 11.947→11.426 ms. These sequential experiments are not additive estimates.

Captured whole-function machine size falls from 29,552 to 17,484 bytes (40.8%). The local stack reservation
grows from 1,088 to 1,200 bytes, excluding saved registers: register pressure remains a target despite the
smaller function. Compiler IR and allocated assembly are compressed alongside LIR. No new CPU sample was
collected for this change; the preceding report's generated-code/pow attribution should not be treated as
an updated profile. Next work should inspect hot-block spills and hoist stable tuple-to-array paths.

## Validation

181 tests pass in debug and release. The final expanded region test also passes independently.
All-target Clippy with warnings denied is clean; cargo fmt is applied. Coverage includes existing allocation,
GC, callback, aliasing and deoptimization cases plus tuple/range/float source changes and sentinel handling.

## Reproduction

```sh
cargo build --release --examples
python3 tools/compare_nbody.py --baseline /path/to/old-worker --candidate target/release/examples/nbody_bench \
  --tiers native --fresh-processes --output /tmp/source-ssa
python3 tools/compare_runtimes.py --run target/release/examples/run --worker target/release/examples/nbody_bench \
  --python /path/to/python3 --pypy /path/to/pypy3 --tiers native --output /tmp/source-runtimes
```
