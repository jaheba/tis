# Single-write native image initialization experiment

Started05:43 UTC; hard cutoff07:00 UTC/08:00 Berlin. Latest retained baseline name-load-{worker,run,nbody}; root checkpoint remains there until measured retention. Backup /private/tmp/tis-profile/entry-init-before/native_runtime.rs.

Only native_runtime::execute initialization changed: instead of resize/default every Slot then overwrite Some shadow entries, push each stack-window Slot exactly once (Some boxed and rooted; None default), then resize/default raw/result/aux remainder. Explicit window<=image check before writes. Each Scratch registers Vec is logically empty on acquisition; cleanup clear remains unchanged on success/error/panic. No uninitialized memory, stale-slot reuse, unsafe, native ABI, state/rootmap or copied-storage changes. Root indices same ascending order. Direct invariant-local seeding/scalarOSR/metadata follows initialization unchanged. Both native and copy-patch use this bridge.

No new test for mechanical reversible initialization; existing full GC/deopt/OSR/regions/pool/error tests required. Fmt/alltargetClippy/debug/release/releaseexamples active session94162, entry-init-*.log. After gates freeze examples/bench,run,nbody_bench to entry-init-{worker,run,nbody}. Ten fresh warmed Barnes pairs vs name-load; retain only measured improvement, otherwise restore exact backup, retain report/evidence. Never overlap builds/tests/profiles with formal timing. Remaining~75min enough for bounded attempt and controls, reserve finalreporttime.

05:48 UTC closed/reverted: marginal0.44% with broad overlap, insufficient improvement. NativeRuntime restored byte-for-byte to name-load-before experiment. No activejobs. User body-inlining request next.
