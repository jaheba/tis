# Buffered native mutation and resumable exits

Acyclic read/modify/write methods can now execute primitive field updates as closed native regions.
The implementation covers particle updates and center-of-mass updates in Barnes–Hut.
It establishes resumable effects; it does not compile allocation-heavy `insert` or `subdivide` into one native region.
The feature is opt-in and Cranelift-only.

## Profile and scope

A five-second macOS sample of the previous read-only build validated eight warmed runs.
39.7% of exclusive samples were unresolved generated-code addresses.
Other prominent costs included generic callback handling (5.9%), native entry setup/cleanup (3.9%), heap reads (3.8%),
bytecode/window execution (2.7%), attribute reads (2.1%) and root synchronization (1.9%).
The sample does not attribute every anonymous address to an individual Python method.
Particle updates provided a bounded first target for write/resume semantics.
Supporting acyclic branches also admitted `update_mass_distribution`.

## Execution contract

The region lowers generic LIR and admits numeric arithmetic, comparisons, local variables, instance field reads,
and int/float writes to existing instance slots.
Every path returns `None`; HIR CFG cycles, calls, allocation, globals, unpromoted locals and escaping results are excluded.
Write-only initialization is skipped because its fields generally do not yet exist.
Unsupported values can leave the region at the original HIR instruction.

Native code appends writes to a fixed-capacity private buffer.
Later reads forward through the executed write prefix, including aliases through different attribute paths.
Single-block regions use SSA forwarding; branching regions scan only the initialized dynamic prefix.
The backing heap is unchanged until native code returns, so heap data and weak cache reads carry Cranelift's `readonly`
flag for that invocation.
The write buffer and its publication count retain ordinary mutable memory dependencies.

On success, Rust commits numeric stores through normal mutable heap guards.
Commits group independent receivers while retaining write order within each receiver; no collection or Python observer
can intervene, and the already-validated slots cannot change during this phase.
Weak directory rows refresh through the existing mutable-guard protocol.
No raw stores into Rust-owned heap payloads were introduced, and the copied heap internals were not changed.

A guard failure after writes commits exactly the executed prefix, materializes live HIR values/locals, and resumes the
ordinary VM at the failing operation.
It does not replay earlier writes or the branch that selected them.
A failure before any write may safely use the ordinary entry path while a weak cache learns.
Generic mutation regions retain their artifact on these resumable exits and record the resume site's failure reason and
operand types; integer overflow and floating arithmetic failures remain distinct from type guards.
No arbitrary runtime calls or native recursive mutation are admitted yet.
Those require publishing roots and resuming a native continuation across the call/allocation boundary.

Register images, source states and field-miss details are emitted only on cold exits.
Guards for the same operation/state share a cold spill block.
Scratch buffers retain weak bits rather than GC roots; every readable exit value and published write is refreshed by
its current invocation before use.
The native invocation cannot reenter Rust, allocate, collect or mutate the heap.

## Validation

325 tests pass in debug and release, with focused rechecks after the final metadata changes.
Formatting and all-target Clippy with warnings denied pass.
Tests exercise stack and register bytecode, zero-callback success, allocation-free success, aliased receivers,
conditional writes, partial-write arithmetic errors, non-numeric store fallback, unknown calls and unpromoted locals.
An error after an increment preserves exactly one increment and resumes subsequent calls correctly.
A forced-collection test detaches the last heap owner in the native prefix, then allocates a bound method in resumed
bytecode; the reconstructed local keeps the detached object alive, and later collection releases it.
Barnes–Hut matches bytecode's floating-point result exactly in both bytecode modes and after collection.
Captured mutation IR contains no runtime call instructions.

## Quick measurements

One timed sample per comparison with two warmups, serial execution without overlapping builds/tests.
All outputs validate; timed tis samples have no compilation, deoptimizations, cold misses or mutation guard exits.
These are directional measurements, not stability estimates.

| Comparison | Before/off | After/on |
| --- | ---: | ---: |
| Previous read-only build vs new read-only + mutation build | 0.502587 s | 0.491023 s |
| Same new binary, mutation disabled/enabled | 0.507124 s | 0.487220 s |

The isolated mutation sample improves about 3.9%; the complete build comparison improves about 2.3%.
This is a modest gain, not another step change.
54,544 region invocations commit 223,632 writes in the timed Barnes–Hut run.
Runtime callbacks fall from 3,756,537 to 3,463,817, and root synchronizations from 1,349,987 to 1,280,899.
Physical positional frames remain 378,319, which limits the gain from this step.
Insertion, subdivision, bound-method lookup, allocation and VM call handling remain outside the new mutation subset.
The experiment has not measured the benefit of native continuations across those boundaries.

A separate same-workload runtime sample gives:

| Runtime | Time |
| --- | ---: |
| tis, both native region options | 0.493503 s |
| CPython 3.15.0, JIT unavailable | 0.622762 s |
| PyPy 7.3.16 / Python 3.10.14 | 0.196097 s |

Tis remains faster than CPython in this sample and about 2.5× slower than PyPy.
Two warmups do not establish fully settled PyPy performance, and variation between these samples is not a proven change
in the PyPy gap.
No long stability, Ticket Search, or long nbody run was added.

Use `--tier native --readonly-regions --mutation-regions` with the `run` example, or both region flags with `bench`.
The benchmark tools record both options; `NativeDiagnostics::mutation_clif` exposes generated region IR.
The incremental patch is relative to the uncommitted snapshot in `baseline-source-sha256.json`.
Existing changes were preserved. No commits or pushes were made.
