# Barnes–Hut JIT checkpoint

Latest completed checkpoint: `../2026-10-29-body-inline/CHECKPOINT.md` (validated actual readonly body inlining; use body-inline-checked binaries as the next baseline; PyPy parity remains unmet).

## Current status — 2026-10-28 23:23 UTC (29 October 00:23 Europe/Berlin)

This checkpoint is complete and reviewable. No benchmark, build, test or profile process remains active. No commits or pushes. All pre-existing shared-frame and instance-shape changes remain.

Current source matches frozen `/private/tmp/tis-profile/fields-hinted-{worker,run,nbody}`. `source-sha256.json` records compiled Rust/build/fixture inputs. Baseline is `/private/tmp/tis-profile/fields-baseline-{worker,run,nbody}` (the original shapes binaries). Use the hinted candidate as the next baseline; do not accidentally use older all-site/first experiments.

Measured final results (>=10 serial interleaved validated samples each):

- Freshly warmed native Barnes: 4.656899167 -> 3.599100604 s, 22.7% less time; zero timed compilation/deopts in all 20 samples.
- Reused warmed native Barnes: 4.6479161045 -> 3.6144255415 s, 22.2% less time.
- Cold native Barnes: 4.679346313 -> 3.6788105415 s, 21.4% less time.
- Native callbacks: 114,782,260 -> 50,713,113 (-55.8%); 34,447,355 direct field loads and 29,621,792 quiet block entries; root synchronizations unchanged at 18,689,295.
- Cold native ticket regression fixed by LIR Instance-receiver hints: final baseline 70.349 -> 68.450ms (-2.7% time); isolated vs all-site candidate 75.000 -> 69.824ms (-6.9%).
- Native ticket warm -2.2% time, nbody native -2.6%. Small bytecode/copy-patch regressions remain documented: reused bytecode Barnes +1.5%, nbody +1.6%, ticket +4.4%; fresh-worker ticket bytecode is flat (-0.3%), copy-patch +1.8%. Investigate disabled native metadata/mutation-guard overhead and process layout, not benchmark algorithms.

Validation: cargo fmt --all, all-target Clippy -D warnings, all 221 debug + 221 release tests pass. `README.md` in this directory is the completed report; raw JSON, hashes, profiles and logs are included.

## Next authorized work

1. Read the completed report and inspect current time. Hard cutoff: 2026-10-29 07:00 UTC / 08:00 Europe/Berlin. Do not start any operation that can cross it.
2. Highest simple JIT lead: `src/tier.rs::Vm::lir_helper` still sends all-positional `CoreOp::Call(names)` through `Vm::operation`, allocating positional/keyword scratch and cloning builtin callees. LIR/native callers already publish roots and extend transient_roots around helpers. Consider reusing `call_from_registers(args[0], &args[1..])` only when names are all None, preserving keyword/star binder. Verify native/LIR/copy-patch, defaults/bound calls, keyed-sort callbacks, GC and error cleanup. Measure vs fields-hinted candidate before retaining.
3. Profile/count generic helpers before larger numeric specialization, field value-type propagation, method fusion/inlining. Mixed Int/Float Rectangle/QuadTree operations may prevent monomorphic lowering; generic native entry and root synchronization remain material. Do not fuse methods across effects or side exits without explicit reconstruction support.
4. Keep source runnable and finish one optimization plus full validation/serial comparisons before beginning another. Never overlap formal timing with builds/tests/profiles. No commits/pushes.

Existing heartbeat `barnes-hut-jit-overnight` targets this chat every 15 minutes through cutoff; do not create a duplicate. At/after cutoff, only finalize the overall report and stop implementation/benchmarking.

## Earlier chronological notes


Deadline: 2026-10-29 07:00 UTC (08:00 Europe/Berlin).
Started: 2026-10-28 22:39 UTC.

Pre-existing uncommitted shared-frame and instance-shape work must be preserved. No commits or pushes authorized.

Baseline frozen in /private/tmp/tis-profile/fields-baseline-{worker,run,nbody}, copied from validated shapes binaries. Initial evidence: instance-shapes/README.md.

Next: profile warmed native Barnes–Hut, then implement one measured improvement with GC/ABI invariants explicit. Run formatting, all-target Clippy, tests; compare >=10 serial interleaved validated samples plus ticket/nbody and fresh-process checks. No profiling/build/test overlap with formal timings.

2026-10-28 22:51 UTC: two six-second warmed native profiles completed under normal sandbox escalation. Field probes account for 13.39% inclusive; Context::step 17.97% exclusive. Raw profiles: /private/tmp/tis-profile/fields-before.

Implemented inline guarded loads in src/native/fields.rs, weak native heap row directory, explicit Value discriminants/ObjectId C layout, inline counters. Initial frozen experiment fields-first-worker reduces runtime callbacks from 114,797,125 to 80,345,517 and samples native around 4.03 s vs 4.70 s. Ten-sample interleaved comparison still finishing at /private/tmp/tis-profile/fields-first-barnes.

Current source advances the first experiment: native-only lazy side directory preserves compact generation storage for bytecode; invalidates rows before handing out mutable guards (mem::forget safe); refreshes on drop. Not built yet while formal timings run. Next: finish timing, format/test/build this hardened version, freeze candidate and compare. Additional opportunity found: native CheckLocal still goes through the generic callback on every SSA local read; guarded tag check can avoid those callbacks while preserving UnboundLocalError fallback. Do not overlap formal timings with builds/tests/profiling.

Snapshots of pre-experiment files: /private/tmp/tis-profile/fields-source-before. These preserve all user-authorized uncommitted changes; never git-reset the tree.

2026-10-28 22:57 UTC: first experiment complete: native median 4.7060617505 s -> 4.0324768545 s (14.3% reduction), bytecode flat 2.52295 -> 2.51780 s. Hardened lazy directory frozen as fields-safe-{worker,run,nbody}. Exact inline counter confirms ~34.45m callback-free field hits.

A CheckLocal inline experiment (frozen checklocals-worker, /private/tmp/tis-profile/checklocals-barnes) executes zero warmed inline checks on Barnes–Hut and does not improve time; removed all CheckLocal-only code/tests/counters, preserving field work. Current source next experiment: apply existing lazy native root publication to all non-feedback artifacts instead of only artifacts containing loop regions. Build/tests must wait until checklocals timings finish.

2026-10-28 22:58 UTC: lazy-root experiment passed full debug/release suites and all-target Clippy. Ten-sample isolated improvement 4.0697585 -> 3.945674521 s (3.0%). Frozen fields-lazy-worker. Quiet-block experiment also retained: /private/tmp/tis-profile/fields-blocks-barnes, ten interleaved native medians 3.947532208 -> 3.6423650835 s (7.7%). It removes 29,621,792 ENTER callbacks per run; total callbacks 50,709,513, inline field loads 34,450,955, root syncs 18,689,295. New native_blocks test checks exact 20-backedge count and eager profile observation. No CheckLocal experiment code remains.

Current code is complete for this checkpoint: guarded direct field loads + lazy root publication + quiet block bookkeeping. Post-change profile running in /private/tmp/tis-profile/fields-after; next perform final formatting/Clippy/debug+release tests, freeze final executables, then >=10 overall Barnes, ticket and nbody comparisons plus one-shot and publish evidence here. Do not start another optimization before this checkpoint is fully validated. Existing overnight heartbeat already targets this chat every 15 minutes through deadline; do not duplicate it.

Remaining-bottleneck lead (read-only inspection, NOT IMPLEMENTED): src/tier.rs::Vm::lir_helper still sends positional CoreOp::Call through Vm::operation, which allocates positional/keyword scratch and clones callee objects via call_keywords. Register bytecode already uses call_from_registers with fast borrowed builtin names. LIR/native callers already publish roots and extend transient_roots for operands. A narrow shared LIR-helper branch for Call(names) where all names are None could reuse call_from_registers(args[0], &args[1..]); preserve general keyword/star binder. This may remove ~6.7m positional scratch allocations and improve ~3.9m leaf calls on Barnes. Profile/measure after finalizing current checkpoint.

Other follow-up leads: exact helper-op histogram on an untimed diagnostic run; mixed Int/Float arithmetic and comparisons in Rectangle/QuadTree prevent some monomorphic float lowering; method GetAttr+Call still allocates bound objects. Avoid unsafe method fusion across intervening effects/side exits. Post-change profile shows Context::step 16.72% exclusive, root sync 6.30%, native entry 7.43%; field callback no longer a top hotspot.

Final gates complete: 220 debug tests and 220 release tests pass; cargo fmt --all and all-target Clippy -D warnings pass. Logs archived here. Frozen fields-final-{worker,run,nbody} match source. Overall reused-worker Barnes median native 4.705020375 -> 3.668930583 s (22.0% reduction); bytecode 2.5108447505 -> 2.495632542 s (0.6%). Ticket native +1.2% time, nbody bytecode +1.4%, nbody native -2.9%; other regressions below 1%. Treat small changes cautiously. Raw reused Barnes samples contain delayed compilations up to 10.6ms (rounds 4/10); retain them and explicitly disclose. Planned additional native comparison uses --fresh-processes and two warmups per fresh worker to avoid profile-phase drift.

Currently /private/tmp/tis-profile/fields-final-compare.py is still executing the serial one-shot comparisons (exec session 12795). Wait for completion before starting /private/tmp/tis-profile/fields-final-fresh.py. No builds/tests/profiles overlap. Final report pending cold/fresh outputs. No code experiments are unfinished.

Cold regression found before finalizing: native ticket one-shot 71.020 -> 74.195ms (+4.5%), while cold Barnes improves 20.3%. Source refinement underway: LIR Attribute.inline_instances records observed Instance receiver feedback; Cranelift emits full inline guards and enables the native directory only for such sites. Builtin-only sites retain the original probe/general helpers. A test covers a warmed builtin-copy site later receiving an instance with a callable own field. This source refinement is NOT YET BUILT/TESTED because the old-candidate fresh-process timing is running (session 38203, /private/tmp/tis-profile/fields-final-barnes-fresh). Finish that operation before formatting/gates/build. Old all-site source snapshots fields-all-sites-{native,lir,shape}.rs in tmp; all preexisting changes remain intact.

Receiver-hinted refinement now passes 221 debug + 221 release tests, fmt and all-target Clippy. Frozen fields-hinted-{worker,run,nbody}, source-sha256.json records all Rust inputs. New final serial comparison script /private/tmp/tis-profile/fields-hinted-compare.py is running in exec session 68316: warmed Barnes/ticket/nbody, cold Barnes/ticket, fresh warmed native Barnes (10 fresh workers/backend), isolated native cold ticket vs all-site candidate. Wait for completion; no code changes/builds/tests/profiles during these timings. Previous all-site evidence retained as all-sites-*.json.
