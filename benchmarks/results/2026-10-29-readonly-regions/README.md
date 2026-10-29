# Closed recursive native regions

Barnes–Hut now has an opt-in Cranelift region for its read-only recursive force calculation.
One native invocation traverses the tree without inner VM frames, Python tuple allocations or runtime callbacks.
This is a generic LIR transformation, not a replacement implementation of Barnes–Hut.

## Profile and implementation

Five-second macOS samples compared ordinary native execution with CFG inlining after tuple-return fusion.
Root synchronization accounted for 3.9% of exclusive samples by default and 7.2% with CFG inlining.
Generic callback step handling accounted for 9.5% and 12.4%, respectively.
Native invocation setup/cleanup accounted for 13.2% and 11.3%.
Unknown JIT addresses represented roughly 11%; these samples do not identify individual generated operations.
The profiles supported removing complete invocation/runtime transitions rather than adding more bounded body copies.
Compressed call trees, summaries, commands and binary hashes are under `profiles/`.

The new region lowers generic LIR into a separate native function with direct recursive calls.
A whitelist admits primitive arithmetic, comparisons, local variables, instance fields, list/tuple reads, `len`,
`math.sqrt`, and direct recursive method calls returning one to four primitive tuple components.
Eligibility rejects loops, arbitrary calls, heap writes and escaping tuple temporaries.
Intermediate tuple components remain in native variables and return buffers; only the outer result tuple is allocated.

Entry preparation resolves current global bindings and verifies the recursive method identity, revision and environment.
Generated reads guard heap identity, generation, bounds and field shapes.
Recursive receivers must have the entry receiver's class and shape, so instance method shadowing cannot be skipped.
Builtin tokens require the actual `len`/`math` native objects; rebound Python callables use the ordinary VM.
Weak field caches learn after a failed attempt without executing Python or allocating VM objects.
The heap directory now describes list/tuple lengths and boxed elements as well as instance fields.
Compact float lists expose length only; their element accesses fall back.
Allocation, mutable-borrow guards and collection retain the existing directory refresh/invalidation protocol.
The copied arena internals and copy-and-patch stencils were not changed.

The region cannot allocate, collect, call Rust/Python or mutate the heap while native code runs.
A failed guard propagates to its outer entry and replays the original VM body.
This entry-replay contract depends on the whitelist and guards preventing observable effects before failure.
It does not add arbitrary mid-region deoptimization or virtual-object reconstruction.
Native recursion has a 32-frame attempt budget and also observes the remaining language recursion allowance.
The ordinary VM handles deeper attempts, errors, unsupported values, eager profiling and OSR.

## Validation

317 tests pass in debug and release; the final guard-handling changes also pass the focused release tests.
Formatting and all-target Clippy with warnings denied pass.
Tests cover stack/register bytecode, native frame/allocation elimination, changed shapes and globals, builtin rebinding,
stale bound methods after class rebinding, errors, recursion exhaustion, and rejection of writes/unknown calls.
Numeric differential checks cover integer tags, large integers, overflow, signed zero, infinities, NaNs and booleans.
Barnes–Hut matches bytecode's floating-point result exactly on the test workload, including after collection.
Captured region IR has one imported function reference: its own recursive entry, with no runtime callback imports.

## Quick measurements

One timed sample, two warmups per process, serial execution with no builds/tests overlapping timing.
These are directional measurements, not estimates of stable performance.
All outputs validate; tis timed samples have zero compilation, deoptimizations and cumulative compilation errors.

| Native Barnes–Hut | Time |
| --- | ---: |
| Previous default native build | 2.271834 s |
| New build, read-only regions enabled | 0.506673 s |

That is about 4.48× faster.
20,000 region entries complete the outer force calls with zero machine-code guard misses in the timed sample.
Runtime callbacks fall from 32,265,625 to 3,756,537, root synchronizations from 7,479,319 to 1,349,987,
and physical positional frames from 2,502,535 to 378,319.
The remaining work includes tree construction, particle updates and ordinary VM execution.

A separate same-workload runtime comparison gives:

| Runtime | Time |
| --- | ---: |
| tis, read-only regions | 0.505414 s |
| CPython 3.15.0, JIT unavailable | 0.622639 s |
| PyPy 7.3.16 / Python 3.10.14 | 0.184466 s |

The experimental tis path is about 19% faster than CPython in this sample and remains about 2.74× slower than PyPy.
Two warmups do not establish fully settled PyPy performance; no long stability run was performed.
A default-disabled control sample is 2.226458 s before versus 2.278368 s after, about 2.3% slower.
That small change may be noise or the larger weak directory's overhead; default performance needs monitoring.
The feature remains opt-in and is currently Cranelift-only.
No Ticket Search or long nbody comparison was added in this experiment.

Use `--tier native --readonly-regions` with the `run` example or `--readonly-regions` with `bench`.
The benchmark tools record this option in their result metadata.
`NativeDiagnostics::readonly_clif` exposes the generated region when code capture is enabled.
The incremental patch is relative to the uncommitted source snapshot in `baseline-source-sha256.json`.
Existing changes were preserved. No commits or pushes were made.
