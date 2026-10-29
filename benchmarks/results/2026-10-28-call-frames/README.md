# Shared call-frame storage

Uncaptured locals occupy ranges in one reusable VM buffer. Frame headers store offsets, length and code metadata. Entry SSA parameter IDs are cached in bytecode; arity-checked positional arguments and trailing defaults initialize locals directly without an intermediate binding vector. Active local ranges are GC roots and are truncated on return/error. Deoptimization copies locals into the current range; native panic containment restores local/register lengths and frame headers.

Register bytecode reuses checked-out argument, parallel-transfer and prepared-method buffers. Nested invocations get independent buffers; returning clears every logical buffer before pooling it. Cleared capacity is not a root. Profiling still gathers argument observations during active windows.

Rust-recursive dispatch, caller-side argument gathering, bound-receiver argument vectors and captured heap environments remain. Keyword/starred calls retain the general binder. No inlining, trampoline dispatcher or JIT calling convention change is included.

## Profile

Before edits, two six-second macOS `sample` passes sampled warmed Barnes–Hut bytecode; every worker response was validated. Exclusive samples included frame execution/setup at 2.8%, function invocation at 1.0%, local-vector collection at 1.4%, and allocator routines at about 8% combined. Allocator costs are not all attributable to calls. Recursive inclusive percentages include the bodies and are not setup overhead. Attribute-map lookup, comparison and dispatch remain larger costs. Raw compressed samples and a compact function summary are included.

## Warmed execution

Baseline `4e28b84`; Apple M1 Pro, macOS 27, Rust 1.99 release defaults. Ten serial interleaved samples with two warmups per worker. All outputs validated, feedback enabled, and no builds/tests/profiling overlapped timed runs. JSON records binary hashes, counters and raw samples.

| Benchmark/backend | Before | After | Time reduction |
| --- | ---: | ---: | ---: |
| Barnes–Hut, bytecode | 3.311 s | 3.065 s | 7.4% |
| Barnes–Hut, native | 5.110 s | 4.987 s | 2.4% |
| Ticket search, bytecode | 33.698 ms | 33.650 ms | 0.1% |
| Ticket search, native | 56.986 ms | 56.588 ms | 0.7% |
| Ticket search, copy-patch | 58.813 ms | 58.530 ms | 0.5% |
| Nbody (20k), bytecode | 244.697 ms | 242.121 ms | 1.1% |
| Nbody (20k), native | 8.778 ms | 8.917 ms | -1.6% |
| Nbody (20k), copy-patch | 161.888 ms | 163.377 ms | -0.9% |

Barnes–Hut bytecode improves 7.4% and native 2.4%. Ticket-search changes are below 1%. Nbody changes are small (including a 1.6% native slowdown in this run); the raw distributions are retained.

A warmed bytecode Barnes–Hut run enters 2,777,838 positional frames and reuses scratch buffers 2,777,838 times, with zero new scratch instances. This is not a claim of zero allocations: bound receivers, Python objects and general binding can still allocate.

## Fresh process

Ten ticket-search samples without warmup, including startup and compilation:

| Backend | Before | After | Time reduction |
| --- | ---: | ---: | ---: |
| bytecode | 43.462 ms | 43.197 ms | 0.6% |
| native | 71.792 ms | 71.899 ms | -0.1% |
| copy-patch | 67.138 ms | 67.365 ms | -0.3% |

Cold results are essentially flat. Outliers remain in JSON. CPython's fresh-process median was 33.265 ms, including startup, not a warmed comparison.

## Validation

All 211 debug and release tests pass; all-target Clippy with warnings denied and formatting checks pass.

Tests cover GC during defaulted/bound calls, keyword/star arguments, native side exits, frame reuse after exceptions, recursion limits, unbound locals and abandoned prepared methods. The private local stack and frame headers are checked empty after return/errors, and inactive scratch capacity does not retain heap arguments.

Reproduce with `tools/compare_nbody.py` and `tools/bench_oneshot.py`, supplying the frozen executables recorded in JSON. Generic workers handle Barnes/ticket and `nbody_bench` handles nbody.
