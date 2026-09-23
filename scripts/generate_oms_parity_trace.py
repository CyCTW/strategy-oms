#!/usr/bin/env python3
"""Deterministic input shared verbatim by C++ and the actual Rust Engine."""
from pathlib import Path
root=Path(__file__).resolve().parents[1]
lines=[]
def add(*xs): lines.extend(xs)
def reset(orders=128,req=2048,execution=1024,reports=4096,qty=100,book=500,journal=10000):
    add(f'RESET {orders} {req} {execution} {reports} {qty} {book} {journal}')
reset()
add('NEW 1 1 1 0 100 10','REPLACE 1 2 -1 101 10','REPLACE 1 3 -1 102 10',
    'RAW 1 1 1 FILL 1 2 100','REPORT 1 1 2 ACCEPT 1 101 100 10','DISPATCH',
    'REPORT 1 1 3 REPLACED 3 101 102 10','REPORT 1 1 3 REPLACED 3 101 102 10',
    'REPORT 1 2 1 REPLACED 3 101 102 10','RAW 1 2 2 REPLACED 3 101 103 10',
    'REPLACE 1 4 -1 102 10','REPLACE 1 4 -1 102 10','REPLACE 1 5 0 103 10',
    'RAW 1 1 4 CORRECT 1 1 1 100','RAW 1 1 5 CORRECT 1 1 1 100',
    'RAW 1 1 6 FILL 1 2 100','REPLACE 1 6 -1 103 10','DISPATCH',
    'REPORT 1 1 7 RECONCILE 102 10 1 9 1','DISPATCH','REPORT 1 1 8 REPLACED 6 101 103 10',
    'CANCEL 1 7 -1','REPLACE 1 8 -1 104 10','CANCEL 1 9 -1',
    'REPORT 1 1 9 REASON 7 1','DISPATCH','REPORT 1 1 10 CANCELED 9','DISPATCH','RECOVER')
# Deferred risk is checked against current Book exposure; cancel priority.
reset(book=20)
add('NEW 1 1 1 0 100 10','NEW 2 2 1 0 101 10','REPLACE 1 3 -1 102 15',
    'CANCEL 2 4 -1','REPORT 1 1 1 ACCEPT 1 101 100 10','REPORT 2 1 2 ACCEPT 2 102 101 10',
    'DISPATCH','REPORT 2 1 3 CANCELED 4','DISPATCH','REPORT 1 1 4 REPLACED 3 101 102 15',
    'REPLACE 1 5 -1 103 16','REPORT 1 1 5 REPLACED 5 101 103 30',
    'REPLACE 1 6 -1 104 29','REPORT 1 1 6 REPLACED 6 101 104 29','NEW 3 7 1 0 100 1',
    'CANCEL 1 8 -1','REPORT 1 1 7 CANCELED 8')
# Invalid guarded ingress quarantines all open orders, no automatic resend.
reset()
add('NEW 1 1 1 0 -65 10','NEW 2 2 2 1 9223372036854775807 10',
    'REPORT 1 1 1 ACCEPT 1 101 -65 10','REPORT 2 1 2 ACCEPT 2 102 9223372036854775807 10',
    'REPORT 1 1 4 FILL 1 1 -65','REPLACE 1 3 -1 -64 10','DISPATCH','RECOVER',
    'REPORT 1 1 3 RECONCILE -65 10 0 10 1','DISPATCH','TIMEOUT 1 3','HOLD',
    'REPORT 1 1 4 REPLACED 3 101 -64 10','REPORT 1 1 5 RECONCILE -64 10 0 10 1',
    'REPORT 2 1 6 RECONCILE 9223372036854775807 10 0 10 1','RECOVER')
# Queued replace becomes unexecutable after a fill; unsolicited cancel resolves pending.
reset()
add('NEW 1 1 1 0 -9223372036854775808 10','REPORT 1 1 1 ACCEPT 1 101 -9223372036854775808 10',
    'REPLACE 1 2 -1 -1 10','REPLACE 1 3 -1 0 5','REPORT 1 1 2 FILL 1 6 -1',
    'REPORT 1 1 3 REPLACED 2 101 -1 10','DISPATCH','CANCEL 1 4 -1',
    'REPORT 1 1 4 CANCELED 0','RAW 1 1 5 CANCELED 4','REPORT 1 1 5 FILL 2 1 -1',
    'RAW 1 1 6 CORRECT 2 1 0 -1','REPORT 1 1 7 RECONCILE -1 10 6 0 3')
# Resource limits and append-failure ordering.
reset(journal=1)
add('NEW 1 1 1 0 100 10','REPORT 1 1 1 ACCEPT 1 101 100 10','CANCEL 1 2 -1','RECOVER')
reset(req=1)
add('NEW 1 1 1 0 100 10','REPORT 1 1 1 ACCEPT 1 101 100 10','REPLACE 1 2 -1 101 10')
reset(reports=1)
add('NEW 1 1 1 0 100 10','REPORT 1 1 1 ACCEPT 1 101 100 10','RAW 1 1 2 FILL 1 1 100','REPORT 1 1 1 ACCEPT 1 101 100 10')
reset(execution=1)
add('NEW 1 1 1 0 100 10','REPORT 1 1 1 ACCEPT 1 101 100 10','RAW 1 1 2 FILL 1 1 100','RAW 1 1 3 FILL 2 1 100')
reset(orders=1)
add('NEW 1 1 1 0 100 10','NEW 2 2 1 0 100 10','NEW 0 3 1 0 100 10','NEW 1 0 1 0 100 10','TIMEOUT 999 1')
# New rejection, full fill before ACK, rejection after fills, expiry and bad correction.
reset()
add('NEW 1 1 1 0 100 10','REPORT 1 1 1 REJECT 1','RAW 1 1 2 FILL 1 1 100',
    'NEW 2 2 1 0 100 10','REPORT 2 1 2 FILL 2 10 100','RAW 2 1 3 REJECT 2',
    'REPORT 2 1 3 ACCEPT 2 102 100 10','RAW 2 1 4 CORRECT 2 2 0 100',
    'REPORT 2 1 4 CORRECT 2 1 0 100','REPORT 2 1 5 RECONCILE 100 10 0 10 1',
    'REPLACE 2 3 -1 101 10','REPORT 2 1 6 EXPIRED','DISPATCH')
# Replay 80 complete randomized-price lifecycles across eight Books and both sides.
reset()
seq=0;req=0;seed=19
for id in range(1,81):
    seed=(seed*6364136223846793005+1)&((1<<64)-1)
    p=(seed>>32)%1000000-500000
    base=req+1;req+=6
    add(f'NEW {id} {base} {id%4+1} {id%2} {p} 50')
    seq+=1;add(f'REPORT {id} 1 {seq} ACCEPT {base} {id+100} {p} 50')
    add(f'REPLACE {id} {base+1} -1 {p+1} 50',f'REPLACE {id} {base+2} -1 {p+2} 45',f'REPLACE {id} {base+3} -1 {p+3} 40')
    seq+=1;add(f'REPORT {id} 1 {seq} FILL {id} 2 {p}')
    seq+=1;add(f'REPORT {id} 1 {seq} REPLACED {base+1} {id+100} {p+1} 50','DISPATCH')
    seq+=1;add(f'REPORT {id} 1 {seq} REPLACED {base+3} {id+100} {p+3} 40')
    if id%5==0:
        add('RECOVER');seq+=1;add(f'REPORT {id} 1 {seq} RECONCILE {p+3} 40 2 38 1')
    add(f'CANCEL {id} {base+4} -1',f'CANCEL {id} {base+5} -1')
    seq+=1;add(f'REPORT {id} 1 {seq} CANCELED {base+4}','DISPATCH')
    if id%7==0:add(f'REPORT {id} 1 {seq} CANCELED {base+4}')
path=root/'tests/data/oms-parity.trace';path.parent.mkdir(exist_ok=True)
path.write_text('# C++ / Rust single-order semantic parity trace v1\n'+'\n'.join(lines)+'\n')
print(path, len(lines), 'steps')
