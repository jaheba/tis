# Custom benchmark variations

`generate.py` writes 17 standalone programs outside the repository and checks equivalent rewrites against CPython.
The four families cover Boolean fills, numeric helpers and sequence layouts, dictionary updates and Unicode keys,
and object fields/helpers/methods/constructor layouts.
The seed changes histogram input keys; the other data variants use explicit fixed alternative sizes and values.

`run.py` measures all programs in bytecode, copy-and-patch, Cranelift, CPython and PyPy.
It first uses the archived Flying Circus adapter for cold and reused-worker fresh sessions.
It then creates a separate worker per case/runtime, retaining functions across 20 warmup and 10 measured calls.
The runner defaults to the frozen counter-loop worker and the local archived harness; their paths are defined at the top.
Startup and fixture preparation are excluded from request times; retained setup/warmup times are saved separately.
No source rewriting or output substitution happens inside the workers.

```sh
python3 benchmarks/variations/generate.py --output /tmp/tis-custom-new/programs --seed 1807
python3 benchmarks/variations/run.py /tmp/tis-custom-new
python3 benchmarks/variations/analyze.py /tmp/tis-custom-new
```

Use `--skip-fresh` to repeat retained measurements without rerunning Flying Circus.
Retained warmup and sample counts are configurable; the report includes sample spread rather than assuming stability.
Raw sources/results/binaries belong outside Git.
See [the first comparison](../results/2026-10-31-custom-variations/README.md) for results and limitations.
