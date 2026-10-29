# Scalarize direct tuple returns into immediate unpacking

CFG inlining now passes tuple components along return edges when every callee return constructs a fresh tuple of the same arity (one to four fields), and the caller immediately unpacks it with no other uses. Tuple identity must not escape through a named local, closure, branch edge or other operation. Mixed-shape returns and non-direct producers retain ordinary allocation.

The pass checks HIR uses and the actual lowered instruction bundle. It removes tuple construction and the caller unpack/read operations, retains return-profile accounting, and prunes snapshots/exits owned only by the removed instructions. Continuation parameters are the original unpack-item registers. Components retain the ordinary GC-root and scalar-materialization machinery.

No guard, safepoint or effect remains between eliminated construction and completed unpack. Callee exits resume before construction, allowing bytecode to allocate and unpack normally. Entry guard failure executes the original call. This bounded transformation does not add general virtual-object reconstruction recipes. The verifier checks the scalarized return's source, resume point, tuple producer and first component mapping.

This remains behind CFG inlining. No heap-storage, unsafe-code, stencil ABI or default-tier changes were made.

## Validation

310 tests pass in debug and release. Formatting and all-target Clippy with warnings denied pass. Tests cover LIR, Cranelift and copy-and-patch; stack and register bytecode; allocation elimination; branching returns; aliases; callable rebinding; escaping/mixed returns; wrong unpack arity; and deoptimization after a side effect. A forced collection immediately after an inlined return preserves detached heap components, then releases them after the call.

## Quick Barnes–Hut samples

One timed sample per comparison, two warmups, serial execution without overlapping builds or tests. These are directional measurements, not stability estimates. All outputs validate. Timed samples have no backend compilation or deoptimizations, and cumulative compilation errors remain zero.

| Configuration | Before | After |
| --- | ---: | ---: |
| Cranelift, CFG enabled | 2.559115 s | 2.365792 s |
| Copy-and-patch, CFG enabled | 6.295185 s | 6.107205 s |

The native sample improves 7.6%, eliminating 1,250,776 tuple allocations and reducing runtime callbacks from 33,934,448 to 31,432,467. Physical frames and inline entries are unchanged. The copy-and-patch sample improves 3.0%.

A separate comparison still favors the default path: 2.225465 s with CFG disabled versus 2.382100 s with this optimization and CFG enabled. CFG inlining therefore stays opt-in. Eliminating tuple allocation helps, but remaining inlining bookkeeping still costs too much. No new CPython or PyPy measurement was taken.

Raw samples include binary hashes and counters. Source manifests, the incremental patch and validation logs accompany this report. No commits or pushes were made.
