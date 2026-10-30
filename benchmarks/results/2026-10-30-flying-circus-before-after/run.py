from pathlib import Path
import subprocess,os,json,hashlib,platform,shutil
root=Path('/Users/jaheba/p/tis');base=Path('/private/tmp/tis-full-before-after');frozen=root/'benchmarks/results/overnight-json-bisect-csv/binaries'
command=['/Users/jaheba/.local/share/uv/tools/flying-circus/bin/python','-m','flying_circus','--quick','-v','-x1','--diff',
 '-r','before-bytecode='+str(base/'before-worker'),'-r','after-bytecode='+str(base/'after-worker'),
 '-r','before-native='+str(base/'before-worker'),'-r','after-native='+str(base/'after-worker'),
 '-r','monty='+str(frozen/'monty'),'-r','cpython=/opt/homebrew/Cellar/python@3.15/3.15.0/Frameworks/Python.framework/Versions/3.15/bin/python3.15',
 '-r','pypy=/opt/homebrew/Cellar/pypy3.10/7.3.16/bin/pypy3',
 '--format','html','--format','markdown','--format','json','--output',str(base/'quick')]
for label,tier in [('before-bytecode','bytecode'),('after-bytecode','bytecode'),('before-native','native'),('after-native','native')]:
 for arg in ['--tier',tier]+(['--readonly-regions','--mutation-regions'] if tier=='native' else []):command+=['--runtime-arg',label+'='+arg]
(base/'command.json').write_text(json.dumps(command,indent=2)+'\n')
provenance={'before_revision':subprocess.check_output(['git','rev-parse','HEAD^'],cwd=root,text=True).strip(),'after_revision':subprocess.check_output(['git','rev-parse','HEAD'],cwd=root,text=True).strip(),'before_note':'Exact parent runtime source plus identical benchmark-only examples/circus.rs from after; no runtime source changes.','worker_source_sha256':hashlib.sha256((root/'examples/circus.rs').read_bytes()).hexdigest(),'binaries':{name:hashlib.sha256((base/(name+'-worker')).read_bytes()).hexdigest() for name in ['before','after']},'rustc':subprocess.check_output(['rustc','-Vv'],text=True),'environment':{k:os.environ.get(k) for k in ['RUSTFLAGS','CARGO_ENCODED_RUSTFLAGS','CARGO_PROFILE_RELEASE_LTO','CARGO_PROFILE_RELEASE_CODEGEN_UNITS']},'build_command':'CARGO_TARGET_DIR=/private/tmp/tis-leaf-isolated-target cargo build --release --offline --locked --manifest-path SNAPSHOT/Cargo.toml --example circus','release_profile':{'opt-level':3,'lto':False,'codegen-units':16},'full_manifest_sha256':hashlib.sha256(Path('/private/tmp/flying-circus-tis/src/flying_circus/workloads/manifest.json').read_bytes()).hexdigest(),'platform':platform.platform(),'sampling':'--quick serial:3 timings,5 startup,1 repeated warmup,1 RSS; fewer than10 timing samples are inconclusive for significance.'}
(base/'provenance.json').write_text(json.dumps(provenance,indent=2)+'\n')
env=os.environ.copy();env['PYTHONPATH']='/private/tmp/flying-circus-tis/src'
with (base/'run.log').open('w') as log:result=subprocess.run(command,env=env,stdout=log,stderr=subprocess.STDOUT,cwd='/private/tmp/flying-circus-tis')
(base/'exit-code.json').write_text(json.dumps({'exit_code':result.returncode})+'\n');print('Full-suite comparison exit',result.returncode)
