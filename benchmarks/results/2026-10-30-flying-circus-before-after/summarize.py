from pathlib import Path
import json,math,statistics,collections
base=Path('/private/tmp/tis-full-before-after');r=base/'quick'
runtimes=['before-bytecode','after-bytecode','before-native','after-native','monty','cpython','pypy'];modes=['one-shot','repeated']
data={m:{t:json.loads((r/m/(t+'.json')).read_text())['benchmarks'] for t in runtimes} for m in modes};names=list(data['one-shot']['before-bytecode'])
summary={'workloads':len(names),'status_counts':{},'tiers':{}}
lines=['# Full Flying Circus before / after','', 'Before: e2e931d. After: a6ce17c. Both runtime sources were rebuilt with the same rustc, release defaults and benchmark-only worker. Native enables read-only and mutation regions. The full unchanged public manifest ran serially with --quick:3 timing samples per runtime/workload/mode,1 repeated warmup,1 separate RSS sample. Startup is measured separately. Repeated requests use fresh VM sessions; they are not retained-VM warmed JIT timings.','', 'These are rough comparisons. Three samples cannot establish statistical significance, especially for tiny workloads and PyPy with one warmup. Unsupported/error/mismatch cases remain visible and are excluded from paired timing aggregates.','', '| Runtime | One-shot ok / unsupported / failed | Repeated ok / unsupported / failed |','| --- | ---: | ---: |']
for t in runtimes:
 summary['status_counts'][t]={m:dict(collections.Counter(x['status'] for x in data[m][t].values())) for m in modes}
 cells=[' / '.join(str(summary['status_counts'][t][m].get(s,0)) for s in ['ok','unsupported','failed']) for m in modes];lines.append('| '+t+' | '+' | '.join(cells)+' |')
lines+=['','Ratios below1 favor after. Geometric means use only workloads passing in both versions for the given tier/mode; each workload has equal weight. Newly supported programs have no before/after speed ratio.','', '| Tier | One-shot common cases / geometric mean after÷before | Repeated common cases / geometric mean after÷before | RSS common cases / geometric mean after÷before |','| --- | ---: | ---: | ---: |']
for tier in ['bytecode','native']:
 before='before-'+tier;after='after-'+tier;entry={'modes':{}}
 for mode in modes:
  common=[n for n in names if data[mode][before][n]['status']=='ok' and data[mode][after][n]['status']=='ok']
  ratios={n:data[mode][after][n]['median_wall_seconds']/data[mode][before][n]['median_wall_seconds'] for n in common}
  new=[n for n in names if data[mode][before][n]['status']!='ok' and data[mode][after][n]['status']=='ok'];lost=[n for n in names if data[mode][before][n]['status']=='ok' and data[mode][after][n]['status']!='ok']
  entry['modes'][mode]={'common':len(common),'geometric_mean':math.exp(statistics.mean(math.log(x) for x in ratios.values())) if ratios else None,'ratios':ratios,'newly_passing':new,'lost_passes':lost}
 mem={n:data['one-shot'][after][n]['median_peak_rss_bytes']/data['one-shot'][before][n]['median_peak_rss_bytes'] for n in names if data['one-shot'][after][n]['status']=='ok' and data['one-shot'][before][n]['status']=='ok' and data['one-shot'][after][n].get('median_peak_rss_bytes') and data['one-shot'][before][n].get('median_peak_rss_bytes')}
 entry['rss']={'common':len(mem),'ratios':mem,'geometric_mean':math.exp(statistics.mean(math.log(x) for x in mem.values())) if mem else None};summary['tiers'][tier]=entry
 cells=[f"{entry['modes'][m]['common']} / {entry['modes'][m]['geometric_mean']:.3f}" for m in modes]+[f"{len(mem)} / {entry['rss']['geometric_mean']:.3f}"]
 lines.append('| '+tier+' | '+' | '.join(cells)+' |')
for tier,e in summary['tiers'].items():
 lines+=['',f'## {tier.capitalize()} support changes','']
 for mode in modes:
  a=e['modes'][mode];lines.append(mode+': newly passing: '+(', '.join(a['newly_passing']) or 'none')+'. Lost passing cases: '+(', '.join(a['lost_passes']) or 'none')+'.')
 lines+=['',f'## {tier.capitalize()} largest repeated changes','', '| Workload | Before ms | After ms | After÷before |','| --- | ---: | ---: | ---: |']
 ordered=sorted(e['modes']['repeated']['ratios'],key=e['modes']['repeated']['ratios'].get);selected=list(dict.fromkeys(ordered[:8]+list(reversed(ordered[-8:]))))
 for n in selected:
  a=data['repeated']['before-'+tier][n];b=data['repeated']['after-'+tier][n];lines.append(f"| {n} | {a['median_wall_seconds']*1000:.3f} | {b['median_wall_seconds']*1000:.3f} | {e['modes']['repeated']['ratios'][n]:.3f} |")
lines+=['','## Complete paired results','', 'Each timing cell is one-shot / repeated after÷before. n/a means a version did not finish with correct output and all required samples. RSS is before→after MiB. Errors and compatibility details are retained in failures.json and the full report.','', '| Workload | BC ratio | Native ratio | BC RSS MiB | Native RSS MiB |','| --- | ---: | ---: | ---: | ---: |']
for n in names:
 cells=[]
 for tier in ['bytecode','native']:
  cells.append(' / '.join(f"{summary['tiers'][tier]['modes'][m]['ratios'][n]:.3f}" if n in summary['tiers'][tier]['modes'][m]['ratios'] else 'n/a' for m in modes))
 for tier in ['bytecode','native']:
  vals=[data['one-shot'][x+'-'+tier][n].get('median_peak_rss_bytes') for x in ['before','after']];cells.append(' → '.join(f'{v/1048576:.1f}' if v else 'n/a' for v in vals))
 lines.append('| '+n+' | '+' | '.join(cells)+' |')
summary['reference_comparisons']={}
lines+=['', '## After versus reference runtimes', '', '| Comparison | Mode | Correct common workloads | Geometric mean ratio | After faster | After within10% |', '| --- | --- | ---: | ---: | ---: | ---: |']
for a,b in [('after-bytecode','monty'),('after-native','cpython'),('after-native','pypy')]:
 key=a+'/'+b;summary['reference_comparisons'][key]={}
 for mode in modes:
  common=[n for n in names if data[mode][a][n]['status']==data[mode][b][n]['status']=='ok']
  ratios=[data[mode][a][n]['median_wall_seconds']/data[mode][b][n]['median_wall_seconds'] for n in common]
  value={'common':len(common),'geometric_mean':math.exp(statistics.mean(math.log(x) for x in ratios)),'after_faster':sum(x<1 for x in ratios),'within_10_percent':sum(x<=1.10 for x in ratios)}
  summary['reference_comparisons'][key][mode]=value
  lines.append(f"| {key} | {mode} | {value['common']} | {value['geometric_mean']:.3f} | {value['after_faster']} | {value['within_10_percent']} |")
lines+=['', '[Full seven-runtime report](quick/report.html) · [Bytecode comparison](bytecode-comparison/report.html) · [Native comparison](native-comparison/report.html) · [All failure details](failures.json) · [Build provenance](provenance.json)', '']
failures={m:{t:{n:{k:v for k,v in b.items() if k in ['status','errors','compatibility']} for n,b in d.items() if b['status']!='ok'} for t,d in ds.items()} for m,ds in data.items()}
(base/'summary.json').write_text(json.dumps(summary,indent=2)+'\n');(base/'failures.json').write_text(json.dumps(failures,indent=2)+'\n');(base/'README.md').write_text('\n'.join(lines)+'\n')
print(json.dumps({k:v for k,v in summary.items() if k!='tiers'},indent=2))
for tier,e in summary['tiers'].items():print(tier,{m:{k:v for k,v in e['modes'][m].items() if k!='ratios'} for m in modes})
