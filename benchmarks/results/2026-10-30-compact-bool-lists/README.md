# Compact boolean lists after sieve phase profiling

Base revision: 38291ae. The unchanged photographed sieve at 30,000,000 is sieve_photo.py; all runs print 1857859. Source and executable hashes are recorded in provenance.json. Builds and tests were completed before serial benchmark sampling; no interpreter execution is delegated to Python.

## Phase diagnosis

The split allocate/mark/collect program is diagnostic: it changes function/JIT boundaries, and is not the end-to-end performance baseline. Each sample uses a fresh VM/process, with definitions and startup excluded from phase timers; first-call native compilation is included. Rust host roots keep the flags alive between calls. Three samples, median milliseconds. Original pre-change observations are retained in before-phases.json; the baseline phase driver was subsequently rebuilt from byte-identical base source and frozen for reproduction.

| Runtime | Allocate | Mark | Collect |
|---|---:|---:|---:|
| tis before | 45.89 | 828.32 | 386.82 |
| tis after | 2.96 | 590.14 | 365.91 |
| CPython | 26.30 | 2395.81 | 473.29 |
| PyPy | 119.72 | 342.95 | 383.01 |

Marking was the dominant gap. Collection was already close to PyPy despite roughly 1.86 million append callbacks. The change therefore targets flag storage and memory traffic.

## Unchanged end-to-end program

Three paired fresh-process samples per runtime in randomized order, median elapsed milliseconds and peak RSS MiB from wait4 on macOS ARM64. Startup, parsing, HIR/native compilation, execution, stdout capture and exit are included. No workload warmup; executable verification/file caches were primed with --version. Both tis variants use identical --stats flags. The before executable is the prior installed CLI; all four runtimes execute identical full-size source.

| Runtime | Elapsed (ms) | RSS (MiB) |
|---|---:|---:|
| before-bytecode | 4687.08 | 492.9 |
| before-native | 1281.30 | 496.8 |
| after-bytecode | 4568.81 | 63.9 |
| after-native | 976.01 | 67.8 |
| cpython | 2977.12 | 316.5 |
| pypy | 873.48 | 620.5 |

Homogeneous boolean lists can retain one byte per element. Primitive repetition copies the compact buffer directly, avoiding a temporary array of 16-byte Values. Native byte loads/stores guard identity, generation, list storage and bounds. Non-boolean stores use the existing helper before converting to boxed values, preserving bool/int distinctions and arbitrary references. GC scans boxed lists but skips primitive buffers and their boxed snapshots.

A shared boxed snapshot revokes direct native boolean writes via an interior Cell token. Only an exclusive mutation clears the snapshot and republishes a writable pointer. The byte-storage pointer shares the native sequence address field, but kind 8 is decoded only as bytes; it is never dereferenced as a Value. Existing mutation/GC invalidation, cached-view restrictions and generation checks remain in place.

A first candidate regressed unpacking. Assembly showed that merging a Bool constructor with whole-Value reads made LLVM copy enum/padding fields individually. The final getter isolates that constructor behind a non-inlined helper, restoring 16-byte vector copies for ordinary values. Intermediate measurements and assembly are in scalarized-getter-pass/ and probe-getter-assembly.txt.

## Existing workload checks

Three serial paired samples per tier, exact stdout verified against the preceding CPython-validated snapshots. Native Barnes-Hut uses matching readonly/mutation-region flags. Small differences, especially process startup cases, are noisy.

| Workload | Bytecode before | After | Native before | After |
|---|---:|---:|---:|---:|
| sieve | 171.63 | 168.79 | 50.68 | 48.03 |
| nbody | 264.44 | 266.70 | 36.34 | 35.26 |
| barnes-hut | 2796.17 | 2781.99 | 653.04 | 650.30 |
| ticket-search | 41.22 | 41.30 | 89.63 | 89.20 |
| bisect-search | 22.71 | 23.67 | 30.15 | 31.23 |
| bisect-insort | 8.83 | 8.88 | 13.28 | 13.20 |
| perf_unpack_sequence | 164.53 | 169.43 | 214.04 | 215.04 |
| perf_json_loads | 4.57 | 5.03 | 4.76 | 4.85 |
| perf_json_dumps | 12.84 | 12.76 | 15.19 | 15.44 |
| text_json_parse_small | 4.04 | 3.78 | 3.88 | 4.03 |
| text_json_transform_small | 4.47 | 4.02 | 4.13 | 3.91 |
| text_json_lines_small | 4.26 | 4.07 | 4.08 | 3.81 |
| text_csv_aggregate_small | 9.70 | 10.28 | 13.88 | 14.16 |
| text_csv_cleanup_small | 13.54 | 13.76 | 16.97 | 17.14 |

## Follow-up

Five native sieve/PyPy samples, seven unpacking/n-body samples and nine small JSON samples. All output checks pass.

| Workload | Runtime | Median ms |
|---|---|---:|
| photo | before-native | 1267.58 |
| photo | after-native | 970.71 |
| photo | pypy | 787.63 |
| unpack | before-bytecode | 164.36 |
| unpack | before-native | 213.12 |
| unpack | after-bytecode | 168.79 |
| unpack | after-native | 215.06 |
| nbody | before-bytecode | 264.91 |
| nbody | before-native | 36.17 |
| nbody | after-bytecode | 267.81 |
| nbody | after-native | 36.61 |
| json-parse | before-bytecode | 3.93 |
| json-parse | before-native | 4.10 |
| json-parse | after-bytecode | 3.87 |
| json-parse | after-native | 4.07 |

The large unpacking regression is fixed; the final follow-up still has about 2.7% higher bytecode unpacking time and about 1% higher n-body time. These costs are retained in the evidence rather than summarized as universal speedups. PyPy remains faster: its one-shot time varied around 0.79–0.87 seconds here, compared with roughly 0.97 seconds for tis native.

## Validation and reproduction

Formatting, all-target Clippy with warnings denied, and the full suite pass (405 tests). New coverage checks compact repetition/GC without materialization, bool identity, mixed integer/reference writes, aliasing, negative/bool indices, mutation, native stores, and shared snapshot invalidation both with and without intervening GC. The native gateway/provenance review is documented above; copied heap storage modules were not modified.

Build with cargo build --release --offline --locked --bin tis --example sieve_phases --example bench --example run. Run tis --tier native tests/fixtures/sieve_comprehension.py for the real workload. Run the sieve_phases example with this directory's phases.py for diagnostic JSON. Frozen before/after CLI and phase executables are locally retained under binaries/ and gitignored. measure.py and followup.py replay the paired workload samples; python_phases.py measures the same diagnostic phases in CPython/PyPy. Raw commands, outputs, RSS and native compile/stat counters are retained in JSON files, with validation logs under validation/.
