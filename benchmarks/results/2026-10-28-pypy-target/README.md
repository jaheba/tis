# PyPy target and loop comparison, 2026-10-28

Harness and warmed-LIR capture source: `d88a2ba5573cfb4c7799e2e1cb11d79db4b20b1f`.
Tis runtime source remains `22c4085` (unpack batching and scalar truth); this experiment changes no VM execution logic.

PyPy's warmed nbody median is **7.163ms**, compared with **217.725ms** for tis native: a **30.4× gap**.
The performance target is materially different from CPython: its warmed median here is 76.515ms, or 10.7× PyPy.
The next work should target an unboxed loop execution path and typed heap access.
Additional callback fusion alone does not address the representation and code-generation differences observed below.

| Runtime | Fresh-process median (ms) | First kernel call (ms) | Warm kernel median (ms) | Warm / PyPy |
| --- | ---: | ---: | ---: | ---: |
| bytecode | 304.109 | 306.472 | 292.046 | 40.77× |
| lir | 470.796 | 463.537 | 460.371 | 64.27× |
| native | 230.164 | 227.307 | 217.725 | 30.39× |
| copy-patch | 250.711 | 280.594 | 242.226 | 33.81× |
| cpython | 105.012 | 76.579 | 76.515 | 10.68× |
| pypy | 41.912 | 14.317 | 7.163 | 1.00× |

Fresh-process numbers include process startup, source compilation, setup, first execution and stdout validation.
The first kernel column is one worker observation, not a ten-sample statistic, and includes that call's JIT work.
These are separate measurements and must not be subtracted or added as a decomposition of fresh-process time.
Warm kernel samples time only `advance(0.01, 20000)`, restoring identical coordinates and velocities before each call
and validating energy `-0.169089263` outside the timer.

## Measurement

The runner collected ten serial, randomly interleaved fresh-process executions and ten serial, interleaved steady samples
per runtime/tier. Workers made ten kernel warmup calls; every warmup time is retained.
No builds, tests, tracing or other benchmark workers executed concurrently with measurements.
The installed runtimes are native ARM64 PyPy 7.3.16 / Python 3.10.14 and CPython 3.15.0 with its experimental JIT unavailable.
PyPy uses its default JIT parameters, including threshold 1039 and `enable_opts=all`; vectorization is disabled by default.
The source is identical and uses features supported by both Python versions.
Platform: Apple M1 Pro / macOS 27.0 ARM64, Rust 1.99.0, default Cargo release profile.

`comparison.json` retains version/build metadata, binary and source hashes, warnings, all warmups and raw timings.
All fresh-process stdout and all warmed energy results matched.
Tis reported no compilation or deoptimization during the steady samples.
PyPy's compilation time and deoptimization counts are unknown in the timing run, represented by nulls.
An initial sandboxed discovery printed an old-PyPy CPU-cache warning; the unrestricted primary measurement recorded no warning.
No GC nursery or JIT parameters were overridden.

## Separate trace diagnostic

The diagnostic uses [PyPy's compile hook](https://doc.pypy.org/en/latest/jit-hooks.html) with `operations=False` and
[optimized PYPYLOG output](https://doc.pypy.org/man/pypy.1.html#environment), separately from timing.
The compile hook records two `advance` loops and two bridges during the first kernel call.
Its cumulative counts are `[4,4,4,4,4,4,4,4,4,4]` through warmup, with zero new events in ten subsequent calls.
Those diagnostic timings include logging/hook overhead and are not performance results.
The full log includes unrelated startup/import traces; the inspector selects traces by the `advance` source locations.
Backend execution-count sections were unavailable in this build, so the selected traces are not ranked by measured frequency.
The installed build's statistics-snapshot API is unimplemented; the separate compile hook avoids relying on it.

The inspector identifies each internal backedge by matching the final jump's target with the last label.
It excludes the entry preamble from loop-body counts and retains bridges separately.
Synthetic parser checks verify that preamble allocations are excluded from a backedge body while bridge allocations remain visible.

| PyPy optimized loop body | Float array reads | Float array writes | Calls | Allocation operations | Float backedge arguments |
| --- | ---: | ---: | ---: | ---: | ---: |
| Position updates, loop 18 | 6 | 3 | 0 | 0 | 4 |
| Pair interactions, loop 19 | 12 | 6 | 2 | 0 | 13 |

Both bodies read/write `<ArrayF 8>` storage directly, use primitive float arithmetic, and pass primitive floats through backedges.
The pair-loop calls are `ccall_pow` and a thread-local integer helper associated with the math path.
There are no general Python-interpreter operation calls in these bodies.
The exit bridges still contain five and thirteen `new_with_vtable` operations, including materializations;
this is not a claim that the complete function allocates nothing.
Static IR counts describe these traces and cannot be compared as machine instruction counts or execution-time percentages.

Readable selected bodies are in `pypy-advance-18-body.txt` and `pypy-advance-19-body.txt`.
Full selected traces and bridge excerpts are retained beside them, with the complete original log in `pypy.log.gz`.
`pypy-events.json` records compile events, green keys, machine-code lengths and validated responses.

## Corresponding tis LIR

The actual warmed native worker exported `advance` after ten validated calls using `--dump-lir`.
The complete reconstruction/root metadata is retained in `tis-advance.lir.gz`; `tis-b6.lir` and `tis-b10.lir` are readable bodies.
`summary.json` contains per-block operation counts and the extracted PyPy trace counts.

The interaction block b6 contains 24 float arithmetic operations, 24 `BoxFloat` shadows, 16 unboxes,
five verified unpack groups, twenty sequence reads and six `SetItem` calls.
Native batching removes fourteen of those sequence-read callbacks, leaving five batches and six standalone read callbacks.
The six indexed writes still call Rust. Iterator `Next`/`IsDone` and block-entry callbacks occur in surrounding blocks.
A constant negative exponent also remains a generic unary operation, a small constant-folding opportunity.

Tis `BoxFloat` shadows are immediate values in the 32-byte native slot image; they are not Python float heap allocations.
The native lowering emits slot loads/stores; Cranelift may eliminate some locally, but the opaque runtime ABI exposes the
register image at callbacks. Primitive reuse is currently local to a block, and HIR loop parameters remain boxed.
PyPy's traces instead expose primitive loop-carried values and typed array memory operations directly to its native backend.

## Next implementation target

Build a general guarded typed loop path in LIR and native lowering:

1. Add typed float-list reads/writes with explicit guards and mutation/GC lifetime rules.
   Preserve Python list identity, aliasing, mixed-type assignments and fallback by widening or exiting before incompatible writes.
2. Carry primitive loop values through typed backedges and native SSA registers.
   Materialize boxed shadows at real runtime boundaries and side exits rather than after every arithmetic operation.
3. Lower common iterator advancement/exhaustion to native loop control, with precise safepoints and explicit deoptimization state.

The measurable target is an ordinary pair-update backedge with direct typed memory operations and no VM-operation callbacks,
while preserving the math call and guard/error behavior. This should be validated against PyPy's 7.2ms result, not merely
against the previous tis build. The trace comparison motivates this direction; it does not predict an exact speedup.

## Validation and reproduction

All 140 Rust tests pass in debug and release; formatting and all-target clippy pass.
Release examples, the warmed-LIR dump, both Python-worker aliases, and cold/steady output validation were exercised.
The Python tools pass bytecode compilation and their trace-parser invariants were checked.

See "PyPy performance target" in the project README for the comparison, trace and LIR-capture commands.
The benchmark runner uses the same Python worker for CPython and PyPy; its legacy `--cpython-worker` alias remains available.
