use tis::{Tiering, Vm, compile};
fn run(source: &str) {
    for tier in [
        Tiering::Bytecode,
        Tiering::Lir,
        Tiering::Native,
        Tiering::CopyPatch,
    ] {
        let mut p = compile(source).unwrap();
        let mut vm = Vm::new();
        vm.tiering = tier;
        vm.thresholds.function_entries = 2;
        vm.thresholds.loop_backedges = 8;
        vm.profiling.window_ticks = 32;
        vm.run_program(&mut p).unwrap();
        vm.collect_garbage(&[]).unwrap();
    }
}
#[test]
fn hashed_keys_preserve_numeric_equivalence_and_insertion_order() {
    run(
        "d={True:'first',2:'two'}\nd[1.0]='updated'\nassert len(d)==2\nassert list(d)==[True,2]\nassert d[1]=='updated'\nd[-0.0]='zero'\nassert d[False]=='zero'\nassert d[0.0]=='zero'\nd[(True,(0.0,2))]=[123]\nassert d[(1.0,(-0.0,2.0))]==[123]\nd[9007199254740992]='exact'\nd[9007199254740993]='large'\nassert d[9007199254740992.0]=='exact'\nassert d[9007199254740993]=='large'\nd[-9223372036854775807-1]='minimum'\nassert d[-9223372036854775808.0]=='minimum'\nd[9223372036854775807]='maximum'\nd[9223372036854775808.0]='beyond'\nassert d[9223372036854775807]=='maximum'\nassert d[9223372036854775808.0]=='beyond'\n",
    );
}
#[test]
fn dictionary_growth_deletion_copy_and_aliases_keep_the_index_valid() {
    run(
        "d={}\nfor i in range(1500):\n    d[str(i)]=[i]\ncopy=d.copy()\nfor i in range(0,1500,3):\n    assert d.pop(str(i))==[i]\nfor i in range(1500):\n    if i%3!=0:\n        assert d.get(str(i))==[i]\n    assert copy[str(i)]==[i]\nd['0']=[9]\nassert list(d)[-1]=='0'\nassert d.setdefault('0',[10])==[9]\nassert d.pop('missing',42)==42\nalias=d\nd.update(d)\nassert alias is d\nassert len(d)==1001\nd.clear()\nassert len(alias)==0\nassert len(copy)==1500\nd[(1,2)]=3\nassert d[(1.0,True+1)]==3\n",
    );
}
#[test]
fn direct_list_methods_preserve_aliases_compact_floats_and_self_extension() {
    run(
        "xs=[1.0,2.0]\nalias=xs\nxs.append(3.0)\nxs.insert(0,0.0)\nxs.reverse()\nassert alias==[3.0,2.0,1.0,0.0]\nassert xs.pop()==0.0\nassert xs.pop(-2)==2.0\nxs.extend(xs)\nassert xs==[3.0,1.0,3.0,1.0]\nassert xs.count(1)==2\nassert xs.index(1)==1\nxs.remove(3)\nassert xs==[1.0,3.0,1.0]\ncopy=xs.copy()\nxs.append('mixed')\nassert copy==[1.0,3.0,1.0]\nxs.clear()\nassert alias==[]\nxs.extend([4,5])\nassert alias==[4,5]\n",
    );
}
#[test]
fn cached_math_functions_have_stable_identity_and_survive_collection() {
    run(
        "import math\nassert math.sqrt is math.sqrt\nsqrt=math.sqrt\nfor i in range(10000):\n    garbage=[i]\nassert sqrt is math.sqrt\nassert sqrt(4)==2.0\nassert math.log is math.log\n",
    );
}
