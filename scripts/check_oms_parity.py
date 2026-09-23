#!/usr/bin/env python3
"""Compare stepwise result/state/journal digests from actual Rust and C++ OMS."""
import argparse,collections,hashlib,json,subprocess
from pathlib import Path
root=Path(__file__).resolve().parents[1]
p=argparse.ArgumentParser();p.add_argument('--cpp',default='cpp/build/oms_parity');p.add_argument('--rust',default='target/parity/examples/oms_parity');p.add_argument('--output');a=p.parse_args()
trace=root/'tests/data/oms-parity.trace'
commands=[x for x in trace.read_text().splitlines() if x and not x.startswith('#')]
result={'trace_sha256':hashlib.sha256(trace.read_bytes()).hexdigest(),'steps':len(commands),'backends':{}}
for backend in ['standard','pages']:
    outputs={}
    for language,binary in [('rust',a.rust),('cpp',a.cpp)]:
        proc=subprocess.run([str(root/binary),'trace',backend,str(trace)],text=True,capture_output=True,check=True,cwd=root)
        outputs[language]=proc.stdout.splitlines()
    if len(outputs['rust'])!=len(commands) or len(outputs['cpp'])!=len(commands):raise SystemExit('Trace was not fully processed')
    for step,(r,c) in enumerate(zip(outputs['rust'],outputs['cpp'])):
        if r!=c:raise SystemExit(f'{backend} mismatch at step {step}: {commands[step]}\nRust {r}\nC++  {c}')
    result['backends'][backend]={'matched':True,'output_sha256':hashlib.sha256('\n'.join(outputs['rust']).encode()).hexdigest(),'outcomes':dict(collections.Counter(x.split()[1] for x in outputs['rust']))}
    print(backend,len(commands),'steps match')
if a.output:Path(a.output).write_text(json.dumps(result,indent=2)+'\n')
else:print(json.dumps(result,indent=2))
