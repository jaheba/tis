from pathlib import Path
import json,subprocess,os
b=Path('/private/tmp/tis-full-before-after');suite=Path('/private/tmp/flying-circus-tis/src/flying_circus/workloads')
c=json.loads((b/'command.json').read_text());c=c[:c.index('--output')]+['--output',str(b/'count-followup')]+c[c.index('--output')+2:]
c+=['--samples','6','--memory-samples','0','--startup-samples','2']
# Keep only the four tis configurations; external references already ran in the full matrix.
i=0;selected=[]
while i<len(c):
 if c[i]=='-r' and not c[i+1].startswith(('before-','after-')):i+=2;continue
 selected.append(c[i]);i+=1
c=selected
for name in ['rustpython_micro_list_count_builtins_100','rustpython_micro_list_count_builtins_700','rustpython_micro_list_count_builtins_900']:c+=['--benchmark',str(suite/(name+'.py'))]
(b/'followup-command.json').write_text(json.dumps(c,indent=2)+'\n');env=os.environ.copy();env['PYTHONPATH']='/private/tmp/flying-circus-tis/src'
with (b/'followup.log').open('w') as log:p=subprocess.run(c,env=env,stdout=log,stderr=subprocess.STDOUT,cwd='/private/tmp/flying-circus-tis')
print('Follow-up exit',p.returncode)
