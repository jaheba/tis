# tis

A Rust interpreter for a Python subset, built around an explicit value IR.
Ruff AST → HIR → stack bytecode → interpreter.
Runtime profiles feed a separate HIR → typed LIR path for a future native backend.
There is no JIT, stdlib or sandbox infrastructure.

```sh
cargo run --example run -- tests/fixtures/game_nbody.py
# -0.169075164
# -0.169089263
cargo run --example run -- tests/fixtures/language.py
# language core ok
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
The bytecode interpreter remains the execution path; LIR is not read back from bytecode.
Immutable bytecode buffers are shared across calls.

LIR has boxed, integer and boolean registers, explicit block arguments, runtime-helper calls and truth conversion.
String construction and generic Python operations have safepoints, boxed root maps and conservative effect metadata.
Monomorphic integer addition sites lower to tag guards, integer unboxing, checked addition and boxing.
Observed types are hints: both type failure and overflow have explicit side exits.
HIR and LIR verification check definitions, dominance and edge arguments; LIR also checks types, guards and root maps.

A side exit stores the live HIR slots and current lexical environment needed to resume before the original operation.
`lir.reconstruct(...)` materializes a `DeoptFrame` from boxed register locations.
`Vm::resume_hir(...)` checks its code revision and continues bytecode execution at that boundary.
Tests exercise type changes and integer overflow after an earlier mutation, verifying that the mutation is not repeated.
Natural loop headers carry OSR entry state, but native OSR transitions are not implemented.

There is no machine-code emitter, register allocator, executable-memory manager or native calling convention yet.
The native backend will need to implement the helper-call contract, register root locations and entry/exit stubs.
LIR helper effects are deliberately conservative; this stage performs no inlining or escape analysis.

## Heap and frames

Monty's stable paged arena and free list are copied unchanged into `src/heap/`, with their license.
Only those two modules may contain unsafe Rust.
The tis object layer uses generation-checked handles and tracing collection, including cycles and slot reuse.
Monty's reference counting and HeapReader API are not part of this port.

Collection runs between bytecode instructions, with stack slots, active environments, in-flight calls and module globals rooted.
Host-held handles can be retained with `Vm::pin` and released with `Vm::unpin`.
`Vm::collect_garbage(&roots)` also accepts temporary external roots.
Direct `Heap` use has explicit collection only.

Calls currently use Rust recursion with stack growth and a 256-frame language limit.
Both return and error release the VM frame window and active environment roots.

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

The full n-body acceptance test runs with `cargo test`.
The Node check exercises viewer logic with a minimal DOM model; it does not verify browser layout.
