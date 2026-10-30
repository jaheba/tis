# tis

<img src="assets/tis-logo-compact.png" alt="Pixel-art Black Knight with no arms and a red bunny emblem" width="128">

A research Python VM in Rust with a profiling interpreter and two native compilation backends.
Built around an explicit value IR, tis implements a Python subset.
Ruff AST → HIR → register bytecode → interpreter.
Runtime profiles feed HIR → typed LIR → reference execution, Cranelift, or copy-and-patch native code.
There is no stdlib or sandbox infrastructure.

```sh
cargo run --release -- tests/fixtures/game_nbody.py
# -0.169075164
# -0.169089263
cargo run --release -- tests/fixtures/language.py
# language core ok

# Hot loops switch into native code; --stats reports tier entries, exits and emitted bytes.
cargo run --release -- --tier native --stats tests/fixtures/game_nbody.py
cargo run --release -- --tier copy-patch --stats tests/fixtures/game_nbody.py
```

Build the CLI with `cargo build --release`, or install it with `cargo install --path .`.

```sh
tis script.py
tis -c 'print(1 + 2)'
printf 'print(42)\n' | tis -
tis --tier native --readonly-regions --mutation-regions script.py
tis --help
```

`game_nbody.py` is the unchanged Flying Circus workload, including its full 20,000 iterations.
The acceptance test checks its output against CPython and checks that collection bounds heap growth.

## String values

UTF-8 strings of up to 11 bytes are stored directly in the 16-byte `Value`; longer strings use the heap.
The capacity is measured in bytes, so two emoji fit inline and three need a heap object.
The current four-byte tag and native field layout leave less inline space than Monty PR #994's 15-byte encoding.
Equal inline strings share identity under `is`, including independently constructed strings.
`Heap::string` and `Heap::string_owned` select the representation; `Heap::as_str` reads either form.
`Heap::allocate(Object::String(...))` explicitly creates a heap string, including for short text.

## Language core

- Lists, tuples, insertion-ordered dictionaries, sets, indexing, slicing and nested/starred unpacking.
- Numeric arithmetic, comparisons, short-circuit boolean expressions and conditional expressions.
- Functions, recursion, lambdas, definition-time defaults, positional-only and keyword-only arguments,
  `*args`, `**kwargs`, and starred call arguments.
- Shared lexical environments for closures, including `global` and `nonlocal` assignment.
- `for`, `while`, loop `else`, `break` and `continue`.
- List, set and dictionary comprehensions with nested generators and filters, in their own lexical scope.
- Classes with C3 multiple inheritance, constructors, bound methods, decorators and `super()`.
- Arithmetic/reflected/in-place slots, rich comparisons, truth/length, indexing, iteration, calling and string conversion.
- Custom descriptors, properties, static/class methods and attribute access/mutation hooks.
- Hashable user objects in dictionaries and sets, including equality callbacks and keys retained through GC.
- General `try`/`except`/`else`/`finally`, exception subclasses, `except ... as ...`, chaining and bare reraising.
- Synchronous context managers, including suppression and cleanup on return, break and continue.
- Lazy single-clause generator expressions; list slice assignment/deletion and `__index__` conversions.
- Core builtins, container methods, lazy `range`, `enumerate` and `zip`, and basic f-string formatting.

Class bases and object classes are fixed at creation: assigning or deleting `C.__bases__` or `obj.__class__`
raises `TypeError`, including through `setattr`, `delattr` and the `object` attribute bypass methods.

Object protocols use class lookup, independently of instance attributes.
Generic operations may reenter Python, allocate and mutate; their LIR effects retain full GC/deoptimization state.
Guarded builtin operations keep non-reentrant fast paths.
Installing descriptors or attribute hooks, or mutating a class namespace, disables direct field/region shortcuts
for that VM, including previously compiled readers.
This conservative guard can later become a per-type version check.
Builtin-only dictionaries retain indexed lookups; protocol keys use rooted, hash-filtered snapshots and retry
when equality callbacks structurally mutate the mapping.

Protected suites are HIR functions sharing their enclosing environment.
Their explicit control outcomes carry normal completion, return, break, continue or an exception through cleanup.
They do not reparse source or delegate execution to another interpreter.

The language remains a subset: metaclasses, `__slots__`, `__del__` finalizers,
builtin-type subclassing (except exceptions), general `yield`,
async constructs, `except*`, arbitrary module imports and most of the standard library are unsupported.
Integers remain signed 64-bit values.
Tracebacks retain raise/unwind identity and chains, including context-manager callbacks;
Python frame objects, source-line traceback formatting and traceback frame inspection are not implemented.

The executing function retains its HIR and runtime profile.
Detailed feedback follows `Cold → Profiling → Ready → Compiled` for each HIR identity and revision.
Cold execution maintains function-entry and backedge counters without operand, result or call-target observations.
Reaching a hotspot threshold opens a function-wide observation window, including inside a function's first invocation.
The default window ends after 128 completed calls or backedges and records at most 32 samples per site.
Ready bytecode and compiled tiers freeze detailed feedback while execution and failure counters continue.
Guard misses bypass sampling, record unexpected types and side-exit reasons, and reopen a profiling window.
At most three windows run per revision; further misses retire automatic compilation for that revision.
Observations accumulate across windows, so repeatedly polymorphic sites lower to generic operations.

`Vm::profiling` configures window and sampling budgets.
`ProfileConfig::eager()` explicitly enables complete observations for diagnostics and manual lowering experiments.
Site `executions` counts sampled executions, while function entries, returns, errors and backedges are lifetime counters.
Block/edge metadata records visits during profiling windows.
Existing adaptive bytecode addition also keeps its lightweight counters outside those windows.
Argument and return types are sampled when calls enter or return during an active window.

Eight successful integer observations select a guarded integer addition strategy.
Type or overflow guard failure falls back to generic arithmetic without rewriting bytecode.
Name loads, stores, calls and object operations remain explicit HIR operations.
Uncaptured function locals are promoted to SSA values before bytecode generation.
Assignments and reads become value references, with block parameters at branches and loop headers.
Only parameter loads remain in the prologue; possibly unbound reads retain explicit `CheckLocal` operations.
An internal unbound marker crosses joins without becoming Python `None`.
`compile_with_options(source, CompileOptions { promote_locals: false })` retains slot HIR for comparisons.
Names referenced anywhere in a nested scope conservatively stay in shared environments, even when shadowed there.
Module/class namespaces and global/nonlocal bindings retain their named lookup rules.

## Eager comprehension lowering

Eligible list, set and dict comprehensions that capture an enclosing function local share its HIR control flow.
Their targets become SSA values within the comprehension blocks, so shadowing does not leak into the caller.
The first iterable is evaluated before entering the private scope. Ordinary local promotion then keeps
uncaptured outer values in SSA, including a sieve's flag list used by its result comprehension.

Comprehensions without that capture benefit keep smaller separately compiled functions.
Comprehensions that create escaping callables or generators, inspect frame state, or may read an unbound target
retain separate frames. Module/class scopes and exception-control fragments also retain the original path.
Callbacks that mutate an outer cell still read the live environment binding; lowering does not snapshot captures.

## Register bytecode dispatch

Exact list/tuple indexing and checked unpack reads, boxed-list stores, primitive truth tests and builtin range stepping
use guarded runtime services. Custom protocols and unsupported operand kinds retain ordinary Python dispatch.
Range iterators cache immutable start/step parameters while retaining their real shared cursor.
Ordinary instance-field reads and writes skip collection/profile polls while the descriptor/hook guard holds.

Quiet iterator headers combine Next, exhaustion checking and branching without changing HIR result identities
or instruction offsets. Eager/active feedback uses the unfused path; reconstruction after Next resumes at its original offset.
Register frames batch completed-window backedge counters and merge them before new admission or on return/error.
New loop headers still reach their thresholds independently. Completion caches include VM identity, window and numeric mode;
new feedback windows and tier changes retain normal admission.

Collection polls remain at frame entry, after allocating/reentrant instructions, custom truth callbacks and deferred
method materialization. Exact scalar and builtin operations cannot add allocation pressure between those checkpoints.
`TierStats::bytecode_fused_iterations` reports quiet iterator-header executions.

## Values and heap lifetimes

`Value` and `Option<Value>` are 16 bytes, including full-width `i64` and `f64` payloads.
A 32-bit enum discriminant leaves room for a reference's three 32-bit fields:
heap identity, arena slot and slot generation. Heap identities never wrap; each reused slot
advances its generation with checked arithmetic. Each field is limited to `u32::MAX`;
exhaustion panics rather than accepting a stale or foreign reference. Generations count
reuse of one slot, not total allocations across the heap.

Values are non-owning copies. Tracing mark-and-sweep GC retains stack, environment and
explicitly pinned roots; copying a reference performs no reference counting. Host-held
values need `Vm::pin` or an explicit collection root to survive collection.

The native ABI also uses 16-byte slots: a tag word and a payload word. References pack
the heap identity into the tag's upper half and the slot/generation into the payload.
Rust enum layout is not transmuted into this ABI; helpers explicitly encode and decode it.
Both native backends use the same ABI. Float views pack their length and feedback flag
into the tag; loop-region metadata uses separate auxiliary slots.

## Names and source literals

Namespaces store values in stable numeric slots with a name-to-slot index. Replacing a binding
updates its slot; removing a binding leaves an empty slot, so cached addresses never shift.
Register bytecode and LIR name helpers cache the activation environment, target namespace and slot.
A hit reads the current value, including assignments through `global` or `nonlocal`.
Lexical parents and local-name membership are compiler-owned metadata. Paths through class
namespaces retain dynamic lookup because execution can introduce a nearer class binding.
Name caches hold non-owning identities and do not keep environments alive.

Immutable source-string constants are interned for the lifetime of one VM. Bytecode constants and
LIR helpers have weak site caches checked against the current heap; the VM's literal pool roots
the canonical objects during collection. Sharing compiled code between VMs does not share heap
objects. Dynamically constructed strings are not interned. This changes neither the 16-byte value
layout nor the tracing-GC ownership model.

## Call frames

Uncaptured locals occupy ranges in one reusable VM local-value stack. A frame stores offsets,
length and code metadata; nested calls can grow the buffer without invalidating a parent's range.
GC scans the active local-value stack, and both normal return and errors truncate the callee's range.
Native side exits restore locals into that range. Native callback panic recovery restores the saved
register/local-stack lengths and frame headers before returning an error.

Ordinary positional calls bind arguments and trailing defaults directly into local slots. Entry
parameter IDs are cached in bytecode instead of copied from HIR on every call. Profile windows
still collect argument observations. Keyword/starred calls retain the general binder.
Register execution checks out reusable argument, edge-transfer and prepared-method buffers.
Each active invocation owns separate buffers; returned buffers are cleared and are not GC roots.

Calls still recurse through Rust with `stacker` protection. Register calls still gather arguments
into their scratch buffer, bound receivers can require an argument vector, and captured variables
still use heap environments. This is shared frame storage, not an inlining or trampoline dispatcher.

## Instance shapes

Each class owns an empty shape. Assigning a new instance field follows a shared transition to
an immutable layout; instances with the same class and field-addition order share that layout.
An instance stores its values in a vector indexed by the layout. Overwriting a field keeps the
shape, while adding one changes it. Following an existing transition copies no field-name string.
Shapes use `Rc` for Rust layout metadata; copying a Python `Value` still performs no reference counting.

Register bytecode field sites and LIR attribute helpers cache a shape ID and slot index. A matching
shape reads or writes the vector directly. IDs never wrap, and caches hold neither instances nor
pointers into their vectors. Different layouts miss the guard and resolve the slot by name.
Class attributes and inherited methods are not cached, so their replacement remains visible.
The stack interpreter uses the same shaped storage through the general runtime operations.

Cranelift's existing field-probe callback also uses these guarded slots, and copy-and-patch uses
the shared LIR helpers. Generated code still calls into Rust for field access; direct machine-code
loads and stores require a later LIR/heap-view lowering. The 16-byte value/native-slot ABI is unchanged.

## Rust API

```rust
use tis::{compile, Value, Vm};

let mut program = compile("def add(a, b=3):\n    return a + b\n")?;
let mut vm = Vm::new();
vm.run_program(&mut program)?;
let add = vm.get_global("add")?;
assert_eq!(vm.call(add, &[Value::Int(2)])?, Value::Int(5));
println!("{}", vm.function_hir(add)?.dump());
# Ok::<(), tis::Error>(())
```

`Vm::output` captures `print` output.
`Program::function` exposes compile-time templates for inspection; `Vm::function_hir` inspects a live callable's feedback (`function_ir` remains an alias).
The lower-level `Vm::run` API remains available for functions constructed through the Rust IR builder.

## Inspect HIR, LIR and profiles

```sh
cargo run --example ir -- --source examples/sum.py --function sum
cargo run --example ir -- --source examples/sum.py --function sum --format register
cargo run --example ir -- --source examples/sum.py --function sum --format html --output /tmp/tis-ir.html
cargo run --example ir -- --demo add --phase fallback
cargo run --example ir -- --demo add --phase warm --format lir
cargo run --example ir -- --demo sum --format profile
cargo run --example hotspots
cargo run --example hotspots -- tests/fixtures/game_nbody.py
```

Source inspection does not execute the source.
The standalone HTML viewer supports pan, zoom, block/value selection and feedback inspection; DOT export is available.
Functions expose `dump()`, `to_dot()`, `to_html()`, `bytecode()?.disassemble_register()` and
`bytecode()?.disassemble()` (the stack encoding).

## Profiles and compilation requests

Function IDs identify code templates across closures and heap collection.
A structural HIR edit creates a new revision and resets its observations; ordinary clones share code profiles.
The interpreter records admitted function entries, bound argument types, returns/errors, block and edge counts,
natural loop backedges, instruction input/result types, guarded-add fast paths/failures, deoptimizations, and call targets.
Call-target profiles retain four distinct targets and count additional targets without retaining heap handles.

`Vm::thresholds` controls function-entry and loop-backedge thresholds; zero disables requests for that category.
`Vm::take_compilation_requests()` drains requests emitted once per function/revision/hotspot.
Each request contains a detached HIR and profile snapshot with a stable source ID, revision and triggering count.
`request.is_current(&hir)` checks whether it still matches the source, and `request.lower()` produces profiled LIR.
Profiling remains enabled when compilation requests are disabled.
These counters measure execution frequency, not elapsed time or CPU samples.

```rust
use tis::{Vm, profile::HotThresholds};
let mut vm = Vm::new();
vm.thresholds = HotThresholds { function_entries: 100, loop_backedges: 1000 };
// Execute a program, then inspect or submit the detached requests to a compiler.
for request in vm.take_compilation_requests() {
    println!("{} {:?}", request.hir().name(), request.hotspot);
    println!("{}", request.lower()?.dump());
}
# Ok::<(), tis::Error>(())
```

## LIR and interpreter handoff

`hir::Function::lower_lir(LoweringMode::Generic | LoweringMode::Profiled)` lowers HIR directly.
`ir` remains an alias for `hir` for existing Rust callers.
Bytecode is the default tier; set `vm.tiering = Tiering::Lir`, `Tiering::Native` or `Tiering::CopyPatch` to enable tiering.
LIR is lowered from HIR, never reconstructed from bytecode.
Immutable bytecode buffers are shared across calls.

Register bytecode executes HIR values directly in frame slots. Each semantic operation has one fixed-size instruction;
block edges copy parameters in parallel through reusable scratch storage. Constants, runtime services, feedback sites,
GC roots and tier handoffs are shared with the stack interpreter. Detailed feedback still follows the staged policy.
`Vm.bytecode = BytecodeMode::Stack` selects the original encoding; the run and nbody benchmark examples also accept
`--bytecode stack|register`. Both encodings are retained in each code object for controlled comparisons, so compilation
and resident code memory currently include both. `--format bytecode` inspects stack code; `--format register` inspects
register code. HIR resume points select the appropriate encoding when a compiled tier exits.

Completed staged observation windows also install guarded numeric plans for register bytecode.
Monomorphic integer and float arithmetic and comparisons read frame operands directly, avoiding generic argument
collection and operator-string dispatch. Integer overflow and float domain errors fall back to the original operation;
a type miss disables that site's plan, records the unexpected kinds and starts a bounded observation window.
Mixed integer/float comparisons and integer power remain generic to preserve numeric semantics.
Plans belong to the cached code revision, use shared scalar arithmetic, and permit recursive frames through `Cell` storage.
`Vm.numeric = NumericMode::Disabled` or `--numeric disabled` keeps the generic path for comparisons.
`TierStats.numeric_*` reports installed plans, successful executions, type misses and arithmetic fallbacks.
Eager diagnostic profiling retains generic execution and its per-site feedback counters.



LIR has boxed, integer, float and boolean registers, explicit block arguments, runtime-helper calls and truth conversion.
String construction and generic Python operations have safepoints, boxed root maps and conservative effect metadata.
Monomorphic integer addition sites lower to tag guards, integer unboxing, checked addition and boxing.
Stable float and mixed integer/float arithmetic lower to exact tag guards, conversion and typed operations.
Float comparisons specialize only when both operands are floats, preserving exact mixed integer/float comparisons.
Observed types are hints: type changes, integer overflow, division by zero and invalid power domains have explicit side exits.
HIR and LIR verification check definitions, dominance and edge arguments; LIR also checks types, guards and root maps.

A side exit stores live HIR temporaries, the current SSA bindings of Python locals and the lexical environment.
Local binding maps distinguish unbound locals from Python `None` and keep their heap references rooted.
`lir.reconstruct(state, environment, locals, read_register)` materializes a `DeoptFrame` at a pre-effect boundary.
SSA bindings replace stale slot contents during reconstruction; prologue bindings can still use initial frame slots.
`Vm::resume_hir(...)` checks its code revision and continues bytecode execution at that boundary.
Tests exercise type changes and integer overflow after an earlier mutation, verifying that the mutation is not repeated.
A hot loop can enter LIR or native execution at its header during the first invocation (OSR).
Edge arguments transfer in parallel in all three tiers.

The reference LIR backend executes the same guards and runtime-helper contract as native code.
Hotspot thresholds automatically install an artifact and subsequent calls select it by HIR ID and revision.
A failed guard retires the artifact and resumes bytecode before the original operation, without repeating earlier effects.
Automatic retry of a retired artifact is disabled; `vm.compile_lir(&hir)` explicitly rebuilds it from current observations.
`vm.install_lir(&hir, lir)` validates a manually lowered artifact before installation.
Stale HIR revisions never select an older artifact.

The native backend uses Cranelift for code generation, register allocation, linking and executable memory.
It emits primitive arithmetic, tag guards, comparisons, branches, parallel edge transfers and returns.
Python operations call shared Rust helpers with the current environment and live boxed roots.
The native register buffer has a fixed C layout, independent of Rust's `Value` enum layout.
Runtime callback panics are caught before they can unwind through generated code.
An executable module is owned until its last active native frame returns, including recursive retirement.
The Cranelift emitter is in `src/native.rs`; both native backends use `src/native_runtime.rs` and the same C-layout slots.
The Cranelift emitter follows the [Cranelift JIT API](https://docs.rs/cranelift-jit/0.137.0/cranelift_jit/).

Helper and checked sequence results publish heap references; scalar-only instructions need no callback in staged native code.
Safepoints read register images directly and publish only heap references to the collector.
Eager diagnostics retain scalar-result callbacks; switching to eager mode rebuilds an artifact that omitted them.
Native contexts cache whether detailed feedback is active, refreshing after block probes and reentrant helpers.
Frozen callbacks skip observer calls and result-profile borrows; guard failures still reopen bounded observation windows.
`TierStats.native_feedback_probes` and `native_root_syncs` count observer calls and GC root publications.
Within a block, typed arithmetic reuses existing primitive SSA registers across operations and helper calls.
Boxed shadow values remain for reconstruction; boxing is not yet deferred to helper calls and side exits.
Per-scalar native fast-path counts are available in eager mode; staged code keeps tier-entry, backedge and failure counts.

Hot list/tuple unpacking and indexing lower to `SequenceUnpack` and `SequenceGet` operations.
They check exact container kinds, current lengths and integer bounds through non-allocating Rust heap accessors.
Guard misses resume generic bytecode before the operation, preserving errors and earlier side effects.
These reads cannot call Python, mutate containers or collect; they skip full root synchronization and publish result references.
They still read mutable heap state and cannot be moved across writes or Python calls.
Unpack views are limited to verified contiguous element reads before assignment targets can mutate the source.
Staged native code fuses each verified unpack and its element reads into one callback with one kind/length check.
The batch fills the same boxed register image used by later GC safepoints and deoptimization; it cannot collect or call Python.
Eager artifacts retain individual callbacks. A reopened window in an active staged artifact observes each original read.
Native scalar truth tests for None, bool, integer and float run directly in generated code, including NaNs and both zeros.
Heap-backed or invalid boxed values retain the runtime truth path.
`TierStats.native_unpack_batches` and `native_truth_fallbacks` expose batch executions and slow truth tests.
Tuples can be reused during generic unpacking; generic list unpacking copies a snapshot without creating an iterator.
Iterators and starred targets retain generic collection behavior.
LIR helper effects distinguish Python heap allocation, Python calls and writes.
Numeric reads, truth tests, local/name access, comparisons and indexed mutation do not allocate Python heap objects.
Their complete deoptimization states are retained, while their GC maps are empty and no collection checkpoint runs there.
Allocating or calling helpers retain full GC maps and publish roots before execution and collection afterwards.
Generic iteration, calls, attributes and formatting retain conservative effects.
The verifier rejects effects that understate a helper's requirements and checks each GC map against its effects.
Homogeneous boolean lists can use one byte per element. Repetition copies primitive buffers directly,
and non-boolean writes convert the list to ordinary Values. Native loads/stores guard heap identity,
generation, storage kind and bounds. A shared boxed snapshot revokes native boolean writes through
an interior token; an exclusive mutation discards the snapshot before publishing a writable pointer again.
GC skips primitive buffers instead of materializing their boxed snapshots.

Homogeneous float lists use contiguous `f64` storage. Typed LIR guards their storage and bounds, then loads
and stores elements directly. Views remain within one block and cannot survive calls, collection or shape mutation;
the verifier enforces these boundaries. Generic mutations can widen a list back to mixed values.
Float loop parameters have unboxed companions with boxed shadows for GC and deoptimization. Incoming edges and OSR
entries check the observed types. Cranelift keeps primitive values in SSA across backedges; copy-and-patch still
uses its register image. Typed indexed operations currently require constant integer indices.
LIR dumps include float boxing recipes. In staged Cranelift code, eligible `BoxFloat` results stay in native
SSA values. Helpers materialize their operands; edge transfers and returns materialize their outgoing values.
Guard failures reconstruct the live frame image on the failure branch. Native GC maps omit these proven floats;
the original frame states remain available for deoptimization. Imported OSR values retain tagged storage until checked,
and eager feedback artifacts retain immediate boxing. Reopened feedback windows materialize values for observation.
Float-list views remain block-local; deferred boxing does not extend their lifetime across callbacks or backedges.
Profiled, fixed-length unpacking of homogeneous float lists uses one exact-length storage guard and direct element
loads. Lowering requires a contiguous eager unpack bundle and observed float results at every element site.
All elements are read before assignment targets can mutate the source. Mixed lists, tuples, iterators and starred
unpacking retain the existing paths; failed guards resume the original unpack without repeating source evaluation.
Float-view acquisition has a narrow runtime entry point with panic containment and no frame checkpoint, since it
cannot execute Python, collect or change frames. Views still end at invalidating helpers and block edges.
LIR retains original HIR observation operands so reopened feedback branches keep only their own inputs and result live.

Staged Cranelift artifacts can execute complete two-block sequence loops as native regions. Verified metadata names
the header, body, imported values and entry snapshot. An explicit instruction allowlist excludes Python calls,
allocating operations and shape changes. Builtin list, tuple, float-list and range iterators advance directly;
iterator indices and exhaustion are written to their stable heap payloads. Range arithmetic uses checked wide integers.
A float power remains a pure math call. Loop backedges are counted in native code and published in bulk at boundaries.
Eager feedback and unsupported loop shapes retain the observing backend path.

Entry prepares a C-layout descriptor table and tagged snapshots of immutable/read-only sequence storage. Float data
stays in its original writable buffer. Generic allocating, calling or mutating helpers invalidate the cache; harmless
builtin iteration preserves it. Float views are renewed after other reads, and each collection advances a heap epoch
that invalidates all cached descriptors and guard schemas. These caches are not GC roots. Preparation currently falls
back when the arena has more than 65,536 slots.

When a loop's imported object identities are invariant, an entry schema validates its item shapes, exact unpack lengths,
index bounds and scalar types before executing any iteration. Successful validation is cached while those guarantees
hold. Numerical failure checks remain in the loop. Object phis that can change identity keep per-iteration guards.
Failed entry validation resumes before the original next operation; later numerical exits preserve completed stores
and consumed iterator items. The verifier reconstructs the schema and rejects modified metadata or reversed done edges.
Cranelift runs with its speed optimization pipeline enabled.

`TierStats.native_region_entries`, `native_region_iterations`, `native_region_failures` and `native_runtime_calls`
separate region work from runtime crossings. LIR dumps expose region imports and hoisted-guard counts. Copy-and-patch
and reference LIR keep executing the original operations; region metadata currently accelerates Cranelift only.

Staged native artifacts with regions keep boxed registers as four SSA words as well as primitive float recipes.
Phi edges perform parallel SSA transfers, preserving references, `None` and unbound locals. Only live values needed
by a runtime operation or side exit are materialized into the register image. Block bookkeeping does not publish
lazy parameters, and region entry publishes imports; an entry failure materializes its complete snapshot.
Allocating helpers receive fresh tagged roots, and callback-produced results reload the SSA cache after success.

Set `Vm.capture_native_code` before compilation to retain `NativeDiagnostics` for a callable. The benchmark worker
accepts `--dump-native PREFIX`, writing input Cranelift IR, allocated machine disassembly and the finalized live
machine image as `.clif`, `.asm` and `.bin`. Capture is off by default; timed runs do not retain diagnostic strings.


`TierStats.native_float_views` and `native_float_view_failures` count view acquisitions and storage guard failures.
There is no inlining or escape analysis yet.
Guard failures permit bounded reprofiling and recompilation, using accumulated observations.
Native execution is currently tested on macOS ARM64; the bytecode and reference LIR paths remain available.
`vm.tier_stats` exposes reference/native entries, OSR entries, compilation errors, deoptimizations and cumulative emitted bytes.

## Copy-and-patch experiment

```sh
cargo run --release -- --tier copy-patch --stats tests/fixtures/game_nbody.py
cargo run --release --example backend_compare
```

`build.rs` invokes rustc once per target build to compile `src/copy_patch/stencils.rs` at optimization level 3.
It extracts 25 self-contained function bodies from the object file into the executable's stencil library.
The build rejects external relocations and indirect non-tail continuations.
The shared scalar module supplies float arithmetic and comparison semantics; Python helpers, GC and side exits
use the same runtime bridge as Cranelift.

At runtime the linker copies the selected machine-code bodies into an anonymous writable image.
It patches adjacent records containing operand offsets, constants, helper addresses, and code/data continuations.
The image becomes read/execute, instruction caches are flushed, and entries are published only afterward.
Linking calls neither rustc nor Cranelift.
The copied C-ABI stencils tail-call their continuations; branch targets and loop entries select another stencil.
Parallel block arguments use scratch slots before any destination is overwritten.
An active execution owns its image, so retirement during a recursive call cannot unmap active code.

This first version patches data records rather than embedding every operand as an instruction immediate.
It uses memory-backed registers and indirect continuations, without cross-stencil register allocation or fusion.
Supported stencil targets are AArch64 and x86-64; execution is currently tested on macOS ARM64.
The long-loop test uses a small thread stack to check that continuations do not accumulate native frames.

The comparison example warms the same floating-point kernel, discards three compilation samples, then reports
20-sample median backend compilation time, image sizes, and a separate 10,000-call execution measurement.
Backend timing excludes common HIR-to-LIR lowering and the first execution.
A release run on macOS ARM64 with Rust 1.99.0 produced:

| Backend | Median compilation | Code bytes | Explicit patch records |
| --- | ---: | ---: | ---: |
| Current Cranelift backend | 446.6 µs | 948 | — |
| Copy-and-patch | 10.2 µs | 2,092 | 3,720 bytes |

These are small-kernel measurements with the current backend settings, not `game_nbody` timings or total resident sizes.
The copied image trades more code/data for cheaper compilation; the tool reports execution time independently.
`TierStats` separates copy-and-patch entries, OSR, compilation time, copied bytes, stencil count and continuation relocations.
Cranelift compilation counters remain zero in copy-and-patch-only runs.

## Heap and frames

Monty's stable paged arena and free list are copied unchanged into `src/heap/`, with their license.
Native execution also uses unsafe Rust at the documented ABI and executable-memory boundary.
The tis object layer uses generation-checked handles and tracing collection, including cycles and slot reuse.
Monty's reference counting and HeapReader API are not part of this port.

Collection runs between bytecode instructions, at fully rooted LIR/native entry, and at allocating or calling helpers.
Temporary slots, local slots, active environments, in-flight calls and module globals are rooted.
LIR/native safepoints include SSA local bindings in their root maps and retain the initial local frame.
Host-held handles can be retained with `Vm::pin` and released with `Vm::unpin`.
`Vm::collect_garbage(&roots)` also accepts temporary external roots.
Direct `Heap` use has explicit collection only.

Calls currently use Rust recursion with stack growth and a 256-frame language limit.
Both return and error release the VM frame window, local slots and active environment roots.
Iterator and representation recursion have bounded depth and stack growth; class hierarchy lookup is iterative.

## Current limits

Integers are `i64`, with overflow errors; arithmetic with floats uses `f64`.
This is a research implementation with trusted IR and no sandbox guarantees.
Exceptions propagate to Rust; Python `try`/`except`, imports other than builtin `math`, generators/yield, async, decorators,
multiple inheritance, descriptors and user-defined operator protocols are not implemented.
Dictionary keys currently support None, bool, integer, non-NaN float, string and recursively hashable tuples.
Containers and builtins implement a subset of Python's APIs.
Unsupported constructs return errors.

## Checks

```sh
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
node tests/viewer.mjs /tmp/tis-ir.html
```

The full n-body acceptance test runs in bytecode, reference LIR, Cranelift and copy-and-patch modes with `cargo test`.
The Node check exercises viewer logic with a minimal DOM model; it does not verify browser layout.

## CPU sampling on macOS

Build an optimized worker with symbols, then sample it after warmup:

```sh
CARGO_PROFILE_RELEASE_DEBUG=2 cargo build --release --example nbody_bench
python3 tools/profile_nbody.py --worker target/release/examples/nbody_bench --output /tmp/tis-cpu
python3 tools/summarize_sample.py /tmp/tis-cpu/bytecode-1.sample.txt \
  /tmp/tis-cpu/bytecode-2.sample.txt --output /tmp/tis-bytecode-summary.json
```

The runner alternates bytecode and native workers for two ten-second passes, validates energy after each advance,
and retains raw `sample` reports and worker responses. No compilation runs inside the recorded workload.
Sampling includes small scene resets and energy checks between advances; profiled timings are diagnostic, not benchmarks.
The summarizer reports exclusive counts and inclusive counts with recursive frames deduplicated.
It accepts gzipped reports and an optional `--demangler /path/to/rustfilt` for Rust symbol names.
Generated native code appears as anonymous frames; Rust callback stacks remain visible.

## PyPy performance target

Compare the installed runtimes using the same nbody input, with fresh-process and warmed measurements kept separate:

```sh
cargo build --release --examples
python3 tools/compare_runtimes.py --run target/release/examples/run \
  --worker target/release/examples/nbody_bench --python /path/to/python3 --pypy /path/to/pypy3 \
  --output /tmp/tis-runtimes
```

The default is ten fresh processes and ten steady samples per runtime, with ten kernel warmups before steady samples.
The runner interleaves runtimes serially and validates stdout or energy after every execution.
Readiness retains the first kernel call and the complete warmup sequence.
PyPy backend compile time and deoptimization counts remain unknown in timing results.
Only the known old-PyPy CPU-cache detection warning is accepted on stderr, and it is retained in the results.

Capture optimized PyPy traces and compile events separately from timing, then inspect them alongside warmed tis LIR:

```sh
python3 tools/trace_pypy.py --pypy /path/to/pypy3 --output /tmp/pypy-traces
target/release/examples/nbody_bench --tier native --warmups 10 --dump-lir /tmp/advance.lir < /dev/null
python3 tools/inspect_pypy.py --log /tmp/pypy-traces/pypy.log --tis-lir /tmp/advance.lir --output /tmp/loop-comparison
```

The trace inspector separates loop backedge bodies from entry preambles and exit bridges.
Its operation counts are static IR descriptions, not instruction timing or dynamic execution counts.
The next loop work targets typed sequence access and primitive values across backedges; this comparison should track progress.

## Native loop nests

The Cranelift tier recognizes closed outer loops containing supported sequence loops.
It prepares invariant source graphs once, reuses private builtin iterators, and keeps their
positions, exhaustion flags and backedge counters in SSA. Normal exits and side exits publish
those fields before interpreter execution resumes. Boxed register values also stay in SSA;
frame slots are written only for runtime inputs, collection roots and exit reconstruction.

Nest recognition requires invariant inner sources, nonescaping iterators and allocation-free
bodies. Calls, container shape changes and iterator sources with shared identity use the ordinary
path. Inner-loop OSR first uses a simple region; a later function entry can compile the complete nest.
`Vm::disable_native_nests` and the benchmark worker's `--no-nests` flag retain the simple-region path
for diagnostics. Eager profiling continues to observe instructions individually.

Capture compiler IR, allocated assembly and the live relocated machine image outside timed runs:

```sh
target/release/examples/nbody_bench --tier native --warmups 10 --dump-native /tmp/advance < /dev/null
```

This writes `/tmp/advance.clif`, `.asm` and `.bin`. Set `Vm::capture_native_code` before compilation
and use `Vm::native_diagnostics` when embedding tis. Capture is disabled by default.

## Object-heavy benchmark targets

Barnes–Hut and ticket search complement the arithmetic-focused nbody target:

- Barnes–Hut: recursive quadtrees, classes, methods, attributes and allocation; 200 particles, 100 steps.
- Ticket search: strings, dictionaries, TF-IDF weights, comprehensions and keyed sorting; 1,600 tickets.

Both run through HIR, bytecode and all native tiers. A builtin-only `math` import provides `pi`,
`sqrt`, `sin`, `cos` and `log`; other imports remain unsupported. `list.sort` supports stable
`key` and `reverse` sorting, evaluates keys once, and roots them across callback collections.
Fixture provenance and the two source adaptations are documented in `tests/fixtures/README.md`.

```sh
cargo build --release --examples
python3 tools/bench_suite.py --worker target/release/examples/bench \
  --python /path/to/python3 --pypy /path/to/pypy3 --output /tmp/tis-object-benchmarks
```

The default runner warms each process once, then collects three serial, interleaved samples for
bytecode, Cranelift, CPython and optional PyPy. Each call starts a fresh scene or index; input
construction and Barnes–Hut energy calculations are included, matching the workload function.
Compilation and module definitions precede timing. Complete output is validated against CPython
on every sample; backend compilation and deoptimization are retained rather than assumed absent.
Use `--tiers lir copy-patch` to measure the other execution tiers. The existing nbody harness remains
separate so its reset-and-time-advance protocol does not change.

## Sieve of Eratosthenes

`sieve` marks primes through 1,000,000 using a boolean list and scalar indexed stores.
Each invocation creates a fresh list and prints the prime count, sum and largest prime:
`78498 37550402023 999983`.
It exercises integer loops, indexing and mutable list elements without library calls.

```sh
tis tests/fixtures/sieve.py
tis --tier native tests/fixtures/sieve.py
python3 tools/bench_suite.py --worker target/release/examples/bench \
  --benchmarks sieve --samples 3 --warmups 1 --output /tmp/tis-sieve-warmed
python3 tools/bench_oneshot.py --run target/release/examples/run \
  --benchmarks sieve --samples 3 --output /tmp/tis-sieve-oneshot
```

Both benchmark runners include sieve in their default selections.

Cranelift probes exact boxed lists, tuples and builtin range iterators directly, including blocks
without operand feedback. Guards check current heap identity, generation, representation, index and bounds.
List stores use pointers published by exclusive heap borrows and fall back in artifacts with cached heap views.
The iterator cursor is a `Cell<usize>` in the real heap object, so aliases observe each completed step immediately.
Custom protocols, exhaustion, promotions, index conversions and active feedback retain the general runtime path.
Quiet native loops accumulate backedge counts in invocation-owned scratch and merge them on return, error or deoptimization.
Observing loops keep immediate profiling callbacks. CFG-expanded artifacts retain their existing bookkeeping.

## One-shot performance

Measure each complete fixture once in a fresh process, with no preparatory calls:

```sh
python3 tools/bench_oneshot.py --run target/release/examples/run \
  --python /path/to/python3 --pypy /path/to/pypy3 --output /tmp/tis-oneshot
```

The default is three serial, interleaved fresh processes per runtime for nbody (20,000 steps),
Barnes–Hut (200 particles / 100 steps), ticket search (1,600 tickets) and sieve (primes through 1,000,000). Timing includes process
startup, source parsing, compilation, JIT work during that first execution, output and process exit.
All stdout is validated. This is separate from the warmed worker protocol; OS file caches are not flushed.
Use `--benchmarks` and `--tiers` to select workloads and tis execution tiers. Results retain commands,
binary/source hashes, runtime versions, exact output, stderr and every timing sample.

## Call and object fast paths

Functions whose locals are all indexed frame slots can bind simple positional arguments directly
into the frame and use their lexical parent without allocating an environment. Captured locals,
nonlocal/global declarations, keyword binding and explicit namespace access retain the environment path.
The eligibility check comes from HIR and is cached with immutable bytecode metadata.

Cranelift reads field sites with observed instance receivers directly after checking heap identity,
slot generation,
shape identity and field bounds. A weak native directory initializes on demand, invalidates rows
before mutable object access and refreshes them after mutation. Native pointers are consumed before
the next callback; collection clears dead rows. Cache misses use the nonallocating field gateway,
then the general attribute helper when needed. Builtin-only sites retain the compact callback path;
new receiver types remain supported. Active feedback windows retain runtime observation. Attribute writes remain noncollecting under the current
object model, which has no descriptors or Python setters. Other allocating/calling operations retain
fresh root maps. Inactive native register buffers are cleared and reused; active nested frames always
own distinct buffers. Retained buffer capacity follows the invocation high-water mark.

Stable Cranelift artifacts publish live roots at allocating/calling safepoints. Quiet ordinary blocks
update predecessor bookkeeping directly; loop headers, region exits and observation windows retain
callbacks, preserving backedge counts and reopened feedback. Values and native slots remain 16 bytes.
Field loads use explicit Value discriminants and the C layout of ObjectId without retaining Python
objects through native caches. Measurements and validation are in
[the native-fields report](benchmarks/results/2026-10-28-native-fields/README.md).

For before/after measurement of object workloads, use the existing comparison runner with
`--benchmark barnes-hut` or `--benchmark ticket-search` and generic `examples/bench` binaries.
The one-shot runner accepts `--baseline-run /path/to/old-run` to interleave an older executable
with the current one. The default nbody comparison and no-baseline one-shot protocols are unchanged.

## Containers and call-frame setup

Dictionaries use insertion-ordered entries plus a randomized hash index. Numeric-equivalent bool,
integer and float keys hash consistently, including signed zero and the i64 boundaries; tuple keys
apply the same rules recursively. String reads borrow their existing storage. Deletion preserves
order by shifting entries and rebuilding the index, so it remains linear in dictionary size.
The public `Object::Dict` payload is `Dict`; use `Dict::default()` or convert a vector with `.into()`.

List and dictionary methods mutate their receiver directly. Explicit copies and input snapshots for
self-extension/update remain separate operations. Float-list append, insert, remove, clear and reverse
preserve compact storage where operand types permit it. Builtin math callables are cached and rooted
with the module cache, preserving identity across repeated lookup and collection.

Call entry initializes empty register windows directly on the VM stack. Parameter-to-local-slot
indices are computed once with bytecode metadata, avoiding repeated name comparisons during binding.
These runtime paths serve bytecode, LIR, Cranelift and copy-and-patch execution.

LIR, Cranelift and copy-and-patch helpers share register bytecode's positional dispatcher when every
argument is positional. Root publication and call effects stay explicit at the helper boundary;
keyword/starred calls retain the general binder. Borrowed builtin names avoid cloning callable
objects, and ordinary positional calls reuse the already gathered argument slice. The legacy
`bytecode_positional_calls` and `bytecode_leaf_calls` counters also count this shared helper path.
See [positional helper measurements](benchmarks/results/2026-10-29-positional-helpers/README.md).

Stable native entries seed boxed values for invariant uncaptured local slots when at least two
load sites can share the setup. Explicit slot writes disable this optimization; OSR, unbound values
and active feedback retain the original helpers and their error order. Values stay rooted through
the original frame local range. See [invariant local measurements](benchmarks/results/2026-10-29-invariant-locals/README.md).

Native and copy-and-patch entries pool register images, rooted-index vectors and helper-argument
vectors together. Every active recursive invocation owns separate buffers, which are cleared on
return or error before reuse. See [native scratch measurements](benchmarks/results/2026-10-29-native-scratch/README.md).

Profiled LIR specializes numeric-only Int/Float union addition with explicit type and overflow
exits. Int+Int stays checked integer arithmetic; any Float operand produces a Float. Unsupported
operands resume the original operation before effects. See [numeric union measurements](benchmarks/results/2026-10-29-numeric-add/README.md).

Typed literal comparisons guard exact Int/Float values and compare integers without rounding.
The floating path is available only for integer literals exactly representable in f64; unsupported
values resume the original comparison. See [literal comparison measurements](benchmarks/results/2026-10-29-number-compare/README.md).

Unary `not` lowers to explicit truth conversion, typed boolean inversion and boxing. Native
primitive truth avoids the general helper; heap truth and eager observation keep their existing
paths. See [typed unary-not measurements](benchmarks/results/2026-10-29-bool-not/README.md).

Profiled positional `len` and `math.sqrt` calls can use an audited scalar gateway with read-only
heap access. Actual callable names are guarded; eager/mismatching calls keep the original full
rooted fallback. See [scalar call measurements](benchmarks/results/2026-10-29-leaf-calls/README.md).

Native entry copies its initial register image and records existing shadow roots in one frame
scan. See [entry scan measurements](benchmarks/results/2026-10-29-entry-scan/README.md).

Quiet native tuple literals guard the allocation budget before using an audited allocation gateway.
Pressure falls back to the full rooted helper before allocation. See [tuple budget measurements](benchmarks/results/2026-10-29-tuple-budget/README.md).

Quiet native name loads use the audited existing cache/scope lookup through a closed gateway.
See [name-load measurements and backend tradeoff](benchmarks/results/2026-10-29-name-load/README.md).

Bounded readonly Python callee bodies can be incorporated into native callers with identity/revision,
binding and primitive operand guards. See [body inlining, safety and measured PyPy gap](benchmarks/results/2026-10-29-body-inline/README.md).

## Matched Monty comparisons

`tools/compare_monty.py` uses Flying Circus's Monty/CPython workers and the `request_bench` tis adapter.
Run it with a Python environment that has Flying Circus installed:

```sh
cargo build --release --examples
/path/to/flying-circus/python tools/compare_monty.py --monty /path/to/monty \
  --monty-revision MONTY_COMMIT --tis-revision TIS_COMMIT --tis-run target/release/examples/run \
  --tis-worker target/release/examples/request_bench --python /path/to/python3 --output /tmp/tis-monty
```

Ten samples per runtime/workload/protocol are collected serially. One-shot includes process startup
and exit. Repeated measurements reuse processes but create fresh application sessions, timing the
whole script including parsing/compilation and output delivery. They do not reuse a warmed function
or JIT artifact from the previous request. Session reset, fixture file loading and worker startup are
outside repeated timing. The tis adapter preloads source; Monty's worker transports it per request.
All sources are identical and outputs are validated. Monty CLI summaries, when present, are checked
using Flying Circus's legacy-output normalization and retained in raw results.

## Logical inline frames

LIR side exits can carry `inline_frames::Snapshot`: an ordered stack of logical
scopes, each with its own HIR identity/revision, environment, live-value map and
local recipes. A suspended caller records a pending return destination and resumes
after its original call. This lets deoptimization continue inside a callee after
completed effects without replaying the call.

The reference LIR, Cranelift and copy-and-patch bridges reconstruct these stacks.
The existing physical caller is reused; child frames resume in bytecode, return
into their callers and unwind normally on errors. All snapshot values are rooted
through reconstruction and execution. LIR dumps show the scope chain and pending
returns. Verification checks ancestry, revisions, live values, register types,
continuations and coverage by the existing physical root map.

Profiled positional calls to closed, single-block leaf functions can be copied
into the caller's LIR. These bodies may read/write attributes, allocate containers
and perform arithmetic. Parameters become explicit caller-register bindings.
Compilation refreshes callee feedback after call-site sampling ends. Each scope
retains its source HIR identity/revision and value origins; additional boxed
registers have separate shadow root slots. Safepoints retain caller and
callee snapshots. Numeric addition and floating arithmetic use profiled guards;
a miss after a write resumes the callee at that arithmetic operation.

A callable guard checks the captured function, revision, actual bound receiver,
argument count, defaults and recursion limit before effects. Methods retain the
callable captured before argument evaluation. The default leaf pass excludes Python calls and lexical reads, so its helpers share the caller's execution context.
The original call handles guard misses. Live callee profiles count logical calls,
returns, errors and guard misses; eager feedback keeps physical callee execution.
LIR dumps include scope ranges and register-to-HIR origin mappings.
Artifacts without copied bodies bypass shadow-frame setup and select a runtime
callback that omits inline root handling.

The default leaf pass accepts one to four positional parameters including the receiver,
96 HIR instructions per callee and 192 copied instructions per caller artifact.
It excludes defaults, keyword/star arguments, lexical access, nested Python calls,
branches and loops. The readonly scalar expression inliner remains preferred for
its supported bodies.

Measured results and current limits: [effectful leaf inlining](benchmarks/results/2026-10-29-leaf-inline/README.md).

Native backends fuse the leaf guard and environment/receiver reads into one callback.
Reopened profiling windows retain ordinary helper observation.
[Entry-fusion measurements](benchmarks/results/2026-10-29-inline-entry-batch/README.md) show 60,000 fewer callbacks per
Barnes–Hut invocation, with effectively unchanged warmed execution time.

Experimental CFG inlining is available through `Vm::cfg_inlining = true` before feedback collection,
or `--cfg-inlining` in the `bench` and `run` examples.
It supports acyclic multi-block bodies, phi arguments, multiple returns, lexical reads and nested or recursive calls.
Each copied call charges a logical recursion level and uses the actual callee environment.
Branches and safepoints retain caller/callee reconstruction state; synthetic blocks do not create physical HIR observations.
The pass accepts up to 48 blocks and 384 HIR instructions per callee, with 768 copied instructions per artifact.
Defaults, captured local writes, keyword/star arguments at the inline boundary, and loops retain ordinary calls.
Loop-region/nest discovery is currently skipped in artifacts whose CFG was expanded.
`lir::Function::lower_with_cfg` exposes this lowering for inspection.

This option defaults to false because the current runtime-helper and root-map costs exceed its frame savings.
See the [CFG-inlining experiment](benchmarks/results/2026-10-29-cfg-inline/README.md) for validation and measurements.

Benchmark tools default to three samples and one warmup for quick iteration, and accept `--samples 1`.
Increase `--samples` and `--warmups` explicitly when confirming a gain.

Copied bodies use ordinary profiled HIR → LIR lowering, followed by register/state/exit remapping.
This retains comparison, sequence and scalar specialization, including typed float phi arguments.
After entry, root publication uses the suspended caller continuation and logical callee state;
entry-guard misses keep the original pre-call reconstruction point.
[Shared-lowering results](benchmarks/results/2026-10-29-shared-inline-lowering/README.md) show fewer callbacks and root
synchronizations, but CFG inlining still defaults to false because it remains slower than default execution.

Caller OSR imports exclude float caches owned by copied inline scopes; those caches retain their local guards.
[Scalar-import fix and quick results](benchmarks/results/2026-10-29-inline-scalar-imports/README.md) cover the
`advance_system` validation failure and restored inlining coverage.
Benchmark JSON includes cumulative compilation-error counts alongside validated output.

### Closed recursive native regions

Set `Vm::readonly_regions = true` before compilation, or pass `--readonly-regions` to the `run` or `bench` example.
This opt-in Cranelift experiment lowers generic LIR into a separate recursive native function.
It supports bounded acyclic method bodies, numeric arithmetic, instance fields, list/tuple reads, `len`, `math.sqrt`,
and direct self-recursion returning one to four numeric tuple components.
Internal calls use native stack frames and pass tuple components without allocating Python objects.

The region cannot call Python, mutate the heap, allocate, or collect.
Guards check heap identity, generation, bounds, field shapes, recursive method identity and current global bindings.
A failed guard restarts the whole method through the ordinary VM; only read-only work can be repeated.
Only the final result tuple is materialized.
Native recursion is capped at 32 frames per attempt and also respects the remaining VM recursion allowance.
Unsupported bodies, eager feedback, OSR entries and guard failures use the existing backend.
The generated region is available as `NativeDiagnostics::readonly_clif` when code capture is enabled.
Measurements and implementation details are in the
[recursive-region report](benchmarks/results/2026-10-29-readonly-regions/README.md).

### Buffered native mutation regions

Set `Vm::mutation_regions = true` before compilation, or pass `--mutation-regions` to the `run` or `bench` example.
Combine it with `--readonly-regions` to enable both experiments.
Acyclic read/modify/write methods returning `None` can execute numeric field updates without VM callbacks.
`Particle.update`, `apply_force`, and `update_mass_distribution` fit this subset.
Allocation, arbitrary calls, recursive mutation, non-numeric stores and unpromoted locals use the existing backend.
Write-only initialization is excluded because it generally needs new field layouts.

Native code records ordered writes in a private buffer and forwards later reads through that buffer, including aliases.
Successful execution commits the buffer through ordinary mutable heap guards.
A failed guard commits only the completed prefix, materializes its live HIR values and locals, and resumes bytecode at
that operation; it does not repeat the committed writes.
These resumable exits retain the generic region and record failure reasons and operand types.
A cold guard before any write can safely use the ordinary entry path while its weak field cache learns.

The backing heap stays immutable during native execution.
Heap reads carry Cranelift's `readonly` flag, and register spills and cache-miss details are emitted only on cold exit paths.
Scratch storage contains weak bits and is reused only after the non-reentrant native invocation finishes.
`NativeDiagnostics::mutation_clif` exposes the generated code when capture is enabled.
Measurements, profiling and validation are in the
[buffered-mutation report](benchmarks/results/2026-10-29-buffered-mutation/README.md).
