from pathlib import Path
import shutil,json,hashlib,tarfile
root=Path('/Users/jaheba/p/tis');base=Path('/private/tmp/tis-full-before-after');out=root/'benchmarks/results/2026-10-30-flying-circus-before-after';out.mkdir(exist_ok=False)
for name in ['quick','bytecode-comparison','native-comparison','count-followup']:shutil.copytree(base/name,out/name)
for name in ['README.md','summary.json','failures.json','command.json','provenance.json','before-build.log','after-build.log','run.log','exit-code.json','run.py','summarize.py','pair_reports.py','followup.py','followup-command.json','followup.log','archive.py']:shutil.copy2(base/name,out/name)
bin=out/'binaries';bin.mkdir();(bin/'.gitignore').write_text('*\n!.gitignore\n')
for name in ['before-worker','after-worker']:shutil.copy2(base/name,bin/name)
shutil.copy2(root/'examples/circus.rs',out/'measured-circus.rs')
suite=Path('/private/tmp/flying-circus-tis/src/flying_circus/workloads');manifest=json.loads((suite/'manifest.json').read_text());snap=out/'workloads';snap.mkdir();shutil.copy2(suite/'manifest.json',snap/'manifest.json')
for spec in manifest.values():
 p=snap/spec['file'];p.parent.mkdir(parents=True,exist_ok=True);shutil.copy2(suite/spec['file'],p)
shutil.copy2(root/'benchmarks/results/2026-10-29-flying-circus-quick/harness-snapshot.tar.gz',out/'harness-snapshot.tar.gz')
p=out/'README.md';t=p.read_text();a=t.index('Before: e2e931d.');b=t.index('\n\nThese are rough',a)
t=t[:a]+'''Before: `e2e931d`. After: `a6ce17c`.
Both runtime sources were rebuilt with rustc 1.99.0 and the same benchmark-only worker.
Release builds use optimization level 3, no LTO and 16 codegen units.
Native enables read-only and mutation regions.

All 102 unchanged public workloads ran serially with `--quick`: three timing samples per runtime/workload/mode,
one repeated warmup, and one separate RSS sample.
Startup is measured separately.
Repeated requests use fresh VM sessions; they are not retained-VM warmed JIT timings.'''+t[b:]
t=t.replace('Ratios below1','Ratios below 1').replace('After within10%','After within 10%')
t+='''
## Follow-up and remaining gaps

The six-sample counter-test follow-up is in [count-followup/report.html](count-followup/report.html).
The initial 12–27% slowdowns on tiny list-count cases did not reproduce at that magnitude:
follow-up ratios were 0.74–0.92 for most cells and 1.01 for native repeated count-700.
This remains quick sampling, not a significance claim.
All original measurements remain in the full matrix; the summary does not replace them with follow-up values.

Both tiers now pass 71 workloads, with 19 unsupported and 12 failed cases.
The remaining failed cases are fannkuch's non-integer index path, missing frozenset in six workloads,
and custom equality in five list-count workloads.
No previously passing workload was lost in either timing mode.
The command exits nonzero because these visible failures remain, not because the matrix stopped early.
CPython and PyPy pass all 102; Monty passes 92.

Across the 53 common workloads, native unpacking improves from 2298.7 to 220.1 ms in repeated mode,
while Barnes–Hut is effectively unchanged at 584.1 versus 585.0 ms in this short run.
Do not interpret the aggregate gain as a uniform improvement to long-running numerical code.

## Reproduction

`provenance.json` records the exact revisions, binary hashes, compiler and build settings.
The before runtime is the exact parent commit with only the identical benchmark worker added for measurement.
The after source is committed unchanged; no implementation changes were made during this comparison.
Frozen workers are in `binaries/` and excluded from Git.
All original workload bytes and expected stdout are in `workloads/manifest.json` and the accompanying source files.
The harness snapshot includes the tested tis adapter; the original Flying Circus checkout was not edited.

`run.py` records the complete invocation and local paths.
To relocate the run, extract the harness snapshot, adjust its PYTHONPATH and the worker paths,
and choose a fresh output directory.
`pair_reports.py` generates tier-specific reports from the original samples without rerunning measurements.
New comparison evidence is left uncommitted; the requested implementation commit was made before benchmarking.
'''
p.write_text(t)
files=[out/'provenance.json',out/'measured-circus.rs',*bin.glob('*'),*snap.rglob('*')]
(out/'evidence-sha256.json').write_text(json.dumps({str(p.relative_to(out)):hashlib.sha256(p.read_bytes()).hexdigest() for p in files if p.is_file()},indent=2)+'\n')
print(out)
