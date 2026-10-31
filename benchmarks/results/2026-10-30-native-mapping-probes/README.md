# Closed native string and mapping probes

Baseline: main 60e48bc plus the pending general augmented-assignment change in
`../2026-10-30-general-augmented/README.md`. Candidate preserves that work and adds
closed probes in the existing Cranelift gateway. This is a Cranelift change;
copy-and-patch retains its general runtime path.

Exact builtin dictionary reads, membership, string-keyed stores and scalar item
updates can run without rebuilding VM roots. Mappings with protocol keys use the
original helper. Contiguous ASCII slices with primitive/default bounds also use
a closed path; heap-string allocation requires the same one-object, no-collection
budget as the existing tuple probe. Unicode, other steps/index hooks, missing
keys, active feedback and allocation pressure retain the original rooted path.
Guards miss before effects. Buffered region writes are flushed first; successful
results reload native boxed SSA values. Heap identities/generations remain checked.

No HIR/LIR semantic effects are weakened. Runtime services remain independent of
frame layout. Native boundaries still count as runtime calls: these probes avoid
the full dispatcher, not the Rust C boundary itself.

## Matching retained execution

One compiled module/code object and one VM or Python namespace, seven executions:
first separately recorded, three further warmups, three measured samples. Whole
module execution includes input-adaptor construction and output capture; frontend
compilation is outside the samples. Identical source/output across runtimes.

| Runtime | Retained median ms |
| --- | ---: |
| tis native before | 304.1 |
| tis native after | 187.3 |
| CPython 3.15 | 74.4 |
| PyPy 7.3.16 | 50.6 |

The native gain is about 1.6×; tis remains about 3.7× slower than PyPy and 2.5×
slower than CPython. All measured tis native compilation deltas are zero.
Compilation does not explain the remaining gap. This is quick sampling, not a
statistical significance claim or a claim of full JIT convergence.

## Fresh sessions

Flying Circus `--quick -x1`: three timing samples/mode, one repeated warmup, one
RSS sample. Worker startup and marker-delimited fixture preparation are excluded;
kernel compilation/execution/output are included. Repeated requests use fresh
VMs, so these are separate from the retained measurements above. The identical
FASTA n=20,000 program and full-size input are unchanged.

| Runtime | One-shot ms | Repeated ms | Peak RSS MiB |
| --- | ---: | ---: | ---: |
| tis bytecode | 222 | 225 | 24.7 |
| tis native | 192 | 192 | 29.4 |
| CPython 3.15 | 75.1 | 73.4 | 34.7 |
| PyPy 7.3.16 | 55.1 | 51.3 | 89.3 |

A separate five-sample alternating full-program CLI comparison includes launch,
frontend and input-adaptor setup: native 323 → 206 ms; bytecode 234 → 233 ms.
Exact stdout matches before/after and CPython.

## Counters and remaining costs

Counters come from separate full-program CLI runs, outside timing samples.

| Counter | Before | After |
| --- | ---: | ---: |
| Native root synchronizations | 2,621,447 | 27,262 |
| Native C runtime boundaries | 2,662,469 | 2,665,828 |
| Closed dictionary reads/membership | — | 1,194,333 |
| Closed builtin stores/updates | — | 698,833 |
| Closed string slices | — | 701,019 |

Root rebuilding drops by about 99%. Boundary counts include misses followed by
the general helper, explaining the small increase in C crossings. Both runs have
zero compilation errors and deoptimizations.

A separate one-second CPU sample of repeated kernels is led by the guarded
gateway, SipHash, Slot decoding, dictionary lookup, string creation and UTF-8
validation. Root synchronization is no longer a leading leaf. The next large
step is direct typed container/string operations and lookup reuse, reducing
boxed operand traffic and per-instruction gateway dispatch. This change does not
claim those operations are fully inlined into generated machine code.

## Controls and validation

Three-sample CLI controls, before → after in milliseconds:

| Workload | Bytecode | Native |
| --- | ---: | ---: |
| sieve | 169.5 → 168.0 | 47.4 → 48.9 |
| game_nbody | 276.1 → 274.8 | 36.8 → 36.6 |
| barnes_hut | 2904.9 → 2906.1 | 657.8 → 663.0 |
| perf_unpack_sequence | 174.0 → 173.0 | 219.5 → 227.8 |

The initial native unpacking sample looked about 4% slower. A five-sample
alternating follow-up gives 219.7 → 219.8 ms. All control outputs match; short
samples cannot rule out small changes.

Formatting, all-target Clippy with `-D warnings`, and all 423 tests pass. New
regressions cover long-key allocation pressure and actual reclamation, custom
equality/indexing, Unicode/stepped/bounded slices, errors, eager feedback, and
references surviving collection after removal from their dictionary. Existing
native regions and protocol/deoptimization suites also pass.

Release/default features, macOS arm64. Build with `cargo build --release --locked
--bin tis --example circus --example retained_benchmark`. Frozen binary hashes,
source snapshots, adapter source, samples, profiles and validation logs are outside
Git at `~/Library/Caches/tis-benchmarks/2026-10-30-native-mapping-probes.tar.gz`.
`paired.py` and `controls.py` reproduce process comparisons; `run_quick.py` runs
Flying Circus; `python_retained.py` and `binaries/*-retained` reproduce retained
execution. Match the archived runtime versions and CLI flags when replaying.
