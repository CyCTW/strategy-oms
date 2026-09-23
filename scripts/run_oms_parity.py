#!/usr/bin/env python3
"""Serial, alternating, functionally matched Rust/C++ single-order benchmark."""
import argparse,csv,datetime,hashlib,io,json,os,platform,subprocess
from pathlib import Path
root=Path(__file__).resolve().parents[1]
p=argparse.ArgumentParser();p.add_argument('--samples',type=int,default=20000);p.add_argument('--rounds',type=int,default=6);p.add_argument('--tag',default=datetime.date.today().isoformat());a=p.parse_args()
if not(0<a.samples<=200000 and a.rounds>0):p.error('invalid benchmark workload size')
if any(c not in 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_' for c in a.tag):p.error('invalid tag')
out=root/f'docs/results/oms-parity-time-{a.tag}.csv';meta=root/f'docs/results/oms-parity-metadata-{a.tag}.json'
if out.exists() or meta.exists():raise SystemExit('Use a fresh tag; existing results will not be overwritten')
def call(cmd):return subprocess.check_output(cmd,cwd=root,text=True).strip()
cache=(root/'cpp/build/CMakeCache.txt').read_text()
if 'CMAKE_BUILD_TYPE:STRING=Release' not in cache or 'OMS_SANITIZE:BOOL=ON' in cache:raise SystemExit('C++ must be Release, no sanitizer')
compiler=next(x.split('=',1)[1] for x in cache.splitlines() if x.startswith('CMAKE_CXX_COMPILER:'))
binaries={'rust':root/'target/parity/examples/oms_parity','cpp':root/'cpp/build/oms_parity'}
sources=sorted((root/'src').glob('*.rs'))+sorted((root/'cpp/include').glob('*.hpp'))+sorted((root/'cpp/parity').glob('*'))+[root/'examples/oms_parity.rs',root/'examples/parity/codec.rs',root/'Cargo.toml',root/'cpp/CMakeLists.txt',root/'scripts/run_oms_parity.py',root/'tests/data/oms-parity.trace']
metadata={'started_at':datetime.datetime.now().astimezone().isoformat(),'platform':platform.platform(),'machine':platform.machine(),'samples':a.samples,'rounds':a.rounds,'rustc':call(['rustc','-vV']),'clang':call([compiler,'--version']),'rust_profile':'parity: release opt-level=3, lto=false, codegen-units=1; default unwinding; no target-cpu override','cpp_flags':(root/'cpp/build/CMakeFiles/oms_parity.dir/flags.make').read_text(),'cpp_link':(root/'cpp/build/CMakeFiles/oms_parity.dir/link.txt').read_text(),'abseil_commit':call(['git','-C','cpp/third_party/abseil-cpp','rev-parse','HEAD']),'source_sha256':{str(x.relative_to(root)):hashlib.sha256(x.read_bytes()).hexdigest() for x in sources},'binary_sha256':{k:hashlib.sha256(v.read_bytes()).hexdigest() for k,v in binaries.items()},'sizes':{k:call([str(v),'sizes']) for k,v in binaries.items()},'digests':[]}
# Gate timing on both complete stepwise parity backends, not just a checksum at end.
subprocess.run(['python3','scripts/check_oms_parity.py'],cwd=root,check=True,stdout=subprocess.DEVNULL)
with out.open('x',newline='') as stream:
    writer=None
    for r in range(a.rounds):
        for backend in (['standard','pages'] if r%2==0 else ['pages','standard']):
            observed={}
            for lang in (['rust','cpp'] if r%2==0 else ['cpp','rust']):
                print(f'round {r+1}/{a.rounds} {backend} {lang}',flush=True)
                proc=subprocess.run([str(binaries[lang]),'bench',backend,str(a.samples)],cwd=root,text=True,capture_output=True,check=True)
                data=list(csv.DictReader(io.StringIO(proc.stdout)))
                if len(data)!=36:raise SystemExit(f'Unexpected metric count: {len(data)}')
                for row in data:
                    row['round']=str(r)
                    if writer is None:writer=csv.DictWriter(stream,fieldnames=list(row));writer.writeheader()
                    writer.writerow(row)
                stream.flush()
                observed[lang]=[s for s in proc.stderr.splitlines() if s.startswith('DIGEST ')]
                if len(observed[lang])!=5:raise SystemExit('Missing workload digest')
            if observed['rust']!=observed['cpp']:raise SystemExit(f'Benchmark semantic divergence {backend}: {observed}')
            metadata['digests'].append({'round':r,'backend':backend,'matched':True,'values':observed['rust']})
metadata['completed_at']=datetime.datetime.now().astimezone().isoformat();meta.write_text(json.dumps(metadata,indent=2)+'\n');print(meta)
