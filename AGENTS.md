# tis development

This is a research VM. Keep changes small and runnable.
HIR owns Python semantics and is the source for bytecode and typed LIR.
Runtime profiles are keyed by stable HIR identity and code revision, not heap addresses.
Keep speculative guards, side-exit state and GC root maps explicit in LIR.
Keep object operations independent of frames and stack placement.
Do not introduce Monty compatibility, stdlib, sandbox or binding infrastructure unless requested.
Use safe Rust outside the copied Monty storage modules (`src/heap/stable_heap.rs` and `src/heap/free_list.rs`).
Those two files retain their existing unsafe internals with explicit user approval; preserve their safety invariants.
Do not commit unless explicitly asked.

Run `cargo fmt --all`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` after Rust changes.
