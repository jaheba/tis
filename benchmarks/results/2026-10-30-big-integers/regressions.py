from pathlib import Path
import hashlib,json,subprocess,time,statistics
root=Path(__file__).resolve().parent
workloads=root/'workloads'
out={}
for name in ['sieve','game_nbody','barnes_hut','perf_unpack_sequence']:
    source=workloads/f'{name}.py'
    result={'source_sha256':hashlib.sha256(source.read_bytes()).hexdigest(),'samples':{}}
    expected=None
    for tier in ['bytecode','native']:
        for build in ['before','after']:
            key=f'{build}-{tier}'
            command=[str(root/'binaries'/build),'--tier',tier,str(source)]
            samples=[]
            for repeat in range(2):
                start=time.perf_counter_ns()
                p=subprocess.run(command,capture_output=True,text=True,check=True,timeout=120)
                elapsed=time.perf_counter_ns()-start
                if expected is None: expected=p.stdout
                assert p.stdout==expected,(name,key,p.stdout,expected)
                samples.append(elapsed)
            result['samples'][key]={'command':command,'elapsed_ns':samples,'median_ns':statistics.median(samples)}
    result['stdout']=expected
    out[name]=result
    (root/'regressions.json').write_text(json.dumps(out,indent=2)+'\n')
    print(name,{k:round(v['median_ns']/1e6,2) for k,v in result['samples'].items()},flush=True)
