# Overnight copy-and-patch experiment, 2026-10-30/31

Target: close the execution gap to Cranelift without replacing CnP with an optimizing compiler.
The run stopped at its deadline, 2026-10-31 07:00 UTC /08:00 Europe/Berlin.\nThe heartbeat is paused. Execution parity remains incomplete.

## Latest checkpoint: imports-final

All 463 tests, formatting, stencil audits and all-target Clippy pass. No commit/push.
All CnP operations remain copied offline Rust; it invokes no Cranelift compiler or kernel.

|Workload|Initial CnP ms|Latest CnP ms|Current Cranelift ms|Before → current ratio|
|---|---:|---:|---:|---:|
|knucleotide|263.83|179.38|188.49|1.43× → 0.952×|
|nbody|157.45|39.50|8.60|18.58× → 4.595×|
|sieve|391.09|2.93|37.13|9.90× → 0.079×|
|barnes|5396.94|2946.12|561.07|9.12× → 5.251×|

Quick retained whole-module medians. Barnes includes measured recompilation. Broad parity remains unmet:
K is near Cranelift; sieve has a faster range reduction, while nbody/Barnes still lag.
This is also an optimization-coverage comparison: Cranelift has closed numeric loops and readonly/mutation call regions
that CnP still lacks, while CnP has the new Boolean fill/reduction plans.
Backend compilation counters exclude shared HIR-to-LIR lowering; first VM/process results below include it.
Initial binaries/flags and every accepted/rejected source checkpoint remain in the external manifest.

## Experiment 1: quiet block entries

The baseline CnP calls the VM gateway on every LIR block entry, even with a frozen runtime profile.
Cranelift already updates block bookkeeping inline and defers backedge counters until native execution returns.
The candidate shares its quiet-edge planner and adds two offline Rust stencils for the same policy.

Quiet stencils update the previous HIR origin, OSR skip flag and block-hit counter directly.
Loop-edge counters saturate and merge on normal, guard and error exits through the existing native runtime.
Live observation still uses the ordinary block gateway.
Blocks with incomplete inline origins retain required header callbacks.
The copied image patches Context field offsets from the same-target repr(C) layout; it stores no persistent VM pointer.

## Experiments 1–2: retained execution

Milliseconds; the Cranelift column uses the frozen original executable throughout.
Baseline CnP includes the previous closed-probe and edge-copy improvements.
All binaries include existing uncommitted work on top of `60e48bc`.

| Workload | Baseline CnP | Quiet entries | With field probes | Cranelift | Field stage CnP/Cranelift |
|---|---:|---:|---:|---:|---:|
| knucleotide | 263.83 | 223.35 | 226.45 | 184.15 | 1.23× |
| nbody | 157.45 | 152.00 | 153.22 | 8.47 | 18.08× |
| sieve | 391.09 | 306.81 | 319.08 | 39.52 | 8.07× |
| barnes | 5396.94 | 5167.99 | 4082.64 | 591.91 | 6.90× |

Each retained process compiles the frontend once and executes the same whole module seven times in one VM.
First execution is separate; three more executions warm the VM; the last three form the reported median.
This includes module definitions, fixture construction and captured output, but excludes frontend and process startup.
Every execution matches CPython stdout exactly.
Barnes–Hut recompiles during a measured execution in both backends; those costs remain included.
Check the archived compilation deltas rather than interpreting these as compilation-free kernels.
These are quick samples, not confidence intervals.

Alternating follow-ups confirmed quiet-entry improvements of about 14% for k-nucleotide and 20–22% for sieve.
Final field/quiet follow-ups measured sieve at 305–310 / 306–314 ms and k-nucleotide at 226–227 / 227 ms.
Those controls show no meaningful regression from adding field probes.
N-body follow-ups improved only about 2%; the small n-body/Barnes quiet-entry changes are inconclusive.

## Experiment 2: closed instance-field probes

CnP now uses the existing audited field probe before the full GetAttr helper.
An exact instance-field hit cannot allocate, call Python or collect and therefore needs no root publication.
Missing fields, builtin receivers, descriptors/hooks and open profiling windows retain the full helper.
The stencil preserves the field probe's distinct hit/miss/error ABI; errors never retry the operation.
It does not use Cranelift's direct native field directory or machine code.

Barnes follow-up (candidate): 5148.39 ms; compile deltas [0, 458791, 0] ns.
Barnes follow-up (fields): 4076.75 ms; compile deltas [0, 443333, 0] ns.

The full-size ticket-search fixture is an additional control for builtin attribute misses.

| Ticket-search retained | ms |
|---|---:|
| candidate / copy-patch | 61.94 |
| fields / copy-patch | 62.87 |
| baseline / native | 61.36 |

## Compilation, process latency and memory

Backend counters exclude common LIR lowering; first execution includes it and backend compilation.
First execution still excludes frontend parsing and process startup.

| Workload | Field stage CnP first, ms | Cranelift first, ms | CnP backend, ms | Cranelift backend, ms |
|---|---:|---:|---:|---:|
| knucleotide | 247.301 | 197.820 | 0.413 | 12.934 |
| nbody | 154.858 | 30.928 | 0.300 | 9.443 |
| sieve | 311.188 | 44.739 | 0.119 | 4.686 |
| barnes | 4241.308 | 668.597 | 2.296 | 85.485 |

The following independent CLI runs include process launch, frontend and backend compilation, execution and exit.
RSS is Darwin wait4 peak resident bytes, converted to MiB; these are single samples.

| Workload | Baseline CnP ms / MiB | Field stage CnP ms / MiB | Cranelift ms / MiB |
|---|---:|---:|---:|
| knucleotide | 312.5 / 25.0 | 513.8 / 25.2 | 217.5 / 29.3 |
| nbody | 170.6 / 7.6 | 162.4 / 7.6 | 38.6 / 13.4 |
| sieve | 402.0 / 5.9 | 319.8 / 5.9 | 49.6 / 10.2 |
| barnes | 5626.2 / 11.1 | 4230.3 / 11.5 | 665.9 / 22.4 |

The initial quiet-only k-nucleotide CLI sample was 475 ms versus baseline 313 ms, despite faster retained execution.
Two fresh-process follow-ups measured baseline 293–300 ms and quiet-only 240–242 ms.
The final field-probe executable also had an initial 514 ms launch, then 236 ms in two follow-ups.
Matching baseline follow-ups were 272–278 ms.
Keep both initial outliers visible; a single process launch is not reliable evidence about steady execution.
The sandbox denied time -l's clock sysctl, so the replacement harness uses wait4 for process timing/RSS.

## Counters and profiles

Counts and generated byte totals below are cumulative over seven module executions, not live heap/code sizes.

| Workload | Baseline root syncs | Field stage root syncs | Field stage field hits | Code bytes | Patch bytes |
|---|---:|---:|---:|---:|---:|
| knucleotide | 382271 | 248858 | 133413 | 26280 | 38760 |
| nbody | 2657975 | 2657975 | 0 | 18296 | 33000 |
| sieve | 8132876 | 8132876 | 0 | 10040 | 16800 |
| barnes | 313496965 | 72434058 | 241062907 | 116224 | 188160 |

Quiet entries removed about 19.8 million C calls in k-nucleotide and 57.7 million in sieve over seven executions.
They left root-map counts unchanged.
Closed field probes address a different cost: successful reads avoid the full gateway and its root publication.

A separate 2-second macOS CPU sample used the same n-body algorithm at 200k steps solely to keep it alive for profiling.
The quiet-only sample showed Context::step, heap reads, sync_lir_roots and float-view preparation among hot Rust leaves.
It includes initial interpretation/OSR, and copied stencil PCs are partly unsymbolized.
Do not treat its counts as a precise warmed-kernel cost decomposition.
N-body still performs about seven million float-view preparations and 4.9 million unpack batches across seven runs.
Cranelift's prepared loop regions avoid work that CnP has not yet implemented.

## Validation and reproduction

Formatting, stencil formatting, all-target Clippy with warnings denied, and all 425 tests passed.
Coverage includes precise deferred backedges, exception exits, eager profiling, field payloads, mutation, GC pressure,
noninstance replacements, builtin method fallback, late descriptors/hooks, deoptimization and small-stack tail threading.
Build-time stencil relocation and tail-continuation checks passed.
No commits or pushes were made.

Raw scripts, exact sources/outputs, frozen binaries, hashes, logs and CPU profiles are outside Git:
`~/Library/Caches/tis-benchmarks/overnight-copy-patch-2026-10-30/`.
Run `reproduce.sh` from the saved source snapshot, freeze executables, then invoke `measure.py`, `oneshot.py`,
`followup.py` and `fields_measure.py` from that evidence directory.
No builds, tests or sampling ran concurrently with timed benchmarks.
The installed CLI is updated only from a validated frozen executable.

Work continues until the overnight deadline; this report does not claim Cranelift parity.


## Experiment 3: exact sequence reads and cursor steps

Successful list/tuple GetItem and unfinished iterator steps now use closed, nonallocating Rust probes.
They handle boxed values, compact floats and byte boolean storage without publishing a root map.
Out-of-bounds indices, custom index/iterator protocols and exhaustion retain the original full helper.
A cursor advances only after a successful read; exhaustion stays sticky through the ordinary path.

A new offline IsDone stencil returns false for immediates without a runtime call.
Heap sentinels and reopened observation still use the original callback.
Regression tests cover aliasing, list growth/shrink, custom sequence effects, GC after owner removal,
heap exhaustion sentinels and eager observation.

| Workload | Field stage, ms | Latest CnP, ms | Current Cranelift, ms |
|---|---:|---:|---:|
| knucleotide | 228.26 | 219.37 | 186.47 |
| nbody | 153.05 | 121.33 | 8.52 |
| sieve | 311.27 | 239.95 | 38.34 |
| barnes | 4337.30 | 4215.79 | 568.95 |
| ticket_search | 64.08 | 55.35 | 61.58 |

N-body follow-ups measured latest 121.57 ms versus field-stage 155.03 ms.
Sieve follow-ups measured latest 240.08 ms versus field-stage 304.65 ms.
An earlier sequence variant had a noisy sieve follow-up at 290 ms; it remains in the archive.
Barnes–Hut changed only a few percent in this step, so no additional Barnes gain is claimed.

Ticket-search follow-up measured latest CnP 57.72 ms versus field-stage 65.88 ms and frozen Cranelift 59.57 ms.
Its primary sample was 55.35 versus current Cranelift 61.58 ms.
That supports comparable performance on this control, not a general claim that CnP beats Cranelift.

| Workload | Field-stage root syncs | Latest root syncs | Latest C calls |
|---|---:|---:|---:|
| knucleotide | 248858 | 248795 | 23957968 |
| nbody | 2657975 | 559590 | 17346703 |
| sieve | 8132876 | 1125891 | 44842858 |
| barnes | 72434058 | 71912953 | 578897602 |

These counters span seven executions.
N-body float-view preparation and unpack counts remain about 7.0 million and 4.9 million respectively.
Closed probes remove conservative root work while retaining their C boundary; prepared/direct sequence access remains open.

### Two corrected approaches

The first GetItem variant attempted the sequence helper before every dictionary lookup.
Two paired Cranelift controls showed k-nucleotide at 191.6–192.2 ms versus frozen baseline 182.4–183.6 ms.
Dispatching primitive integer/bool indices to the sequence helper keeps string-key dictionaries on their original path.
The final follow-up was 184.79 ms versus frozen baseline 188.57 ms; the primary final result was 186.47 ms.
The first variant is archived, not silently substituted for the corrected build.

LLVM emitted an unsupported literal-pool relocation for a constant Bool Slot in the first IsDone stencil.
The build audit rejected it.
Separate volatile word stores avoid referencing build-time rodata; relocation and tail-continuation audits remain intact.

### First execution and compilation, latest build

First execution excludes the frontend/process and includes common lowering plus backend compilation.
Backend counters exclude common lowering.

| Workload | CnP first, ms | CnP backend, ms | Current Cranelift first, ms | Cranelift backend, ms |
|---|---:|---:|---:|---:|
| knucleotide | 235.233 | 0.380 | 201.135 | 12.411 |
| nbody | 124.814 | 0.301 | 31.512 | 9.466 |
| sieve | 237.558 | 0.111 | 43.913 | 4.605 |
| barnes | 4218.752 | 2.131 | 653.271 | 81.680 |

Barnes still recompiled during one measured execution; the latest CnP deltas were [0, 455792, 0] ns.
Other primary workloads had zero measured compilation deltas.
Compilation remains roughly 30–40 times cheaper in CnP on these samples.

### Fresh processes versus a newly copied executable

The first CLI execution from each newly copied file was unusually slow, including versions without sequence changes.
For the latest build, even --version took 267 ms before any frontend, VM or JIT construction.
This isolates some latency outside VM/JIT work but does not establish its OS-level cause.

The following latest CLI samples ran after that explicit version probe.
Each starts a new process/VM, compiles the entire program and exits; there is no workload/JIT warmup.
The original first-file slow launches remain reported above and archived.

| Workload | Latest fresh-process ms | Peak RSS MiB |
|---|---:|---:|
| knucleotide | 240.54 | 24.86 |
| nbody | 130.52 | 7.58 |
| sieve | 243.71 | 5.92 |
| barnes | 4188.99 | 11.17 |

The --version probe cost is separate and must be included when discussing the first use of a newly copied installation.
It must not be described as VM/JIT warmup or hidden in first-install latency.

The final source and additional frozen binaries are preserved with the other evidence.
This earlier checkpoint used sequence-keyed; the overview names the newest validated build.

## Experiment 4: direct fields with a shared native header

The runtime and offline stencils now share NativeFields, NativeFieldDirectory and FieldSlot declarations.
The standalone stencil crate includes the actual Value definition and the same pure Slot boxing conversion.
Heap construction/refresh and Python protocol fallback remain in their original runtime modules.
No second Value layout decoder was introduced.

A repr(C) NativeHeader is Context's first field, with a compile-time offset-zero assertion.
It borrows the live directory Cell and protocol flag and holds shared quiet-block/profile counters.
Both backends use offsets from this header; the rest of execution state remains private to Context.

A direct field read checks live observation, reference kind, heap ID, directory bounds, generation, instance kind,
protocol mode, shape identity and cached slot bounds before reading the typed Value.
No row/value borrow survives its fallback or tail continuation.
The cache belongs to Rc-held immutable LIR; no instance pointer is cached in code.
Misses retain the existing field probe/full helper and their distinct error conventions.
The native directory is enabled only for artifacts with eligible direct instance reads.

| Workload | Previous CnP, ms | Direct CnP, ms | Current Cranelift, ms |
|---|---:|---:|---:|
| knucleotide | 219.45 | 218.80 | 188.64 |
| nbody | 123.08 | 122.39 | 8.54 |
| sieve | 240.44 | 240.29 | 37.60 |
| barnes | 4287.63 | 3668.74 | 579.55 |
| ticket_search | 56.59 | 58.69 | 59.72 |

Barnes follow-up: previous 4184.33 ms, direct 3681.90 ms, about 12% faster.
N-body, sieve and k-nucleotide changed only a few percent; no improvement is claimed there.
Ticket-search follow-up was 56.87 versus 56.68 ms, supporting no material regression.

Over seven Barnes executions, direct stencils handled about 241 million field hits and removed those C crossings.
Total calls fell from about 579 million to 338 million; root-map counts stayed unchanged.
Generated code grew from about 117 KB to 160 KB; patch data is unchanged.
These cumulative code totals are not live RSS.
This result shows that eliminating many calls does not eliminate the remaining frame, helper and stencil costs.

### Rejected global inlining policy

The first shared boxing conversion used inline(always) in both runtime and offline builds.
Ticket-search regressed from 56.59 to 66.53 ms in CnP; current Cranelift was also slower at 67.79 ms.
The first artifact and samples remain archived as direct-fields.
The sampled symbol span of Context::step(false) grew from roughly 10,080 to 10,716 bytes; this suggests codegen cost,
but does not establish a complete CPU attribution.

The corrected build forces inlining only in the standalone stencil compilation via tis_offline_stencils.
Runtime callers retain normal inlining choices.
This preserves the self-contained stencil audit without imposing the same policy on every runtime call site.
The corrected artifact is direct-scoped; both backends recovered ticket-search throughput.

### Compilation and fresh processes

Backend counters exclude common lowering; first execution includes it but excludes frontend and process startup.

| Workload | CnP first, ms | CnP backend, ms | Current Cranelift first, ms | Cranelift backend, ms |
|---|---:|---:|---:|---:|
| knucleotide | 222.646 | 0.226 | 205.170 | 11.220 |
| nbody | 124.233 | 0.283 | 31.580 | 9.459 |
| sieve | 246.589 | 0.120 | 43.087 | 4.783 |
| barnes | 3716.092 | 2.309 | 660.649 | 80.825 |

Barnes CnP compilation deltas during measured executions were [0, 462500, 0] ns.
Its costs remain included.
Other primary measured executions had no compilation deltas.

| Workload | Previous fresh process ms / MiB | Direct fresh process ms / MiB |
|---|---:|---:|
| knucleotide | 240.54 / 24.86 | 254.01 / 25.66 |
| nbody | 130.52 / 7.58 | 138.26 / 7.47 |
| sieve | 243.71 / 5.92 | 242.80 / 5.95 |
| barnes | 4188.99 / 11.17 | 3714.76 / 11.83 |

Both sets use the explicit --version preflight described earlier, without workload/VM/JIT warmup.
The first direct-scoped --version launch cost 310 ms and remains a separate first-file cost.
Single process samples remain noisy; use retained comparisons for the field speedup conclusion.
Barnes RSS rose modestly with the shadow directory and larger code.

Validation includes all payloads (inline UTF-8, heap strings, references, NaN and signed zero), directory growth,
collection, stale/reused and foreign identities, late descriptors/hooks, eager profiling and small-stack tail threading.
All 428 tests, formatting, all-target Clippy and relocation/tail-continuation checks passed.
The corrected source snapshot and hashes are archived; the installed CLI is verified against its frozen binary.

## Experiment 5: direct float views and small exact unpack

Float view stencils now load fresh, generation-checked rows and require exact float storage, expected length,
mutable pointer provenance and a live snapshot-write permission token.
A shared generic borrow revokes permission when it caches boxed values; the exclusive fallback clears the snapshot
and renews pointer/permission before a generated write can occur.
All observing view flags and original guard/exception paths remain available.

Native float pointers now come from Vec::as_mut_ptr, avoiding an escaping temporary unique slice borrow.
Heap row publication and region preparation use that same raw projection.
No const-derived pointer is cast writable.

Small exact unpack stencils check source kind and exact length, then copy up to three typed Values.
The linker validates every extra destination offset and lowering validates the adjacent SequenceGet bundle.
Observing windows and misses retain the original UNPACK_BATCH callback.
No heap reference is retained outside the invocation image or across an unrooted safepoint.

| Workload | Direct-fields CnP pilot control, ms | Views/unpack CnP, ms | Current Cranelift, ms |
|---|---:|---:|---:|
| knucleotide | 221.46 | 211.07 | 175.75 |
| nbody | 123.62 | 87.90 | 8.59 |
| sieve | 246.54 | 233.79 | 38.42 |
| barnes | 3673.87 | 3547.36 | 567.22 |
| ticket_search | 58.11 | 56.40 | 58.96 |

Follow-up retained medians, including any backend compilation:
- nbody: views-final 89.07 ms (compile deltas [0, 0, 0] ns); direct-scoped 123.93 ms (compile deltas [0, 0, 0] ns).
- sieve: views-final 232.66 ms (compile deltas [0, 0, 0] ns); direct-scoped 239.34 ms (compile deltas [0, 0, 0] ns).
- barnes: views-final 3568.38 ms (compile deltas [0, 461208, 0] ns); direct-scoped 3694.17 ms (compile deltas [0, 471833, 0] ns).

An initial sieve pilot reached 286 ms, suggesting a regression.
Two paired repeats gave views 243.30/242.74 versus control 239.90/242.42 ms.
All its code size/call/root counters were unchanged; the initial outlier is preserved, not treated as a verified regression.

The first regression test had zero inline-view hits because leaf bootstrap handled its view preparation in Rust.
Adding an ordinary loop forced execution through the intended stencil path.
Specific hit counters matter even when the CnP tier itself is active.

| Workload | Inline float-view hits | Inline unpack hits | Total C calls | Root syncs |
|---|---:|---:|---:|---:|
| knucleotide | 0 | 0 | 23824604 | 248795 |
| nbody | 6994546 | 4896225 | 5455932 | 559590 |
| sieve | 0 | 0 | 44842858 | 1125891 |
| barnes | 0 | 18724485 | 319138604 | 71912953 |

Counts span seven module executions.
Direct view/unpack operations remove C crossings without changing their semantic effect or root-publication contract.

### Latest compilation and process measurements

First execution excludes frontend/process startup and includes common lowering plus backend compilation.
Backend counters exclude common lowering.

| Workload | CnP first, ms | CnP backend, ms | Current Cranelift first, ms | Cranelift backend, ms |
|---|---:|---:|---:|---:|
| knucleotide | 212.098 | 0.224 | 192.565 | 11.290 |
| nbody | 109.830 | 0.537 | 32.902 | 11.186 |
| sieve | 233.572 | 0.120 | 43.755 | 4.904 |
| barnes | 3553.981 | 2.173 | 651.669 | 81.648 |

The one-shot samples below use fresh processes/VMs with full compilation and the explicit version-file preflight.
No workload/JIT warmup runs precede them.
The initial version-file cost remains archived separately.

| Workload | Fresh-process ms | Peak RSS MiB |
|---|---:|---:|
| knucleotide | 242.61 | 25.61 |
| nbody | 96.59 | 7.77 |
| sieve | 237.00 | 5.95 |
| barnes | 3569.59 | 11.70 |

Validation includes snapshot invalidation/renewal, storage promotion, custom iteration effects, eager observation,
GC/aliases, native regions and the original four workload outputs.
All 431 tests and relocation/tail-continuation audits passed.
views-final and its source/hashes are archived; the installed CLI is verified against the frozen image.

## Experiment 6: direct builtin cursors and reference exhaustion checks

CnP now advances generation-checked builtin range/list/tuple cursors without a runtime call.
Range arithmetic uses an i128 intermediate and checks the i64 result before advancing.
List iterators resolve the current source length on every step, so appends remain visible until exhaustion.
Exhaustion, custom sources, unpublished storage and reopened observation windows retain the original probe/helper.
Reference IsDone checks can return false for known live row kinds; unsupported objects and the Done sentinel fall back.
All row pointers are consumed before the next callback, and misses leave cursors untouched.

The first parallel targeted suite crashed because a newly allocated compact float list had a valid kind/length but
no published payload pointer.
The initial directory publishes float pointers only after exclusive preparation.
Adding a nonnull payload guard fixed the crash; the same missing guard was found and added to small direct unpacks.
The new regression repeatedly passes freshly allocated float lists through both operations, with exact output checks.
A test initially used unsupported object(); it was corrected to use an existing custom instance.
The failed runs remain in the external evidence cache.

| Workload | Prior views CnP, ms | Iterator CnP, ms |
|---|---:|---:|
| nbody | 87.01 | 68.20 |
| sieve | 287.07 | 217.57 |
| knucleotide | 210.13 | 201.84 |
| ticket_search | 58.03 | 57.28 |
| barnes | 3535.61 | 3579.51 |

These are quick matched retained medians, not stable estimates.
The prior sieve control was a high outlier relative to the preceding ~234 ms checkpoint; use follow-ups below.
Barnes shows no established improvement from this change.
There is no Cranelift compilation under CnP.


### Iterator confirmation

N-body reproduced at 68.06 ms versus the frozen prior views build at 87.23 ms (~22% faster).
Sieve reproduced at 212.90 ms; its paired old control was 292.28 ms, above the earlier 233.79 ms checkpoint.
Both old controls in this experiment were high (~287–292 ms); do not report the entire paired difference as a stable gain.
K-nucleotide's final 213.48 ms did not reproduce the pilot's small gain; treat it as unchanged.
Ticket search remains ~56 ms; Barnes remains ~3.55 s.

| Workload | CnP retained, ms | Current Cranelift retained, ms | CnP/Cranelift | CnP first, ms | CnP backend compilation, ms | Cranelift backend compilation, ms |
|---|---:|---:|---:|---:|---:|---:|
| knucleotide | 213.48 | 178.48 | 1.20× | 203.83 | 0.282 | 11.752 |
| nbody | 68.04 | 8.53 | 7.98× | 85.53 | 0.444 | 10.260 |
| sieve | 214.52 | 37.98 | 5.65× | 212.05 | 0.120 | 4.745 |
| barnes | 3544.74 | 577.99 | 6.13× | 3532.29 | 2.220 | 80.552 |

First execution excludes frontend/process startup and includes common lowering and backend compilation.
Backend counters exclude common lowering.
All measured retained compilation deltas are zero except Barnes: CnP [0, 462333, 0] ns and
Cranelift [0, 8717874, 0] ns.
Across seven n-body executions, runtime crossings fell 5,455,932 -> 1,119,270 with 2,238,277 direct cursor steps.
Root publication stayed at 559,590; code size grew 22,616 -> 24,312 bytes.
CnP stats report zero Cranelift compilations and zero compilation errors.

| Workload | Fresh-process CnP, ms | Peak RSS MiB |
|---|---:|---:|
| knucleotide | 227.65 | 26.12 |
| nbody | 77.59 | 7.81 |
| sieve | 217.37 | 6.03 |
| barnes | 3568.22 | 11.67 |

Fresh processes include frontend/backend compilation after an explicit version-file preflight, without workload/JIT warmup.
The first version-file cost is archived separately.
All outputs matched the archived CPython output exactly.
Formatting, all-target Clippy with warnings denied, all 432 tests, and stencil relocation/tail checks passed.
The installed executable matches the archived iterator-final image; source and hashes are preserved externally.

## Experiment 7: direct item operations and release code partitioning

Direct item stencils handle exact list/tuple reads and list stores with primitive integer/boolean indices.
Negative indices use checked bounds normalization; custom indices, slices, unsupported storage and observation windows
retain the original closed probe/full helper before any effect.
Boxed stores decode primitive/reference slots; inline-string stores fall back.
Compact Bool/Float stores require exact value type, a mutable Vec-derived pointer and a live snapshot permission token.
Type promotion, Python protocol calls and errors remain in the runtime.
No GC occurs in these stencils; verified owners/inputs remain live until the next safepoint.
Effect-only SetItem operations do not write the default output slot.
The linker validates the additional input slot offset.

The initial variant cut sieve from 211.34 to 119.32 ms but regressed n-body 68.20 -> 73.22 ms,
ticket search 55.59 -> 63.56 ms, and Barnes 3522.79 -> 3775.45 ms.
A store-only variant retained regressions.
Moving the new counter outside Context, then removing it and restoring the original Vm/TierStats layouts, did not help.
Those hypotheses were rejected; the final code reuses the existing sequence-load/list-store counters.
N-body executed zero new item stencils, with unchanged generated code size and runtime crossing count.
Symbol inspection nevertheless showed different optimized callback/observation functions in the Rust binary.

A matched source comparison using one release codegen unit for the tis package restored the controls:

| Workload | Prior iterator source, CGU1 ms | Direct item source, CGU1 ms |
|---|---:|---:|
| nbody | 67.29 | 67.32 |
| ticket_search | 56.87 | 54.50 |
| sieve | 203.97 | 115.96 |
| knucleotide | 213.12 | 211.75 |
| barnes | 3527.54 | 3512.80 |

This supports codegen partitioning/inlining instability as the cause; it is an inference from emitted-code differences
and controlled builds, not a proof of the exact compiler decision.
The repository now sets profile.release.package.tis.codegen-units=1; dependency settings remain unchanged.
Rust release rebuilding took ~29 s versus ~11 s in these incremental runs.
That is host binary build cost, separate from measured runtime JIT compilation latency.
CnP stencil generation already used codegen-units=1 and remains independent of Cranelift.

The control source was extracted into a separate temporary directory.
Its first Cargo invocation reused a shared-target artifact without recompiling older-mtime extracted sources.
That result was discarded; all extracted Rust sources were touched, compilation was confirmed in the log,
and distinct binary hashes/source behavior were checked before the comparison.
Use independent targets or force source freshness when comparing extracted snapshots.

The new regression covers Bool/Float snapshot invalidation, promotion, aliases/self-reference, collection,
negative/boolean indices, custom __index__/getitem/setitem effects, and bounds errors.
All original workloads retain exact archived CPython output checks.

Callback symbol spans reinforce the compiler explanation: the two Context::step variants changed from
10,068/9,992 bytes in the default iterator build to 9,204/9,124 bytes in the default item build.
Both CGU1 builds have matching 10,372/10,096-byte spans; guarded_helper_callback is 4,812 bytes in both.
Matching spans do not prove byte-identical instructions, but remove the observed size difference.
The external items-symbol-spans.json preserves addresses, names and spans.

### Final item confirmation

Sieve reproduced at 119.34 ms versus the frozen prior iterator executable at 209.77 ms (~43% faster).
N-body was preserved: 67.34 versus 67.98 ms.
Ticket search CnP was 55.54 ms.
A Cranelift ticket-search sample was 120.37 ms; explicit follow-ups were 58.77 and 60.77 ms,
versus the frozen prior executable at 59.72 ms and matched CGU1 control at 59.59 ms.
The slowdown did not reproduce; the outlier remains archived and is not evidence of a sustained regression.

| Workload | CnP retained ms | Current Cranelift retained ms | CnP/Cranelift | CnP first ms | CnP backend ms | Cranelift backend ms |
|---|---:|---:|---:|---:|---:|---:|
| knucleotide | 209.19 | 184.02 | 1.14× | 211.65 | 0.278 | 11.161 |
| nbody | 67.77 | 8.49 | 7.99× | 79.96 | 0.460 | 10.916 |
| sieve | 120.29 | 37.43 | 3.21× | 121.65 | 0.103 | 4.594 |
| barnes | 3541.79 | 562.08 | 6.30× | 3504.52 | 2.186 | 82.201 |

First execution excludes frontend/process startup and includes common lowering/backend compilation.
Backend counters exclude common lowering.
Only Barnes had retained compilation deltas: CnP [0,453542,0] ns and Cranelift [0,8803000,0] ns.
Across seven sieve runs runtime crossings fell 22,989,657 -> 1,129,459 in the direct-item pilot;
root publication stayed at 1,125,891. Later confirmation counters are archived in full.
CnP recorded zero Cranelift compilations and zero compilation errors on all five controls.

| Workload | Fresh-process CnP ms | Peak RSS MiB |
|---|---:|---:|
| knucleotide | 233.52 | 26.12 |
| nbody | 78.93 | 7.58 |
| sieve | 127.36 | 5.97 |
| barnes | 3551.29 | 11.56 |

Fresh-process samples include frontend/backend compilation after a separate version-file preflight,
without workload/JIT warmup. The first version-file cost remains archived separately.
All exact archived CPython outputs matched.
All 433 tests, all-target Clippy with warnings denied, fmt, stencil fmt and relocation/tail audits pass.
The final source, manifests, negative variants and measured executable are archived externally;
the installed executable is checked against the frozen image.

## Experiment 8: adjacent scalar fusion

CnP now fuses verified adjacent arithmetic/boxing, guard/unboxing, and float-load/unboxing pairs.
Dedicated offline Rust templates retain both original SSA results, so later raw users and canonical register images
remain valid. Only an indirect continuation and redundant operand read disappear.
The linker bounds-checks additional output slots.
Float-load feedback still runs after the boxed load and before the raw result is published.
Type/overflow/division/power guards retain their original exit IDs and fail before publishing new results.
Observation-mode plans remain unfused, and no pair crosses a block, callback or bootstrap-read boundary.

The initial arithmetic-only pilot reduced n-body 67.59 -> 63.14 ms, with 275 -> 254 emitted stencils.
Extending fusion to guard/unbox and float-load/unbox pairs gave:

| Workload | Prior items CnP ms | Fusion CnP ms | Prior/fused stencil count |
|---|---:|---:|---:|
| nbody | 67.94 | 61.92 | 275/231 |
| sieve | 116.28 | 117.71 | 140/140 |
| knucleotide | 213.88 | 208.74 | 323/320 |
| ticket_search | 57.12 | 56.75 | 449/446 |
| barnes | 3533.49 | 3445.43 | 1568/1377 |

These quick samples suggest ~9% improvement for n-body, little effect on sieve, and a modest Barnes improvement.
Barnes retained samples still include recompilation; short follow-ups determine what reproduces.
N-body emitted code shrank 24,312 -> 23,896 bytes; Barnes 170,560 -> 167,504 bytes.
No extra runtime ABI fields or counters were added.

The first unit warmup never opened a profiling window, so lowering produced generic calls and the assertion that
fusion ran failed. Explicit eager warmup fixed the coverage setup.
The tests now inspect selected templates, retain a raw value used after boxing, reject fusion in observation mode,
execute fused code, and check a float-list storage guard exit after switching to integer storage.
Existing NaN/signed-zero/division/power/overflow, GC and reopened-window regressions remain in the full suite.

### Fusion confirmation

N-body reproduced at 60.12 ms versus the frozen prior items executable at 68.43 ms (~12% faster).
Sieve follow-up was unchanged: 117.89 versus 117.58 ms.
Ticket search was 54.03 ms, with current Cranelift at 59.48 ms.
The Barnes retained median was 3450.23 ms; its single fresh-process sample was slower at 3674.34 ms.
Do not claim a stable Barnes one-shot improvement from these noisy samples.

| Workload | CnP retained ms | Current Cranelift ms | CnP/Cranelift | CnP first ms | CnP backend ms | Cranelift backend ms |
|---|---:|---:|---:|---:|---:|---:|
| knucleotide | 208.93 | 184.38 | 1.13× | 212.16 | 0.252 | 10.994 |
| nbody | 59.88 | 8.56 | 7.00× | 79.47 | 0.465 | 10.327 |
| sieve | 116.56 | 37.34 | 3.12× | 117.46 | 0.117 | 4.423 |
| barnes | 3450.23 | 563.69 | 6.12× | 3464.22 | 2.147 | 82.897 |

First execution excludes frontend/process startup and includes common lowering/backend compilation.
Backend counters exclude common lowering.
Only Barnes had retained compilation deltas: CnP [0,453750,0] ns, Cranelift [0,8347958,0] ns.
CnP recorded zero Cranelift compilations and zero compilation errors on all five controls.

| Workload | Emitted stencils | Code bytes | Patch bytes | Fresh-process CnP ms | Peak RSS MiB |
|---|---:|---:|---:|---:|---:|
| knucleotide | 320 | 33016 | 38400 | 233.57 | 25.88 |
| nbody | 231 | 23896 | 27720 | 69.67 | 7.69 |
| sieve | 140 | 13496 | 16800 | 124.70 | 5.89 |
| barnes | 1377 | 167504 | 165240 | 3674.34 | 11.28 |

Fresh processes include frontend/backend compilation after the separate version-file preflight, without workload/JIT warmup.
The first version-file cost remains archived separately; all outputs matched archived CPython output exactly.
All 435 tests, all-target Clippy with warnings denied, fmt/stencil fmt and relocation/tail checks pass.
The installed executable matches the archived fusion-final image; source/flags/hashes are preserved externally.

Next hypothesis, not implemented in this checkpoint: patch hot/cold layout and alignment.
The host reports 128-byte cache lines, 64 KiB L1D and 128 KiB L1I.
The initial 64-byte assumption was corrected after reading sysctl; cache-geometry.txt records the result.
Any alignment experiment must report patch-data growth and compare actual controls before adoption.

## Experiment 9: rejected patch alignment/layout

The host reports 128-byte lines. Two 128-byte-aligned Patch layouts increased records from 120 to 128 bytes (+6.7%).
Layout A grouped continuation pointers first; layout B additionally paired source operands and moved tag cold.
Both retained exact output checks and build-time relocation/tail audits.

| Workload | Original/A ms | B/original ms |
|---|---:|---:|
| nbody | 60.64/59.93 | 60.03/60.59 |
| sieve | 117.08/120.91 | 118.67/120.06 |
| ticket_search | 56.05/55.43 | 58.36/55.42 |
| knucleotide | 206.19/206.96 | 205.40/206.43 |

Neither ordering gave a clear general win; A was slower on sieve and B was slower on ticket search.
N-body backend compilation samples grew ~0.35 -> 0.55–0.67 ms, while patch data grew 27,720 -> 29,568 bytes.
The extra alignment was rejected and the original Patch source restored byte-for-byte.
No broad cache-line claim follows from these few samples; code/patch footprint and allocation costs are competing effects.

## Experiment 10: iterator fusion and selective cold continuations

Sieve's measured LIR has Next -> IsDone -> Truth in its iterator headers.
A fused header retains all three canonical slots and original callback locations.
An initial variant kept C fallbacks inline (sieve 117.98 -> 108.54 ms, n-body 59.75 -> 61.82 ms).
Disassembly showed an unconditional 48-byte frame on both the original Next and the initial fused function.
Tail-jumping to separate slow stencils removed that frame; its pilot was 118.69 -> 103.32 ms on sieve,
with n-body 59.62 -> 59.03 ms.
After a successful Next, an unsupported result continues at IsDone, never at Next, so cursor effects are not replayed.

A broader split of item/field/view/unpack probes and float-memory/Truth templates was not a general win.
It enlarged n-body patch data 27,720 -> 35,040 bytes and regressed its sample 59.97 -> 62.92 ms.
Sharing immutable operands/continuations restored patch data near baseline (28,080 bytes), but not the timing.
Moving cold code out of the hot code area and making quiet block entry frameless still left n-body ~64 ms.

Closer disassembly corrected the premise: LLVM had already shrink-wrapped float-memory and Truth stack setup.
Their hot entries started with loads; their stack operations occurred only on cold callback paths.
Those unnecessary transformations were reverted.
The selected variant splits only probes with proven unconditional stack traffic, plus quiet block entry.
The header fusion remains; original float-memory and Truth implementations are retained.

| Workload | Frozen fusion CnP ms | Selective CnP ms |
|---|---:|---:|
| nbody | 59.94 | 60.18 |
| sieve | 119.07 | 99.32 |
| ticket_search | 54.28 | 55.59 |
| knucleotide | 212.84 | 203.90 |

Cold wrappers reuse the fast node's immutable patch only when operands and next continuation are identical.
Header fallback stages retain distinct records because their continuation and callback roles differ.
Physical patch IDs are separate from code-node IDs; aliases do not allocate or overwrite a second record.
Cold code is placed after hot code; all targets still use explicit relocated pointers.
The original 120-byte Patch layout is retained.

Build-time LLVM checks reject non-tail runtime calls in the selected fast templates, in addition to existing
relocation and tail-continuation checks. LLVM intrinsics are allowed; host disassembly verifies actual entry stack behavior.
The selected architecture does not imply every LLVM intrinsic is frameless on every target.
The new regression asserts template selection/eager exclusion, custom-iterator exhaustion counts, error effects once,
and clean stack. Existing load guard coverage also checks that cold code shares patch storage after lazy compilation.
An initial size assertion ran too early (zero counters before execution); it was moved after native execution.

### Selective confirmation

Sieve reproduced at 99.33 ms versus the frozen fusion executable at 119.54 ms (~17% faster).
N-body was preserved: 59.39 versus 59.71 ms.
Ticket search was 55.34 ms versus the prior 54.03 ms checkpoint; the paired pilot also showed a small slowdown.
Generic iterator fallback currently adds 117,784 IsDone C boundaries across seven ticket runs, while root syncs are unchanged.
A future refinement should resume at the fast IsDone check after cold Next, rather than always calling its gateway.
Barnes showed no material new gain; the point of this change is not parity on every workload.

| Workload | CnP retained ms | Current Cranelift ms | CnP/Cranelift | CnP first ms | CnP backend ms | Cranelift backend ms |
|---|---:|---:|---:|---:|---:|---:|
| knucleotide | 204.22 | 184.55 | 1.11× | 203.78 | 0.266 | 11.825 |
| nbody | 59.51 | 8.79 | 6.77× | 68.57 | 0.365 | 10.927 |
| sieve | 99.54 | 37.20 | 2.68× | 99.91 | 0.127 | 4.510 |
| barnes | 3430.23 | 556.53 | 6.16× | 3438.26 | 2.199 | 82.999 |

First execution excludes frontend/process startup and includes common lowering/backend compilation.
Backend counters exclude common lowering. Only Barnes had retained compile deltas:
CnP [0,491125,0] ns; Cranelift [0,8908958,0] ns.
CnP recorded zero Cranelift compilations and zero compilation errors on all five controls.

| Workload | Code entries | Unique patch records | Code bytes | Patch bytes | Fresh-process ms | Peak RSS MiB |
|---|---:|---:|---:|---:|---:|---:|
| knucleotide | 385 | 324 | 36260 | 38880 | 231.21 | 26.81 |
| nbody | 257 | 234 | 25088 | 28080 | 68.83 | 7.84 |
| sieve | 168 | 142 | 14928 | 17040 | 104.90 | 5.94 |
| barnes | 1657 | 1389 | 181356 | 166680 | 3449.26 | 11.61 |

Fresh processes include frontend/backend compilation after the separate version-file preflight, without workload/JIT warmup.
Initial version-file cost is retained separately. All exact archived CPython outputs matched.
The full 436-test suite, all-target Clippy -D warnings, fmt/stencil fmt, relocation/tail and fast-call audits pass.
The installed executable matches the archived selective-final image; source, flags and hashes are external.
The Boolean range-fill recognizer is only an external draft; it is not part of this measured build.

## Experiment 11: shared Boolean range-fill region

The shared LIR recognizer in region/fill.rs accepts a strict two-block loop: Next/IsDone/Truth header,
a literal Boolean plus exact SetItem body, and a backedge carrying the last loop variable.
The destination and iterator must be invariant through the body; extra calls, stores and control flow are excluded.
CnP consumes this analysis; the Cranelift emitter does not yet consume this new region description.

A new offline Rust stencil guards cached builtin range iteration, current identities/generations, compact Bool storage,
a real mutable Vec-derived pointer, and the snapshot permission token.
It proves first/last indices fit both i64 and Python's [-len,len-1] bounds before any effect.
Monotonicity covers every intermediate; negative indices normalize individually.
Misses preserve the ordinary loop, including partial writes before IndexError and custom __setitem__ effects.
Volatile byte stores prevent LLVM introducing an external memset relocation.

The kernel allocates nothing, calls no runtime code, and uses heap pointers only within its call-free region.
It publishes the last loop variable and body scalar results, advances the cursor to its end,
and updates precise deferred backedges and simulated block-entry counters.
It then resumes the ORIGINAL Next, which owns sticky exhaustion and Done allocation.
The descriptor is Box-owned by the artifact, contains only offsets/scalars, and its frame offsets are bounds-checked.

| Workload | Prior selective CnP ms | Region pilot CnP ms | Region iterations across seven runs |
|---|---:|---:|---:|
| sieve | 101.76 | 56.98 | 14853207 |
| nbody | 62.59 | 59.72 | 0 |
| ticket_search | 56.20 | 55.29 | 0 |
| knucleotide | 208.21 | 212.85 | 0 |
| barnes | 3436.90 | 3452.18 | 0 |

The pilot is a ~44% sieve improvement, not a stable estimate for the unchanged controls.
No benchmark name, source filename or fixed bound participates in recognition.
The original full Python workload and checksum/output remain unchanged.

New regressions cover positive/negative strides and indices, crossing zero, invalid bounds with prefix writes,
snapshot invalidation/renewal, boxed storage fallback, partial/sticky iterators, custom store effects,
empty-loop unbound targets, eager exclusion and collection.
OSR/deferred backedge totals are compared exactly with bytecode on the same program.
The previously noted header fallback was also refined to run fast IsDone after a cold Next;
this preserves the after-advance continuation while avoiding needless gateway calls for ordinary results.

### Range-fill confirmation

Sieve reproduced at 56.81 ms versus the frozen selective executable at 99.98 ms (~43% faster).
N-body follow-up was 57.99 ms; no region was selected on that workload.
The first k-nucleotide/ticket samples were higher than the prior checkpoint, so paired controls were repeated:
ticket 55.78/56.62 ms versus selective 57.23 ms; k-nucleotide 204.51/206.58 ms versus selective 202.84 ms.
No material control regression is established by these quick, noisy samples.
Ticket's C boundary count is restored to 4,441,238 across seven runs, with root syncs unchanged at 1,596,915.

| Workload | CnP retained ms | Current Cranelift ms | CnP/Cranelift | CnP first ms | CnP backend ms | Cranelift backend ms |
|---|---:|---:|---:|---:|---:|---:|
| knucleotide | 214.80 | 186.70 | 1.15× | 207.57 | 0.243 | 11.429 |
| nbody | 58.21 | 8.61 | 6.76× | 80.64 | 0.635 | 10.677 |
| sieve | 57.91 | 37.06 | 1.56× | 58.05 | 0.127 | 4.645 |
| barnes | 3423.36 | 563.43 | 6.08× | 3422.12 | 2.298 | 81.627 |

First execution excludes frontend/process startup and includes common lowering/backend compilation.
Backend counters exclude common lowering. Only Barnes had retained compile deltas:
CnP [0,471916,0] ns; Cranelift [0, 8778499, 0] ns.
CnP recorded zero Cranelift compilations and zero compilation errors on all five controls.

| Workload | Code bytes | Patch bytes | Fresh-process CnP ms | Peak RSS MiB |
|---|---:|---:|---:|---:|
| knucleotide | 36964 | 38880 | 233.12 | 25.59 |
| nbody | 25616 | 28080 | 67.70 | 7.62 |
| sieve | 15872 | 17160 | 62.57 | 5.84 |
| barnes | 183468 | 166680 | 3466.45 | 11.50 |

Rust-owned region descriptors are additional to the patch-byte counter; RSS includes them.
Fresh processes include frontend/backend compilation after a separate version-file preflight, without workload/JIT warmup.
Initial version-file cost is retained separately. All exact archived CPython outputs matched.
All 439 tests, all-target Clippy -D warnings, fmt/stencil fmt and relocation/tail/fast-call audits pass.
The installed executable matches the archived fill-final image; source, flags and hashes are external.
The Boolean reduction recognizer remains a plan only, not part of this measured build.

## Experiment 12: guarded Boolean reductions

The shared LIR analysis now recognizes a range loop whose exact Boolean-list predicate conditionally
updates integer counts, index sums or the last selected index. It traces both SSA arms and edge arguments,
rejects unsupported operations, escaping internal results and discarded arithmetic evaluations.
Profiled NumericAdd and generic addition describe the same supported recurrence; the runtime still guards exact integers.

An offline Rust stencil scans the remaining range with local accumulators. Exact iterator/list identity,
generation, storage pointers and endpoint bounds are checked before scanning. Conservative i64 sum bounds
prevent repeated unsuccessful suffix scans; checked additions remain inside the kernel.
All cursor, slot and accounting changes occur only after the complete read-only scan succeeds.
Misses retain the original loop, preserving arbitrary integer/protocol behavior and valid prefixes before errors.
The kernel keeps an initial last-selected reference on all-false input, publishes the final loop target,
updates exact synthetic block/backedge counts and resumes the original Next for sticky exhaustion.
It allocates nothing and crosses no runtime boundary while holding heap pointers.

The initial pilot reduced sieve from 57.91 to 3.86 ms retained; nbody 58.37, ticket 57.56,
k-nucleotide 203.41 and Barnes 3436.57 ms remained near their frozen controls.
These quick samples establish a large covered-loop improvement, not general backend parity.
The full unchanged sieve checksum matched the archived CPython output on every execution.
Cranelift does not yet consume the new shared reduction description; CnP invokes no Cranelift compiler.

### Reduction confirmation

Paired sieve samples reproduced at 3.03 ms versus frozen fill-final 56.49 ms (~18.7× faster).
N-body's paired control was 58.37 versus 58.47 ms. All exact archived CPython outputs matched.
Sieve recorded 21,853,200 real combined fill/reduction iterations across seven executions;
other controls selected neither region. No Cranelift compilation or compilation error occurred in CnP.

| Workload | CnP retained ms | Current Cranelift ms | CnP/Cranelift | CnP first ms | CnP backend ms | Cranelift backend ms |
|---|---:|---:|---:|---:|---:|---:|
| knucleotide | 203.85 | 186.30 | 1.09× | 206.38 | 0.285 | 11.503 |
| nbody | 57.81 | 8.50 | 6.80× | 71.52 | 0.521 | 10.309 |
| sieve | 2.99 | 36.86 | 0.081× | 3.75 | 0.164 | 4.684 |
| barnes | 3449.02 | 555.28 | 6.21× | 3468.54 | 2.137 | 80.513 |

First execution excludes frontend/process startup and includes common lowering/backend compilation.
Backend counters exclude common lowering. Only Barnes had retained compile deltas:
CnP [0,510541,0] ns; Cranelift [0,8749542,0] ns. These Barnes samples are not compilation-free steady state.
Ticket's practical control was 57.98 ms CnP versus 58.85 ms Cranelift, with zero retained compile deltas.
Quick samples remain noisy; unchanged controls do not establish small wins or regressions.

| Workload | Code bytes | Patch bytes | Fresh-process CnP ms | Peak RSS MiB |
|---|---:|---:|---:|---:|
| knucleotide | 36964 | 38880 | 234.18 | 25.69 |
| nbody | 25616 | 28080 | 67.55 | 7.92 |
| sieve | 17792 | 17280 | 9.07 | 6.00 |
| barnes | 183468 | 166680 | 3512.58 | 11.63 |

Region descriptors are additional to patch bytes; RSS includes them. Fresh processes include frontend/backend
compilation after a separate version-file preflight, without workload/JIT warmup. Initial file cost is retained separately.
All 441 tests, all-target Clippy -D warnings, fmt/stencil fmt and relocation/tail/fast-call audits pass.
The measured reduce-final source snapshot, binaries, flags, hashes and raw samples remain in the external evidence cache.
Sieve now beats this Cranelift path through a new general loop optimization; the remaining workloads still fall short of parity.

## Experiment 13: native builtin exhaustion

N-body still crossed the runtime gateway 1,119,270 times and synchronized roots 559,590 times across seven runs.
Most inner iterators exhausted through an allocating Next followed by IsDone/Truth callbacks.
A native invocation now lazily retains its first real Done object in one reserved transient-root slot.
The root survives nested calls/GC and is released after return, deoptimization or error reconstruction.
No Done object is allocated eagerly at each native function entry.

Exact builtin iterators expose their sticky exhausted flag through Cell<bool>, using the same interior-mutation
contract as their cursor. Native rows publish this pointer only for iterator kinds.
The CnP header can reuse the rooted sentinel once current exact range/list/tuple bounds prove exhaustion;
all three SSA results stay canonical. Direct stepping checks the sticky flag even when its row predates the write.
Custom sources, dict/set mutation checks, an uncached sentinel and observing profiling windows retain the original path.
Existing Cranelift region descriptors now address Cell storage without changing their runtime pointer representation.

The pilot measured nbody 52.38 ms, sieve 2.93, k-nucleotide 205.57, ticket 53.67 and Barnes 3434.27 ms.
N-body runtime calls fell to 279,912 and root syncs to 279,804; inline stepping counts now include builtin exhaustion.
The much smaller time reduction than crossing reduction points to the remaining per-operation code/image cost.
Code grew from 25,616 to 26,528 bytes on n-body; patch data stayed 28,080 bytes.

### Exhaustion confirmation

Paired n-body reproduced at 51.74 ms versus frozen reduce-final 58.77 ms (~12% faster).
Sieve remained near its reduction result (3.07 versus 2.98 ms paired).
Ticket CnP follow-ups were 55.95/54.26 ms versus frozen 56.36 ms; this smaller movement is not a stable win estimate.
The high primary Cranelift ticket sample (90.72 ms) did not reproduce: follow-ups were 62.64/59.10 ms versus
frozen 60.58 ms. The raw primary remains archived. No material general control regression is established.

| Workload | CnP retained ms | Current Cranelift ms | CnP/Cranelift | CnP first ms | CnP backend ms | Cranelift backend ms |
|---|---:|---:|---:|---:|---:|---:|
| knucleotide | 201.86 | 186.83 | 1.08× | 206.87 | 0.272 | 11.407 |
| nbody | 53.92 | 8.77 | 6.15× | 72.71 | 0.533 | 10.681 |
| sieve | 2.92 | 37.54 | 0.078× | 3.48 | 0.129 | 4.609 |
| barnes | 3436.22 | 564.10 | 6.09× | 3448.39 | 2.194 | 82.567 |

First execution excludes frontend/process startup and includes common lowering/backend compilation.
Backend counters exclude common lowering. Only Barnes had retained compile deltas:
CnP [0,498916,0] ns; Cranelift [0,8578749,0] ns. Those samples remain mixed execution/compilation measurements.
CnP recorded zero Cranelift compilations and zero compilation errors on every control.

| Workload | Code bytes | Patch bytes | Fresh-process CnP ms | Peak RSS MiB |
|---|---:|---:|---:|---:|
| knucleotide | 38180 | 38880 | 236.47 | 26.78 |
| nbody | 26528 | 28080 | 62.03 | 7.81 |
| sieve | 18400 | 17280 | 8.96 | 5.98 |
| barnes | 187116 | 166680 | 3449.93 | 11.59 |

Descriptors remain additional to patch bytes and included in RSS. Fresh processes include frontend/backend compilation
following a separate version-file preflight without workload/JIT warmup; initial file cost is archived separately.
All 443 tests, all-target Clippy -D warnings, fmt/stencil fmt and relocation/tail/fast-call audits pass.
Regression checks cover the cached path, source growth/sticky exhaustion, custom iterator calls, eager exclusion,
a real collection during the cached invocation, retained outer roots and cleanup after errors.
The installed exhaust-final binary and source snapshot match the external hashes. No commit or push was made.

## Experiment 14: direct continuation relocation

Actual AArch64 arithmetic/constant/copy templates repeatedly ended with
`ldp x3,x2,[x2,#56]; br x3`: serial loads of both next-code and next-Patch addresses.
A conservative linker now recognizes this exact suffix only in audited straight-line templates.
It patches an ADD/SUB immediate for the next Patch and a direct B for the next code address.
The native tail ABI has only x0/x1/x2 arguments; x3 is scratch. Flags and the return address remain unchanged.
Offset assertions tie the suffix to the actual C-layout Patch fields.

Unknown names/patterns, unrepresentable branch/patch deltas, cold aliases and other architectures retain indirect tails.
The copied offline Rust code still performs every operation and publishes the same canonical slots.
This is continuation relocation, with no Cranelift invocation, new runtime helper or runtime instruction selection.
All patching precedes make_exec and instruction-cache synchronization.

The direct-tail pilot measured nbody 43.64 ms versus the prior exhaustion sample near 52 ms;
K201.74, ticket54.76, sieve3.06 and Barnes3395.40 ms remained near their controls.
A separately measured AArch64 layout uses architectural 4-byte instruction alignment instead of 16-byte alignment
for every stencil. An immediately adjacent continuation replaces the branch with NOP.
The denser pilot measured nbody 42.23, sieve2.96, K203.89, ticket54.45 and Barnes3412.08 ms.
The small difference between layout pilots is inconclusive, while the direct-tail nbody gain is larger.

N-body links129 tails and shrinks code26,528 ->24,424 bytes; Barnes links390 and shrinks187,116 ->175,960.
Patch bytes and runtime-call counts are unchanged. Most guarded field/control templates are deliberately outside
this initial suffix contract, so this experiment does not yet address all indirect continuations.
Unit checks decode positive/negative/shared Patch moves and branch targets, reject unknown/out-of-range/misaligned
patterns without mutation, and verify the actual generated arithmetic suffixes on AArch64.

### Direct-tail confirmation

Paired nbody reproduced at 42.24 ms versus frozen exhaust-final 52.72 ms (~20% faster).
Sieve stayed near its existing reduction result (3.01 versus 2.94 ms).
The final current controls remain quick samples; small ticket/K/Barnes changes are not stable estimates.
Every archived CPython output matched. CnP had zero Cranelift compilations and zero compilation errors.

| Workload | CnP retained ms | Current Cranelift ms | CnP/Cranelift | CnP first ms | CnP backend ms | Cranelift backend ms |
|---|---:|---:|---:|---:|---:|---:|
| knucleotide | 202.17 | 186.11 | 1.09× | 209.45 | 0.295 | 11.246 |
| nbody | 42.06 | 8.69 | 4.84× | 61.90 | 0.366 | 10.787 |
| sieve | 2.90 | 37.31 | 0.078× | 3.58 | 0.136 | 4.557 |
| barnes | 3400.93 | 559.90 | 6.07× | 3409.17 | 2.243 | 82.964 |

First execution excludes frontend/process startup and includes common lowering/backend compilation.
Backend counters exclude common lowering. Only Barnes had retained compile deltas:
CnP [0,515458,0] ns; Cranelift [0,8445709,0] ns. Do not label those samples compilation-free steady state.

| Workload | Code bytes | Patch bytes | Fresh-process CnP ms | Peak RSS MiB |
|---|---:|---:|---:|---:|
| knucleotide | 35960 | 38880 | 225.65 | 26.67 |
| nbody | 24424 | 28080 | 52.00 | 7.89 |
| sieve | 17300 | 17280 | 8.69 | 5.94 |
| barnes | 175960 | 166680 | 3396.40 | 11.59 |

Descriptors remain additional to patch bytes and included in RSS. Fresh processes include frontend/backend compilation
after a separate version-file preflight, without workload/JIT warmup; initial file execution cost is archived separately.
All 447 tests, all-target Clippy -D warnings, fmt/stencil fmt and offline relocation/tail/fast-call audits passed.
The installed tails-final source/binaries match the archived external hashes. No commit or push was made.
The remaining float/field/control gap is substantial despite the lower serial continuation cost.

## Experiment 15: conditional tails and two-operation float stencils

The warmed nbody LIR contains adjacent add/subtract/multiply operations with only boxing between them.
Nine offline Rust pair stencils now execute two such operations, keeping a dependent intermediate in an FP register.
Both original raw results and every original boxed result stay canonical. The second boxing may be absent when a
following float store consumes the raw result. No guard, callback or potentially failing divide/power crosses the pair.
Observing compilation retains individual operations. All repurposed frame offsets are checked before publication.

Conditional division/memory tails also gain exact suffix relocation. Type-guard success tails are linked only when
the original eight-byte failure return block remains intact. AArch64 B/BL, conditional, compare and test branch targets
are decoded to reject entry into the middle of a rewritten pair. Shared indexed-load field tails remain untouched.
New checks cover these incoming edges, unchanged guard failures, every arithmetic family, all dependency shapes,
optional boxing, signed zero, subnormal/infinite/NaN behavior, a non-FMA rounding case, snapshots and late guard effects.

The generic pair pilot measured nbody 39.09 ms. A separate 72-variant experiment specialized dependency and boxing flags
at offline compilation. It shrank nbody code 24864 -> 24172 bytes, but alternating execution samples did not establish a win:

| Round | Frozen tails ms | Generic 9 pairs ms | Specialized 72 pairs ms |
|---|---:|---:|---:|
|0|42.30|39.08|39.28|
|1|41.83|39.30|40.10|
|2|41.97|38.96|38.81|

The larger catalogue was reverted. First backend compile counters in these alternating runs overlapped;
its high initial 0.71 ms compile sample did not reproduce. Do not infer a stable compile penalty from that sample.
The final library has 57 templates (48 prior plus 9 pair families). The rejected 120-template source/binaries remain external.
Restored sources were freshly written so Cargo could not reuse the later candidate through old archive mtimes.

### Pair confirmation

Final paired nbody was 38.93 ms versus frozen tails-final 41.87 ms (~7% faster).
Sieve remained near its prior reduction result (2.98 versus 2.95 ms).
All exact archived CPython outputs matched, with zero Cranelift compilations/errors in CnP.

| Workload | CnP retained ms | Current Cranelift ms | CnP/Cranelift | CnP first ms | CnP backend ms | Cranelift backend ms |
|---|---:|---:|---:|---:|---:|---:|
|knucleotide|202.24|185.25|1.09×|204.46|0.281|11.206|
|nbody|39.26|8.78|4.47×|61.96|0.590|10.834|
|sieve|2.94|37.75|0.078×|3.58|0.190|4.541|
|barnes|3389.38|564.18|6.01×|3392.62|2.310|80.388|

First execution excludes frontend/process startup and includes common lowering/backend compilation.
Backend counters exclude common lowering. Only Barnes had retained compile deltas:
CnP [0,473166,0] ns; Cranelift [0,8401541,0] ns. These remain mixed execution/compilation samples.
Ticket's control was 55.96 ms CnP versus 60.62 ms Cranelift; small changes among quick controls remain inconclusive.

|Workload|Code bytes|Patch bytes|Fresh-process CnP ms|Peak RSS MiB|
|---|---:|---:|---:|---:|
|knucleotide|35960|38880|235.80|25.81|
|nbody|24864|26520|49.38|7.92|
|sieve|17300|17280|9.72|6.02|
|barnes|176200|165240|3396.64|11.95|

Nbody has 13 fewer instruction stencils/records (260 ->247); runtime-call counters are unchanged.
Descriptors remain additional to patch bytes and included in RSS. Fresh processes include frontend/backend compilation
after a separate version-file preflight without workload/JIT warmup; initial file cost is archived separately.
All 451 tests, all-target Clippy -D warnings, fmt/stencil fmt and offline relocation/tail/fast-call audits pass.
Installed pair-final source/binaries match the external hashes. No commit or push was made.

## Remaining-gap profile

A separate instrumented frozen tails-final Barnes process was sampled for 5 seconds at 1 ms intervals.
The 3321 sampled stacks had leading named leaves in Context::step 365, Heap::get 319, execute_tier_inner 304,
execute_window 138 and sync_lir_roots 124. Unresolved copied-code leaves and small leaves are not all represented
in that summary; these counts are not a complete attribution of execution time. Instrumented elapsed output is excluded
from benchmark comparisons. The raw profile and warmed nbody LIR dump remain external.

Counters across seven Barnes runs show 19,305,866 positional frames in CnP versus 2,651,896 in Cranelift.
CnP has no closed readonly/mutation kernel entries; Cranelift has 140,242 readonly and 379,125 mutation entries.
Root syncs are 71,908,732 versus 9,963,500. This is a backend capability gap as well as a code-quality gap.
Sharing those closed-region plans remains a larger opportunity than assuming continuation tweaks alone can close it.


## Experiment 16: direct ASCII short slices

Exact ASCII heap strings expose a generation-checked, read-only native byte row. The existing allocation-time ASCII
metadata avoids scanning the whole input on each slice. A copied Rust stencil normalizes exact integer/None bounds
and returns an inline string of at most eleven bytes; Unicode, long results, non-unit steps, index hooks and active
profiling retain the original guarded callback. GetSlice already has four operands: no temporary slice object was removed.

The final paired k-nucleotide comparison improved 204.23 ->184.77 ms (~9.5%). Primary results below put it near
Cranelift; quick samples do not establish a stable advantage. Nbody 39.00 ->39.09 and sieve 2.93 ->2.94 ms were unchanged.

|Workload|CnP retained ms|Cranelift ms|Ratio|CnP first ms|CnP backend ms|Cranelift backend ms|
|---|---:|---:|---:|---:|---:|---:|
|knucleotide|180.16|187.73|0.960×|183.93|0.271|11.125|
|nbody|39.45|8.63|4.570×|58.20|0.427|10.530|
|sieve|3.03|37.51|0.081×|3.68|0.224|4.961|
|barnes|3375.92|553.86|6.095×|3392.14|2.336|82.572|
|ticket_search|55.21|58.67|0.941×|56.56|0.526|16.725|

Only Barnes had retained compilation: CnP [0,491959,0] ns, Cranelift [0,8455250,0] ns. Other deltas were zero.
Fresh-process CnP samples (frontend/backend included, separate version-file preflight, no workload warmup):
K 212.58 ms /25.64 MiB; nbody48.75 /7.61; sieve8.83 /5.89; Barnes3409.03 /11.67.
Every retained/first/fresh output matched archived CPython. All 454 tests, all-target Clippy -D warnings,
fmt/stencil fmt and strict relocation/continuation audits pass. New checks cover heap metadata mutation/invalidation,
GC, profiling fallback, integer/None/Unicode/step bounds, observable index hooks and 200k slice operations on a 256KiB stack.

The first packing loop vectorized into an external literal pool and failed the existing relocation audit.
Volatile reads of the local source words prevent that transformation; the audit remains strict.
An initial 64KiB stack failure also occurred in a one-iteration bytecode parser control before execution.
The native test uses the project's established 256KiB stack instead; raw failed controls remain external.
Sources, frozen binaries, per-run JSON and source/hash manifests are external. No commit/push.


## Experiment 17: share invariant local imports

CnP now uses the existing invariant-local analysis and native-runtime entry imports. A readiness slot after the return
and parallel-edge scratch slots guards the copied load stencil. Fresh entries seed boxed values once; OSR, unbound
values and observing windows retain the original helper. The original local frame remains a GC root. Inlined parameter
loads alias caller operands, so remaining LoadLocal helpers still describe physical locals.
The first conservative version excluded all inlined artifacts; removing that restriction passed an explicit inlining
test and removed another 3475 callbacks across seven Barnes runs, without an established additional timing gain.

The paired Barnes confirmation improved 3378.99 ->3195.07 ms (~5.4%). Runtime calls across seven runs fell
317,803,295 ->263,742,944; positional frames remain unchanged. Other controls do not show an improvement:
paired nbody39.07 ->39.43, sieve2.95 ->3.03, K178.57 ->181.32 ms. These small slowdowns remain visible.

|Workload|CnP retained ms|Cranelift ms|Ratio|CnP first ms|CnP backend ms|Cranelift backend ms|
|---|---:|---:|---:|---:|---:|---:|
|knucleotide|181.55|190.59|0.953×|184.38|0.257|11.401|
|nbody|39.74|8.71|4.560×|72.63|0.410|10.674|
|sieve|3.03|37.50|0.081×|3.72|0.148|4.664|
|barnes|3216.15|555.92|5.785×|3229.35|2.236|79.809|
|ticket_search|53.55|61.32|0.873×|56.57|0.493|16.491|

Only Barnes retained compilation is nonzero: CnP [0,495208,0] ns; Cranelift [0,8528625,0] ns.
Alternating K checks measured native191.29/CnP183.60 and native192.91/CnP277.33 ms. The latter CnP samples were
[283.41,277.33,180.64] ms with zero compilation; preserve that outlier rather than claiming stable backend superiority.
Fresh-process CnP (frontend/backend included, separate version-file preflight): K209.22 ms/25.67MiB;
nbody49.54/7.72; sieve9.08/5.91; Barnes3179.24/11.42. No workload warmup preceded these process samples.

All 459 tests, all-target Clippy -D warnings, fmt/stencil fmt and relocation/tail audits pass.
New checks cover missing locals after prior effects, changing arguments without load callbacks, heap imports surviving
collection before use, reopened profiling, inlined argument aliases and captured/reassigned/default/keyword bindings.
A numeric-add test initially still had one runtime callback per invocation: the generic addition remains a separate
operation; the zero-load-callback check now isolates parameter reads and a Boolean branch.
Frozen conservative/shared/final binaries, sources, hashes and raw outputs remain external. No commit/push.


## Experiment 18 (rejected): indexed continuation relocation

Build-time metadata certified the full five-word hot tail: MOV offset64, MOV offset56, indexed LDR code,
indexed LDR Patch, BRx3. It rejected direct branches entering the sequence interior and unknown indirect jumps;
cold80/72 tails and shared-entry sites stayed unchanged. Link-time patching preserved the two MOVs and rewrote only
the two loads/BR to ADD/SUB Patch, NOP, direct B. Range/alignment failures preserved original bytes.
Four new unit tests and real-template audits passed; all six selected templates had an eligible hot site.

The alternating results did not establish a win:
round0 Barnes3208.31 control/3192.76 candidate, nbody39.14/39.28, ticket53.90/54.09 ms;
round1 reversed order Barnes3174.59/3213.39, nbody40.37/39.32, ticket56.41/57.70.
The pilot was also unconvincing (Barnes3178.01, nbody40.71). The entire source change was reverted before experiment19.
Frozen binaries, rejected source and raw metadata/tests remain external. Fewer dependent loads alone do not prove
a useful execution gain; no broad indexed-tail linker was added to the project.

## Experiment 19: direct guarded scalar builtins

Native rows identify the actual len/math.sqrt object by its immutable name and generation, refreshed after host
mutation. Quiet copied stencils count exact list/tuple/ASCII rows or inline Unicode code points, and compute sqrt for
exact int/bool/float operands. Custom protocols, rebinding, other numbers, Unicode heap strings and sqrt domain errors
take the original helper before any effect. Call counters remain equivalent; active profiling uses the helper.

Paired Barnes improved 3160.09 ->3069.94 ms (~2.9%), following the previous local-import improvement.
Runtime calls across seven Barnes runs fell 263,742,944 ->236,388,161. Frames/root syncs remain unchanged.
K alternating confirmations were CnP179.67/180.15 versus current Cranelift191.63/188.16 ms; earlier outliers remain archived.

|Workload|CnP retained ms|Cranelift retained ms|Ratio|CnP first ms|Cranelift first ms|CnP backend ms|Cranelift backend ms|
|---|---:|---:|---:|---:|---:|---:|---:|
|knucleotide|181.60|189.65|0.958×|188.73|199.89|0.359|11.477|
|nbody|39.14|8.55|4.578×|57.20|31.89|0.579|10.444|
|sieve|2.97|37.47|0.079×|3.76|42.95|0.148|4.806|
|barnes|3052.26|559.12|5.459×|3052.57|640.05|2.209|81.743|
|ticket_search|53.10|60.46|0.878×|56.93|75.80|0.504|16.557|

First VM execution excludes process/frontend startup and includes shared lowering/backend compilation.
Only Barnes retained backend deltas are nonzero: CnP [0,492291,0] ns; Cranelift [0,9032917,0] ns.
Backend timers alone are not full JIT compilation latency. Frontend timers are archived separately.

|Workload|CnP retained-process peak MiB|Cranelift retained-process peak MiB|
|---|---:|---:|
|knucleotide|60.72|63.02|
|nbody|8.09|15.50|
|sieve|12.73|16.66|
|barnes|12.81|24.62|
|ticket_search|18.34|22.84|

These Darwin wait4 RSS peaks cover all seven retained executions, including compilation and retained VM state.
Fresh-process CnP samples (frontend/backend included, no workload warmup, separate version-file preflight):
K212.32 ms/25.80MiB, nbody49.22/7.91, sieve8.93/5.94, Barnes3083.54/11.70.
The two RSS boundaries must not be mixed.

All 461 tests, all-target Clippy -D warnings, fmt/stencil fmt and offline relocation/continuation audits pass.
New tests cover exact builtin identity/accounting, Unicode bounds, negative zero/NaN/infinity, domain errors,
custom __len__, rebound globals, host-mutated native names and observation-mode exclusion.
All benchmark outputs match archived CPython, with zero Cranelift compilations under CnP.
No commits/pushes. Source/binary hashes and exact reproductions remain external.


## Experiment 20: avoid scanning empty fresh-frame slots

Fresh native windows initialize only Code.entry_parameters in the HIR shadow image; Python parameter bindings live
in the separately rooted local frame. Import those registers directly. OSR/resumed execution keeps the full image scan.
VM shadow-window clearing, native register-image initialization, argument validation and GC root publication are unchanged.
A debug assertion checks the entry contract. Both native backends share the change.

Paired Barnes confirmed 3063.04 ->2956.90 and 3108.68 ->2953.05 ms (~3.5–5%).
All operation/frame/root counters remain equivalent; the avoided work is the entry scan.
Nbody follow-up39.57 ->40.31 ms was slightly slower. K185.65 ->178.80 and sieve3.09 ->3.04 varied with the controls.
The initial ticket follow-up55.75 ->57.61 looked worse, but extra alternating controls overlapped:
candidate53.78/55.09 versus prior54.20/53.92 ms. Keep the spread visible rather than assigning every change to this patch.

|Workload|CnP first / retained ms|Cranelift first / retained ms|Retained ratio|Backend CnP / Cranelift ms|Retained-process peak CnP / Cranelift MiB|
|---|---:|---:|---:|---:|---:|
|knucleotide|184.53 / 179.38|200.50 / 188.49|0.952×|0.314 / 11.088|59.50 / 61.25|
|nbody|59.14 / 39.50|32.81 / 8.60|4.595×|0.400 / 11.305|7.97 / 14.45|
|sieve|3.52 / 2.93|42.29 / 37.13|0.079×|0.147 / 4.736|12.56 / 15.58|
|barnes|2966.01 / 2946.12|645.95 / 561.07|5.251×|2.242 / 81.314|12.67 / 26.20|
|ticket_search|57.89 / 56.75|78.10 / 65.53|0.866×|0.562 / 16.618|18.53 / 22.69|

First VM execution excludes process/frontend startup but includes common lowering and backend compilation.
Backend timings alone exclude common lowering. Retained medians use the final three of seven whole-module runs,
after the first execution and three warmups. Only Barnes retained backend deltas are nonzero:
CnP [0,649083,0] ns; Cranelift [0,8670584,0] ns. These are mixed execution/compilation samples.
K confirmations measured CnP180.08/178.83 versus Cranelift189.31/187.34 ms.

Fresh-process CnP (frontend/backend included, separate version-file preflight, no workload warmup):
K215.27 ms/26.17MiB, nbody50.80/7.70, sieve9.22/5.91, Barnes3003.94/11.83.
Retained-process RSS above includes all seven executions and compilation; it is a different memory boundary.

|Workload|Cumulative CnP linked code bytes|Cumulative patch bytes|Cumulative instruction stencils|
|---|---:|---:|---:|
|knucleotide|43624|38880|401|
|nbody|25216|26520|251|
|sieve|17300|17280|172|
|barnes|182952|165240|1699|
|ticket_search|58588|54720|530|

All 463 tests, all-target Clippy -D warnings, fmt/stencil fmt and offline audits pass.
New regression checks keep direct HIR reference parameters alive across 10000 allocating iterations in both native
backends, and enter CnP through OSR with a live list created after the initial parameters.
All measured outputs match archived CPython. No Cranelift compilation occurs in CnP.

A separate final two-second/1ms CPU sample was collected per backend, starting0.5s after process launch.
Instrumented run results are explicitly marked and excluded from timing tables. CnP's leading named leaves included
Heap::get128, Context::step103, execute_tier_inner86, execute_window68 and sync_lir_roots67.
Cranelift still had heap/helper/frame costs too. Unknown generated-code leaves and smaller leaves are omitted:
these counts are not a complete time partition or equal-work comparison. Profiles and exact commands remain external.

The remaining larger step is sharing region planning. Cranelift nbody used39792 closed-region entries/2218385 iterations
across seven executions; CnP used zero. Barnes still has19.3M versus2.65M positional frames and71.9M versus9.96M root syncs.
The next backend-independent extraction should separate native/readonly.rs Plan/preflight/replay and ordered mutation
metadata from its Cranelift emitter, then add real offline CnP kernels. Do not disguise Cranelift calls or a generic
LIR interpreter as a CnP kernel. Numeric LoopRegion/schema/nest planning is already shared but lacks a CnP emitter.

Current release: imports-final; source/binary hashes, frozen controls and rejected variants remain external.
No commit/push; no Monty or original Flying Circus implementation was edited.

## Reproduce the final boundary

Frozen source/binary hashes and flags are in manifest.json in the external evidence directory.
The retained helper verifies CPython stdout itself on all seven executions:

```sh
tis_evidence=/Users/jaheba/Library/Caches/tis-benchmarks/overnight-copy-patch-2026-10-30
"$tis_evidence/binaries/imports-final-retained" "$tis_evidence/workloads/nbody.py" copy-patch "$tis_evidence/nbody.stdout"
"$tis_evidence/binaries/imports-final-retained" "$tis_evidence/workloads/nbody.py" native "$tis_evidence/nbody.stdout"
```

Use knucleotide.py, sieve.py or barnes.py and its matching stdout file for the other cases.
The archived imports-final-measure.py records all primary, paired, RSS and fresh-process commands.
final-profile.py is a separate diagnostic; never mix its instrumented elapsed values into timing comparisons.
Source archives include the pending work above HEAD60e48bc; current release builds use package.tis.codegen-units=1.


## Installed-CLI closeout confirmation

The installed executable's hash matches imports-final. Serial fresh-process controls ran the frozen original
baseline and this actual installed CLI in both tiers, without workload warmup. Each stdout matched archived CPython.
All include process startup, frontend/common lowering/backend compilation, execution and output delivery.

|Workload|Original CnP process ms|Installed CnP process ms|Installed Cranelift process ms|Ratio|Installed CnP / Cranelift peak MiB|
|---|---:|---:|---:|---:|---:|
|knucleotide|283.47|195.15|219.80|0.888×|25.72 / 28.55|
|nbody|185.76|50.60|36.75|1.377×|7.81 / 12.69|
|sieve|395.42|8.85|47.46|0.187×|5.86 / 10.11|
|barnes|5421.29|2968.17|652.88|4.546×|11.78 / 24.61|

These are single quick samples, after explicit version-file preflights; their spread versus earlier process
samples remains archived. They do not replace the retained medians above.
Exact commands, both stdout/stderr and wait4 RSS are in closeout-oneshot.py and its raw outputs outside Git.
