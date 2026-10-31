# Shared Boolean loop plans in Cranelift

Baseline revision: d0f1aab. Frozen baseline binaries: imports-final.
The candidate enables the existing LIR Boolean fill/reduction recognizers in Cranelift.

## Implementation

The new native/boolean.rs emitter validates exact heap identity/generation, cached range state, byte Boolean storage,
endpoint/index bounds and write permission before the fill. Reductions guard integer accumulators and conservative
sum bounds, use checked additions, and publish results/cursor only after the complete read-only scan succeeds.
Any miss leaves the original LIR loop responsible for execution, effects and errors.
Empty/exhausted iterators and sticky Done state remain owned by the original Next operation.
No runtime gateway, allocation or GC occurs inside the emitted loops.

Both backends use region/fill.rs and region/reduce.rs unchanged. Cranelift emits its own loops; it does not call the
CnP Rust stencil. Neither backend has a sieve-name or prime-specific shortcut.
The existing backedge accounting and profiler reopening guards remain active.
Typed nest/region bodies retain their existing emitter; the Boolean probe does not overlap those regions.

## Pilot and refinement

Frozen Cranelift sieve: 38.25ms retained; original CnP: 3.08ms.
Initial generated loops: 3.54ms Cranelift /3.01ms CnP.
The first reduction used selects and checked add-zero updates even for false predicates.
Moving updates to the selected branch improved Cranelift 3.50 ->2.63ms; CnP measured2.98ms.
The archived branchless and conditional binaries preserve both experiments.
Across seven sieve runs, Cranelift gateway calls fell 1,128,276 ->29,304 and root syncs 1,125,891 ->26,919.
Both backends execute the same 21,853,200 synthetic fill/reduction iterations in these retained runs.

## Validation

The Boolean-region tests now live in tests/boolean_regions.rs and exercise both backends.
Coverage includes negative/cross-zero indices, empty and partially consumed iterators, sticky exhaustion, snapshots,
invalid bounds with prior effects, BigInt overflow fallback, custom store/get/add protocols, discarded additions,
unbound loop targets, eager observation, exact backedge profiles and a no-per-element-gateway check.
The latter renews write permission before measuring; a fresh row may correctly take one ordinary store first.

All 464 tests, all-target Clippy-Dwarnings, formatting and stencil audits pass.
Raw binaries, profiles, logs, hashes and JSON stay outside Git under:
~/Library/Caches/tis-benchmarks/2026-10-31-cranelift-boolean-regions/

## Final serial comparisons

Milliseconds; each retained process executes the unchanged complete program seven times in one VM.
The first execution is separate; three additional warmups precede the final three-sample median.
Every output is checked against archived CPython. No build/test ran during timing.

|Workload|Frozen Cranelift retained|New Cranelift retained|Current CnP retained|New Cranelift first VM|New backend compilation|
|---|---:|---:|---:|---:|---:|
|sieve|39.18|2.80|3.09|11.30|7.83|
|nbody|8.90|8.87|40.35|32.53|9.92|
|knucleotide|200.68|190.85|182.82|211.56|12.05|
|ticket_search|60.93|61.13|57.17|76.13|15.93|
|barnes|558.19|560.49|3018.27|655.77|82.88|

Follow-up sieve comparisons: Cranelift 2.57/2.45ms, CnP 2.92/3.01ms, frozen Cranelift 37.69/38.04ms.
These are quick samples, not confidence intervals. The k-nucleotide improvement is not attributed to these
Boolean plans; it has no matching region and the small control changes are noisy.
Barnes retains compilation in the measured window: new Cranelift [0,8418667,0]ns, frozen [0,8653667,0]ns.
Sieve/nbody/K/ticket retained compilation deltas are zero.

The first-VM column includes common lowering/backend compilation but excludes frontend/process startup.
Backend compilation excludes common lowering. Follow-up new Cranelift backend costs were 5.50/5.58ms,
compared with CnP 0.13/0.17ms; initial process/compiler samples varied considerably.
Cranelift linked sieve machine code grew 8812 ->10320bytes.
Gateway calls fell 1,128,276 ->29,304 and root syncs 1,125,891 ->26,919 across seven runs.
CnP and new Cranelift report the same 1183 region entries and 21,853,200 combined kernel iterations.

Fresh-process sieve (no workload warmup, separate version-file preflight):
frozen Cranelift 51.40ms/9.98MiB, new Cranelift 16.61ms/10.36MiB, CnP 9.40ms/5.94MiB.
This includes process startup, frontend, common lowering, backend compilation, execution and captured output.
Thus Cranelift now wins warmed execution in these samples, while CnP still wins this cold process boundary.

## Reproduction and source state

Base commit d0f1aab; the changes remain uncommitted. Raw evidence includes hashes, source snapshot, copied workloads,
expected stdout, measure.py, both pilot variants, final binaries, Clippy/tests and all timing/RSS results.
Run measure.py from the external evidence directory to reproduce the serial retained/first/process boundaries.
Each retained binary takes SOURCE native|copy-patch EXPECTED_STDOUT.
No original Monty or Flying Circus implementation was modified.
