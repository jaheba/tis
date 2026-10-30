# Direct HIR slice reads

Baseline: 0354d7b. Candidate adds `CoreOp::GetSlice(receiver, start, stop, step)`
for direct read syntax. Bytecode and LIR use the same runtime operation. Exact
builtin strings, lists, tuples and bytes normalize bounds and slice without
allocating an intermediate Python slice object. Custom receivers materialize the
original slice and use the existing indexing protocol.

Bounds expressions retain left-to-right evaluation. Index conversion runs step,
start, stop; zero steps fail before converting start or stop. Receiver and raw
bounds stay rooted across user callbacks. Augmented assignments retain one slice
object shared by the get/set calls; extended tuple indices retain their slices.
LIR effects remain conservative because index conversion can execute Python.

## Measurement

Unchanged Benchmarks Game k-nucleotide, FASTA n=20,000 (100,000-character third
record). Release/default features on macOS arm64. Flying Circus `--quick -x1`,
three timing samples/mode, one repeated warmup and one RSS sample. Timings include
request compilation/execution/output and exclude startup and fixture preparation.
Repeated mode uses fresh VMs, not retained warmed JIT code. Exact output checked
against CPython. Quick samples are approximate.

| Runtime | One-shot ms | Repeated ms | Peak RSS MiB |
| --- | ---: | ---: | ---: |
| tis bytecode | 266 | 260 | 23.8 |
| tis native | 376 | 373 | 29.3 |
| CPython 3.15 | 78.0 | 75.6 | 30.9 |
| PyPy 7.3.16 | 57.3 | 53.2 | 85.4 |

A separate five-sample alternating CLI comparison includes launch and fixture
setup: bytecode 316 → 273 ms (14% less), native 461 → 387 ms (16% less).
The initial three-sample comparison also improved, but had first-launch outliers;
all samples are retained externally. Native remains slower than bytecode and the
Python baselines.

Runtime counters confirm removal of 701,044 callbacks and root syncs:

| Counter | Before | After |
| --- | ---: | ---: |
| Native runtime calls | 3,363,513 | 2,662,469 |
| Native root syncs | 3,322,491 | 2,621,447 |
| Native code bytes | 26,232 | 23,852 |

Both runs had zero compilation errors and deoptimizations.

## Controls and validation

Three-sample CLI controls, before → after in milliseconds:

| Workload | Bytecode | Native |
| --- | ---: | ---: |
| sieve | 170.4 → 174.1 | 49.1 → 51.3 |
| game_nbody | 276.8 → 275.8 | 36.8 → 37.1 |
| barnes_hut | 2838.3 → 2842.6 | 666.4 → 659.5 |
| perf_unpack_sequence | 171.8 → 182.2 | 217.7 → 216.2 |

The apparent unpacking bytecode and sieve native regressions disappeared in
five-sample alternating follow-ups: unpacking 174 → 170 ms, sieve 48.8 → 48.7 ms.
All control outputs match before/after. These short runs cannot rule out small
regressions.

Formatting, all-target Clippy with `-D warnings`, and all 417 tests pass.
The new regression runs both bytecode encodings and all four execution tiers,
covering Unicode, builtin sequences, custom raw bounds, extended indexing,
augmented key identity, conversion order, zero steps, and receiver mutation.
The same Python regression passes in CPython.

Raw sources, scripts, frozen release binaries, hashes, Flying Circus reports,
control samples and validation logs are archived outside Git at
`~/Library/Caches/tis-benchmarks/2026-10-30-fused-slices.tar.gz`.
The archive includes `compare.py`, `followup.py`, and `run_quick.py` for reproduction,
plus the Flying Circus adapter source. Source snapshots include the complete
unchanged standalone k-nucleotide program. Build: `cargo build --release --locked
--bin tis --example circus`, default features; native region flags enabled.
