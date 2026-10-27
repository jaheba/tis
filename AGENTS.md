# tis development

This is a research VM. Keep changes small and runnable.
The IR is the semantic representation and the only input to bytecode generation; runtime feedback enriches its execution strategy.
Keep object operations independent of frames and stack placement.
Do not introduce Monty compatibility, stdlib, sandbox or binding infrastructure unless requested.
Use safe Rust outside the copied Monty storage modules (`src/heap/stable_heap.rs` and `src/heap/free_list.rs`).
Those two files retain their existing unsafe internals with explicit user approval; preserve their safety invariants.
Do not commit unless explicitly asked.

Run `cargo fmt --all`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` after Rust changes.
