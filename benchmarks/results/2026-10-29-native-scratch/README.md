# Reuse native entry scratch buffers

Validated 29 October 2026. Freshly warmed native Barnes–Hut improves **7.0%**, **3.198 → 2.975 s**, beyond invariant input loading. One-shot improves **7.6%**, **3.289 → 3.040 s**. A separate direct cumulative control against the original shared-frame/shape baseline measures **36.4%** less time, **4.689 → 2.982 s**. Stage percentages are not added.

## Change and safety

The shared native bridge allocated new rooted-index and helper-argument vectors on every invocation, while pooling only its register image. It now pools all three buffers together. Each active recursive invocation owns a separate bundle; inactive bundles have zero logical lengths. Only capacities are reused. No live Python Values or GC roots remain in the pool.

Context continues to own its vectors during execution. An inner result closure covers native invocation, view flush, helper error handling, return decoding and deoptimization reconstruction. Buffers are reclaimed after that closure on successful returns, side exits and errors, including recovered helper panics. Every buffer is cleared before pool return. Early setup errors also pass through the outer cleanup. No pointers into movable buffers are retained.

Context fields/layout and the VM pool's Vec layout are preserved. LIR effects, safepoint root maps, side-exit state, helper behavior, native Slot ABI and stencils remain unchanged. Copy-and-patch uses the shared bridge and can reuse the same scratch capacities.

This change reduces allocation work at entry. Native Barnes callbacks remain 42,926,781 and root synchronizations remain 18,689,295 per warmed run; no callbacks/safepoints were removed. The preceding [scalar publication experiment](../2026-10-29-scalar-roots/README.md) was 7.2% slower and was reverted before this work.

## Measurement

Baseline frozen binaries: `/private/tmp/tis-profile/invariant-minimal-*`. Candidate: `/private/tmp/tis-profile/native-scratch-*`. Original-shapes control: `fields-baseline-worker`. At least ten validated samples per binary/backend, serial randomized interleaving. Fresh workers warm twice before each timed call; one-shot includes startup, parsing, compilation, execution and exit. Nbody and copy-patch Barnes reuse warmed workers. No builds/tests/profiles overlap formal timings. Outliers remain included. All twenty fresh native samples in each native comparison have zero timed backend compilation/deoptimization. JSON includes executable hashes, platform, every sample and quartiles; source hashes and validation logs are archived.

| Fresh native comparison | Before (ms) | After (ms) | Time reduction |
| --- | ---: | ---: | ---: |
| Scratch pooling | 3197.846 | 2975.185 | 7.0% |
| Combined overnight work vs shapes | 4688.706 | 2981.830 | 36.4% |

| Warmed regression control | Before (ms) | After (ms) | Time reduction |
| --- | ---: | ---: | ---: |
| Ticket, bytecode | 33.615 | 33.233 | 1.1% |
| Ticket, native | 54.012 | 54.086 | -0.1% |
| Ticket, copy-patch | 56.120 | 54.753 | 2.4% |
| Nbody (20k), bytecode | 243.484 | 243.569 | -0.0% |
| Nbody (20k), native | 8.439 | 8.412 | 0.3% |
| Nbody (20k), copy-patch | 162.205 | 162.312 | -0.1% |
| Barnes–Hut, copy-patch | 5697.646 | 5393.010 | 5.3% |

| One-shot control | Before (ms) | After (ms) | Time reduction |
| --- | ---: | ---: | ---: |
| Barnes–Hut, native | 3289.194 | 3040.409 | 7.6% |
| Ticket, bytecode | 42.469 | 42.545 | -0.2% |
| Ticket, native | 68.191 | 68.345 | -0.2% |
| Ticket, copy-patch | 63.024 | 62.801 | 0.4% |

Negative reduction means slower execution. Small process/layout variations are not attributed to scratch reuse without additional evidence.

Two six-second CPU samples collected after all timing (all sampled worker runs validated) show remaining exclusive shares: Context::step 19.06%, native entry 13.91%, root synchronization 7.62%, Heap::get 4.46%, callback 3.97% and lir_helper 3.56%. Raw samples, worker responses and parsed summary are archived here. Inlining/attribution changed, so these percentages are not an absolute before/after cost comparison.

## Validation and follow-up

`cargo fmt --all`, `cargo clippy --all-targets -- -D warnings`, **229 debug tests**, **229 release tests**, release examples build and diff whitespace checks pass. The new unit test inspects empty pooled buffers after nested native calls and recovered helper panic, then verifies GC and reuse. [Mixed-result coverage](../../../tests/native_roots.rs) checks reference/scalar helper results across GC/type deoptimization, eager execution, overflow cleanup and post-return release. Existing all-tier semantics and GC suites pass.

See [CHECKPOINT.md](CHECKPOINT.md) for exact continuation state. Mixed Int/Float inplace arithmetic, equality/unary-not lowering and allocating/calling root synchronization remain unfinished. Stop implementation and timings at 07:00 UTC / 08:00 Europe/Berlin; no commits/pushes.
