use tis::{Object, Tiering, Value, Vm, compile};

#[test]
fn native_field_loads_decode_every_payload_and_refresh_after_mutation() {
    let mut vm = Vm::new();
    vm.tiering = Tiering::Native;
    vm.thresholds.function_entries = 1;
    vm.thresholds.loop_backedges = 4;
    vm.run_program(
        &mut compile(
            r#"
class C:
    pass
c=C()
c.x=1
def read(o):
    return o.x
for i in range(300):
    assert read(c)==1
"#,
        )
        .unwrap(),
    )
    .unwrap();
    let object = vm.get_global("c").unwrap();
    let read = vm.get_global("read").unwrap();
    let reference = vm.heap.allocate(Object::List(vec![Value::Int(42)].into()));
    for value in [
        Value::None,
        Value::Bool(false),
        Value::Bool(true),
        Value::Int(i64::MIN),
        Value::Int(i64::MAX),
        Value::Float(-0.0),
        Value::Float(f64::INFINITY),
        Value::Float(f64::from_bits(0x7ff8_0000_0000_1234)),
        reference,
    ] {
        {
            let mut instance = vm.heap.get_mut(object).unwrap();
            let Object::Instance { attributes, .. } = &mut *instance else {
                panic!()
            };
            attributes.insert("x".into(), value);
            // Reallocation and shape changes must update the native row after the borrow ends.
            for index in 0..40 {
                attributes.insert(format!("field{index}"), Value::Int(index));
            }
        }
        let before = vm.tier_stats.native_inline_field_hits;
        let hits = vm.tier_stats.native_field_hits;
        for _ in 0..20 {
            let result = vm.call(read, &[object]).unwrap();
            match (result, value) {
                (Value::Float(actual), Value::Float(expected)) => {
                    assert_eq!(actual.to_bits(), expected.to_bits());
                }
                _ => assert_eq!(result, value),
            }
        }
        assert!(vm.tier_stats.native_field_hits > hits);
        assert!(vm.tier_stats.native_inline_field_hits > before);
    }
    vm.collect_garbage(&[object, read, reference]).unwrap();
    assert_eq!(vm.call(read, &[object]).unwrap(), reference);
}

#[test]
fn native_field_directory_survives_growth_collection_and_noninstance_replacement() {
    let mut vm = Vm::new();
    vm.tiering = Tiering::Native;
    vm.thresholds.function_entries = 1;
    vm.thresholds.loop_backedges = 4;
    vm.run_program(
        &mut compile(
            r#"
class C:
    pass
c=C()
c.x=42
def read(o):
    return o.x
for i in range(300):
    assert read(c)==42
"#,
        )
        .unwrap(),
    )
    .unwrap();
    let c = vm.get_global("c").unwrap();
    let read = vm.get_global("read").unwrap();
    for _ in 0..3 {
        for _ in 0..10000 {
            vm.heap.allocate(Object::Tuple(vec![]));
        }
        assert_eq!(vm.call(read, &[c]).unwrap(), Value::Int(42));
        vm.collect_garbage(&[c, read]).unwrap();
        assert_eq!(vm.call(read, &[c]).unwrap(), Value::Int(42));
    }
    *vm.heap.get_mut(c).unwrap() = Object::Tuple(vec![]);
    assert!(vm.call(read, &[c]).is_err());
}

#[test]
fn builtin_receiver_feedback_does_not_exclude_later_instance_fields() {
    let mut vm = Vm::new();
    vm.tiering = Tiering::Native;
    vm.thresholds.function_entries = 1;
    vm.thresholds.loop_backedges = 4;
    vm.run_program(
        &mut compile(
            r#"
def copy(o):
    return o.copy()
for i in range(300):
    assert copy([i])==[i]
class C:
    pass
c=C()
c.copy=lambda:[42]
assert copy(c)==[42]
assert copy([123])==[123]
"#,
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(vm.stack_len(), 0);
}
