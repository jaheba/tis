# Pressure-guarded tuple literals

Validated 29 October 2026. Fresh warmed native Barnes–Hut improves **1.3%, 2.290 → 2.260 s** beyond the combined entry scan; one-shot improves **1.4%, 2.350 → 2.318 s**. A separate direct comparison against the original shared-shape baseline measures **50.1% less time, 4.610 → 2.300 s**. Do not add stage percentages or compare medians from different rounds as a matched control.

## Change and safety

Quiet native tuple literals use the existing guarded C gateway after checking that one projected VM allocation remains strictly below the collection threshold. The predicate is shared with the existing collection checkpoint and uses checked addition. Pressure, eager observation, regions and nested execution retain the original fully rooted helper. A miss occurs before allocation; allocation and side effects are never replayed. Builtin tuple(iterable) is unchanged.

The audited allocator installs exactly one Object::Tuple and updates live count, generations and the weak field directory. It cannot collect or enter Python. Rust argument-vector allocation does not allocate a VM object. No arena empty-slot borrow is held across allocation; copied storage modules are unchanged. Result references reside in the native image until the next explicit root map or caller publication. Native return/frame cleanup cannot collect; register/stack callers publish the result before their collection checkpoint. No VM fields, statistics layout, LIR effects, side-exit states or root maps changed. Existing scalar descriptor guards retain their behavior.

Audit clarification: Heap::allocate takes &self through interior mutability. Read-only heap access alone does not prove absence of allocation. Scalar gateway safety comes from its audited body and callees; the earlier scalar documentation has been corrected.

Root synchronizations fall **10,388,891 → 7,714,644** (−2,674,247; **25.7%**). Callbacks rise **32,164,624 → 32,165,647** (+1,023 actual pressure/feedback misses). Savings equal 2,675,270 tuple calls minus 1,023 rooted fallbacks. Compared with the original baseline, callbacks remain about 72% lower and root synchronization about 59% lower.

## Measurements

Frozen baseline `/private/tmp/tis-profile/entry-scan-*`, candidate `tuple-budget-*`, cumulative original `fields-baseline-worker`. At least ten validated serial randomized interleaved samples per binary/backend. Fresh workers warm twice per sample; one-shot includes startup, parsing, compilation, execution and exit. Nbody and copy-patch Barnes use warmed reused workers. Builds/tests/profiles never overlap formal timings. All fresh native timed samples have zero compilation/deoptimization. Raw JSON includes hashes, validation, warmups and dispersion. Negative reductions mean slower execution; the modest gain should not be overstated.

| Comparison | Before (ms) | After (ms) | Time reduction |
| --- | ---: | ---: | ---: |
| Barnes–Hut fresh native | 2290.072 | 2259.872 | 1.3% |
| Barnes–Hut one-shot native | 2349.662 | 2317.565 | 1.4% |
| Cumulative native vs original shapes | 4609.534 | 2300.212 | 50.1% |
| Ticket fresh bytecode | 31.395 | 31.337 | 0.2% |
| Ticket one-shot bytecode | 40.210 | 40.616 | -1.0% |
| Nbody warmed bytecode | 239.062 | 237.965 | 0.5% |
| Ticket fresh native | 50.655 | 49.565 | 2.2% |
| Ticket one-shot native | 65.503 | 65.739 | -0.4% |
| Nbody warmed native | 8.346 | 8.389 | -0.5% |
| Ticket fresh copy-patch | 52.843 | 52.974 | -0.2% |
| Ticket one-shot copy-patch | 61.390 | 61.706 | -0.5% |
| Nbody warmed copy-patch | 162.523 | 162.380 | 0.1% |
| Barnes–Hut warmed copy-patch | 4866.110 | 4843.007 | 0.5% |

## Validation

Formatting, all-target Clippy with warnings denied, **250 debug tests**, **250 release tests**, release examples and whitespace checks pass. New tests cover exact collection-pressure boundaries with a hidden native field reference absent from physical local storage, overflow fallback, aliases, signed zero, eager execution, recursive tuples across callee GC and subsequent errors, and a 9,000-tuple OSR chain across all tiers. Explicit collection verifies both survival and eventual release. The boundary test forces collection at a temporary tuple which does not itself contain the hidden reference.

Post-timing samples are archived separately and do not contribute benchmark timings. See CHECKPOINT.md for remaining work. No commits or pushes; implementation/timing stops at 07:00 UTC / 08:00 Europe/Berlin.
