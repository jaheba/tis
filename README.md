# tis

A Rust interpreter for a Python subset, built around an explicit value IR.
Ruff AST → HIR → stack bytecode → interpreter.
Runtime profiles feed HIR → typed LIR → reference execution or Cranelift native code.
There is no stdlib or sandbox infrastructure.

```sh
cargo run --example run -- tests/fixtures/game_nbody.py
# -0.169075164
# -0.169089263
cargo run --example run -- tests/fixtures/language.py
# language core ok

# Hot loops switch into native code; --stats reports tier entries, exits and emitted bytes.
cargo run --example run -- --tier native --stats tests/fixtures/game_nbody.py
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
Eight successful integer observations select a guarded integer addition strategy.
Type or overflow guard failure falls back to generic arithmetic without rewriting bytecode.
Name loads, stores, calls and object operations remain explicit HIR operations.

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
cargo run --example ir -- --source examples/sum.py --function sum --format bytecode
cargo run --example ir -- --source examples/sum.py --function sum --format html --output /tmp/tis-ir.html
cargo run --example ir -- --demo add --phase fallback
cargo run --example ir -- --demo add --phase warm --format lir
cargo run --example ir -- --demo sum --format profile
cargo run --example hotspots
cargo run --example hotspots -- tests/fixtures/game_nbody.py
```

Source inspection does not execute the source.
The standalone HTML viewer supports pan, zoom, block/value selection and feedback inspection; DOT export is available.
Functions expose `dump()`, `to_dot()`, `to_html()` and `bytecode()?.disassemble()`.

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
Bytecode is the default tier; set `vm.tiering = Tiering::Lir` or `Tiering::Native` to enable tiering.
LIR is lowered from HIR, never reconstructed from bytecode.
Immutable bytecode buffers are shared across calls.

LIR has boxed, integer, float and boolean registers, explicit block arguments, runtime-helper calls and truth conversion.
String construction and generic Python operations have safepoints, boxed root maps and conservative effect metadata.
Monomorphic integer addition sites lower to tag guards, integer unboxing, checked addition and boxing.
Stable float and mixed integer/float arithmetic lower to exact tag guards, conversion and typed operations.
Float comparisons specialize only when both operands are floats, preserving exact mixed integer/float comparisons.
Observed types are hints: type changes, integer overflow, division by zero and invalid power domains have explicit side exits.
HIR and LIR verification check definitions, dominance and edge arguments; LIR also checks types, guards and root maps.

A side exit stores the live HIR slots and current lexical environment needed to resume before the original operation.
`lir.reconstruct(...)` materializes a `DeoptFrame` from boxed register locations.
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
The implementation is in `src/native.rs`; it follows the [Cranelift JIT API](https://docs.rs/cranelift-jit/0.137.0/cranelift_jit/).

This is a correctness baseline: boxed results still invoke profiling callbacks, and registers are materialized in memory.
LIR helper effects remain conservative; there is no inlining, escape analysis or automatic reoptimization yet.
Native execution is currently tested on macOS ARM64; the bytecode and reference LIR paths remain available.
`vm.tier_stats` exposes reference/native entries, OSR entries, compilation errors, deoptimizations and cumulative emitted bytes.

## Heap and frames

Monty's stable paged arena and free list are copied unchanged into `src/heap/`, with their license.
Native execution also uses unsafe Rust at the documented ABI and executable-memory boundary.
The tis object layer uses generation-checked handles and tracing collection, including cycles and slot reuse.
Monty's reference counting and HeapReader API are not part of this port.

Collection runs between bytecode instructions and at LIR/native runtime boundaries.
Stack slots, active environments, in-flight calls and module globals are rooted; LIR/native safepoints use their live root maps.
Host-held handles can be retained with `Vm::pin` and released with `Vm::unpin`.
`Vm::collect_garbage(&roots)` also accepts temporary external roots.
Direct `Heap` use has explicit collection only.

Calls currently use Rust recursion with stack growth and a 256-frame language limit.
Both return and error release the VM frame window and active environment roots.
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

The full n-body acceptance test runs in bytecode, reference LIR and native modes with `cargo test`.
The Node check exercises viewer logic with a minimal DOM model; it does not verify browser layout.
