# tis

A Rust interpreter for a Python subset, built around an explicit value IR.
Ruff AST → IR → stack bytecode → interpreter.
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

The executing function retains its IR and addition feedback.
Eight successful integer observations select a guarded integer addition strategy.
Type or overflow guard failure falls back to generic arithmetic without rewriting bytecode.
Name loads, stores, calls and object operations remain explicit IR operations.

## Rust API

```rust
use tis::{compile, Value, Vm};

let mut program = compile("def add(a, b=3):\n    return a + b\n")?;
let mut vm = Vm::new();
vm.run_program(&mut program)?;
let add = vm.get_global("add")?;
assert_eq!(vm.call(add, &[Value::Int(2)])?, Value::Int(5));
println!("{}", vm.function_ir(add)?.dump());
# Ok::<(), tis::Error>(())
```

`Vm::output` captures `print` output.
`Program::function` exposes compile-time templates for inspection; `Vm::function_ir` inspects a live callable's feedback.
The lower-level `Vm::run` API remains available for functions constructed through the Rust IR builder.

## Inspect IR and bytecode

```sh
cargo run --example ir -- --source examples/sum.py --function sum
cargo run --example ir -- --source examples/sum.py --function sum --format bytecode
cargo run --example ir -- --source examples/sum.py --function sum --format html --output /tmp/tis-ir.html
cargo run --example ir -- --demo add --phase fallback
```

Source inspection does not execute the source.
The standalone HTML viewer supports pan, zoom, block/value selection and feedback inspection; DOT export is available.
Functions expose `dump()`, `to_dot()`, `to_html()` and `bytecode()?.disassemble()`.

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
