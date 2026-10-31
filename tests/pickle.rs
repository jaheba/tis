use tis::{Tiering, Vm, compile};
#[test]
fn binary_pickle_preserves_aliases_cycles_dates_and_large_integers() {
    for tier in [
        Tiering::Bytecode,
        Tiering::Lir,
        Tiering::Native,
        Tiering::CopyPatch,
    ] {
        let mut vm = Vm::new();
        vm.tiering = tier;
        vm.run_program(&mut compile(r#"
import pickle
import datetime
date=datetime.date(1980,5,7)
assert date.year==1980 and date.month==5 and date.day==7
assert type(date) is datetime.date and isinstance(date,datetime.date)
assert str(date)=='1980-05-07'
assert repr(date)=='datetime.date(1980, 5, 7)'
assert date < datetime.date(2000,1,1)
shared=[42]
value={'date':date,'text':'hé😀','bytes':b'\x00\xff','items':[shared,shared], 'tuple':(True,None,-5,2**100,1.25)}
for protocol in [4,5]:
    result=pickle.loads(pickle.dumps(value,protocol))
    assert result==value
    assert result['items'][0] is result['items'][1]
cycle=[]
cycle.append(cycle)
restored=pickle.loads(pickle.dumps(cycle))
assert restored[0] is restored
assert dict.fromkeys([1,2,1],42)=={1:42,2:42}
sparse=pickle.loads(b'\x80\x04]q\x05]\x94\x86h\x01\x86.')
assert sparse[0][1] is sparse[1]
for data in [b'', b'\x80', b'\x80\x05K', b'\x80\x05h\x02.', b'\x80\x05X\xff\xff\xff\xff']:
    failed=False
    try:
        pickle.loads(data)
    except Exception:
        failed=True
    assert failed
"#).unwrap()).unwrap();
        vm.collect_garbage(&[]).unwrap();
    }
}
