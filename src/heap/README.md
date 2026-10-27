# Storage provenance

`stable_heap.rs` and `free_list.rs` are copied unchanged from
`pydantic/monty`, `crates/monty/src/heap/`, at revision `0f243e6ae6df760f09040a17604e23d5264b65ba` on 2026-10-27.
`LICENSE-MONTY` retains the upstream license.

The paged allocator keeps live entries at stable addresses while allocating through a shared reference. Freeing or obtaining mutable entry access requires an exclusive borrow. Keep the upstream safety comments and tests with these files.

The surrounding tis heap is safe Rust. It adds generation checks and explicit-root tracing over tis objects. It does not implement Monty's HeapReader capabilities, reference counting or cycle collector. Changing the copied unsafe modules requires a separate safety review.
