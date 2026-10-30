# Reusing checked unpackings

The unpacking workload contains 400 adjacent ten-element assignments per loop iteration.
Previously every assignment added another checked native unpack, ten element reads and failure paths.
Distinct cached native variables for their results also inflated Cranelift's SSA graph.

A conservative LIR pass now keeps the first checked builtin list/tuple unpack and replaces adjacent equivalent
bundles with explicit Copy operations. Each Copy retains its original HIR result identity and observation inputs,
so snapshots can still reconstruct the original Python locals and resume positions.
Unused failure snapshots are pruned. Cranelift region copies share their source's boxed SSA variables and initialize
only the source slot, avoiding thousands of redundant variables, entry loads and loop phi candidates.
Copy is also implemented in the reference LIR and copy-and-patch backends and native region emitters.

Reuse requires the same source register, builtin kind and exact length, in adjacent complete bundles in one block.
Any intervening instruction stops reuse. General iterators and copied function bodies remain unchanged.
The original first check remains; wrong types or lengths deoptimize before that unpack.

## Quick comparison

Three interleaved fresh-process samples, full unchanged Flying Circus perf_unpack_sequence source.
Includes process startup, source compilation, JIT compilation and execution.
Native enables read-only and mutation regions. No builds or tests overlapped measurement.
All output matched exactly across both builds, bytecode and CPython.
macOS time -l records peak whole-process RSS; these timings differ from Flying Circus request timings.

| Build/tier | Median elapsed, ms | Median peak RSS, MiB |
| --- | ---: | ---: |
| before_native | 2298.23 | 917.48 |
| after_native | 264.01 | 39.58 |
| bytecode | 203.80 | 28.30 |
| cpython | 44.99 | 18.70 |

Native compilation median: 1945.15 ms → 7.46 ms.
Generated native code: 2,263,392 bytes → 10,468 bytes.
Both builds recorded zero compilation errors and deoptimizations, with one native region entered.
The previous single RSS sample was 723 MiB; this fresh baseline ranged from
901.6 to
1053.6 MiB.
The compiler blowup varied substantially between processes; the improvement is large across all three samples.

This addresses compile-time/code-size growth rather than avoiding native compilation.
Native is still slower than bytecode on a cold execution, despite removing most backend compilation time.
Remaining costs include profiling/interpreter work and IR preparation; warmed kernel timings are not measured here.
Three samples are directional and do not establish stable small performance differences.

## Validation

Formatting and all-target Clippy pass; all 335 tests pass.
New tests check reuse across adjacent assignments, barriers around mutation/calls, preserved values across GC,
wrong-length errors, general iterator inputs and guard failures after warmup in all LIR backends.
All 54 previously supported Flying Circus programs match manifest stdout in bytecode and native modes: 108 checks.
The installed CLI is updated only after these measurements, preserving the before binary during the comparison.
Raw samples, source/binary hashes, reproduction scripts and validation logs are retained alongside this report.
