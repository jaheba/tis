//! Non-allocating instance field loads. Every pointer is consumed before another callback.
use super::*;
use crate::heap::{NativeFieldDirectory, NativeFields};
use std::mem::{offset_of, size_of};

/// Emit guards to a local miss block; all general Python behavior stays in the existing helper.
#[allow(clippy::too_many_arguments)]
pub(super) fn load(
    b: &mut FunctionBuilder<'_>,
    scalars: &ScalarVars,
    host: ir::Value,
    registers: ir::Value,
    receiver: Register,
    destination: Register,
    attribute: &crate::Attribute,
    done: ir::Block,
) -> Result<()> {
    // The reference payload packs index below generation in a little-endian word.
    if !cfg!(target_endian = "little") {
        return Ok(());
    }
    let miss = b.create_block();
    macro_rules! guard {
        ($condition:expr) => {{
            let next = b.create_block();
            b.ins().brif($condition, next, &[], miss, &[]);
            b.switch_to_block(next);
        }};
    }
    let (vm, row) = checked_row(b, scalars, host, registers, receiver, miss)?;
    let kind = b.ins().load(
        types::I64,
        MemFlagsData::new(),
        row,
        offset_of!(NativeFields, kind) as i32,
    );
    let instance = b.ins().icmp_imm_s(IntCC::Equal, kind, 1);
    guard!(instance);
    let protocols = b.ins().load(
        types::I8,
        MemFlagsData::new(),
        vm,
        offset_of!(Vm, attribute_protocols) as i32,
    );
    let plain = b.ins().icmp_imm_s(IntCC::Equal, protocols, 0);
    guard!(plain);
    // Attribute lives in the Rc-owned LIR for the entire compiled artifact lifetime.
    let cache = b
        .ins()
        .iconst(types::I64, attribute as *const crate::Attribute as i64);
    let shape = b.ins().load(
        types::I64,
        MemFlagsData::new(),
        row,
        offset_of!(NativeFields, shape) as i32,
    );
    let expected = b.ins().load(types::I64, MemFlagsData::new(), cache, 0);
    let same_shape = b.ins().icmp(IntCC::Equal, shape, expected);
    guard!(same_shape);
    let slot = b.ins().load(types::I64, MemFlagsData::new(), cache, 8);
    let len = b.ins().load(
        types::I64,
        MemFlagsData::new(),
        row,
        offset_of!(NativeFields, len) as i32,
    );
    let in_bounds = b.ins().icmp(IntCC::UnsignedLessThan, slot, len);
    guard!(in_bounds);
    let values = b.ins().load(
        types::I64,
        MemFlagsData::new(),
        row,
        offset_of!(NativeFields, values) as i32,
    );
    let offset = b.ins().ishl_imm_u(slot, 4);
    let value = b.ins().iadd(values, offset);
    decode(b, scalars, registers, value, destination, miss)?;
    let count = b.ins().load(
        types::I64,
        MemFlagsData::new(),
        host,
        offset_of!(crate::native_abi::NativeHeader, inline_field_hits) as i32,
    );
    let count = b.ins().iadd_imm_s(count, 1);
    b.ins().store(
        MemFlagsData::new(),
        count,
        host,
        offset_of!(crate::native_abi::NativeHeader, inline_field_hits) as i32,
    );
    b.ins().jump(done, &[]);
    b.switch_to_block(miss);
    Ok(())
}

/// Resolve a generation-checked row. Pointers are consumed before any runtime boundary.
pub(super) fn checked_row(
    b: &mut FunctionBuilder<'_>,
    scalars: &ScalarVars,
    host: ir::Value,
    registers: ir::Value,
    receiver: Register,
    miss: ir::Block,
) -> Result<(ir::Value, ir::Value)> {
    macro_rules! guard {
        ($condition:expr) => {{
            let next = b.create_block();
            b.ins().brif($condition, next, &[], miss, &[]);
            b.switch_to_block(next);
        }};
    }
    // Recursion can reopen an observation window even in an artifact compiled without feedback.
    let collecting = b.ins().load(
        types::I8,
        MemFlagsData::new(),
        host,
        offset_of!(crate::native_abi::NativeHeader, collecting) as i32,
    );
    let quiet = b.ins().icmp_imm_s(IntCC::Equal, collecting, 0);
    guard!(quiet);
    let tag = load_cached(b, scalars, registers, receiver, 0, types::I64)?;
    let kind = b.ins().band_imm_s(tag, KIND_MASK as i64);
    let is_ref = b.ins().icmp_imm_s(IntCC::Equal, kind, REF as i64);
    guard!(is_ref);

    // Context owns the original VM borrow. Derive pointers afresh; recursive calls may move
    // the directory, and any object mutation refreshes its row before returning to native code.
    let vm = b.ins().load(
        types::I64,
        MemFlagsData::new(),
        host,
        offset_of!(crate::native_runtime::Context<'_>, vm) as i32,
    );
    let payload = load_cached(b, scalars, registers, receiver, 8, types::I64)?;
    let row = identity_row(b, vm, tag, payload, miss)?;
    Ok((vm, row))
}

pub(super) fn identity_row(
    b: &mut FunctionBuilder<'_>,
    vm: ir::Value,
    tag: ir::Value,
    payload: ir::Value,
    miss: ir::Block,
) -> Result<ir::Value> {
    macro_rules! guard {
        ($condition:expr) => {{
            let next = b.create_block();
            b.ins().brif($condition, next, &[], miss, &[]);
            b.switch_to_block(next);
        }};
    }
    let kind = b.ins().band_imm_s(tag, KIND_MASK as i64);
    let reference = b.ins().icmp_imm_s(IntCC::Equal, kind, REF as i64);
    guard!(reference);
    let directory = b.ins().iadd_imm_s(
        vm,
        (offset_of!(Vm, heap) + offset_of!(crate::Heap, native_fields)) as i64,
    );
    let heap = b.ins().load(
        types::I64,
        MemFlagsData::new(),
        directory,
        offset_of!(NativeFieldDirectory, heap) as i32,
    );
    let receiver_heap = b.ins().ushr_imm_u(tag, 32);
    let same_heap = b.ins().icmp(IntCC::Equal, receiver_heap, heap);
    guard!(same_heap);
    let index = b.ins().ireduce(types::I32, payload);
    let index = b.ins().uextend(types::I64, index);
    let len = b.ins().load(
        types::I64,
        MemFlagsData::new(),
        directory,
        offset_of!(NativeFieldDirectory, len) as i32,
    );
    let in_bounds = b.ins().icmp(IntCC::UnsignedLessThan, index, len);
    guard!(in_bounds);
    let entries = b.ins().load(
        types::I64,
        MemFlagsData::new(),
        directory,
        offset_of!(NativeFieldDirectory, entries) as i32,
    );
    let offset = b.ins().imul_imm_s(index, size_of::<NativeFields>() as i64);
    let row = b.ins().iadd(entries, offset);
    let generation = b.ins().ushr_imm_u(payload, 32);
    let actual = b.ins().load(
        types::I64,
        MemFlagsData::new(),
        row,
        offset_of!(NativeFields, generation) as i32,
    );
    let same_generation = b.ins().icmp(IntCC::Equal, generation, actual);
    guard!(same_generation);
    Ok(row)
}

/// `repr(u32)` Value has a C union of tagged C structs. Read only initialized payload
/// bytes: bool and ObjectId begin at byte 4; aligned int/float payloads begin at byte 8.
pub(super) fn decode(
    b: &mut FunctionBuilder<'_>,
    scalars: &ScalarVars,
    registers: ir::Value,
    value: ir::Value,
    destination: Register,
    miss: ir::Block,
) -> Result<()> {
    let kind = b.ins().load(types::I32, MemFlagsData::new(), value, 0);
    let scalar = b.create_block();
    let other = b.create_block();
    let ready = b.create_block();
    let int = b.ins().icmp_imm_s(IntCC::Equal, kind, 3);
    let float = b.ins().icmp_imm_s(IntCC::Equal, kind, 4);
    let numeric = b.ins().bor(int, float);
    b.ins().brif(numeric, scalar, &[], other, &[]);
    b.switch_to_block(scalar);
    let bits = b.ins().load(types::I64, MemFlagsData::new(), value, 8);
    let tag = b.ins().uextend(types::I64, kind);
    let tag = b.ins().iadd_imm_s(tag, -1);
    store_word_cached(b, scalars, registers, destination, 0, tag)?;
    store_word_cached(b, scalars, registers, destination, 8, bits)?;
    b.ins().jump(ready, &[]);
    b.switch_to_block(other);
    let reference = b.create_block();
    let primitive = b.create_block();
    let is_ref = b.ins().icmp_imm_s(IntCC::Equal, kind, 5);
    let is_inline = b.ins().icmp_imm_s(IntCC::Equal, kind, 6);
    let packed = b.ins().bor(is_ref, is_inline);
    b.ins().brif(packed, reference, &[], primitive, &[]);
    b.switch_to_block(reference);
    let heap = b.ins().load(types::I32, MemFlagsData::new(), value, 4);
    let heap = b.ins().uextend(types::I64, heap);
    let heap = b.ins().ishl_imm_u(heap, 32);
    let reference_tag = b.ins().iconst(types::I64, REF as i64);
    let inline_tag = b.ins().iconst(types::I64, INLINE_STRING as i64);
    let prefix = b.ins().select(is_ref, reference_tag, inline_tag);
    let tag = b.ins().bor(heap, prefix);
    let payload = b.ins().load(types::I64, MemFlagsData::new(), value, 8);
    store_word_cached(b, scalars, registers, destination, 0, tag)?;
    store_word_cached(b, scalars, registers, destination, 8, payload)?;
    b.ins().jump(ready, &[]);
    b.switch_to_block(primitive);
    let boolean = b.create_block();
    let none = b.create_block();
    let is_bool = b.ins().icmp_imm_s(IntCC::Equal, kind, 2);
    b.ins().brif(is_bool, boolean, &[], none, &[]);
    b.switch_to_block(boolean);
    let bit = b.ins().load(types::I8, MemFlagsData::new(), value, 4);
    let bit = b.ins().uextend(types::I64, bit);
    store_scalar_cached(b, scalars, registers, destination, BOOL, bit)?;
    b.ins().jump(ready, &[]);
    b.switch_to_block(none);
    let is_none = b.ins().icmp_imm_s(IntCC::Equal, kind, 1);
    let valid_none = b.create_block();
    b.ins().brif(is_none, valid_none, &[], miss, &[]);
    b.switch_to_block(valid_none);
    let zero = b.ins().iconst(types::I64, 0);
    store_scalar_cached(b, scalars, registers, destination, NONE, zero)?;
    b.ins().jump(ready, &[]);
    b.switch_to_block(ready);
    Ok(())
}
