from pathlib import Path
import json,hashlib,tarfile,shutil
r=Path('/Users/jaheba/p/tis');o=r/'benchmarks/results/overnight-json-bisect-csv'
p=o/'candidate20-all-source-sha256.json';manifest=json.loads(p.read_text())
for f in [r/'build.rs',*(r/'examples').rglob('*.rs')]:manifest[str(f.relative_to(r))]=hashlib.sha256(f.read_bytes()).hexdigest()
p.write_text(json.dumps(manifest,indent=2)+'\n')
assert all(hashlib.sha256((r/n).read_bytes()).hexdigest()==h for n,h in manifest.items())
with tarfile.open(o/'candidate20-source.tar.gz','w:gz') as archive:
 for name in ['Cargo.toml','Cargo.lock','build.rs','src','examples','tests','experiments','AGENTS.md']:archive.add(r/name,arcname=name)
(o/'REPRODUCE.md').write_text('''# Reproducing the final comparison

Frozen release executables are in binaries/. candidate20-source.tar.gz contains the exact Rust sources, Cargo files, build script, examples and regression fixtures. candidate20-all-source-sha256.json covers the build script, Rust sources, examples, tests and Cargo files. candidate20-build-provenance.json records hashes, compiler, flags and runtime configuration. Monty compiler flags remain unverified; its frozen binary hash is recorded. The archived CPython executable requires its Homebrew framework and standard library; use the pinned Cellar path in provenance.

The harness snapshot is ../2026-10-29-flying-circus-quick/harness-snapshot.tar.gz. Extract it into /private/tmp/flying-circus-tis (the archive contains src/flying_circus). The existing Flying Circus Python environment supplies its dependencies. The original harness checkout was not edited.

Run overnight_fc_final.py with a fresh label beginning with final and the absolute path to binaries/tis-candidate20-worker. It runs --quick serially, overridden to12 timing samples and3 startup samples, with the14 unchanged source snapshots in workloads/. Both modes include compilation, execution and output delivery. Change the root and pinned interpreter paths in the script only if relocating this evidence. Final raw commands are in overnight-final20-command.json.

For retained-VM execution, build examples/retained_benchmark.rs and run the archived overnight_retained20.py after adjusting its binary path to binaries/tis-candidate20-retained. This is a separate measurement boundary. The28 retained results and CPython reference stdout are in candidate20-retained/. Sampling-profiler runs are separate and their perturbed timings are excluded from comparisons.
''')
notes='''## Final profiling findings

Fresh diagnostic CSV aggregate medium executes4,471,128 runtime helpers and695,394 root synchronizations; cleanup medium executes9,521,799 helpers and1,604,592 root synchronizations. Quiet native blocks already avoid callbacks, so removing a presumed callback on every block would not address the actual implementation.

One-second retained-VM CPU samples confirm the remaining costs. Bytecode aggregate has312 top-of-stack samples in execute and74 in record_block_and_phase, followed by heap access/GC/iteration. Native cleanup has79 in Context::step,57 in collection,33 in Heap::get,32 in GC child tracing,28 in UTF8 validation,27 in guarded_helper_callback and24 in Slot::value. These are leaf sample counts, not percentages or exhaustive native attribution; unsymbolized generated code is also present. Profiles and commands are in candidate20-profile/ and overnight_profile20.py. Initial sandboxed sampling failed; the authorized unsandboxed retry succeeded for both processes.

The next substantive work should reduce native helper/root traffic and repeated Slot-to-Value string validation, and batch frozen bytecode block bookkeeping while retaining observation windows and exact guard/deopt state. Small CSV inputs also spend about5.6ms compiling native code: tier policy/cheaper initial native compilation matters separately from steady execution. No speculative last-minute code changes were installed from these observations.

Final source and installed CLI hashes were verified; the installed CLI exactly matches binaries/tis-candidate20.376 tests, all-target Clippy, formatting and diff whitespace checks pass. All14 unchanged workloads and all28 retained tier/workload checks pass. Targets remain incomplete:9/14 bytecode and2/14 native-mode workloads satisfy both timing modes at measured medians. The12-sample follow-up and1 RSS sample are preserved without selecting only favorable measurements. No commits or pushes.
'''
notes=notes.replace('executes4','executes 4').replace('and695','and 695').replace('executes9','executes 9').replace('and1,','and 1,').replace('has312','has 312').replace('and74','and 74').replace('has79','has 79').replace(',57',', 57').replace(',33',', 33').replace(',32',', 32').replace(',28',', 28').replace(',27',', 27').replace('and24','and 24').replace('about5.6','about 5.6').replace('.376','. 376').replace('All14','All 14').replace('all28','all 28').replace(':9/14',': 9/14').replace('and2/14','and 2/14').replace('The12','The 12').replace('and1 RSS','and 1 RSS')
with (o/'FINAL.md').open('a') as f:f.write('\n'+notes)
with (o/'PROGRESS.md').open('a') as f:f.write('\n'+notes)
shutil.copy2('/private/tmp/overnight_finalize20.py',o/'overnight_finalize20.py')
print('Final archive and profile findings saved; all source hashes verified.')
