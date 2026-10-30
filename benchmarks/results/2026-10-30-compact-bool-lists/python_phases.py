from pathlib import Path
import sys,time,json,resource
exec(compile(Path(sys.argv[1]).read_text(),sys.argv[1],'exec'))
n=30_000_000
start=time.perf_counter_ns();flags=allocate(n);allocate_ns=time.perf_counter_ns()-start
start=time.perf_counter_ns();mark(flags,n);mark_ns=time.perf_counter_ns()-start
start=time.perf_counter_ns();primes=collect(flags,n);collect_ns=time.perf_counter_ns()-start
assert len(primes)==1857859
print(json.dumps(dict(allocate_ns=allocate_ns,mark_ns=mark_ns,collect_ns=collect_ns,count=len(primes),rss=resource.getrusage(resource.RUSAGE_SELF).ru_maxrss)))
