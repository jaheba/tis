# Lessons learned

Research conclusions and open questions, with links to the experiments that support them.
This document records why we changed the VM and what did not work.
The [benchmark reports](../benchmarks/results/README.md) retain each experiment's methods and evidence.
The main [README](../README.md) describes the implementation.

Measurements below are historical observations on particular builds and workloads.
Compare numbers within their cited experiment; do not combine speedups from different timing protocols.
Quick samples establish direction for large changes, not stable claims about small differences.
Historical source revisions and artifact paths may require the archive and commit map described in the report index.

## Architecture: keep semantics independent of execution tiers

**Decision:** HIR owns Python semantics; bytecode and typed LIR describe execution.
Runtime feedback refers to stable code identity and revision.
Shared object operations should not require the stack layout of a particular interpreter.
This lets interpreter and native changes use the same semantic fallback and provides reference execution for comparison.

**Limit:** choosing an IR is not itself a measured speedup.
Each lowering must preserve evaluation order, effects, source identities, roots and reconstruction state.
The augmented-update and inlining experiments below show where that work remains necessary.

[Design: project architecture](../README.md).

## 1. Hotspot detection and detailed profiling are different jobs

**Observed:** continuously collecting operand and call metadata was expensive even before native execution.
Staged profiling reduced warmed n-body bytecode time from 1.598 to 1.158 seconds in its paired experiment.
Cranelift and copy-and-patch also improved.

**Conclusion:** count entries and backedges cheaply, then collect bounded detailed feedback for hot code.
Freeze observations after compilation, but record unexpected types on guard failures and allow bounded reprofiling.
A sampled site's execution count is not its lifetime instruction count.
Profiles belong to stable HIR identity and code revision; installed artifacts belong to a VM.

**Limit:** eager profiling remains useful for diagnostics and must retain correct behavior.
A quiet profile is not permission to remove semantic guards.

[Evidence: staged profiling](../benchmarks/results/2026-10-28-staged-profiling/README.md).

## 2. Native control flow can still spend most of its time in the runtime

**Observed:** the early n-body native profile attributed about 90% of inclusive samples to the callback boundary.
Root publication was a substantial separate cost.
The interpreter's main dispatch function also contained inlined arithmetic and guards, so its samples were not all
dispatch.

**Conclusion:** inspect helper calls, operand conversion, root publication and generated code, rather than counting
native entries.
Generating machine instructions for branches and arithmetic does not make container operations cheap.
Root synchronization publishes live references before a possible collection; it is distinct from actual GC traversal.
Do not describe synchronization samples as time spent collecting garbage.

[Evidence: CPU profiles](../benchmarks/results/2026-10-28-cpu-profiles/README.md).

## 3. Removing a safety boundary requires a specific execution contract

**Observed:** closed builtin probes reduced k-nucleotide root synchronizations from 2,621,447 to 27,262.
Retained native execution improved from 304 to 187 ms in the final paired comparison.
C boundaries remained approximately 2.66 million.

**Conclusion:** exact builtin operations can avoid full root publication when they cannot execute Python or collect.
A bounded allocation can use an explicit no-collection budget, with a rooted fallback before allocation pressure.
This is not a rule that arbitrary allocating helpers can skip safepoints.
Pending writes must be committed correctly, successful results must update native SSA caches, and misses must precede
effects.
Keep complete deoptimization state even when a fast path does not need a GC map.

**Limit:** 99% fewer root syncs yielded about 1.6× faster execution, not a corresponding reduction in total runtime.
The Rust gateway, hashing, slot decoding and string construction still cost time.
The probes retain conservative HIR/LIR semantics and the general path for protocols, profiling and pressure.

[Evidence: native mapping probes](../benchmarks/results/2026-10-30-native-mapping-probes/README.md).

## 4. Repeated property checks can create an algorithmic problem

**Observed:** short k-nucleotide slices repeatedly checked whether the entire 100,000-character source was ASCII.
Roughly 700,000 slices caused tens of gigabytes of redundant scans after the whole-source copy had already been removed.
Caching ASCII reduced paired CLI bytecode time from 923 to 326 ms and native time from 1,083 to 470 ms.

**Conclusion:** inspect the work inside a fast-path guard as well as the operation it protects.
Store reusable string properties once when their invalidation rules are clear.
The ASCII flag fits existing heap-generation padding.
Invalidate it before exposing mutable host access, even if a host forgets the mutation guard, and reset it on slot
reuse.

**Limit:** strings are immutable to Python, but the Rust heap API can replace or edit them.
A cache justified only by Python immutability would be unsound for that API.

[Evidence: ASCII metadata](../benchmarks/results/2026-10-30-knucleotide-ascii/README.md).

## 5. A more general HIR operation does not automatically remove machine work

**Observed:** general augmented item updates made arithmetic and writeback one operation with arbitrary RHS expressions.
The initial k-nucleotide native gain was small, and callback counts were unchanged.
The old native lowering already performed numeric addition without a callback.

**Conclusion:** HIR should describe reusable semantics; backend cost must be measured separately.
`target[key] += rhs` loads the old value before evaluating `rhs`.
RHS callbacks may remove or replace the entry, and `__iadd__` may mutate objects before the setter runs.
Use the originally loaded value and preserve those effects.
Any later lookup fusion needs a proof about intervening effects and exceptions.

**Implementation pitfall:** the first general representation hid existing float-list region and adaptive arithmetic
patterns.
The final implementation required corresponding LIR lowering, bytecode numeric plans over the correct inputs,
and the existing checkpoint policy for callback-free stores.
An optimization in one representation can regress another backend if its lowering is incomplete.

[Evidence: general augmented updates](../benchmarks/results/2026-10-30-general-augmented/README.md).

## 6. Fewer frames and callbacks are diagnostics, not performance goals

**Observed:** experimental CFG inlining removed about half the physical positional frames in Barnes–Hut,
but made execution 43% slower in the same-binary on/off comparison.
Helper calls and root synchronizations increased.
A separate fused-entry experiment removed 60,000 callbacks per invocation while warmed Barnes–Hut time stayed flat.

**Conclusion:** inlining must preserve or improve downstream specialization and state handling.
Count generated helpers, roots, copied logical frames and code size alongside removed calls.
Keep experimental inlining disabled when end-to-end measurements lose.
Reduced counters suggest where work changed; they do not attribute CPU time by themselves.

[Evidence: CFG inlining](../benchmarks/results/2026-10-29-cfg-inline/README.md).
[Evidence: fused entries](../benchmarks/results/2026-10-29-inline-entry-batch/README.md).

## 7. Successful Python output does not prove the intended JIT path ran

**Observed:** a verifier rule rejected an inlined caller while ordinary execution continued to produce correct output.
Copied callee scalar caches had been incorrectly treated as physical caller OSR imports.
Fixing the import set restored the missing compilation coverage.

**Conclusion:** benchmark readiness must check cumulative compilation errors, tier entries and expected optimization
counters.
Record compilation and deoptimization during measurement separately from warmup.
A successful fallback can conceal a failed experiment.
Synthetic callee state, physical caller imports and logical source identity must remain distinct.

[Evidence: scalar imports and OSR](../benchmarks/results/2026-10-29-inline-scalar-imports/README.md).

## 8. Complete native regions can matter more than isolated fast instructions

**Observed:** native sequence-loop regions reduced n-body kernel time from about 155 to 40 ms in their paired
experiment.
The copy-and-patch comparison was roughly unchanged because it did not use that region implementation.
Typed float storage and scalar loop values had already improved both backends in an earlier experiment.

**Conclusion:** keep values and iteration state native across enough operations to amortize preparation and guards.
Validate views and bounds, preserve real iterator progress, and resume after completed writes without replaying them.
Guard hoisting requires an actual lifetime and invalidation argument.

**Limit:** common HIR/LIR semantics do not imply identical optimization coverage across backends.
Name which backend executes the optimization and which retains the generic path.

[Evidence: native regions](../benchmarks/results/2026-10-28-native-regions/README.md).
[Evidence: typed loops](../benchmarks/results/2026-10-28-typed-loops/README.md).

## 9. Value and container layout affect the interpreter as well as the JIT

**Observed:** reducing boxed values and native slots from 32 to 16 bytes improved several workloads across backends.
A later byte buffer for boolean lists reduced the photographed 30-million sieve's native RSS from about 497 to 68 MiB.
Its end-to-end native time improved from about 1,281 to 976 ms in that experiment.

**Conclusion:** measure bytes moved, allocated and traced before assuming dispatch is the dominant cost.
Specialized container storage needs widening, aliasing, snapshot and invalidation rules.
A compact representation must preserve heap identity and generation checks, not discard them for pointer convenience.
The Rust value enum and C-layout native slot are separate representations with explicit conversion.

[Evidence: 128-bit values](../benchmarks/results/2026-10-28-value128/README.md).
[Evidence: compact boolean lists](../benchmarks/results/2026-10-30-compact-bool-lists/README.md).

## 10. A small Rust source change can worsen unrelated generated code

**Observed:** the first compact-list getter combined boolean construction and ordinary whole-value reads.
LLVM then copied enum and padding fields individually, regressing unpacking.
Isolating the boolean constructor behind a non-inlined helper restored ordinary 16-byte vector copies.

**Conclusion:** profile and inspect assembly when a local change regresses a seemingly unrelated workload.
Source-level simplicity and fewer branches do not guarantee a better machine-code layout.
Keep representative control benchmarks and do not assume a representation win benefits every access path.

[Evidence: getter code generation](../benchmarks/results/2026-10-30-compact-bool-lists/README.md).

## 11. Cold performance includes IR and compiler growth

**Observed:** repeated checked unpackings expanded one simple workload into about 2.26 MB of native code.
Conservative reuse reduced it to about 10 KB; native compilation fell from roughly 1,945 to 7.5 ms.
Cold process time fell from about 2,298 to 264 ms.

**Conclusion:** inspect HIR/LIR size, snapshots, native register variables and emitted bytes when startup looks wrong.
Preserve one required semantic check and reuse only across proven-safe operations.
Source identities and reconstruction points must remain available after redundant work is removed.

**Limit:** the final cold native execution still lost to bytecode and CPython in that experiment.
Fixing compilation growth did not establish fast execution.

[Evidence: checked-unpack reuse](../benchmarks/results/2026-10-29-unpack-reuse/README.md).

## 12. Runtime services improve every execution tier

**Observed:** ticket-search bytecode improved after removing temporary GC traversal vectors,
borrowing builtin names and avoiding copies of already-rooted call operands.
The experiment changed no JIT emitter.
Builtin dictionary reads and writes later improved by eliminating duplicate key conversion and probes.

**Conclusion:** keep reusable object operations independent of frame and stack placement.
Interpreter improvements are worth measuring even when the long-term target is native performance.
A future JIT cannot recover costs hidden inside an opaque runtime helper simply by compiling its caller.

[Evidence: ticket-search bytecode](../benchmarks/results/2026-10-28-ticket-bytecode/README.md).
[Evidence: dictionary probes](../benchmarks/results/2026-10-30-dict-probes/README.md).

## 13. Timing boundaries are part of the result

A process-level sample includes startup, parsing and whatever setup the program performs.
Flying Circus one-shot measures the first request after readiness; repeated mode reuses the worker but creates fresh
sessions.
Marker-delimited fixture preparation is outside its request timing.
A retained-VM measurement reuses compiled code and state, so it answers a different question.
Phase-separated diagnostic programs can change function boundaries and optimization coverage.

**Conclusion:** record identical workload/input, output checks, binary hashes, flags, host, sample count and timing
boundaries.
Compare one-shot, fresh-session and retained results separately.
Do not explain a warmed execution gap as compilation overhead when measured compile deltas are zero.
Keep builds, tests and CPU profiling outside benchmark samples.
Treat RSS as process peak memory rather than live object size.
Preserve outliers and failed/unsupported cases instead of selecting favorable subsets.

Keep conclusions and reproduction instructions in Git.
Keep raw samples, profiles, binaries and copied workload trees in the external evidence cache.

[Evidence: benchmark archive policy](../benchmarks/results/README.md).
[Evidence: retained comparison](../benchmarks/results/2026-10-30-native-mapping-probes/README.md).

## 14. Storage shape is not the whole object protocol

**Observed:** shared instance shapes improved Barnes–Hut bytecode by about 15% in their paired experiment.
However, an unchanged layout does not prove that attribute access can bypass descriptors or hooks.
Installing a method or descriptor after compilation can change behavior without moving the instance's fields.

**Conclusion:** guard semantic assumptions as well as layout, identity and generation.
The current conservative invalidation disables direct shortcuts when relevant class behavior changes.
We chose fixed class bases and instance classes, but class namespaces remain mutable.
Do not treat a fixed inheritance topology as permission to ignore those mutations.

[Evidence: instance shapes](../benchmarks/results/2026-10-28-instance-shapes/README.md).
[Current object model](../README.md#language-core).

## 15. Cheap compilation and fast execution are separate results

**Hypothesis:** CnP can reuse closed runtime probes and reduce redundant copies without changing Python semantics.
**Observed:** retained k-nucleotide improved 35%, sieve 25%, and n-body 11% in a quick comparison.
The roughly 4% Barnes–Hut change is inconclusive given 2–3% movement in unchanged Cranelift controls.
Cranelift still ran these workloads about 1.4–18 times faster than the improved CnP backend.

CnP compilation/linking cost about 30–50 times less on the main diagnostics and application samples.
A short integer sum reached its first result in 0.60 ms versus Cranelift's 2.00 ms, excluding the frontend.
Once retained, that sum took 0.38 ms versus 0.035 ms.
Larger workloads favored Cranelift even on their first execution.

**Conclusion:** keep compilation latency and execution throughput as separate objectives.
The current CnP backend still moves values through memory and lacks several Cranelift region optimizations.
Fused loop stencils and shared region planning are hypotheses for the next experiment, not measured wins.

**Measurement correction:** fixed warmup counts do not prove compilation has stopped.
Whole-module Barnes–Hut replay redefined objects and recompiled during a measured execution in both backends.
The report includes those costs and labels the result retained execution rather than a compilation-free kernel.

[Evidence: CnP comparison](../benchmarks/results/2026-10-30-copy-patch/README.md).

## 16. Share the quiet-execution policy across backends

**Hypothesis:** frozen profiles should avoid the same per-block VM work in CnP as in Cranelift.
**Observed:** quiet entry stencils reduced retained k-nucleotide by about 14–15% and sieve by about 20–22%.
They removed roughly 20 million and 58 million runtime calls respectively over seven module executions.
Root-map counts stayed unchanged.
N-body and Barnes–Hut improved only a few percent in that experiment; those small changes are inconclusive.

**Conclusion:** backend policy differences can explain part of a performance gap before instruction selection matters.
Share quiet-edge planning and preserve exact backedge counts, OSR skipping, error exits and reopened observation.
More inline bookkeeping increases stencil/patch size; measure compilation and memory alongside execution.

**Counterexample:** the first independent quiet-only k-nucleotide CLI sample was slower despite faster retained execution.
Two follow-ups confirmed faster process execution, but the initial launch outlier remains in the report.
One process launch does not establish an execution regression or improvement.

## 17. Closed field probes help CnP without providing direct field access

**Hypothesis:** reuse the audited no-GC instance-field probe before CnP's general GetAttr gateway.
**Observed:** the initial Barnes–Hut sample fell from 5.40 seconds to 4.08 seconds with quiet entries and field probes.
The report includes a separate follow-up and the small builtin-attribute control, ticket search.
These quick results still leave a large Cranelift gap.

**Conclusion:** a field hit can avoid root publication without embedding heap layout in a stencil.
Descriptors, hooks, missing attributes and reopened profiling retain the full helper.
The field probe uses a different error status from other closed helpers; the stencil preserves that distinction.
This is a Rust runtime probe, not Cranelift's direct field access or a compiled loop region.

A separate n-body CPU sample still showed runtime dispatch, heap reads, root synchronization and float-view work.
It includes interpretation/OSR and partly unsymbolized copied code, so it does not establish warmed-kernel percentages.
Next experiments should target remaining sequence operations and region preparation rather than assuming arithmetic dominates.

[Evidence for both experiments](../benchmarks/results/overnight-copy-patch/README.md).

## 18. Exact sequence probes remove root work without removing every C call

**Hypothesis:** exact builtin reads and unfinished iterator steps can use the same no-GC contract as closed mappings.
**Observed:** n-body improved from about 153 to 121 ms and sieve from 311 to 240 ms in matched quick samples.
Follow-ups supported those gains, although an earlier sieve sample reached 290 ms.
Root synchronizations fell about 79% in n-body and 86% in sieve across seven executions.
Neither workload reached Cranelift parity.

Immediate IsDone checks now stay inside an offline stencil.
Heap sentinels, custom protocols, out-of-bounds indices, exhaustion and open observation retain the full path.
A successful cursor probe advances once; a miss advances nothing.

**Counterexample:** probing sequences before every dictionary lookup slowed the unchanged Cranelift mapping workload by 5%.
A primitive-key dispatch restored its prior throughput.
Check shared helpers against both backends; a CnP-focused change can affect Cranelift too.

**Build constraint:** LLVM introduced a literal-pool relocation for a constant Bool Slot.
The audit rejected it; separate word stores preserved a self-contained stencil.
Keep the relocation check rather than admitting references to discarded build-time data.

## 19. A fresh executable file and a fresh VM are different startup states

**Observed:** the latest newly copied CLI took 267 ms merely to print --version.
That path exits before frontend, VM and JIT construction.
Subsequent fresh processes still compile the whole workload with a new VM, but avoid that first-file anomaly.

**Conclusion:** preserve the slow first-file samples and name the startup preparation used in comparisons.
Do not attribute that latency to JIT compilation or claim an OS-level cause without further evidence.
A version probe is not workload/JIT warmup, but its cost belongs in first-install latency.

[Evidence for both findings](../benchmarks/results/overnight-copy-patch/README.md).

## 20. Share layout facts, and scope compiler policies to the artifact that needs them

**Hypothesis:** CnP can use direct fields without duplicating the runtime's Value or heap-view layout.
**Observed:** a shared native header and checked direct-field stencil improved Barnes–Hut about 12% in a paired follow-up.
About 241 million C crossings disappeared over seven runs, while root-map counts stayed unchanged.
The remaining runtime/dispatch work still leaves a roughly sixfold Cranelift gap.

Native row/cache declarations and the actual Value conversion are shared with offline Rust compilation.
Heap identity, generation, kind, protocol, shape and bounds guards remain explicit.
Directory pointers are reloaded after callbacks; Rc-held LIR owns the cache pointer.
The directory/code cost is visible in RSS and generated bytes.

**Counterexample:** forcing the shared conversion inline everywhere regressed ticket-search by about 18% in CnP.
The runtime step function grew in the emitted artifact; codegen changes are a plausible contributor, not a complete profile.
Forcing inlining only for self-contained offline stencils restored the runtime's throughput.
Sharing semantics does not require identical compilation policy in both artifacts.

[Evidence](../benchmarks/results/overnight-copy-patch/README.md).

## 21. Writable native pointers need an invalidation contract beyond address stability

**Hypothesis:** checked rows can prepare float views and small unpack batches without a Rust gateway.
**Observed:** direct operations eliminated nearly all n-body view/unpack callbacks in the initial release pilot.
The report records the final paired samples, code/compile costs and RSS.

A stable data pointer is insufficient when generic Rust code can cache boxed snapshots.
Shared snapshot creation now revokes a token; direct view preparation misses until an exclusive borrow clears the cache.
Both native metadata and view preparation derive pointers from Vec itself, avoiding persistent pointers from temporary
unique slice borrows.
Guarded unpack copies exact builtin contents and preserves the original observing/custom-iterator fallback.

**Measurement limit:** one sieve pilot looked 16% slower despite unchanged hot code/call/root counts.
Paired repeats did not support that regression.
Keep the outlier rather than modifying unrelated code to explain it.

**Coverage correction:** the first view regression test exercised leaf bootstrap, so its inline-view hit count was zero.
A loop made the test reach the intended stencil.
Tier entry counters alone do not prove an individual optimization ran.

[Evidence](../benchmarks/results/overnight-copy-patch/README.md).

## 22. A native row kind does not imply a published payload

**Hypothesis:** direct builtin cursor advancement can remove the remaining per-element runtime crossings.

The n-body pilot fell from 87 to 68 ms; K-nucleotide improved slightly and Barnes did not improve.
The direct path checks current identity, generation, kind, cursor and source bounds before advancing.
Exhaustion remains in the runtime to preserve sticky state and sentinel creation.

**Failed experiment:** the first sequence path dereferenced a null float payload pointer.
A compact float list can have a valid metadata row before an exclusive preparation publishes its data pointer.
Kind and length guards alone are insufficient.
The corrected iterator and unpack paths treat unpublished storage as a miss before any effect.
A regression uses fresh float lists rather than only lists prepared by earlier native views.

The range arithmetic is shared semantic behavior, but its stencil implementation remains independent of Cranelift.
Current CnP compilation still consists of copied offline Rust stencils and patched continuations.

[Measurements and failure evidence](../benchmarks/results/overnight-copy-patch/README.md).

## 23. Runtime compilation costs and Rust binary optimization are different variables

**Hypothesis:** direct checked item reads/stores should remove sieve's remaining builtin crossings.
The initial sieve pilot improved 211 -> 119 ms, but unrelated n-body and ticket-search controls regressed.
A store-only variant and two counter-layout changes did not remove the regressions.
N-body ran no new item stencils and kept the same generated code size and crossing count.
Its Rust callback code nevertheless changed.

A matched comparison with one release codegen unit for tis restored n-body to 67.3 ms on both sources.
Sieve then improved 204 -> 116 ms, ticket search improved slightly, and Barnes remained unchanged.
This supports compiler partitioning/inlining instability as the explanation; the exact decision is not established.
The package now uses one release codegen unit while dependencies keep their normal settings.
Host release builds became slower in the experiment; JIT compilation latency is measured independently.

**Evidence trap:** extracted source files with old mtimes allowed a shared Cargo target to reuse a newer artifact.
The first control build was discarded and explicitly rebuilt after refreshing source timestamps.
Freeze executable hashes and verify that controls really compile the intended source.

Direct compact stores retain the existing snapshot-token/provenance contract and miss before type promotion or Python calls.
Effect-only LIR stores must not overwrite a default output register.
The existing load/store counters avoid adding another hot runtime layout variable.

[Controlled comparisons and failed hypotheses](../benchmarks/results/overnight-copy-patch/README.md).

## 24. Fuse dispatch without changing the canonical value image

**Hypothesis:** adjacent scalar operations spend measurable time on tail continuations and rereading the same payload.
Arithmetic/boxing fusion reduced the n-body pilot ~7%; adding guard/unbox and float-load/unbox reached ~9%.
Sieve was unchanged, while Barnes showed a smaller, noisy gain.

The fused stencil keeps both SSA outputs in their original slots.
This avoids assuming the raw result is dead, changing GC/deoptimization maps, or introducing a register allocator.
Float-load observation still occurs between the boxed load and raw publication.
All original failure IDs remain in place; observation-mode plans stay unfused.
A skipped instruction still retains its location entry, so later runtime callbacks address the original LIR instruction.

This is a bounded win: it removes one dispatch per recognized pair while retaining most register-image stores.
Larger gains require loop/region planning or a deliberate register-cache ABI, not an ever-growing collection of tiny pairs.

**Coverage lesson:** a cold function run is not necessarily a profiling run.
The first unit setup produced generic calls, and its fusion assertion correctly failed.
Eager warmup plus template-selection assertions prove which optimization the semantic test exercises.

[Measurements and validation](../benchmarks/results/overnight-copy-patch/README.md).

## 25. Better alignment can lose to a larger working set

**Hypothesis:** grouping continuation pointers and aligning patch records to the host cache line should reduce metadata traffic.
The host reported 128-byte lines; the initial 64-byte assumption was corrected before the experiment.
Two aligned orderings increased records 120 -> 128 bytes, but neither delivered a clear general execution win.
Sieve or ticket search regressed, and backend compilation samples increased.
The original layout was restored.

Cache-line geometry alone does not establish the bottleneck.
Padding also enlarges compiler node allocations and patch working sets; fewer straddled loads can lose to those costs.
Keep the experiment negative rather than choosing the fastest isolated workload/sample.

[Both layouts and compilation samples](../benchmarks/results/overnight-copy-patch/README.md).

## 26. Inspect the hot path, not just the calls in a function

**Hypothesis:** a C fallback can force stack saves on every successful stencil operation.
The original Next and its first fused variant had unconditional 48-byte frames.
A tail continuation to a separate cold wrapper removed that frame and improved the sieve pilot.

Applying the idea everywhere was a mistake.
LLVM had already shrink-wrapped stack setup for float-memory and Truth operations.
Their hot paths were already frameless, and extra cold nodes/metadata regressed n-body.
Sharing immutable patch records and grouping cold code helped working-set discipline, but did not fix that mistaken scope.
Those transformations were reverted; only proven unconditional-frame probes and block entry remain split.

A successful iterator step must not replay when the subsequent IsDone check falls back.
Cold continuations keep the original location/root metadata and resume after completed effects.
All original result slots remain canonical, including the raw branch condition.

A patch can be shared only when both operands and next continuation match.
Code-node identity and data-record identity are now explicit, so cold wrappers need no duplicate data.
A build-time no-runtime-call check is useful, but disassembly and measured regressions still decide whether it helps.

[All variants, including the rejected broad split](../benchmarks/results/overnight-copy-patch/README.md).

## 27. A small region removes more than a faster operation

**Hypothesis:** repeated iterator/list guards and image dispatch dominate a simple Boolean store loop.
A generic shared LIR recognizer plus an offline Rust range-fill stencil reduced the sieve pilot ~102 -> 57 ms.
It performed ~14.9 million real stores across seven full executions; output still matched CPython.
Other controls did not select this region.

The win comes from proving the entire remaining range safe once, not weakening per-item semantics.
Bounds failures miss before effects, so the original loop still performs its valid prefix before raising.
A compact-list snapshot revokes permission; the first ordinary store can renew it before the remaining region runs.
No callback, allocation or GC occurs while pointers are borrowed.

Batching execution also batches accounting and state publication.
The kernel restores the last loop variable and exact backedge/block counts, then uses the original Next for exhaustion.
A bytecode comparison verifies OSR backedge totals; regressions cover empty targets, partial iterators,
negative indices, snapshots, custom stores and eager profiling.
Descriptors keep semantic register analysis separate from backend byte offsets and remain artifact-owned.

Cranelift does not yet consume this new shared region description.
The optimization is deliberately narrow; unsupported bodies retain their existing execution path.

[Whole-workload measurements and coverage](../benchmarks/results/overnight-copy-patch/README.md).

## 28. Read-only speculation can defer all loop publication

**Hypothesis:** after range-fill acceleration, sieve's final Boolean scan still pays for per-item iterator,
list, arithmetic and slot dispatch. A guarded reduction can move these checks out of the loop.
The pilot reduced retained time from 57.91 to 3.86 ms with the original program and CPython checksum.
Other controls did not select this region and stayed near their frozen timings.

The shared analysis traces both SSA arms to their common backedge and admits count, index-sum and
last-selected recurrences. Both generic and profiled NumericAdd must be recognized; looking only at
the unspecialized dump initially produced no coverage. Coverage assertions caught that silent fallback.

The offline stencil reads exact Boolean storage into local accumulators, then publishes cursor, results
and accounting together. Bounds/type/overflow misses occur before observable effects.
Conservative sum bounds avoid repeatedly scanning a suffix that must overflow; unusual integer or
protocol behavior falls back to the original loop. An all-false scan preserves an initial reference result.
No runtime call, allocation or GC occurs during the borrowed-pointer interval.

Discarded arithmetic is observable too. The recognizer requires every admitted addition to correspond
to one supported output recurrence; otherwise even an unused result could hide an exception or a hook call.
Regression checks compare exact backedge totals with bytecode and cover custom protocols, overflow,
partial iterators, negative indices, empty targets, eager profiling and collection.

Cranelift does not consume this new shared description yet. This is a large win for covered loops,
not a claim that generic CnP execution has reached Cranelift performance.

[Measurements and timing boundaries](../benchmarks/results/overnight-copy-patch/README.md).

## 29. Exhaustion is a runtime crossing too

**Hypothesis:** direct iterator steps leave an expensive end-of-loop path behind. N-body still created
Done objects and entered callbacks at every inner-loop exhaustion after successful steps had become native.
A lazy, invocation-rooted real sentinel plus guarded builtin exhaustion reduced the pilot from about 58 to 52 ms.
Runtime calls fell 1,119,270 -> 279,912 and root syncs 559,590 -> 279,804 across seven executions.

The cache needs a real GC lifetime, not a raw pointer that happens to survive a sample.
One reserved transient-root slot exists throughout each invocation, including nested helpers and reconstruction,
and a regression forces collection while the sentinel is cached and verifies root cleanup after errors.
Iterators publish Cell<bool> storage for sticky exhaustion; direct successful steps recheck that flag.
The result/IsDone/Truth slots remain canonical, so no dead-value assumption or fabricated heap handle is needed.
Custom protocols and observing windows keep ordinary execution. Source growth cannot resurrect an exhausted iterator.

Removing three quarters of measured crossings did not remove three quarters of execution time.
The remaining float-heavy gap requires inspecting serial patch-pointer and register-image traffic, not assuming
that more callback removal alone will reach Cranelift.

[Whole-workload results and boundaries](../benchmarks/results/overnight-copy-patch/README.md).

## 30. Continuation loads are part of the dependency chain

**Hypothesis:** after callback removal, tiny copied stencils still serialize on loading the next Patch pointer.
Actual AArch64 suffixes load both continuation addresses and branch indirectly.
Relocating audited suffixes to an immediate Patch adjustment and direct branch reduced the nbody pilot
from roughly 52 to 44 ms without changing runtime-call counts or slot publication.
A denser layout measured 42 ms and reduced nbody code size about 8%; its incremental timing effect is less certain.

This does not require optimizing Python operations at link time.
The operations remain offline Rust machine code. The linker patches only verified continuation operands,
checks branch/immediate ranges and alignment, and leaves unknown patterns/architectures unchanged.
A whitelist and exact suffix/ABI checks keep cold aliases, other indirect calls and failure tails outside this first contract.
An adjacent target may fall through an architectural NOP; the original stack/return-address discipline remains intact.

The lesson is to inspect the actual dependency chain, not count only interpreter/runtime crossings.
A single pointer chase between each small arithmetic operation can matter even when its target is predictable.
Guarded field/control templates still retain most of their original continuations; broader coverage needs its own audit.

[Experiments and confirmation](../benchmarks/results/overnight-copy-patch/README.md).

## 31. Bounded fusion helps; more template variants need evidence

**Hypothesis:** adjacent typed arithmetic can keep an intermediate in an FP register while preserving every
original raw/boxed image. Nine total-operation pair templates cover add/subtract/multiply combinations,
including independent, left/right/both-dependent inputs and an optional second boxing before a float store.
Paired nbody samples improved 41.87 -> 38.93 ms (~7%), alongside exact conditional-tail linking.
No guard, callback or failing divide/power is crossed; observing compilation retains its original instructions.

Keeping canonical outputs makes later deoptimization ordinary. Checks compare all operator combinations with bytecode,
including signed zero, non-FMA rounding and a late guard failure after a prior side effect.
A list snapshot remains independent when a paired result feeds a guarded float store.

A 72-variant offline catalogue removed dependency/boxing branches and reduced emitted code, but alternating timings
overlapped the simpler 9-variant version. It was reverted. Fixed per-copy branches may already predict well;
the samples do not establish the precise reason. A smaller instruction count or code image alone is not a speed result.
Restoring a control also requires fresh mtimes or isolated targets to avoid Cargo reusing a later artifact.

[Full comparison and rejected source evidence](../benchmarks/results/overnight-copy-patch/README.md).

## 32. Backend capability differences still dominate Barnes

The frozen CnP Barnes profile has leading named leaves in native callback dispatch, heap lookup, tier/frame setup
and root synchronization. A five-second sample is a diagnostic, not a precise time partition of unresolved copied code.
Across seven identical runs CnP creates 19.3 million positional frames versus 2.65 million in Cranelift.
Cranelift's closed readonly/mutation kernels bypass recursive frames and tuple allocations that CnP still performs.
The remaining roughly 6x gap cannot be assigned solely to copy-and-patch instruction selection.

This suggests sharing closed-region planning and preserving its replay/ordered-write guarantees in a CnP implementation.
Calling Cranelift for that path would conceal the missing capability rather than improve CnP.

[Profile/counter evidence](../benchmarks/results/overnight-copy-patch/README.md).

## 33. Reuse ASCII metadata to make short slices call-free

GetSlice already carries source/start/stop/step separately. Its remaining cost was callback setup, heap lookup and
word packing. Generation-checked ASCII rows plus an offline Rust stencil produce inline results up to eleven bytes
without allocation or runtime crossing. Bounds/steps/Unicode/hooks and profiling preserve the original fallback.
Paired k-nucleotide improved 204.23 ->184.77 ms, bringing it close to Cranelift in quick samples.

Cached ASCII classification must be invalidated before host mutation, even when a mutable guard is forgotten.
Tests exercise that boundary and confirm non-ASCII strings cannot borrow the byte-index shortcut.
The initial packing loop generated a vector literal relocation rejected by the stencil audit; local volatile reads
prevent that transformation without weakening the audit. A tiny-stack parser failure reproduced in bytecode and
was kept separate from native continuation safety. Source size and instruction counts alone did not establish this win.

[Measurements, failed controls and validation](../benchmarks/results/overnight-copy-patch/README.md).

## 34. Entry imports remove load callbacks, not frame costs

The shared invariant-local analysis already proved which physical slots cannot change during an invocation.
CnP can seed their boxed registers once, then guard each original load for readiness, observation and Unbound.
This preserves error order and OSR behavior; the original local frame remains the GC root.
Inlined parameter aliases do not require separate imports.

Barnes removed 54 million runtime calls across seven executions and improved 3379 ->3195 ms in a paired check.
Its frame count did not change. Small nbody/sieve/K slowdowns remain visible, and a K follow-up had two large samples
without compilation. These quick results support the Barnes gain without establishing stable CnP superiority on K.
Importing locals also does not remove a generic numeric-add helper merely because its operands are now cached.

[Measurements, variance and regression coverage](../benchmarks/results/overnight-copy-patch/README.md).

## 35. Removing more continuation loads did not establish another win

An indexed-tail candidate certified full offset-selection/load/branch sequences at build time and rejected interior
branch entries. Real-template tests passed, but alternating Barnes/nbody/control samples changed direction.
The candidate was reverted. On these larger guarded templates, fewer loads did not establish a useful gain.
The earlier simple arithmetic-tail result remains valid; it does not predict every continuation rewrite.

## 36. Exact builtin identity permits small call-free operations

Generation-checked rows identify len/math.sqrt by their actual callable names. Exact sequence/string lengths and
primitive square roots can then run in copied Rust stencils without entering the scalar gateway.
Custom protocols, rebound callables, invalid domains and observation windows retain the original helper.
Preserving call accounting and error order matters as much as the arithmetic itself.

The paired Barnes result improved 3160 ->3070 ms, with another 27 million gateway calls removed across seven runs.
Frame/root counts did not change. Its remaining large gap, and nbody's closed-loop advantage in Cranelift,
still point to missing region capabilities. Backend-only compilation counters also exclude common lowering;
first VM/process results include it and should accompany cheap-linking claims.

[Rejected tail experiment and scalar measurements](../benchmarks/results/overnight-copy-patch/README.md).

## 37. Import the initialized portion of a fresh frame

The native bridge scanned every HIR slot even though fresh windows initialize only entry parameters.
Python bindings already live in the separately rooted local frame. Importing those parameter registers directly
improved paired Barnes samples by roughly 3.5–5%; operation/frame/root counters did not change.
OSR and resumed windows still require their complete live image. Reference/collection and OSR regressions check both paths.

This is not permission to retain uninitialized or stale values. VM shadow windows still clear and native images still
initialize as before. Root publication and argument validation remain intact. The nbody/ticket control spread also
shows why a small frame optimization needs matched controls rather than one timing.

The final profile and counters still point to frame/helper/heap/root work. Sharing closed numeric and recursive
regions is a larger next step than assuming another continuation tweak will close the remaining gap.

[Final comparisons, RSS, compilation boundaries and profile](../benchmarks/results/overnight-copy-patch/README.md).

## 38. Equal loop coverage largely removes the sieve backend gap

Cranelift now consumes the same Boolean fill/reduction LIR plans as CnP and emits its own guarded machine loops.
The sieve comparison changed from roughly 38ms Cranelift versus 3ms CnP to 2.45–2.80ms versus roughly 3ms.
The previous result primarily measured optimization coverage, not an intrinsic advantage of copying code.
Both backends account for the same selected iterations and preserve the original loop on misses.

The initial Cranelift reduction updated accumulators with add-zero work on false predicates.
Keeping checked additions and last-selected updates on the selected arm improved 3.50 ->2.63ms in the pilot.
A successful plan does not replace ordinary iterator exhaustion, snapshots, error ordering or profiling.
Shared tests now cover both backends, including exact backedge counts and gateway-free bulk work.

Cold costs still differ. New Cranelift backend compilation took roughly 5.5–7.8ms versus CnP 0.13–0.17ms;
a fresh-process sieve took 16.6ms versus 9.4ms. Those boundaries include different startup/lowering costs from
the retained execution comparison and should accompany claims that either backend is faster.

[Implementation, controls and reproduction](../benchmarks/results/2026-10-31-cranelift-boolean-regions/README.md).

## Open questions, 2026-10-31

- Can readonly/mutation Plan and its preflight/replay contract move out of native/readonly.rs so CnP can reuse it?
  The new backend must retain recursion budgets, exact guards, ordered writes and deoptimization without Cranelift.
- Can CnP use the shared LoopRegion/schema/nest plans to hoist view checks and run closed numeric loops?
  Direct fields/views/cursors and range fill/reduction are implemented; general closed numeric loops remain open.
- Can scalar values stay in machine registers across CnP stencils while preserving observation and side-exit images?
  Local imports remove load callbacks; they are not a hardware register allocator.
- How much of the remaining k-nucleotide cost comes from gateways versus hashing and string construction?
  ASCII short slices and guarded scalar calls are now call-free; roughly2.19M gateway calls remain per full execution.
- Can a longer slice remain a view until it escapes, with an explicit owner/lifetime and exit reconstruction?
- Which mapping lookup results can be reused across membership/load/update without skipping equality hooks or RHS effects?
- Can broader inlining reduce frame/root costs without increasing helper, state and compilation costs?

These are hypotheses, not measured wins.

## 39. Fresh-session suites measure different tradeoffs from retained kernels

The full 120-workload Flying Circus run tested whether retained CnP and Cranelift gains carry over to short programs.
Both tiers took about 2.11 times CPython's elapsed time across their 88 common passing workloads in repeated fresh sessions.
Their aggregate CnP/Cranelift ratio was 0.999, although Cranelift was much faster on Barnes–Hut.
These requests include compilation; they do not contradict the retained-kernel measurements.
Bytecode took 1.38 times Monty's elapsed time across 87 common passing workloads.

All tis tiers passed the same 88 workloads, with 17 unsupported and 15 failures.
Eight new Unicode workloads failed on missing string `find`; six workloads failed on missing `frozenset`.
The remaining failure was a class-base case.
Broad suite coverage and short-program compilation remain separate work from optimizing retained native loops.
Three timing samples and one PyPy warmup support rough comparisons only.

Evidence: [full Flying Circus report](../benchmarks/results/2026-10-31-flying-circus-full/README.md).

## 40. Share semantic bodies as well as compiled bytecode

The full suite exposed a 3× unpacking gap and heavy allocation in closure-heavy record building.
A CPU sample showed HIR cloning and destruction alongside allocator and GC work.
Closure creation already shared compiled bytecode and profiles, but deep-cloned semantic blocks and scope metadata.
Copy-on-write HIR bodies reduced record-building from 881 to 548 ms in a seven-sample comparison.
A follow-up measured 865 to 554 ms, while Monty took 329 ms.
Captures and defaults remain separate; editing a body detaches it and invalidates cached code.
Legacy addition feedback still mutates instruction data and can detach a body on first use.
Separating that state from semantic HIR remains an opportunity.

Guarded register-bytecode unpack batches reduced repeated unpacking from 152 to 46 ms, near Monty's 48 ms.
They require exact builtin sequences, adjacent reads, no other token users and a quiet feedback window.
Elements are copied before further execution; custom iterator effects and length errors retain the original path.
Cached ASCII indexing reduced random ASCII access from 15.6 to 4.1 ms, versus Monty's 2.8 ms.
Direct string-key dictionary construction did not establish a standalone throughput gain.

These wins did not establish suite-wide parity.
Across 87 common passing workloads, bytecode remained 1.24× Monty one-shot and 1.39× in repeated fresh sessions.
The paired frozen tis control was 1.31× and 1.43×, respectively.
Three-sample microbenchmark regressions largely disappeared in follow-up; Unicode memory cases still cost about 9–11% more.
Backend coverage: unpack dispatch is register-bytecode only; string/runtime and HIR storage changes are shared.
All execution tiers passed semantic tests; performance claims here concern bytecode.

Evidence: [Monty parity experiment](../benchmarks/results/2026-10-31-monty-parity/README.md).

## 41. UTF-8 offsets need selective caching and borrowed operations

Monty PR #1195 motivated removing repeated scans and full receiver copies before changing execution backends.
String methods and encoding now borrow receivers; bounds callbacks and custom join iteration finish before that borrow.
Unicode indexing, search and contiguous slices share character/byte offsets.
A cursor handles nearby reads; stride-32 checkpoints appear only after distant traversals scan 32 times the text bytes.
Cache keys include heap identity and generation; host mutable access invalidates them before exposing the object.
GC discards entries without keeping strings alive, and heap generation slots remain eight bytes.

The first version cached every Unicode length and probed the offset table on every heap mutation.
A quick retained-Latin-1 case regressed from 14.0 to 24.4 ms.
A per-heap recent-length cache avoids entries for length-only scans; actual Unicode string mutations alone probe invalidation.
Short Unicode indexing also regressed about 17% through the cursor table.
Strings up to 64 bytes now retain bounded scans; a seven-sample confirmation returned that case near baseline.
The full quick suite's larger memory regressions did not recur in confirmation; mixed-Unicode memory remained about 4% slower.
Small differences remain uncertain.

In confirmation, CJK random access improved from 35.8 to 5.7 ms, Unicode slices from 3.39 to 0.91 ms,
and repeated Unicode length from 18.1 to 1.50 ms.
ASCII-run case conversion improved from 20.3 to 4.84 ms.
Dense/short-run mixed text did not benefit and retains the standard decoder; contextual sigma uses full-string lowercase.
New search methods made eight agent workloads pass, raising full-suite coverage from 88 to 96 of 120.
The 88 paired passing workloads improved about 6% one-shot and 7% in repeated fresh sessions.
The Monty reference predates PR #1195; these are not comparisons against its optimized build.

Backend coverage: all four tiers use the shared services; all 27 confirmation programs pass in the final tis tiers.
No direct string-loop stencils or native UTF-8 lowering were added, so native runtime crossings remain.
Tests cover reentrant bounds conversion that searches the same string and triggers GC, host mutation and slot reuse.
Overall Monty parity remains unmet, and Chinese agent workloads still trail both reference runtimes.

Evidence: [borrowed string runtime experiment](../benchmarks/results/2026-10-31-string-runtime/README.md).
Source: [Monty PR #1195](https://github.com/pydantic/monty/pull/1195).

## 42. Workload coverage depends on runtime protocols as well as syntax

The remaining 24 Flying Circus failures clustered around a small set of missing services: immutable sets,
seeded randomness/codecs, regular expressions, lazy adapters, list subclasses, generators and object-copy protocols.
Implementing shared services raised coverage from 96 to 120 unchanged workloads in bytecode, LIR, Cranelift and copy-and-patch.
The full quick suite passed all 120 in both one-shot and repeated fresh-session modes.
This establishes workload coverage; the new modules still expose partial Python APIs.

Generator functions need an explicit HIR flag even when their yield is unreachable.
Otherwise a scalar planner can inline a function that should return a generator object.
`Yield` and delegation remain HIR operations. Suspension stores the bytecode position, frame values and locals in the heap;
the ordinary collector visits those values. Both bytecode encodings resume their saved frames.
Generator bodies currently reject native lowering and scalar inlining, and yield values do not become function return-type feedback.
Eagerly collecting generators would fail infinite pidigits input and change side effects, so that approach was excluded.

List subclasses retain class identity and user attributes alongside separately traced builtin list storage.
Inherited descriptors delegate to that storage; overrides and `super()` keep normal class lookup.
Cached instance dictionary views retain their owner and read the current attributes rather than returning snapshots.
Deepcopy roots source snapshots and memoized copies across reduction/state callbacks, preserving cycles and aliases.
Review added a dataclass repr recursion guard and roots for a temporary comparison operand across a second attribute callback.

Round trips alone cannot establish wire compatibility. Separate CPython checks matched MT19937 seeding/word packing,
all six codec outputs, and protocol 4/5 pickle in both directions.
Sparse pickle memo indices require dictionary cardinality for `MEMOIZE`, not the largest assigned index plus one.
The decoder checks operand lengths and rejects unsupported globals; it never imports or invokes host Python.

Remaining work includes native generator suspension, complete dictionary views, broader dataclass/copy hooks,
Decimal semantics and full regex/pickle APIs. The full-suite result does not establish CPython feature parity or performance parity.

Evidence: [all 120 Flying Circus workloads](../benchmarks/results/2026-10-31-all120/README.md).

## 43. Equivalent loop and call forms need independent performance checks

A frozen runtime passed 48 program controls/variants across 11 families, but equivalent structural rewrites exposed large cliffs.
Names, inputs and a Barnes–Hut field-layout variation stayed much steadier in the tested cases.
The generator preserves fixture timing boundaries, verifies AST round-trip controls and checks outputs against CPython.
CPython-normalized ratios distinguish a change in work from a loss of relative performance.

With ten samples, sieve's inner `for range` rewritten as `while` makes CnP 46–50× slower and Cranelift 14–15× slower.
Bytecode rises about 12%, while CPython rises about 27%. Runtime crossings jump from thousands to millions.
This is missing loop-form coverage, not evidence that the changed limit or identifier names need special handling.
General induction-loop lowering should serve both source forms and both native backends.

Extracting n-body's squared-distance expression into a helper makes CnP about 2× slower and Cranelift about 8.5× slower.
Cranelift still records 198,920 successful arithmetic body inlines, but preflight callbacks remain and region fusion shrinks.
Inlining arithmetic without eliminating/hoisting the call boundary does not preserve the original fused loop.
Experimental CFG inlining does not repair the measured counters in diagnostic runs.
Unpacking also warns against assuming static unrolling helps: a looped form is over twice as fast for fresh-session Cranelift.
Profiling, lowering, compilation and execution all contribute to these timings.

The broad three-sample naming/data shifts are inconclusive; several one-shot outliers do not recur in repeated mode.
No runtime changes were made during this probe. Future optimization work needs fixed regression variants and fresh seeded
holdouts, plus first-execution and retained-execution measurements. The tested seed is not evidence of universal generalization.

Evidence: [performance generalization probe](../benchmarks/results/2026-10-31-variations/README.md).

## 44. Share loop plans across iterator and induction-variable syntax

The Boolean fill kernel previously required a `Next` iterator header.
An equivalent integer-counter `while` loop incurred millions of callbacks.
The shared LIR plan now recognizes guarded integer induction loops for both native backends.
It proves direction, finite inline arithmetic and all store bounds before writing, and publishes the final counter.
Misses retain the original loop and its partial effects, custom arithmetic and arbitrary-precision values.

Ten-sample paired medians reduce the 1M sieve while variant from 187 to 3.75 ms in CnP and 125 to 8.09 ms in Cranelift.
The for-range controls take 3.70 and 8.20 ms respectively.
A renamed, reversed-comparison holdout at a fresh limit improves similarly.
Barnes–Hut and unpacking controls remain within about 1% in these samples.
Timings include compilation in fresh VM sessions; they do not measure retained JIT throughput.

This is a shared recognizer, not general induction-loop normalization.
A held-out `counter < limit + 1` header still misses because it computes its bound inside the header.
It remains 144 ms in CnP and 95 ms in Cranelift.
Loop-invariant expression hoisting and the n-body helper/preflight fusion gap remain unfinished.
Keep these failures alongside the successful variants instead of rewriting tests to fit the recognizer.

Evidence: [counter-loop fills](../benchmarks/results/2026-10-31-counter-loops/README.md).

## 45. New applications expose gaps that benchmark rewrites can miss

Seventeen custom programs vary loop form, helper boundaries, sequence layouts, dictionary update syntax,
Unicode data, bound methods and constructor store order.
All 255 checks across five runtimes and three timing boundaries produce the expected output.
These workloads were frozen before timing; the runtime was not changed to fit them.

The direct retained numeric loop takes 0.24 ms with Cranelift versus 1.01 ms with CPython and 0.11 ms with PyPy.
Extracting the arithmetic helper changes Cranelift to 5.51 ms, CPython to 1.30 ms and PyPy to 0.29 ms.
The helper boundary gap therefore reproduces outside n-body.
Dictionary updates remain slower with retained native code than with bytecode, CPython or PyPy.
Bound methods lose the advantage CnP achieves for direct object fields.

The custom counter fill also misses the new region plan: its literal stride adds a constant instruction to the body.
LIR inspection confirms the four-instruction body where the recognizer expects three.
The range form selects the kernel, but generic `list.count(False)` still contributes substantial runtime-library cost.
Preserve these application costs and failures rather than changing the checksum or lifting constants in the benchmark.
General invariant handling remains necessary.

Keep cold/fresh sessions distinct from retained sessions with the same function objects.
Even 20 retained warmups leave outliers in these short PyPy samples; medians alone do not establish stable throughput.
Record all warmups and measured calls, and report absolute times as well as ratios.

Evidence: [custom variation comparison](../benchmarks/results/2026-10-31-custom-variations/README.md).

## 46. Runtime crossings and frame/root work are different costs

The custom variation suite was frozen before changing the compiler/runtime.
Guarded literal/affine induction plans and exact Boolean count reduce retained CnP fills from 43–45 ms to 0.46 ms.
Bytecode benefits from count and dictionary runtime improvements without a JIT change.

CnP now copies numeric helper expressions after the same preflight used by Cranelift.
In the numeric-helper diagnostic, runtime crossings remain 38,596, while root synchronizations drop from 18,898 to 26
and positional frames from 20,002 to 1,130.
Retained execution improves from 4.66 to 1.77 ms.
Counting callbacks alone would miss the gain: a closed guard is cheaper than constructing and executing a frame.
Code/patch growth and compile cost remain measured; copying a body is not free.

The two backends also had different float-view acquisition paths.
Giving Cranelift the guarded directory/token path already available in CnP improves the fresh-session extracted-helper
n-body variant from 268 to 118 ms. The original n-body control remains near 31 ms.
The equivalent CnP helper variant improves from 86 to 55 ms, while its original control remains near 43 ms.

Auditing field-expression copying found a missing descriptor/hook guard in the shared inliner.
Shape and float checks cannot justify deduplicating property reads.
The shared preflight now rejects field plans when attribute protocols are active, preserving both getter calls.
The guard is conservative until a class/version proof replaces it.

Builtin dict.get can use a closed probe only after checking the actual bound receiver and key protocols.
String reads borrow existing storage; existing-key writes replace a slot without cloning its key.
Protocol fallbacks must retain default values through callbacks and collection.
Dictionary time improves about 30%, but dispatch and method acquisition remain far from CPython/PyPy.
Per-call inline validation and method-object construction also remain; no backend parity claim follows from these wins.

Evidence: [custom variation fixes](../benchmarks/results/2026-10-31-custom-fixes/README.md).

## 47. Inlining needs a loop-lifetime proof to remove repeated validation

Copying a helper body removes frames, but per-call guards still prevent closed-loop execution.
A shared region proof now checks unbound argument-only float helpers at entry, with callable revision/type guards.
Retained numeric-helper time drops from 4.51 to 0.276 ms with Cranelift and 1.77 to 0.844 ms with CnP.
The direct loop remains about 0.245 ms and 0.747 ms respectively. Extracted-helper n-body improves
123 → 34.7 ms with Cranelift, while original n-body remains about 32 ms.

Prepare metadata once: scanning the IR at every entry regressed original n-body about 20%, despite selecting
no new helper plan there. Verified call/name site lists and skipping preparation for ordinary regions removed most
of that cost. The final original n-body sample is still about 2.4% slower; short samples do not resolve its cause.

Callback invalidation, sticky failed attempts and conservative nest boundaries are part of the optimization.
Missing names cannot raise before an empty loop or preceding effect; overloaded arithmetic must retain callbacks,
rebinding and GC behavior. Methods, field reads and other effects remain excluded. Runtime crossings fall by orders
of magnitude, but this does not address method acquisition or dictionary updates and does not establish PyPy parity.

Evidence: [loop-entry inlining](../benchmarks/results/2026-10-31-loop-inlining/README.md).

## Updating this notebook

After a substantive architecture or performance experiment, add or revise the applicable lesson.
State the hypothesis, observed result, conclusion, limits and link to evidence.
Record negative and inconclusive outcomes even when their implementation is reverted or disabled.
Mark superseded conclusions and explain which later experiment changed them.
Keep detailed commands and per-run measurements in the experiment report.
