# Typed float-list and loop values

Implementation: `d2a8168`. Baseline runtime: `22c4085`.
Apple M1 Pro, macOS ARM64, release builds. Same 20,000-step nbody fixture and expected energy
`-0.169089263`. Raw measurements include binary hashes and runtime metadata.

## Paired before/after

Ten serial, interleaved samples per tier after two warmups. Medians:

| Tier | Before | After | Time change |
| --- | ---: | ---: | ---: |
| Bytecode | 293.52 ms | 298.40 ms | +1.7% |
| Reference LIR | 465.97 ms | 477.66 ms | +2.5% |
| Native | 216.51 ms | 160.82 ms | -25.7% |
| Copy-and-patch | 238.83 ms | 195.90 ms | -18.0% |

Every timed native/copy-and-patch sample acquired 500,000 float views with zero storage guard failures,
compilation or deoptimizations. Results include list storage changes, typed accesses and scalar loop values together;
they do not isolate each optimization's contribution. The interpreter regressions remain to investigate.

## Fresh cross-runtime comparison

Ten cold process samples and ten steady samples after ten warmups. Steady medians:

| Runtime | Time |
| --- | ---: |
| PyPy | 7.32 ms |
| CPython | 76.50 ms |
| tis native | 175.86 ms |
| tis copy-and-patch | 196.67 ms |
| tis bytecode | 302.81 ms |
| tis reference LIR | 490.61 ms |

Native timings varied between the two runs; use the paired run for the before/after effect and this run for
cross-runtime ratios. Native remains about 24 times slower than PyPy and 2.3 times slower than CPython.

`advance.lir.gz` captures the warmed native artifact. Float-list views have block-local lifetimes enforced by
the verifier. Loop-carried floats use Cranelift SSA variables, while boxed shadows remain for GC and deoptimization.
Copy-and-patch retains its memory-backed primitive register image. Indexed specialization currently requires
constant integer indices. Generic mutation may widen float storage; later guards can promote homogeneous storage again.

Validation: 148 tests passed in debug and release, clippy all targets with warnings denied, cargo fmt and separate
stencil rustfmt. Tests cover aliasing, storage widening, cached snapshots, parallel phi swaps, OSR, guard exits after
side effects and rejection of view use across callbacks. Copied Monty storage internals were unchanged.
