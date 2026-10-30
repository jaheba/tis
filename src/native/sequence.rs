//! Exact list/tuple reads use the refreshed native directory, never a pointer across callbacks.
use super::*;
use crate::heap::NativeFields;
use std::mem::{offset_of, size_of};

#[allow(clippy::too_many_arguments)]
pub(super) fn load(
    b: &mut FunctionBuilder<'_>,
    scalars: &ScalarVars,
    host: ir::Value,
    registers: ir::Value,
    receiver: Register,
    kind: Option<lir::SequenceKind>,
    index: lir::SequenceIndex,
    destination: Register,
    done: ir::Block,
) -> Result<()> {
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
    let (vm, row) = fields::checked_row(b, scalars, host, registers, receiver, miss)?;
    let actual = b.ins().load(
        types::I64,
        MemFlagsData::new(),
        row,
        offset_of!(NativeFields, kind) as i32,
    );
    let same = if let Some(kind) = kind {
        let expected = match kind {
            lir::SequenceKind::List => 2,
            lir::SequenceKind::Tuple => 3,
        };
        b.ins().icmp_imm_s(IntCC::Equal, actual, expected)
    } else {
        let list = b.ins().icmp_imm_s(IntCC::Equal, actual, 2);
        let tuple = b.ins().icmp_imm_s(IntCC::Equal, actual, 3);
        b.ins().bor(list, tuple)
    };
    guard!(same);
    let index = match index {
        lir::SequenceIndex::Constant(index) => b.ins().iconst(types::I64, index),
        lir::SequenceIndex::Value(index) => {
            let tag = load_cached(b, scalars, registers, index, 0, types::I64)?;
            let integer = b.ins().icmp_imm_s(IntCC::Equal, tag, INT as i64);
            guard!(integer);
            load_cached(b, scalars, registers, index, 8, types::I64)?
        }
    };
    let len = b.ins().load(
        types::I64,
        MemFlagsData::new(),
        row,
        offset_of!(NativeFields, len) as i32,
    );
    let negative = b.ins().icmp_imm_s(IntCC::SignedLessThan, index, 0);
    let adjusted = b.ins().iadd(index, len);
    let index = b.ins().select(negative, adjusted, index);
    let valid = b.ins().icmp(IntCC::UnsignedLessThan, index, len);
    guard!(valid);
    let values = b.ins().load(
        types::I64,
        MemFlagsData::new(),
        row,
        offset_of!(NativeFields, values) as i32,
    );
    let offset = b.ins().imul_imm_s(index, size_of::<Value>() as i64);
    let value = b.ins().iadd(values, offset);
    fields::decode(b, scalars, registers, value, destination, miss)?;
    let offset =
        (offset_of!(Vm, tier_stats) + offset_of!(crate::TierStats, native_sequence_loads)) as i32;
    let count = b.ins().load(types::I64, MemFlagsData::new(), vm, offset);
    let count = b.ins().iadd_imm_s(count, 1);
    b.ins().store(MemFlagsData::new(), count, vm, offset);
    b.ins().jump(done, &[]);
    b.switch_to_block(miss);
    Ok(())
}

/// Write only through a pointer published by an exclusive boxed-list borrow. This path is used
/// only in artifacts without cached heap views, so no detached sequence snapshot can become stale.
#[allow(clippy::too_many_arguments)]
pub(super) fn store(
    b: &mut FunctionBuilder<'_>,
    scalars: &ScalarVars,
    host: ir::Value,
    registers: ir::Value,
    receiver: Register,
    index: Register,
    input: Register,
    destination: Register,
    done: ir::Block,
) -> Result<()> {
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
    let (vm, row) = fields::checked_row(b, scalars, host, registers, receiver, miss)?;
    let kind = b.ins().load(
        types::I64,
        MemFlagsData::new(),
        row,
        offset_of!(NativeFields, kind) as i32,
    );
    let list = b.ins().icmp_imm_s(IntCC::Equal, kind, 2);
    guard!(list);
    let values = b.ins().load(
        types::I64,
        MemFlagsData::new(),
        row,
        offset_of!(NativeFields, writable_values) as i32,
    );
    let writable = b.ins().icmp_imm_s(IntCC::NotEqual, values, 0);
    guard!(writable);
    let tag = load_cached(b, scalars, registers, index, 0, types::I64)?;
    let integer = b.ins().icmp_imm_s(IntCC::Equal, tag, INT as i64);
    guard!(integer);
    let index = load_cached(b, scalars, registers, index, 8, types::I64)?;
    let len = b.ins().load(
        types::I64,
        MemFlagsData::new(),
        row,
        offset_of!(NativeFields, len) as i32,
    );
    let negative = b.ins().icmp_imm_s(IntCC::SignedLessThan, index, 0);
    let adjusted = b.ins().iadd(index, len);
    let index = b.ins().select(negative, adjusted, index);
    let valid = b.ins().icmp(IntCC::UnsignedLessThan, index, len);
    guard!(valid);
    let tag = load_cached(b, scalars, registers, input, 0, types::I64)?;
    let kind = b.ins().band_imm_s(tag, KIND_MASK as i64);
    let ordinary = b
        .ins()
        .icmp_imm_s(IntCC::UnsignedLessThanOrEqual, kind, REF as i64);
    let inline = b.ins().icmp_imm_s(IntCC::Equal, kind, INLINE_STRING as i64);
    let valid = b.ins().bor(ordinary, inline);
    guard!(valid);
    let bits = load_cached(b, scalars, registers, input, 8, types::I64)?;
    let offset = b.ins().imul_imm_s(index, size_of::<Value>() as i64);
    let pointer = b.ins().iadd(values, offset);
    let normal = b.ins().iadd_imm_s(kind, 1);
    let inline_kind = b.ins().iconst(types::I64, 6);
    let rust_kind = b.ins().select(inline, inline_kind, normal);
    let rust_kind = b.ins().ireduce(types::I32, rust_kind);
    b.ins().store(MemFlagsData::new(), rust_kind, pointer, 0);
    let boolean = b.ins().icmp_imm_s(IntCC::Equal, kind, BOOL as i64);
    let bool_block = b.create_block();
    let other = b.create_block();
    let ready = b.create_block();
    b.ins().brif(boolean, bool_block, &[], other, &[]);
    b.switch_to_block(bool_block);
    let bit = b.ins().icmp_imm_s(IntCC::NotEqual, bits, 0);
    b.ins().store(MemFlagsData::new(), bit, pointer, 4);
    b.ins().jump(ready, &[]);
    b.switch_to_block(other);
    let reference = b.ins().icmp_imm_s(IntCC::Equal, kind, REF as i64);
    let packed = b.ins().bor(reference, inline);
    let packed_block = b.create_block();
    let scalar = b.create_block();
    b.ins().brif(packed, packed_block, &[], scalar, &[]);
    b.switch_to_block(packed_block);
    let high = b.ins().ushr_imm_u(tag, 32);
    let high = b.ins().ireduce(types::I32, high);
    b.ins().store(MemFlagsData::new(), high, pointer, 4);
    b.ins().store(MemFlagsData::new(), bits, pointer, 8);
    b.ins().jump(ready, &[]);
    b.switch_to_block(scalar);
    let none = b.ins().icmp_imm_s(IntCC::Equal, kind, NONE as i64);
    let number = b.create_block();
    b.ins().brif(none, ready, &[], number, &[]);
    b.switch_to_block(number);
    b.ins().store(MemFlagsData::new(), bits, pointer, 8);
    b.ins().jump(ready, &[]);
    b.switch_to_block(ready);
    store_constant_cached(b, scalars, registers, destination, Slot::boxed(Value::None))?;
    count(b, vm, offset_of!(crate::TierStats, native_list_stores));
    b.ins().jump(done, &[]);
    b.switch_to_block(miss);
    Ok(())
}

/// Advance the actual Cell cursor, so aliases, callbacks and side exits see every completed step.
#[allow(clippy::too_many_arguments)]
pub(super) fn range_next(
    b: &mut FunctionBuilder<'_>,
    scalars: &ScalarVars,
    host: ir::Value,
    registers: ir::Value,
    iterator: Register,
    destination: Register,
    done: ir::Block,
) -> Result<()> {
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
    let (vm, row) = fields::checked_row(b, scalars, host, registers, iterator, miss)?;
    let kind = b.ins().load(
        types::I64,
        MemFlagsData::new(),
        row,
        offset_of!(NativeFields, kind) as i32,
    );
    let is_iterator = b.ins().icmp_imm_s(IntCC::Equal, kind, 6);
    guard!(is_iterator);
    let source_tag = b.ins().load(
        types::I64,
        MemFlagsData::new(),
        row,
        offset_of!(NativeFields, class_tag) as i32,
    );
    let source_payload = b.ins().load(
        types::I64,
        MemFlagsData::new(),
        row,
        offset_of!(NativeFields, class_payload) as i32,
    );
    let source = fields::identity_row(b, vm, source_tag, source_payload, miss)?;
    let kind = b.ins().load(
        types::I64,
        MemFlagsData::new(),
        source,
        offset_of!(NativeFields, kind) as i32,
    );
    let range = b.ins().icmp_imm_s(IntCC::Equal, kind, 5);
    guard!(range);
    let cursor = b.ins().load(
        types::I64,
        MemFlagsData::new(),
        row,
        offset_of!(NativeFields, shape) as i32,
    );
    let index = b.ins().load(types::I64, MemFlagsData::new(), cursor, 0);
    let len = b.ins().load(
        types::I64,
        MemFlagsData::new(),
        source,
        offset_of!(NativeFields, len) as i32,
    );
    let valid = b.ins().icmp(IntCC::UnsignedLessThan, index, len);
    guard!(valid);
    let start = b.ins().load(
        types::I64,
        MemFlagsData::new(),
        source,
        offset_of!(NativeFields, class_tag) as i32,
    );
    let step = b.ins().load(
        types::I64,
        MemFlagsData::new(),
        source,
        offset_of!(NativeFields, class_payload) as i32,
    );
    // Valid range members fit i64; modular intermediates preserve even extreme negative steps.
    let scaled = b.ins().imul(index, step);
    let value = b.ins().iadd(start, scaled);
    let next = b.ins().iadd_imm_s(index, 1);
    b.ins().store(MemFlagsData::new(), next, cursor, 0);
    store_scalar_cached(b, scalars, registers, destination, INT, value)?;
    count(b, vm, offset_of!(crate::TierStats, native_range_steps));
    b.ins().jump(done, &[]);
    b.switch_to_block(miss);
    Ok(())
}

fn count(b: &mut FunctionBuilder<'_>, vm: ir::Value, field: usize) {
    let offset = (offset_of!(Vm, tier_stats) + field) as i32;
    let count = b.ins().load(types::I64, MemFlagsData::new(), vm, offset);
    let count = b.ins().iadd_imm_s(count, 1);
    b.ins().store(MemFlagsData::new(), count, vm, offset);
}
