# 128-bit boxed values

`Value`, `Option<Value>` and native ABI `Slot` are 16 bytes, down from 32. The Rust enum uses a 32-bit discriminant and 96-bit reference identity. Native slots explicitly pack that identity into the tag/payload pair; the Rust enum is not transmuted into the C ABI. Full-width integers and floats are preserved, including signed zero, infinities and NaN payload bits.

Heap identity, slot index and generation remain independently checked. Each is 32 bits; exhaustion panics rather than wrapping. Generations now count reuse of each slot, avoiding a limit on total heap allocations. Tracing GC, rooting and pinning remain unchanged. Copied stable-heap/free-list internals are untouched.

Both Cranelift and copy-and-patch use the smaller slots. Native boxed SSA and parallel transfers carry two words. Views pack length/feedback into their tag word; loop metadata occupies two auxiliary slots outside boxed values. Stencils were rebuilt from their Rust source.

## Warmed execution

Apple M1 Pro, macOS 27, Rust 1.99 release defaults. Ten serial interleaved samples, two warmups per worker, all outputs validated. Runtime feedback remains enabled. Baseline is `2fbe2ce` plus the preceding prepared-method optimization, frozen before the representation change. Binary hashes and raw samples are recorded in JSON.

| Benchmark/backend | Before | After | Time reduction |
| --- | ---: | ---: | ---: |
| Ticket search, bytecode | 43.841 ms | 36.891 ms | 15.9% |
| Ticket search, native | 71.502 ms | 59.476 ms | 16.8% |
| Ticket search, copy-patch | 70.868 ms | 61.171 ms | 13.7% |
| Nbody (20k), bytecode | 302.122 ms | 241.689 ms | 20.0% |
| Nbody (20k), native | 11.688 ms | 8.767 ms | 25.0% |
| Nbody (20k), copy-patch | 186.689 ms | 160.708 ms | 13.9% |

| Barnes–Hut, bytecode | 3.890 s | 3.400 s | 12.6% |
| Barnes–Hut, native | 5.862 s | 5.225 s | 10.9% |

## Fresh process

Ten samples without warmup, including startup and compilation:

| Ticket search/backend | Before | After | Time reduction |
| --- | ---: | ---: | ---: |
| bytecode | 52.417 ms | 46.951 ms | 10.4% |
| native | 84.761 ms | 75.297 ms | 11.2% |
| copy-patch | 78.342 ms | 69.853 ms | 10.8% |

The raw samples retain scheduling outliers. Fresh-process CPython measured 33.179 ms; this includes startup and is not a warmed interpreter comparison.

## Validation

All 201 debug and release tests passed; all-target Clippy with warnings denied passed. Layout assertions cover `Value`, optional register values and native slots. Tests cover round-trip scalar/reference bits, freed slots, slot reuse, foreign heaps, collection, native roots, side exits, loop regions and copy-and-patch relocation/execution.

Reproduce with `tools/compare_nbody.py` (generic `bench` workers for ticket/Barnes; `nbody_bench` for nbody) and `tools/bench_oneshot.py`. Supply the frozen baseline and candidate executables recorded in the JSON files. Build/test processes did not overlap timings.
