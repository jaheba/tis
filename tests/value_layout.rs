use tis::{Heap, Object, ObjectId, Value};

#[test]
fn values_and_optional_registers_are_two_words() {
    assert_eq!(size_of::<Value>(), 16);
    assert_eq!(size_of::<Option<Value>>(), 16);
    assert_eq!(size_of::<ObjectId>(), 12);
}

#[test]
fn compact_handles_reject_freed_reused_and_foreign_slots() {
    let mut heap = Heap::new();
    let stale = heap.allocate(Object::List(vec![Value::Int(123)].into()));
    assert_eq!(heap.collect(&[]).unwrap(), 1);
    assert!(heap.get(stale).is_err());
    assert!(heap.get_mut(stale).is_err());
    let fresh = heap.allocate(Object::List(vec![Value::Int(456)].into()));
    assert_eq!(heap.slot_count(), 1);
    assert_ne!(stale, fresh);
    assert!(heap.get(stale).is_err());
    assert!(heap.get(fresh).is_ok());
    let foreign = Heap::new();
    let other = foreign.allocate(Object::List(vec![].into()));
    assert!(heap.get(other).is_err());
    assert!(foreign.get(fresh).is_err());
    // Both first allocations have index zero and generation one: only heap identity differs.
    assert!(foreign.get(stale).is_err());
    assert!(heap.collect(&[stale]).is_err());
    assert!(heap.collect(&[other]).is_err());
    assert!(heap.get(fresh).is_ok());
}
