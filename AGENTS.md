# tis development

This is a research VM. Keep changes small and runnable.
HIR owns Python semantics and is the source for bytecode and typed LIR.
Runtime profiles are keyed by stable HIR identity and code revision, not heap addresses.
Keep speculative guards, side-exit state and GC root maps explicit in LIR.
Keep object operations independent of frames and stack placement.
Python class bases and object classes are fixed at creation. Reject assignment/deletion of class `__bases__` and object
`__class__`, including attribute hooks, object bypass methods and native write paths.
Do not introduce Monty compatibility, stdlib, sandbox or binding infrastructure unless requested.
Unsafe Rust is permitted. Keep native execution boundaries small and document pointer, ABI and lifetime invariants.
Preserve the safety invariants of the copied Monty storage modules.
Copy-and-patch stencils compile as a separate build-time Rust crate; preserve relocation and tail-continuation checks.
Run `rustfmt --edition 2024 src/copy_patch/stencils.rs` when editing that source.
Do not commit unless explicitly asked.

Run `cargo fmt --all`, `cargo clippy --all-targets -- -D warnings`, and `cargo test` after Rust changes.
