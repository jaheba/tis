from pathlib import Path
import json
b=Path(__file__).resolve().parent
rows=[]
for mode in ['one-shot','repeated']:
 results={label:json.loads((b/'quick'/mode/(label+'.json')).read_text())['benchmarks'] for label in ['before-bytecode','after-bytecode','before-native','after-native']}
 for name in results['after-bytecode']:
  row={'mode':mode,'workload':name}
  for label,data in results.items():
   d=data[name];row[label]={'status':d['status'],'milliseconds':(d.get('median_wall_seconds') or 0)*1000,'rss_bytes':d.get('median_peak_rss_bytes')}
  for tier in ['bytecode','native']:
   old,new=row['before-'+tier],row['after-'+tier]
   row[tier+'_ratio']=new['milliseconds']/old['milliseconds'] if old['status']==new['status']=='ok' else None
  rows.append(row)
(b/'summary.json').write_text(json.dumps(rows,indent=2)+'\n')
for row in rows:print(row['mode'],row['workload'], 'BC',row['bytecode_ratio'],'native',row['native_ratio'])
