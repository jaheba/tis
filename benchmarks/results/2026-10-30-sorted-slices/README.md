# Sorted support and string slicing

`sorted()` now accepts key and reverse, sharing list.sort’s stable merge sort.
The former insertion-sort path is removed. Key evaluation, comparison callbacks and reverse truth conversion keep
temporary values rooted across GC; sorted consumes its iterable before validating sort keywords, matching Python 3.14+.

Short string slices previously built a character array for the entire source on every operation.
Contiguous ASCII slices now use byte offsets and copy just the result, keeping short results inline.
Other string slices stream characters instead of building that array. ASCII detection still scans the source;
this change does not claim constant-time slicing.

## Results

The unchanged k-nucleotide kernel now passes against CPython’s exact output.
Canonical Benchmarks Game coverage rises from four to five of ten tasks.
The previously supported binary-trees, fannkuch-redux, n-body and spectral-norm programs were rechecked in both tiers.

Input: the same FASTA n=20,000 fixture and serial Python kernel as the preceding Game run.
Only the runtime changed; the corrected in-memory I/O adapter and output formatting are unchanged.
Three samples, one repeated warmup and one RSS sample, run serially with Flying Circus --quick.
Worker startup and fixture setup are excluded; compilation, kernel execution and output are included.
Repeated requests use fresh VM sessions, so these are not retained-VM warmed-JIT times.
Native enables experimental readonly/mutation regions.

| Runtime | One-shot ms | Repeated ms | Peak process RSS MiB |
|---|---:|---:|---:|
| tis-bytecode | 916.49 | 923.92 | 22.98 |
| tis-native | 1050.25 | 1046.69 | 28.67 |
| cpython | 75.50 | 76.04 | 30.92 |
| pypy | 58.11 | 51.98 | 86.30 |

Sorting alone got the kernel past the missing keyword support, but bytecode then timed out after 30 seconds.
After removing the full-source slice copy, complete CLI runs take about 0.91 seconds in bytecode and 1.05 seconds native.
Those CLI timings include startup, fixture setup and compilation; they are separate from the worker table above.
Native remains slower than bytecode and substantially slower than CPython/PyPy.

A focused probe takes 5,000 18-character windows from a 100 KB string.
With compilation/startup included, three shuffled fresh-process bytecode samples give medians of
**311.20 ms before / 10.84 ms after** (about 28.7× faster).
The first after launch was a 256 ms outlier; all initial and follow-up samples are retained externally.
These quick timings are diagnostic rather than significance claims.

## Validation and regressions

cargo fmt --all; all-target Clippy with -D warnings; cargo test --no-fail-fast: 413 passed.
The sorting and slicing fixtures pass CPython 3.14.2/3.15.0 and all four tis tiers with both bytecode encodings.
Tests cover stable reverse ordering, key-once behavior, allocating callbacks, exceptions and 72 reference slice cases.
The Monty fixture audit remains at 66 exact passes; broader fixtures still reach unrelated missing features.
Previously supported Game cases match their recorded stdout in eight output checks.
Short sieve, n-body, Barnes-Hut and unpacking checks match output; no material timing regression was observed.
A noisy initial sieve sample was followed up: bytecode 169.54→169.06 ms, native 48.39→48.43 ms.

## Next performance target

A separate native k-nucleotide diagnostic records 3.36 million runtime calls and 3.32 million root synchronizations,
with about 17 ms spent compiling. These counters suggest profiling generic string/dictionary helpers and temporary
slice objects next; they are not sampled time attribution.

## Reproduction and evidence

Git keeps this report and reproduce.py only. Raw samples, profiles, binaries, source hashes and snapshots are cached at:

`~/Library/Caches/tis-benchmarks/2026-10-30-sorted-slices.tar.gz`

On this machine, with the Flying Circus environment and CPython/PyPy installed:

```sh
python3 reproduce.py --output /tmp/tis-sorted-rerun
```

Use --worker /path/to/target/release/examples/circus to test a newly built worker instead of the frozen one.
Raw output must be outside the repository. Source/build provenance and all unsuccessful/control runs are in the archive.
No commits were made for these changes.
