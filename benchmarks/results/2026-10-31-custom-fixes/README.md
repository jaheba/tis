# Custom variation fixes — 2026-10-31

Runtime/compiler changes improve the unchanged 17-program source/data variation suite.
Cold and retained results remain separate. Remaining helper/method/dictionary gaps are not parity wins.

## Changes

- Shared Boolean fill plans accept effect-free literals and checked integer affine bounds in both high-level and scalar-expanded LIR.
- Exact Boolean-list count avoids generic equality/protocol handling; other operands/storage retain the original path.
- Builtin dictionary reads borrow string storage and avoid protocol roots. Existing string-key writes replace values without cloning keys.
- Profiled dict.get calls use the closed-builtin gateway only after checking the actual bound method/receiver and absence of protocol keys.
- CnP emits arithmetic-body stencils after the shared identity/revision/binding/type preflight. Misses execute the original Call once and skip the copied body.
- Cranelift can acquire an exact float-list view without a Rust call, using generation, storage, length and write-permission checks.
- Inline preflight uses bounded stack storage instead of allocating an argument vector.

No workload source was altered. Expected output is checked for every runtime/case/mode.
Python hashing/equality, GC-sensitive defaults, overloaded arithmetic, integer overflow, shape changes, eager profiling,
function revision/replacement and logical recursion limits retain guarded fallbacks.

## Retained execution

Each runtime/case retains its function objects and VM, with 20 full warmup calls and ten timed calls.
Times include the small invocation-wrapper compilation and transport, but exclude setup.
Both native tiers enable readonly and mutation regions. Milliseconds, medians.

| Case | Bytecode before → after | CnP before → after | Cranelift before → after | CPython after | PyPy after |
| --- | ---: | ---: | ---: | ---: | ---: |
| fill__range | 30.730 → 13.307 | 17.630 → 0.448 | 17.485 → 0.397 | 10.926 | 5.732 |
| fill__counter | 35.318 → 17.881 | 42.918 → 0.455 | 33.476 → 0.383 | 13.020 | 5.674 |
| fill__expression | 39.286 → 21.742 | 44.618 → 0.455 | 33.858 → 0.394 | 14.963 | 5.742 |
| fill__reversed | 35.234 → 18.020 | 43.169 → 0.464 | 33.666 → 0.391 | 13.038 | 5.755 |
| fill__data | 39.880 → 10.185 | 44.471 → 0.434 | 38.628 → 0.402 | 14.281 | 6.473 |
| numeric__inline | 3.185 → 3.213 | 0.743 → 0.745 | 0.244 → 0.244 | 0.998 | 0.116 |
| numeric__helper | 5.926 → 5.993 | 4.660 → 1.765 | 5.509 → 4.511 | 1.272 | 0.366 |
| numeric__tuples | 3.113 → 3.099 | 0.719 → 0.685 | 0.269 → 0.277 | 0.964 | 0.119 |
| numeric__data | 7.715 → 7.609 | 6.015 → 2.368 | 7.056 → 5.764 | 1.689 | 0.336 |
| histogram__get | 11.210 → 7.641 | 17.963 → 12.355 | 17.492 → 12.371 | 1.199 | 0.998 |
| histogram__membership | 9.657 → 9.372 | 11.410 → 11.203 | 10.444 → 10.155 | 1.424 | 0.905 |
| histogram__helper | 15.667 → 12.059 | 24.298 → 18.604 | 23.819 → 18.380 | 1.816 | 1.046 |
| histogram__unicode | 11.450 → 7.856 | 18.483 → 13.021 | 18.065 → 12.710 | 1.239 | 0.673 |
| objects__fields | 2.785 → 2.790 | 0.610 → 0.597 | 2.582 → 2.591 | 0.615 | 0.108 |
| objects__helper | 4.761 → 4.800 | 3.694 → 1.237 | 3.633 → 3.343 | 0.864 | 0.202 |
| objects__method | 7.288 → 7.252 | 6.933 → 3.830 | 6.182 → 5.880 | 0.895 | 0.125 |
| objects__layout | 2.755 → 2.799 | 0.599 → 0.605 | 2.581 → 2.598 | 0.606 | 0.109 |

## one-shot fresh sessions

Flying Circus serial quick samples: three per case/runtime, three repeated warmups, two startup samples, no RSS.
Fixture preparation/definitions and worker startup are excluded. Benchmark execution feedback and backend compilation are included.

| Case | Bytecode | CnP | Cranelift | CPython | PyPy |
| --- | ---: | ---: | ---: | ---: | ---: |
| fill__range | 13.463 | 0.825 | 3.492 | 11.648 | 6.263 |
| fill__counter | 18.115 | 0.849 | 3.264 | 13.515 | 6.447 |
| fill__expression | 22.929 | 0.873 | 3.449 | 15.384 | 6.610 |
| fill__reversed | 18.772 | 0.814 | 3.417 | 13.476 | 6.830 |
| fill__data | 10.477 | 0.809 | 3.376 | 14.869 | 7.217 |
| numeric__inline | 3.631 | 1.540 | 4.016 | 1.118 | 1.024 |
| numeric__helper | 6.538 | 2.834 | 8.228 | 1.354 | 1.989 |
| numeric__tuples | 3.480 | 1.463 | 4.002 | 1.057 | 0.862 |
| numeric__data | 8.400 | 3.326 | 8.877 | 1.690 | 1.917 |
| histogram__get | 7.903 | 13.364 | 15.920 | 1.347 | 2.232 |
| histogram__membership | 9.778 | 11.816 | 13.907 | 1.523 | 2.419 |
| histogram__helper | 12.636 | 19.311 | 22.995 | 1.984 | 2.843 |
| histogram__unicode | 8.334 | 13.645 | 16.289 | 1.339 | 2.362 |
| objects__fields | 3.188 | 1.206 | 6.152 | 0.680 | 0.882 |
| objects__helper | 5.293 | 2.226 | 7.217 | 0.931 | 1.625 |
| objects__method | 7.835 | 4.989 | 10.430 | 0.996 | 1.019 |
| objects__layout | 3.092 | 1.206 | 5.991 | 0.701 | 0.798 |

## repeated fresh sessions

Flying Circus serial quick samples: three per case/runtime, three repeated warmups, two startup samples, no RSS.
Fixture preparation/definitions and worker startup are excluded. Benchmark execution feedback and backend compilation are included.

| Case | Bytecode | CnP | Cranelift | CPython | PyPy |
| --- | ---: | ---: | ---: | ---: | ---: |
| fill__range | 13.474 | 0.659 | 2.887 | 10.933 | 5.500 |
| fill__counter | 18.308 | 0.678 | 2.718 | 13.026 | 5.554 |
| fill__expression | 21.770 | 0.704 | 2.751 | 15.150 | 5.784 |
| fill__reversed | 18.002 | 0.647 | 2.673 | 13.247 | 5.556 |
| fill__data | 10.179 | 0.673 | 2.703 | 14.335 | 6.658 |
| numeric__inline | 3.491 | 1.395 | 3.535 | 1.043 | 0.898 |
| numeric__helper | 6.365 | 2.651 | 7.482 | 1.327 | 1.711 |
| numeric__tuples | 3.374 | 1.288 | 3.542 | 1.007 | 0.811 |
| numeric__data | 8.132 | 3.085 | 8.676 | 1.693 | 1.675 |
| histogram__get | 8.030 | 12.829 | 15.517 | 1.291 | 1.874 |
| histogram__membership | 9.403 | 11.496 | 13.824 | 1.475 | 1.849 |
| histogram__helper | 12.340 | 19.045 | 21.613 | 1.840 | 2.477 |
| histogram__unicode | 7.919 | 13.342 | 15.763 | 1.257 | 1.626 |
| objects__fields | 2.977 | 1.114 | 5.487 | 0.642 | 0.887 |
| objects__helper | 5.045 | 1.995 | 6.820 | 0.933 | 1.332 |
| objects__method | 7.559 | 4.543 | 9.482 | 0.932 | 1.095 |
| objects__layout | 2.996 | 1.099 | 5.456 | 0.665 | 0.868 |

## Limits

CnP still runs a preflight on every inlined call. Bound method lookup still constructs a method object.
Cranelift does not retain its fused loop advantage across these helper boundaries.
Dictionary get improves but attribute acquisition, callbacks and execution dispatch still dominate these small loops.
The direct numeric/field controls remain useful: faster fill/checksum code cannot explain the helper improvements.

The induction recognizer is deliberately bounded: literals and one exact integer affine addition are covered,
not arbitrary loop-invariant expressions or general loop normalization.
Quick fresh samples and sub-millisecond retained medians do not establish statistical equivalence or steady-state parity.
Raw warmups and sample spread are retained, including PyPy outliers.

## Evidence and reproduction

Raw results, frozen binaries/hashes, source state, diagnostics, commands and validation logs are outside Git:
`~/Library/Caches/tis-benchmarks/2026-10-31-custom-fixes/`.

Use the generator/runner in `benchmarks/variations/`; `run.py --worker PATH` selects a frozen Rust worker.
The archive retains `controls.py`, `verify120.py`, comparison scripts and the exact original workload sources.
No Python delegation, benchmark-specific results or data-specific constants were added to the runtime.

## Transfer controls

Serial three-sample Flying Circus controls; repeated workers reset to fresh VM sessions, so compilation is included.
The original n-body control stays approximately flat while the extracted-helper variant improves on both backends.
Barnes–Hut, sieve and unpacking shifts are small/noisy; three samples do not establish statistical equivalence.

| Control | CnP before → after ms | Cranelift before → after ms |
| --- | ---: | ---: |
| game_nbody__canonical | 43.37 → 43.27 | 31.31 → 30.96 |
| game_nbody__structure | 85.80 → 55.30 | 267.62 → 118.43 |
| perf_barnes_hut__canonical | 3157.32 → 3194.25 | 708.06 → 664.77 |
| perf_unpack_sequence__canonical | 151.86 → 152.47 | 152.96 → 152.93 |
| sieve__canonical | 3.82 → 3.73 | 8.27 → 8.40 |
| sieve__structure | 3.79 → 3.70 | 8.31 → 8.19 |

## Diagnostics and compilation cost

In a separate numeric-helper CLI run, CnP root synchronizations fall from 18,898 to 26 and positional frames from
20,002 to 1,130. Runtime crossings remain 38,596: the fast path makes them closed validation calls instead of full
callee execution. Fills fall from roughly 598K crossings to 37.
CnP numeric-helper code grows from 9,636 to 10,056 bytes and patch data from 11,520 to 12,360 bytes.
Backend compile totals in those single runs rise from 0.101 to 0.153 ms.
These compile diagnostics include CLI setup and are not stable latency estimates.
CnP still copies offline Rust stencils; it does not invoke Cranelift for helper arithmetic.

## Validation and audit

Formatting, separate stencil rustfmt, all-target Clippy with warnings denied, and all 491 tests pass.
All 120 unchanged Flying Circus workloads pass exact output checks in CnP and Cranelift.
All 255 custom case/runtime/mode combinations pass; their generated Python sources are byte-identical to the baseline.
The descriptor audit also repaired the existing shared inliner: copied field reads now miss when attribute protocols
are active. A late property test preserves both getter calls, and a custom attribute hook preserves its results/effects.
This global guard is conservative. Removing unnecessary guards requires a separate class/version proof.

No commit or push was made. The validated CLI was installed after these checks.
