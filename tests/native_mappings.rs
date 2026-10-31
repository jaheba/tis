use tis::{BytecodeMode, Object, Tiering, Value, Vm, compile, profile::ProfileMode};

fn compiled(source: &str, encoding: BytecodeMode, tier: Tiering) -> Vm {
    let mut vm = Vm::new();
    vm.tiering = tier;
    vm.bytecode = encoding;
    vm.thresholds.function_entries = 1;
    vm.thresholds.loop_backedges = 4;
    vm.readonly_regions = true;
    vm.mutation_regions = true;
    vm.run_program(&mut compile(source).unwrap()).unwrap();
    assert_eq!(vm.tier_stats.compilation_errors, 0);
    vm
}

#[test]
fn native_string_mapping_probes_handle_long_keys_and_allocation_pressure() {
    for tier in [Tiering::Native, Tiering::CopyPatch] {
        for encoding in [BytecodeMode::Register, BytecodeMode::Stack] {
            let vm = compiled(
                r#"
def count(text, n):
    counts = {}
    for i in range(n):
        j = i % 20
        key = text[j:j+18]
        if key not in counts:
            counts[key] = 0
        counts[key] += 1
        assert key in counts
    return counts
for _ in range(4):
    result = count('abcdefghijklmnopqrstuvwxyz' * 4, 2500)
    assert len(result) == 20 and sum(result.values()) == 2500
"#,
                encoding,
                tier,
            );
            assert!(vm.tier_stats.native_mapping_reads > 1000);
            assert!(vm.tier_stats.native_builtin_updates > 1000);
            assert!(vm.tier_stats.native_string_slices > 1000);
            assert!(vm.heap.allocation_count() < 10_000);
        }
    }
}

#[test]
fn closed_probes_preserve_protocols_slicing_errors_and_eager_feedback() {
    for tier in [Tiering::Native, Tiering::CopyPatch] {
        let mut vm = compiled(
            r#"
def read(d): return d['x']
def cut(text, start, stop, step): return text[start:stop:step]
for _ in range(200):
    assert read({'x': 1}) == 1
    assert cut('abcdefghijklmnop', 1, 15, 1) == 'bcdefghijklmno'
log = []
class Key:
    def __hash__(self): return hash('x')
    def __eq__(self, other):
        log.append('eq')
        return other == 'x'
key = Key()
assert read({key: 42}) == 42
assert log == ['eq']
try:
    read({})
    assert False
except KeyError:
    pass
class Index:
    def __index__(self):
        log.append('index')
        return 1
log.clear()
assert cut('abcdef', Index(), None, 1) == 'bcdef'
assert log == ['index']
assert cut('aé😀z', None, None, -1) == 'z😀éa'
assert cut('abcdef', None, None, 2) == 'ace'
assert cut('abcdef', -100, 100, 1) == 'abcdef'
assert cut('abcdef', 5, 1, 1) == ''
try:
    cut('abcdef', None, None, 0)
    assert False
except ValueError:
    pass
class Receiver:
    def __getitem__(self, key):
        assert key.start == 1 and key.stop == 3
        return 'custom'
assert cut(Receiver(), 1, 3, None) == 'custom'
"#,
            BytecodeMode::Register,
            tier,
        );
        let read = vm.get_global("read").unwrap();
        let mapping = vm.heap.allocate(Object::Dict(
            vec![(tis::Key::String("x".into()), Value::Int(7))].into(),
        ));
        let closed = vm.tier_stats.native_mapping_reads;
        let probes = vm.tier_stats.native_feedback_probes;
        vm.profiling.mode = ProfileMode::Eager;
        assert_eq!(vm.call(read, &[mapping]).unwrap(), Value::Int(7));
        assert_eq!(vm.tier_stats.native_mapping_reads, closed);
        assert!(vm.tier_stats.native_feedback_probes > probes);
        assert_eq!(vm.stack_len(), 0);
    }
}

#[test]
fn references_returned_by_closed_dict_reads_survive_reentrant_collection() {
    for tier in [Tiering::Native, Tiering::CopyPatch] {
        let mut vm = compiled(
            r#"
def take(mapping, n):
    value = mapping['child']
    mapping.clear()
    for i in range(n):
        garbage = [i]
    return value
for _ in range(200):
    take({'child': []}, 1)
"#,
            BytecodeMode::Register,
            tier,
        );
        let take = vm.get_global("take").unwrap();
        let child = vm.heap.allocate(Object::List(vec![Value::Int(123)].into()));
        let owner = vm.heap.allocate(Object::Dict(
            vec![(tis::Key::String("child".into()), child)].into(),
        ));
        let reads = vm.tier_stats.native_mapping_reads;
        let allocated = vm.heap.allocation_count();
        let result = vm.call(take, &[owner, Value::Int(9000)]).unwrap();
        assert_eq!(result, child);
        assert!(vm.tier_stats.native_mapping_reads > reads);
        assert!(vm.heap.allocation_count() < allocated + 9000);
        assert_eq!(
            vm.heap.get_item(result, Value::Int(0)).unwrap(),
            Value::Int(123)
        );
        vm.collect_garbage(&[result]).unwrap();
        assert!(vm.heap.get(child).is_ok());
        vm.collect_garbage(&[]).unwrap();
        assert!(vm.heap.get(child).is_err());
    }
}
