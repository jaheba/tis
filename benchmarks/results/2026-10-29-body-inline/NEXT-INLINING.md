# Next inlining step toward the measured PyPy target

This is an implementation design, not another implemented optimization. The validated readonly inliner remains runnable. No general recursive, allocating or effectful inliner is claimed here.

The matched ten-sample comparison currently leaves a roughly14.4× warmed gap to local PyPy7.3.16. The prior callee histogram identifies about2.14million calculate_force calls; removing255,303 contains frames leaves2.52million physical positional frames. Reducing the readonly preflight alone, sampled near1% inclusive, cannot close this gap.

## Concrete limits in the present VM

`src/lir.rs::FrameState` has one resume point, one implicit lexical environment and one local layout. `DeoptFrame` owns one FunctionId/revision and one set of slots/locals. `Function::reconstruct` indexes only that function's `hir_values`.

`src/tier.rs::Vm::install_lir` intentionally requires boxed results and parameters to correspond to original caller HIR slots. `sync_lir_roots` writes roots at `base + ValueId`. Reusing callee ValueIds here would alias caller slots. Merely relaxing the verifier would remove a safety check without adding a valid root or reconstruction model.

`src/vm.rs::Vm::finish_lir` resumes one existing physical caller window and its current lexical environment. It cannot resume a callee at an interior operation and then install the return value in a suspended caller. Runtime calls also use `environments.last()` and the physical depth counter. Those assumptions must be made explicit before effectful callee bodies are incorporated.

The current body plan avoids these limits deliberately: no allocation/calls/writes/lexical reads, all operand and callable guards before a nonraising numeric body, primitive result, and a single original-call fallback before effects. Acyclic short-circuit paths can be speculated only because all supported operations are readonly and nonraising after preflight.

## Bounded next experiment before general frame reconstruction

Measure which calculate_force return paths dominate before selecting a prefix. A general effect-free prefix/trace inliner could copy a guarded entry path that returns a primitive or a single tuple, while all other paths miss to the original Call before any effect. This must be selected from HIR and profiling, not a hard-coded function name or Barnes-specific formula.

For example, the source has an early total_mass==0 return. A generic prefix compiler could support a numeric field/literal comparison, a guarded branch and a tuple of primitive constants. The actual function/revision and method binding still need guards. A tuple-return hit must validate every condition and a one-object allocation budget before allocating once, with no possible miss, Python call or collection afterward. Returned references must be published before the next caller checkpoint. A miss cannot leave an extra tuple allocation behind and restart the Call. Integer/float comparison must retain exact semantics, including large integers; the existing NumberCompare rules are relevant.

This smaller extension can demonstrate more real body incorporation without pretending to support the recursive remainder. Its path frequency, frames removed, callbacks, allocations and native time must all be measured. If the path is rare, do not build it merely because it is easy.

## General inlined-frame model

Introduce a verified inline scope identity separate from machine-register identity. Each scope needs immutable source FunctionId/revision, original HIR metadata, a guarded callable/binding description, its lexical environment root and parameter/local mapping. Every copied operation needs a semantic origin `(scope, HIR ValueId)`; the current unqualified caller origin is insufficient for nested diagnostics and reconstruction.

A safepoint snapshot must describe the complete logical stack of active inline scopes. Each scope records an exact HIR resume point, live boxed/unboxed values, local values, environment, and its caller return continuation. Root locations must be assigned independently of callee HIR slot numbers; a verified mapping to dedicated root segments or explicitly traced transient roots is required. Deferred float boxing and later tuple recipes must be explicit materialization recipes. Preserve the16-byte public Value/Slot ABI.

A suspended caller needs a pending-return continuation: original call destination, the caller state after that call, and the exact resume point after installing the callee's eventual result. It cannot simply retain a pre-call snapshot and execute the Call again. Resume positions at the end of a block/terminator need bytecode mapping support, not an assumed instruction+1 offset.

On a callee side exit after a completed write or call, reconstruct all logical frames as GC-visible state, resume the innermost callee at that exact operation, then inject its return value into the suspended caller and continue after the original call. On error, unwind the reconstructed logical stack without executing caller continuations. Completed argument evaluation, attribute lookup, mutations and Python calls must never be repeated.

At every reentrant helper, lexical lookup must use the current callee's environment rather than whichever physical environment happens to be last. Prefer explicit scoped helper inputs or a rigorously managed logical frame bridge. A runtime call must observe physical depth plus active virtual depth, so recursion limits remain correct across mixed inlined and ordinary calls. Restore depth, environments, local frames, roots and scratch on success, error and contained panic.

Callable guards must retain the value captured by original attribute evaluation. Looking up a method again after argument evaluation is wrong if an argument changes the class or instance binding. Eliminating bound-method allocation therefore needs its own virtual value/guard/materialization design; it is separate from body incorporation.

## Extension order and required evidence

1. Add source scopes, pending-return continuations and a reference LIR implementation of nested reconstruction. Keep native emission disabled until verifier and execution tests agree.
2. Extend boxed temporary/root mappings explicitly. Verify dominance, operand types, snapshot ownership, environment roots, materialization dependencies and scope revisions. Reject stale or cross-scope states.
3. Add readonly nested field/global access and guarded primitive builtins. Global and closure bindings must be read from the actual callee environment. Existing pure sqrt can become a native operation only with actual callable and domain guards; argument effects retain source order.
4. Add guarded division and allocation at exact safepoints with nested snapshots. Only then consider effectful bodies, and bound recursion/unroll depth independently of language recursion limits.
5. Propagate types/shapes through the resulting longer paths, reduce boxing and materialize tuple/bound-method values only where their identity or escape requires it. Keep deoptimization recipes explicit.

Before performance claims, cover method/function replacement, same identity/new revision, receiver substitution, default/keyword/closure binding, polymorphism, zero/NaN/infinity/large-int arithmetic, missing attributes, every guard/error edge after prior effects, aliases, recursive depth limits, OSR, eager observation, nested GC, transient reference returns and success/error/panic cleanup. Include a test that clears the last ordinary owner of a reference before a nested safepoint, and another that would visibly duplicate a write if a caller Call were replayed.

Measure at least ten serial validated warmed and fresh-process samples against the last frozen source, with no overlapping build/test/profile jobs. Record actual inline hits, logical/physical frames, callbacks, root publication, allocations and deoptimizations. Refresh matched PyPy numbers on the exact fixture and protocol; retain the absolute gap as the acceptance target rather than only reporting improvements over tis.
