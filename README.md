# tis

A small Rust VM for experimenting with IR and specialization, independent of Monty's stdlib, sandbox and bindings.

Python source → Ruff AST → value IR → stack bytecode → VM.

The IR owns basic blocks, value IDs, block parameters and runtime feedback. Bytecode is generated only from IR and cached on the function. Integer addition specializes after eight successful integer observations; guards check operand types and overflow. A failed guard uses generic arithmetic and resumes observation. Feedback changes the IR strategy without rewriting bytecode.

```rust
use tis::{compile, Value, Vm};

let mut program = compile("def add(a, b):\n    return a + b\n")?;
let add = program.function_mut("add").unwrap();
let mut vm = Vm::new();
assert_eq!(vm.run(add, &[Value::Int(2), Value::Int(3)])?, Value::Int(5));
# Ok::<(), tis::Error>(())
```

## Inspect the pipeline

```sh
cargo run --example ir -- --source examples/sum.py --function sum
cargo run --example ir -- --source examples/sum.py --function sum --format bytecode
cargo run --example ir -- --source examples/sum.py --function sum --format html --output /tmp/tis-ir.html
cargo run --example ir -- --demo add --phase fallback
```

Source inspection always uses cold IR and does not execute the source. Built-in demos support cold, warm and fallback phases. The standalone HTML viewer supports pan, zoom, block/value selection and feedback inspection; DOT export is available too. Functions expose `dump()`, `to_dot()`, `to_html()` and `bytecode()?.disassemble()`.

## Current subset

Ruff parses Python; lowering supports positional functions invoked from Rust, assignments, returns, if/elif/else, while/else, literals, addition, subtraction, less/greater comparisons, lists, tuples, dictionaries, indexing, item assignment and `len`. Control-flow joins use block parameters and parallel edge transfers. Unsupported constructs are rejected explicitly.

There are no Python function calls, imports, stdlib, sandbox, snapshots or JIT. Integers are `i64` with overflow errors; mixed numeric operations use `f64`. Dictionary keys support None, bool, integer and string. IR is trusted input, without a complete verifier.

## Heap

Monty's stable paged arena and free list are copied unchanged into `src/heap/`, with their license. Only those two modules may contain unsafe Rust. The tis object layer uses generation-checked handles and explicit-root tracing collection, including cycles and slot reuse. This does not port Monty's reference counting, HeapReader API or cycle collector.

Values are nonowning copyable handles. Call `vm.collect_garbage(&roots)` with every external value that must survive; VM stack values are included automatically. Collection is explicit, with no automatic trigger. Runtime operations use values and the heap independently of bytecode dispatch.

## Checks

```sh
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
node tests/viewer.mjs /tmp/tis-ir.html
```

The Node check exercises viewer logic with a minimal DOM model; it does not verify browser layout.
