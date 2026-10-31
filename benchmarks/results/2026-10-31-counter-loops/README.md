# Counter-loop Boolean fills — 2026-10-31

Integer counter loops can use the same guarded Boolean-list fill kernel as `for range` loops.

## Method

Paired frozen release workers, ten samples per workload/runtime/mode, serial execution, one repeated warmup,
and two startup samples. Request timings include frontend, profiling, backend compilation and execution.
Repeated workers reset to fresh VM sessions; these are not retained-JIT measurements.
Both native tiers enable `--readonly-regions --mutation-regions`. Exact stdout is checked against CPython.
Builds and tests did not overlap timing. RSS was not sampled.

## Timings

### one-shot

Milliseconds; medians of ten requests.

| Workload | CnP before | CnP after | Cranelift before | Cranelift after | CPython |
| --- | ---: | ---: | ---: | ---: | ---: |
| sieve__canonical | 4.025 | 4.010 | 8.952 | 9.085 | 50.771 |
| sieve__structure | 186.801 | 3.999 | 124.961 | 8.784 | 67.303 |
| sieve__reversed | 244.869 | 4.837 | 162.107 | 9.465 | 89.310 |
| sieve__strict | 145.367 | 144.790 | 95.801 | 95.874 | 60.691 |
| perf_barnes_hut__canonical | 3188.200 | 3186.117 | 679.390 | 676.208 | 620.637 |
| perf_unpack_sequence__canonical | 154.081 | 153.355 | 154.808 | 154.129 | 18.254 |

### repeated

Milliseconds; medians of ten requests.

| Workload | CnP before | CnP after | Cranelift before | Cranelift after | CPython |
| --- | ---: | ---: | ---: | ---: | ---: |
| sieve__canonical | 3.732 | 3.704 | 8.358 | 8.200 | 50.113 |
| sieve__structure | 186.958 | 3.752 | 124.510 | 8.089 | 65.477 |
| sieve__reversed | 244.987 | 4.557 | 161.296 | 8.810 | 89.570 |
| sieve__strict | 145.340 | 144.272 | 95.078 | 95.143 | 59.374 |
| perf_barnes_hut__canonical | 3183.894 | 3181.526 | 675.042 | 672.764 | 620.538 |
| perf_unpack_sequence__canonical | 151.430 | 151.559 | 152.757 | 152.298 | 17.161 |

## Diagnostics

Separate single CLI runs reduce runtime crossings for the while variant from 4,245,181 to 3,343 in CnP,
and 2,124,260 to 3,341 in Cranelift. All report zero compilation errors.
CnP backend compilation increases from 0.139 to 0.237 ms; Cranelift from 5.05 to 6.61 ms.
These individual compile samples show added work, not a stable latency estimate.

## Scope and remaining gaps

The shared LIR plan recognizes ascending/descending integer counters, strict/inclusive comparisons, reversed operands,
and an invariant step and bound. Both native backends guard exact integer operands, step direction, span overflow,
Boolean-list storage, borrow permissions and all store bounds before any write.
Counter and update registers receive the exact final index. Misses execute the original loop, preserving partial writes,
custom operators and arbitrary-precision integer behavior. No benchmark names or limits enter the matcher.

The strict holdout keeps `cursor < limit + 1` inside the header. Its additional arithmetic prevents this matcher from
selecting the kernel. This remains a performance gap: general loop-invariant expression hoisting is unfinished.
The reversed-comparison holdout uses a new limit and renamed induction variable.

N-body helper extraction remains slower. A separate three-sample check reproduced the unchanged gap:
CnP 43.4 → 87.5 ms, Cranelift 30.8 → 264.5 ms, repeated fresh sessions.
The existing arithmetic inliner still performs per-call preflight and loses region fusion.
A future change must validate callee identity/revision/binding at region entry and preserve argument type guards,
logical-frame recursion checks, profiling and GC safety. No helper-call optimization was applied here.

## Validation and evidence

`cargo fmt --all`, stencil `rustfmt --edition 2024`, all-target Clippy with warnings denied, and all 486 tests pass.
The new regression covers final counters, descending/empty loops, negative-index normalization, partial-write errors,
custom arithmetic and the final increment overflowing the inline integer representation.

Raw samples, frozen workers/CLI, source hashes, pending diff, diagnostics, commands and build/test logs are outside Git:
`~/Library/Caches/tis-benchmarks/2026-10-31-counter-loops/`.
The control is the all120/variation worker at revision d0f1aab plus its recorded pending changes.
The candidate provenance records the full source state and toolchain; neither build was committed.
