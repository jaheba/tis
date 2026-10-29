# Bounded multi-block inlining

Multi-block HIR bodies can now be incorporated into caller LIR, including recursive call sites.
This is a tested experimental path, **disabled by default** because Barnes–Hut is slower with it enabled.
Set `Vm::cfg_inlining = true` before feedback collection, or pass `--cfg-inlining` to the `bench`/`run` examples.
`lir::Function::lower_with_cfg` exposes the graph for inspection.

## Implementation and limits

The pass splits the caller at each copied call, remaps callee block arguments, and merges multiple returns through a continuation parameter.
Caller and callee retain separate semantic identities, registers, environments and reconstruction points.
Synthetic LIR blocks do not produce physical caller HIR block observations.
Nested Python calls charge the logical callee's recursion level and use its actual captured environment.
Runtime error paths restore the physical environment and depth.
Lexical reads use the actual environment rather than a constant captured binding.
Copied scalar builtins retain the guarded probe and a fully rooted scoped-call fallback.
Floating-point addition uses the same scalar operator as other floating arithmetic.

Recursive profile capture happens before borrowing caller feedback mutably, so shared caller/callee profiles can be observed.
The existing guard still checks semantic identity/revision, binding, arguments and language depth before effects.
Nested exits preserve completed writes and pending return continuations.
The verifier checks synthetic block origins, scoped runtime environments, dominance and complete logical root recipes.
The existing single-block path remains the default, with its 96/192 instruction limits.

Experimental limits are four positional parameters, 48 blocks and 384 HIR instructions per callee, and 768 copied instructions per artifact.
Loops, defaults, per-call namespace environments and unsupported local/namespace operations retain ordinary calls.
The inline boundary accepts positional calls; nested runtime calls retain their normal binder.
Loop-region/nest discovery is currently skipped for artifacts with expanded CFGs.
Copied comparisons and sequence unpacking do not yet receive every specialization used by ordinary LIR lowering.

## Validation

**297 debug tests and 297 release tests pass**, together with formatting and all-target Clippy with warnings denied.
New tests cover branch merges and multiple returns, both bytecode modes, all three compiled backends,
allocation/collection across nested calls, closure rebinding, guard exits after writes, recursive depth limits,
builtin replacement, scoped domain-error accounting, default exclusion, and corrupted context/origin metadata.
The experimental path also validates the full Barnes–Hut, Ticket Search and game_nbody fixture outputs.
No heap-storage or stencil source was changed.

`targets.log` and `calculate-force.lir.txt` confirm four recursive `calculate_force` expansions when lowering its recorded compilation request.
That lowering produces 159 LIR blocks; the recorded `advance_system` request also incorporates `calculate_force`.
Inspection ran separately after all timing completed.

## Measurements

These measurements were already in progress before the request to use faster benchmarks.
Each warmed comparison has ten randomized interleaved pairs, fresh workers and two warmups per sample.
Builds/tests/profiles did not overlap timing, and each sample validates the unchanged fixture output.

| Native comparison | Baseline | Candidate |
| --- | ---: | ---: |
| Previous build → new default, Barnes–Hut | 2.267707 s | 2.280833 s |
| Same binary, CFG off → on, Barnes–Hut | 2.231014 s | 3.199197 s |
| Previous build → new default, Ticket Search | 50.689 ms | 50.534 ms |
| Same binary, CFG off → on, Ticket Search | 51.235 ms | 51.226 ms |

Default-mode timings are close, with unchanged execution counters.
Experimental Barnes–Hut is **43.4% slower**, despite physical positional frames falling
**2,502,535 → 1,241,377** (50.4% fewer).
Callbacks rise **32,265,625 → 42,928,650** and root synchronizations rise **7,479,319 → 11,674,902**.
Logical copied entries rise from 20,000 to 1,439,670.
All warmed samples have zero backend compilation and deoptimizations.
Ticket Search has unchanged execution counters and no additional copied entries.
The counts expose additional helper/root work; they are not a CPU-time attribution.

Ten zero-warmup fresh-process samples include startup, parsing, JIT compilation, execution and exit.
Barnes–Hut is **2.293781 → 5.282636 seconds**, and Ticket Search **66.945 → 68.478 ms**, with CFG off → on.
No new PyPy run was performed and no narrowing of its previously measured lead is claimed.
The next work is to retain ordinary scalar/sequence specialization in copied bodies and reduce root-map work before enabling this by default.

## Faster iteration and provenance

The benchmark tools now default to **three samples and one warmup**, and accept one sample.
One-shot tooling defaults to three fresh processes.
Single-sample summaries use the sole value for their quartiles.
Quick comparison, one-shot and suite smoke runs pass; their logs accompany the evidence.
Longer confirmation runs remain available through explicit `--samples`/`--warmups` flags.

`source-sha256.json` describes the measured sources; `measured-tools/` preserves the harness before its later fast-default changes.
`delivered-source-sha256.json` records the delivered sources, which differ from measured inputs only in those Python tools.
The original commands/driver relied on ten-sample defaults; add `--samples 10 --warmups 2` when reproducing warmed runs with current tools.
Use `--samples 10` for the recorded cold protocol.
Raw data includes executable hashes and option flags.
`change.patch` is relative to the uncommitted baseline described by `baseline-source-sha256.json`.
No commits or pushes were made.
