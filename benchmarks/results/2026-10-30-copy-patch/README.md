# Copy-and-patch versus Cranelift, 2026-10-30

Copy-and-patch links precompiled Rust stencils quickly, but its generated execution remains slower than Cranelift.
This experiment ports closed builtin probes to CnP, schedules parallel edge moves, and avoids redundant stack publication.

## Retained execution

Milliseconds; smaller is better.
The baseline includes pending general augmented-operation and native mapping work on top of `60e48bc`.

| Workload | CnP before | CnP after | Cranelift after | CnP reduction |
|---|---:|---:|---:|---:|
| k-nucleotide | 415.32 | 269.32 | 188.37 | 35.2% |
| n-body, 20k | 179.57 | 160.46 | 8.82 | 10.6% |
| sieve, 1M | 536.05 | 401.17 | 40.06 | 25.2% |
| Barnes–Hut | 5725.05 | 5512.74 | 576.28 | 3.7% |

These are quick samples, not confidence intervals.
Unchanged Cranelift controls moved about 2–3%; the small Barnes–Hut improvement is inconclusive.
Each process compiles the frontend once and executes the whole module seven times in one VM.
The first execution is separate, three more executions warm it, and the last three form the retained median.
Module execution includes fixture construction and redefinition of functions/classes.
Every execution's stdout matches CPython exactly.

Barnes–Hut recompiled during the middle measured execution in both backends, before and after.
The final measured compilation deltas were CnP `[0, 468208, 0]` ns and Cranelift `[0, 9246084, 0]` ns.
Those costs remain included; this is not a compilation-free kernel measurement.
Other workloads had zero compilation deltas during measured executions.

## First result and compilation

First execution excludes frontend compilation and includes backend compilation.

| Workload | CnP first, ms | Cranelift first, ms | CnP compile, ms | Cranelift compile, ms |
|---|---:|---:|---:|---:|
| knucleotide | 272.664 | 202.013 | 0.265 | 12.824 |
| nbody | 163.781 | 33.029 | 0.290 | 10.484 |
| sieve | 641.060 | 44.891 | 0.117 | 4.810 |
| barnes | 5532.137 | 674.597 | 2.203 | 89.583 |

A five-pair alternating fresh-process follow-up used a 5,000-iteration integer sum.
Median first execution was 0.596 ms for CnP versus 1.996 ms for Cranelift.
Retained medians were 0.384 versus 0.035 ms.
Backend compilation was 0.028 versus 1.686 ms.
Fast compilation helps short work; Cranelift already wins the first execution on the larger workloads above.
These timings exclude process startup and frontend work and must not be called CLI end-to-end latency.

The existing `backend_compare` diagnostic measured 7.167 µs CnP linking versus 211.167 µs Cranelift compilation.
It excludes common LIR lowering and uses eager diagnostics; its call timings do not represent hot application execution.
Code size was 1,300 bytes plus 2,520 bytes patch data for CnP versus 628 bytes code for Cranelift.

## Changes and remaining costs

- Closed builtin probes reuse audited runtime operations and miss before effects.
  Misses, reopened profiling windows, Python callbacks and collecting operations retain the full gateway.
- Parallel edge moves omit self-copies and schedule acyclic copies directly.
  Cycles use one temporary slot and preserve fanout.
- Quiet CnP execution keeps slots canonical and lets collecting helpers publish verified root maps.
  Eager profiling retains stack publication.

CnP still loads/stores its register image between stencils and enters runtime callbacks at block boundaries.
Cranelift also supports sequence-loop regions, recursive read-only kernels and buffered mutation kernels absent from CnP.
These capability differences prevent attributing the entire gap to compilation technique alone.
A larger next experiment should share region planning and add fused, register-aware loop stencils.
Calling Cranelift from CnP would not demonstrate a pure copy-and-patch improvement.

## Reproduction and validation

Build serially, then run samples without builds or tests in parallel:

```sh
cargo build --release --offline --locked --bin tis --example retained_benchmark --example backend_compare
cargo run --release --offline --locked --example backend_compare
# Use the frozen executable, source and CPython output from the external archive:
after-retained workload.py copy-patch workload.stdout
after-retained workload.py native workload.stdout
```

The external archive contains exact workload sources, scripts, raw results, binaries, hashes and validation logs.
Archive: `~/Library/Caches/tis-benchmarks/2026-10-30-copy-patch.tar.gz`.
The source snapshot includes pending earlier work; a Git revision alone cannot reproduce it.
No raw samples or executable artifacts are added to Git.

Validation: formatting, all-target Clippy with warnings denied, and all 424 tests passed.
Regression coverage includes parallel-copy cycles/fanout, GC pressure, custom protocols, Unicode slicing,
profiling windows, deoptimization and tail continuations on a small native stack.
Stencil relocation and tail-continuation build checks passed.
