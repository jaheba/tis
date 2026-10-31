# Overnight CnP checkpoint

Validated candidate: imports-final. Deadline 2026-10-31 07:00UTC. Work stopped at the deadline; heartbeat verified PAUSED.
No commit/push; preserve all pending CLI/inline-string/unpack and preceding optimization work.

## Validation and evidence

- 463 tests; fmt/stencil fmt, all-target Clippy-Dwarnings, strict relocation/tail/fast-call audits pass.
- CPython stdout matches every benchmark run; zero Cranelift compilations/errors under CnP.
- Source set377 files includes untracked fill.rs/reduce.rs/tail.rs. Full hashes/snapshots are external.
- Raw directory /private/tmp/tis-cnp-night-evidence; durable cache
 /Users/jaheba/Library/Caches/tis-benchmarks/overnight-copy-patch-2026-10-30.
- Report README.md and docs/lessons-learned.md record positive, rejected and inconclusive experiments.
- No original Monty/Flying Circus implementation changes.

## Current measurements

Retained CnP/Cranelift milliseconds: K179.38/188.49; nbody39.50/8.59; sieve2.93/37.13;
Barnes2946.12/561.07; ticket56.75/65.53. Quick samples; matched controls and outliers remain in report.
Only Barnes recompiled during retained measurements. Backend timers exclude common lowering; first VM/process includes it.
Both retained-process RSS and fresh-process RSS are saved with distinct boundaries.

## Latest work

ASCII short slices, invariant local imports, guarded len/sqrt stencils, and fresh-frame parameter-only imports.
60 offline templates. Fresh entry contract checked in debug; OSR/resume copies the full image.
Original VM shadow clearing/native initialization/GC maps retained.
Indexed-tail experiment rejected after alternating samples changed direction; rejected source/binaries archived.

## Closeout complete

- Stopped optimization at2026-10-31 07:00UTC /08:00Europe/Berlin.
- Heartbeat bring-copy-and-patch-closer-to-cranelift verified PAUSED.
- Imports-final source/installed binary hash-matched; all pending prior work preserved.
- 463 tests, Clippy-Dwarnings/fmt/stencil audits pass; every benchmark output matches CPython.
- Raw evidence and final manifest copied to durable cache; no commit/push.
- Targets remain partial: nbody/Barnes still lag Cranelift. No parity claim.

## Next larger work

Extract native/readonly.rs pure Plan/preflight/replay/ordered-write metadata from its Cranelift emitter.
Implement actual CnP kernels; no Cranelift delegation or hidden generic LIR interpreter.
Share numeric LoopRegion/schema/nest lowering next. Current CnP lacks those capabilities despite direct operations.
Final profiles still show heap/helper/frame/root work; compare counters and equal-work boundaries, not raw stack counts.
