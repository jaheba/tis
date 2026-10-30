# Full Benchmarks Game coverage: quick diagnostic run

All ten canonical tasks were attempted, plus three legacy Flying Circus variants.
tis passes four canonical tasks in both bytecode and native modes; CPython and PyPy pass all ten.
This is a small-input diagnostic run, not the site’s large-input leaderboard configuration.

## Method

Flying Circus --quick, -x1: three timing samples per runtime/workload/mode, one repeated warmup,
five startup samples and one separate peak-RSS sample. Runtime labels use CPython 3.15.0, PyPy 7.3.16 (Python 3.10.14),
and the current tis release worker at adb81da plus the uncommitted integer changes.
The harness CPython used to capture expected stdout is 3.14.2. All successful runs match that stdout exactly.

The native column enables --readonly-regions and --mutation-regions; these are experimental flags, not default CLI settings.
Bytecode uses register mode and normal adaptive arithmetic. No VM code was changed for this run.
The Rust worker executes tis directly, with no Python delegation.

One-shot measures the first request after readiness. Repeated requests reuse the worker but reset the application session.
Both include kernel compilation, execution and output delivery. Worker startup and fixture preparation are separate.
Repeated results are not retained-VM warmed JIT timings. Peak RSS includes the whole process and untimed fixtures.
Three samples and one warmup provide rough comparisons, especially for PyPy.

## Canonical results

Median milliseconds: one-shot / repeated. A dash means the program could not execute in tis.

| Task | Input | tis bytecode | tis native | CPython | PyPy |
|---|---|---:|---:|---:|---:|
| binarytrees | depth 12 | 460.76 / 432.75 | 597.42 / 589.20 | 53.25 / 53.52 | 41.52 / 40.73 |
| fannkuchredux | n=8 | 160.96 / 154.15 | 217.00 / 216.53 | 22.98 / 22.85 | 16.96 / 16.06 |
| fasta | n=20,000 | unsupported / unsupported | unsupported / unsupported | 22.87 / 21.84 | 34.14 / 27.17 |
| knucleotide | FASTA n=20,000 | unsupported / unsupported | unsupported / unsupported | 75.89 / 73.94 | 57.39 / 52.42 |
| mandelbrot | 256×256 | unsupported / unsupported | unsupported / unsupported | 93.10 / 92.41 | 36.75 / 33.12 |
| nbody | 20,000 steps | 276.21 / 269.79 | 34.51 / 32.80 | 80.49 / 77.51 | 15.24 / 14.37 |
| pidigits | 2,000 digits | unsupported / unsupported | unsupported / unsupported | 288.87 / 285.49 | 141.89 / 140.85 |
| regexredux | FASTA n=20,000 | unsupported / unsupported | unsupported / unsupported | 36.80 / 36.77 | 43.58 / 18.51 |
| revcomp | FASTA n=20,000 | unsupported / unsupported | unsupported / unsupported | 3.37 / 3.07 | 9.81 / 9.60 |
| spectral_norm | n=130 | 289.96 / 288.14 | 426.43 / 420.22 | 69.34 / 67.58 | 17.81 / 17.78 |

## Missing features

| Task | Blocking gap |
|---|---|
| fasta | General yield/generator function support |
| k-nucleotide | sorted keyword arguments (key/reverse) |
| mandelbrot | Complex numbers |
| pidigits | itertools import; generator functions are also used |
| regex-redux | re import |
| reverse-complement | General yield/generator functions; bytes translation/bytearray are also used |

Actual exception text and status classifications remain in the raw reports.
No unsupported program is assigned a timing or counted as passing.

## Peak RSS

One-shot peak process MiB, one sample.

| Task | tis bytecode | tis native | CPython | PyPy |
|---|---:|---:|---:|---:|
| binarytrees | 33.48 | 31.17 | 16.23 | 37.05 |
| fannkuchredux | 7.12 | 16.91 | 15.69 | 34.03 |
| fasta | unsupported | unsupported | 16.83 | 36.39 |
| knucleotide | unsupported | unsupported | 30.92 | 83.05 |
| mandelbrot | unsupported | unsupported | 16.62 | 34.38 |
| nbody | 7.67 | 13.16 | 16.02 | 33.50 |
| pidigits | unsupported | unsupported | 16.30 | 39.41 |
| regexredux | unsupported | unsupported | 18.84 | 49.81 |
| revcomp | unsupported | unsupported | 18.97 | 50.34 |
| spectral_norm | 6.38 | 10.86 | 15.59 | 33.92 |

## Legacy variants

These supplement the ten canonical tasks. Classic fannkuch is distinct from fannkuch-redux; regex-dna is distinct from regex-redux.

| Task | tis bytecode | tis native | CPython | PyPy |
|---|---:|---:|---:|---:|
| legacy_fannkuch | 1934.81 / 1882.93 | 2588.07 / 2539.57 | 275.12 / 270.49 | 86.52 / 84.18 |
| legacy_meteor_contest | failed / failed | failed / failed | 85.68 / 82.64 | 74.39 / 73.76 |
| legacy_regex_dna | unsupported / unsupported | unsupported / unsupported | 258.37 / 247.02 | 180.80 / 156.62 |

Legacy meteor-contest fails in tis with NameError: frozenset. Legacy regex-dna requires re.

## What the run shows

N-body native is about 2.4× faster than CPython and about 2.3× slower than PyPy on these inputs.
The other three supported canonical tasks are slower than CPython, and native is slower than bytecode.
Native-mode counters in diagnostics/stats.json confirm compiled entries or OSR; compilation_errors is zero.
Spectral norm records about 5.76 million native runtime calls and 2.37 million root synchronizations,
while native compilation takes about 15 ms against roughly 420 ms per measured request.
Binary trees records about 6.07 million runtime calls and 2.01 million root synchronizations.
These counts point toward execution/helper overhead as a useful profiling target; they are not sampled time attribution.
Fannkuch-redux instead records three OSR entries followed by three deoptimizations/retirements.

## Sources and adaptations

Original downloaded code and attribution headers are preserved under upstream/, together with the official license.
suite.json records source URLs and source hashes. Flying Circus variants are copied unchanged and retain pyperformance provenance/license.
Official programs use their kernels unchanged; the adaptations replace CLI argv and stdin/stdout with in-memory boundary objects.
Only output-only percent formatting is replaced by equivalent f-strings for three programs.
Binary PBM/reverse-complement output is rendered losslessly as hex for the text worker protocol.
Regex-redux evaluates the official re-based variant’s map serially rather than with multiprocessing.
Six adapted programs have output checked against the original official programs under CPython; see adapter-validation.json.
The regex patterns/substitutions are unchanged; its serial-adapted output is checked across CPython and PyPy.

FASTA stdin fixtures are generated by the official fasta Python program at n=20,000.
Fixture creation and boundary-object construction occur outside request timing using the harness setup marker.
Fixture parsing into algorithm data structures remains timed.

The initial k-nucleotide run exposed an adapter str.find dependency rather than a kernel gap.
The adapter was corrected by preparing input lines in fixture setup, then only k-nucleotide was rerun.
selected-results.json uses that rerun for every k-nucleotide runtime/mode; all other rows use the initial run.
Both raw runs, including the initial adapter failure, remain available.

## Evidence and reproduction

Git retains this report and reproduce.py only. Raw JSON, snapshots, diagnostics and frozen binaries are cached at:

`~/Library/Caches/tis-benchmarks/2026-10-30-benchmark-game.tar.gz`

The archive contains the original and corrected runs, selected results, runtime/source hashes, source URLs,
licenses, adapter output checks and source/harness snapshots.
It is local evidence, not a repository dependency.

To rerun on this machine with Python 3.12+ and the recorded CPython/PyPy installations:

```sh
python3 reproduce.py --output /tmp/tis-game-rerun
```

The script extracts the cached evidence outside the repository and launches the frozen Rust worker through
Flying Circus. The rerun uses the corrected k-nucleotide adapter.
To regenerate from another machine, obtain the evidence archive and pass --archive; source/build provenance is included.
No commits or pushes were made for this benchmark run.
