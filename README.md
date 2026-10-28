# tis

A Rust interpreter for a Python subset, built around an explicit value IR.
Ruff AST → HIR → register bytecode → interpreter.
Runtime profiles feed HIR → typed LIR → reference execution, Cranelift, or copy-and-patch native code.
There is no stdlib or sandbox infrastructure.

```sh
cargo run --example run -- tests/fixtures/game_nbody.py
# -0.169075164
# -0.169089263
cargo run --example run -- tests/fixtures/language.py
# language core ok

# Hot loops switch into native code; --stats reports tier entries, exits and emitted bytes.
cargo run --example run -- --tier native --stats tests/fixtures/game_nbody.py
cargo run --example run -- --tier copy-patch --stats tests/fixtures/game_nbody.py
```

`game_nbody.py` is the unchanged Flying Circus workload, including its full 20,000 iterations.
The acceptance test checks its output against CPython and checks that collection bounds heap growth.

## Language core

- Lists, tuples, insertion-ordered dictionaries, sets, indexing, slicing and nested/starred unpacking.
- Numeric arithmetic, comparisons, short-circuit boolean expressions and conditional expressions.
- Functions, recursion, lambdas, definition-time defaults, positional-only and keyword-only arguments,
  `*args`, `**kwargs`, and starred call arguments.
- Shared lexical environments for closures, including `global` and `nonlocal` assignment.
- `for`, `while`, loop `else`, `break` and `continue`.
- List, set and dictionary comprehensions with nested generators and filters, in their own lexical scope.
- Basic classes, constructors, instance/class attributes, bound methods and single inheritance.
- Core builtins, container methods, lazy `range`, `enumerate` and `zip`, and basic f-string formatting.

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
`TierStats.native_float_views` and `native_float_view_failures` count view acquisitions and storage guard failures.
There is no inlining or escape analysis yet.
Guard failures permit bounded reprofiling and recompilation, using accumulated observations.
Native execution is currently tested on macOS ARM64; the bytecode and reference LIR paths remain available.
`vm.tier_stats` exposes reference/native entries, OSR entries, compilation errors, deoptimizations and cumulative emitted bytes.

## Copy-and-patch experiment

```sh
cargo run --example run -- --tier copy-patch --stats tests/fixtures/game_nbody.py
cargo run --release --example backend_compare
```

`build.rs` invokes rustc once per target build to compile `src/copy_patch/stencils.rs` at optimization level 3.
It extracts 24 self-contained function bodies from the object file into the executable's stencil library.
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
Exceptions propagate to Rust; Python `try`/`except`, imports, generators/yield, async, decorators,
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
