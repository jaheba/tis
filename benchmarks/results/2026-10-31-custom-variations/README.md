# Custom source/data variations — 2026-10-31

Seventeen standalone programs across four families compare unchanged tis runtime code with CPython and PyPy.
Equivalent rewrites are checked for identical CPython output before any timings.
Changed data cases have their own independently computed expected output.

## Workloads

| Family | Variants |
| --- | --- |
| fill | range: range induction form; counter: counter induction form; expression: expression induction form; reversed: reversed induction form; data: new size, stride and binding names |
| histogram | get: get dictionary update; membership: membership dictionary update; helper: helper dictionary update; unicode: Unicode keys; reverse logical input order |
| numeric | inline: inline squared norm; helper: extract pure arithmetic helper; tuples: tuple records instead of list records; data: new record count and values; renamed helper |
| objects | fields: direct fields; helper: helper receiving object; method: bound method; layout: renamed class/fields; reversed constructor stores |

The seed controls histogram input keys. Numeric and object inputs use exact binary fractions.
No source variant was changed or removed after measuring its performance.

## Boundaries and runtimes

Fresh sessions use Flying Circus serially: three timing samples, three repeated-mode warmups,
two startup samples and no RSS samples. Both modes exclude worker startup and fixture preparation.
Definitions and input construction precede the fixture marker. The first benchmark call includes
execution feedback and backend compilation. Reused workers reset the application session.

Retained sessions keep the same functions/data/VM, run 20 full warmup calls, then measure
10 calls. Each call includes compilation of its small invocation wrapper and JSON transport;
callee code and feedback remain retained. Setup and all warmup/sample times are saved separately.
Each case/runtime gets its own worker. No builds/tests overlap samples.

CnP and Cranelift enable readonly and mutation regions. Bytecode is a separate interpreter control.
These are quick measurements: sub-millisecond transport, GC and JIT outliers remain visible.
Extra warmups do not establish universal steady state.

- cpython: Python 3.15.0
- pypy: Python 3.10.14 (75b3de9d9035, May 28 2024, 18:06:40) [PyPy 7.3.16 with GCC Apple LLVM 15.0.0 (clang-1500.3.9.4)]

The frozen tis worker SHA256 is `cf0135b2433c1fed111033ea5d2145b422e4e23a36094ca188db4c31885bf1ba`.
It is the previously verified counter-loop build; no runtime source changes were made for this suite.

## one-shot

Milliseconds, median request time. `error` cases remain in the table.

| Variant | Bytecode | CnP | Cranelift | CPython | PyPy |
| --- | ---: | ---: | ---: | ---: | ---: |
| fill__range | 30.419 | 18.043 | 20.642 | 11.457 | 6.660 |
| fill__counter | 35.387 | 43.097 | 35.891 | 13.502 | 6.644 |
| fill__expression | 39.613 | 44.524 | 36.275 | 15.313 | 6.658 |
| fill__reversed | 35.520 | 43.029 | 35.765 | 13.452 | 6.484 |
| fill__data | 40.293 | 44.538 | 40.943 | 14.670 | 7.607 |
| numeric__inline | 3.503 | 1.497 | 3.977 | 1.082 | 0.948 |
| numeric__helper | 6.409 | 5.412 | 8.796 | 1.339 | 1.859 |
| numeric__tuples | 3.522 | 1.376 | 4.216 | 1.052 | 0.934 |
| numeric__data | 8.255 | 6.790 | 9.981 | 1.741 | 2.161 |
| histogram__get | 11.524 | 18.213 | 21.035 | 1.346 | 2.095 |
| histogram__membership | 9.840 | 11.725 | 14.381 | 1.489 | 2.313 |
| histogram__helper | 16.046 | 24.719 | 27.570 | 1.881 | 2.791 |
| histogram__unicode | 11.916 | 18.765 | 21.471 | 1.306 | 2.266 |
| objects__fields | 3.073 | 1.225 | 6.009 | 0.654 | 0.785 |
| objects__helper | 5.116 | 4.423 | 7.497 | 0.959 | 1.775 |
| objects__method | 7.666 | 7.687 | 10.495 | 0.932 | 1.215 |
| objects__layout | 2.987 | 1.184 | 6.055 | 0.717 | 0.787 |

## repeated

Milliseconds, median request time. `error` cases remain in the table.

| Variant | Bytecode | CnP | Cranelift | CPython | PyPy |
| --- | ---: | ---: | ---: | ---: | ---: |
| fill__range | 30.361 | 17.575 | 19.855 | 11.020 | 5.727 |
| fill__counter | 35.064 | 43.071 | 35.192 | 13.259 | 5.567 |
| fill__expression | 39.235 | 44.342 | 35.757 | 15.029 | 5.834 |
| fill__reversed | 35.318 | 43.087 | 35.716 | 13.024 | 5.604 |
| fill__data | 39.652 | 43.903 | 40.431 | 14.295 | 6.502 |
| numeric__inline | 3.426 | 1.389 | 3.578 | 1.041 | 0.931 |
| numeric__helper | 6.338 | 5.511 | 8.279 | 1.350 | 1.840 |
| numeric__tuples | 3.422 | 1.331 | 3.618 | 0.999 | 0.824 |
| numeric__data | 8.220 | 6.644 | 9.481 | 1.672 | 1.599 |
| histogram__get | 11.248 | 18.386 | 20.513 | 1.291 | 1.939 |
| histogram__membership | 9.813 | 11.737 | 13.953 | 1.473 | 1.847 |
| histogram__helper | 15.864 | 24.448 | 27.129 | 1.885 | 2.540 |
| histogram__unicode | 11.623 | 18.676 | 21.110 | 1.274 | 1.535 |
| objects__fields | 2.964 | 1.117 | 5.537 | 0.697 | 0.783 |
| objects__helper | 5.110 | 4.231 | 7.174 | 0.999 | 1.347 |
| objects__method | 7.746 | 7.418 | 9.719 | 0.932 | 1.046 |
| objects__layout | 2.987 | 1.125 | 5.618 | 0.663 | 0.869 |

## retained

Milliseconds, median request time. `error` cases remain in the table.

| Variant | Bytecode | CnP | Cranelift | CPython | PyPy |
| --- | ---: | ---: | ---: | ---: | ---: |
| fill__range | 30.730 | 17.630 | 17.485 | 11.063 | 5.667 |
| fill__counter | 35.318 | 42.918 | 33.476 | 13.057 | 6.114 |
| fill__expression | 39.286 | 44.618 | 33.858 | 15.222 | 6.017 |
| fill__reversed | 35.234 | 43.169 | 33.666 | 13.092 | 5.631 |
| fill__data | 39.880 | 44.471 | 38.628 | 14.468 | 5.819 |
| numeric__inline | 3.185 | 0.743 | 0.244 | 1.013 | 0.108 |
| numeric__helper | 5.926 | 4.660 | 5.509 | 1.296 | 0.290 |
| numeric__tuples | 3.113 | 0.719 | 0.269 | 0.997 | 0.116 |
| numeric__data | 7.715 | 6.015 | 7.056 | 1.652 | 0.361 |
| histogram__get | 11.210 | 17.963 | 17.492 | 1.181 | 0.746 |
| histogram__membership | 9.657 | 11.410 | 10.444 | 1.397 | 0.932 |
| histogram__helper | 15.667 | 24.298 | 23.819 | 1.782 | 0.967 |
| histogram__unicode | 11.450 | 18.483 | 18.065 | 1.268 | 0.712 |
| objects__fields | 2.785 | 0.610 | 2.582 | 0.612 | 0.110 |
| objects__helper | 4.761 | 3.694 | 3.633 | 0.890 | 0.203 |
| objects__method | 7.288 | 6.933 | 6.182 | 0.884 | 0.132 |
| objects__layout | 2.755 | 0.599 | 2.581 | 0.605 | 0.111 |

## Findings

All 255/255 case/runtime/mode combinations pass exact output checks.

Numeric helper extraction is a large retained-code cliff for tis.
The direct Cranelift loop is faster than CPython but remains slower than PyPy.
Helper extraction raises its time from about 0.24 to 5.51 ms; CnP rises from 0.74 to 4.66 ms.
CPython rises from 1.01 to 1.30 ms; PyPy from 0.11 to 0.29 ms.
This reproduces a general helper-boundary gap outside n-body.

Direct object fields bring CnP close to CPython, but both helpers and bound methods lose that advantage.
Renaming fields/classes and reversing constructor stores does not cause a comparable cliff in these cases.

Histogram updates remain slower than CPython and PyPy even with retained native code.
For get-based updates, bytecode is faster than either native backend.
A membership branch is faster for tis than get-based updates, while CPython prefers get in this run.

The custom counter fill misses the shared fill kernel: a literal stride adds another instruction in the body.
Separate CLI diagnostics record zero region iterations and roughly 598K runtime crossings,
versus about 299K region iterations and 43 crossings for the range form.
This is another matcher coverage gap beyond the earlier invariant-bound case.
The range version still takes about 18 ms: `list.count(False)` remains part of the measured application.
Its Rust implementation scans values using generic equality/protocol handling.
The suite preserves this cost instead of replacing the checksum with sparse indexing.

## Retained variability

| Runtime | Largest sample max/min ratio | Variant |
| --- | ---: | --- |
| bytecode | 1.07 | numeric__inline |
| cnp | 1.28 | objects__layout |
| cranelift | 1.16 | numeric__tuples |
| cpython | 1.24 | numeric__inline |
| pypy | 5.16 | objects__layout |

Ratios above one describe sample spread, not a confidence interval or proof of continued compilation.
Raw warmups/samples retain these outliers. No parity claim follows from isolated medians.

## Reproduction

The generator and drivers live in `benchmarks/variations/`.
The runner uses the archived tis-compatible Flying Circus adapter and frozen worker by default.

```sh
python3 benchmarks/variations/generate.py --output /tmp/tis-custom-new/programs --seed 1807
python3 benchmarks/variations/run.py /tmp/tis-custom-new
python3 benchmarks/variations/analyze.py /tmp/tis-custom-new
```

Raw programs, manifests, runtime hashes/versions, JSON/HTML reports, warmups, samples, diagnostics and logs:
`~/Library/Caches/tis-benchmarks/2026-10-31-custom-variations/`.
