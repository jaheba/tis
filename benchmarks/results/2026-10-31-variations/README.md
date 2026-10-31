# Performance generalization probe — 2026-10-31

Equivalent source rewrites expose large performance cliffs. Names and the tested input changes are much steadier.
The runtime was frozen throughout: no compiler/runtime changes were made in response to these cases.

## Method

Generated 48 program variants/controls across 11 families: n-body, Barnes–Hut, ticket search, unpacking, JSON transform,
CSV cleanup, Chinese search text, sorting, codecs, generators and sieve. All programs pass in bytecode, copy-and-patch,
Cranelift and CPython, with exact per-program expected output. Data variants have independently computed outputs.

- Verbatim originals and AST round-trip controls separate source presentation from other changes.
  Round-trip ASTs match; fixture setup markers retain their original timing boundaries.
- Binding renaming preserves attributes, builtins and keyword parameter names. Seed 707 generates the tested names.
- Data variants change random seeds, counts, numeric inputs, logical record order, JSON amounts and Unicode names.
  Quoted CSV fields and embedded newlines remain valid.
- Structural variants extract n-body arithmetic into a helper, replace 400 repeated unpack statements with an inner loop,
  and replace sieve's inner `for range` with an equivalent `while` loop.
- A Barnes–Hut variant renames stored attributes and reverses the two Point constructor stores.

CPython verifies equal output for round trips, renamings and structural/layout variants. Both timing modes include
compilation, profiling and execution in a fresh VM session; reused workers do not retain a warmed VM/JIT.

The broad 42-program pilot used default tier flags. A 24-program follow-up enabled `--readonly-regions --mutation-regions`
for both native backends and added sieve/layout cases. Both used serial Flying Circus `--quick`, three timing samples,
one repeated warmup, five startup samples and one RSS sample. The six structural/control cases then ran with ten timing
samples, two startup samples, one warmup and no RSS sampling. Builds/tests did not overlap any samples.

Normalization is `(tis_variant / tis_control) / (CPython_variant / CPython_control)`. A value above one indicates lost
relative performance after accounting for CPython's change in work/cost. It does not isolate compilation from execution.

## Confirmed structural cliffs

Ten-sample medians, repeated workers with fresh VM sessions; native region flags enabled:

| Change | Tier | Control ms | Variant ms | Raw ratio | CPython-normalized ratio |
| --- | --- | ---: | ---: | ---: | ---: |
| n-body helper extraction | Bytecode | 217.55 | 249.69 | 1.15 | 1.10 |
| n-body helper extraction | CnP | 43.42 | 86.12 | 1.98 | 1.90 |
| n-body helper extraction | Cranelift | 30.99 | 266.40 | 8.60 | 8.24 |
| sieve inner `for` → `while` | Bytecode | 164.27 | 184.67 | 1.12 | 0.89 |
| sieve inner `for` → `while` | CnP | 3.75 | 186.16 | 49.70 | 39.27 |
| sieve inner `for` → `while` | Cranelift | 8.36 | 124.23 | 14.86 | 11.74 |
| unpack statements → loop | Bytecode | 44.82 | 48.75 | 1.09 | 1.09 |
| unpack statements → loop | CnP | 151.15 | 171.16 | 1.13 | 1.14 |
| unpack statements → loop | Cranelift | 151.94 | 65.71 | 0.43 | 0.43 |

One-shot results reproduce the direction and scale: sieve CnP 46×, Cranelift 13.8×; helper extraction CnP 1.97×,
Cranelift 8.40×. Cranelift improves on looped unpacking, so source changes do not uniformly make results worse.

## Names, inputs and layouts

Across the ten-family repeated pilot, median CPython-normalized naming ratios are 0.998 bytecode, 1.006 CnP and 0.996
Cranelift. Corresponding input-variation medians are 1.016, 1.016 and 1.024. These three-sample differences are inconclusive.
Barnes–Hut field renaming/store reordering remains near its control in the follow-up; it does not show a structural cliff.

The pilot includes noisy one-shot outliers: renamed sorting appeared 2.15× worse for Cranelift after normalization,
and renamed CSV 1.28× worse. Those large shifts do not recur in repeated mode. They remain unconfirmed;
this experiment does not establish performance invariance for all names, data, APIs or sizes.

## Diagnostics

Separate single CLI executions collected counters, outside timing samples:

| Change | Backend | Runtime crossings: control → variant |
| --- | --- | ---: |
| n-body helper | CnP | 39,798 → 437,638 |
| n-body helper | Cranelift | 119,359 → 2,307,479 |
| sieve `while` | CnP | 3,841 → 4,245,181 |
| sieve `while` | Cranelift | 4,176 → 2,124,260 |

Cranelift records 198,920 successful body inlines for the helper variant. Arithmetic executes in generated code,
but `native_runtime::inline_preflight` still crosses into Rust and flushes region state. Native root synchronizations rise
from 39,786 to 258,599, while region iterations fall from 298,385 to 99,465. Thus existing arithmetic inlining does not
preserve the original loop fusion. Enabling experimental CFG inlining in diagnostic runs does not change these counters.
All diagnostic runs report zero compilation errors. CLI process times are not used in the comparison tables;
the first CLI invocation had a large startup outlier.

Sieve backend compilation remains small: approximately 0.15 ms for CnP, and 5–6 ms for Cranelift. The while rewrite
loses optimized loop coverage and incurs millions of crossings. For unpacking, fresh-session profiling/lowering/compilation
and static code size remain relevant; these results alone do not identify the dominant phase.

The next general improvements are shared induction-loop normalization and retaining native regions across guarded,
inlined helper calls. Adding name-specific cases or tuning the tested constants would not address these gaps.

## Reproduction and evidence

Raw sources, exact output references, samples, reports, frozen binaries and commands are outside Git:

`~/Library/Caches/tis-benchmarks/2026-10-31-variations/`

`results/`, `focus-results/` and `confirm-results/` contain the three runs. `diagnostics.json` retains counters and raw
CLI output. `run.py`, `focus.py` and `confirm.py` record the exact driver logic; command JSON files retain all flags.
The worker SHA256 is `c93e315fde057ca446b234f2aeff9a0bf17ec452aeaead35e376f7c4095bd0af`, from the reviewed all120 build.
Its compiler/source provenance remains in the neighboring `2026-10-31-all120` evidence archive. CPython is 3.15.0.

```sh
python3 benchmarks/results/2026-10-31-variations/generate.py \
  --workloads "$HOME/Library/Caches/tis-benchmarks/2026-10-31-all120/harness/src/flying_circus/workloads" \
  --output /tmp/tis-heldout-707 --seed 707
python3 benchmarks/results/2026-10-31-variations/analyze.py \
  "$HOME/Library/Caches/tis-benchmarks/2026-10-31-variations/confirm-results"
```

Default-seed regeneration was checked byte-for-byte against the timed programs. Use fresh seeds for future holdouts;
only seed 707 was timed here. Keep fixed cases for regression coverage and new generated cases for generalization checks.
