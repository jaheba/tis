//! Direct operations inside a verified, allocation-free region. Descriptor layout is shared with entry preparation.
use super::*;
use crate::region::{DONE, Descriptor, FLOATS, ITERATOR, LIST, LoopRegion, RANGE, TUPLE};
use std::mem::{offset_of, size_of};
pub(super) struct Emitter<'a, 'b> {
    pub b: &'a mut FunctionBuilder<'b>,
    pub scalars: &'a ScalarVars,
    pub registers: ir::Value,
    pub lir: &'a lir::Function,
    pub region: &'a LoopRegion,
    pub nest_vars: Option<&'a super::virtual_iter::NestVars>,
}
impl Emitter<'_, '_> {
    fn guard(&mut self, passed: ir::Value, exit: lir::ExitId) -> Result<()> {
        let success = self.b.create_block();
        let failed = self.b.create_block();
        self.b.ins().brif(passed, success, &[], failed, &[]);
        self.b.switch_to_block(failed);
        if let Some(vars) = self.nest_vars {
            vars.publish(self.b, self.registers, self.lir)?;
        }
        self.scalars.materialize(
            self.b,
            self.registers,
            state_values(&self.lir.states[self.lir.exits[exit.0].state.0]),
        )?;
        let status = self.b.ins().iconst(types::I64, exit.0 as i64 + 1);
        self.b.ins().return_(&[status]);
        self.b.switch_to_block(success);
        Ok(())
    }
    fn field(&mut self, p: ir::Value, offset: usize) -> ir::Value {
        self.b
            .ins()
            .load(types::I64, MemFlagsData::new(), p, offset as i32)
    }
    fn resolve_words(&mut self, words: [ir::Value; 4], exit: lir::ExitId) -> Result<ir::Value> {
        // A verified schema validates every source item and imported path before entry. The region
        // cannot allocate, collect, call Python or change container shapes, so those pointers stay valid.
        let is_ref = self.b.ins().icmp_imm_s(IntCC::Equal, words[0], REF as i64);
        if self.region.schema.is_none() {
            self.guard(is_ref, exit)?;
        }
        let meta = Register(self.lir.registers.len() + 1);
        let len = load(self.b, self.registers, meta, 16, types::I64)?;
        let bounds = self.b.ins().icmp(IntCC::UnsignedLessThan, words[2], len);
        if self.region.schema.is_none() {
            self.guard(bounds, exit)?;
        }
        let base = load(self.b, self.registers, meta, 8, types::I64)?;
        let offset = self
            .b
            .ins()
            .imul_imm_s(words[2], size_of::<Descriptor>() as i64);
        let pointer = self.b.ins().iadd(base, offset);
        let heap = self.field(pointer, offset_of!(Descriptor, heap));
        let generation = self.field(pointer, offset_of!(Descriptor, generation));
        let h = self.b.ins().icmp(IntCC::Equal, heap, words[1]);
        let g = self.b.ins().icmp(IntCC::Equal, generation, words[3]);
        let valid = self.b.ins().band(h, g);
        if self.region.schema.is_none() {
            self.guard(valid, exit)?;
        }
        Ok(pointer)
    }
    fn words(&mut self, r: Register) -> Result<[ir::Value; 4]> {
        Ok([
            load_cached(self.b, self.scalars, self.registers, r, 0, types::I64)?,
            load_cached(self.b, self.scalars, self.registers, r, 8, types::I64)?,
            load_cached(self.b, self.scalars, self.registers, r, 16, types::I64)?,
            load_cached(self.b, self.scalars, self.registers, r, 24, types::I64)?,
        ])
    }
    fn resolve(&mut self, r: Register, exit: lir::ExitId) -> Result<ir::Value> {
        let words = self.words(r)?;
        self.resolve_words(words, exit)
    }
    fn boxed_at(&mut self, pointer: ir::Value, dst: Register) -> Result<()> {
        let scalar = self.region.schema.is_some()
            && self.lir.blocks[self.region.body.0]
                .instructions
                .iter()
                .any(|i| matches!(i.operation,Operation::GuardFloat {value,..} if value==dst));
        if scalar {
            let bits = self.field(pointer, 8);
            store_scalar_cached(self.b, self.scalars, self.registers, dst, FLOAT, bits)?;
        } else {
            for field in [0, 8, 16, 24] {
                let value = self.field(pointer, field as usize);
                store_word_cached(self.b, self.scalars, self.registers, dst, field, value)?;
            }
        }
        Ok(())
    }
    fn element(
        &mut self,
        desc: ir::Value,
        index: ir::Value,
        dst: Register,
        exit: lir::ExitId,
    ) -> Result<()> {
        let kind = self.field(desc, offset_of!(Descriptor, kind));
        let is_range = self.b.ins().icmp_imm_s(IntCC::Equal, kind, RANGE as i64);
        let range = self.b.create_block();
        let sequence = self.b.create_block();
        let finish = self.b.create_block();
        self.b.ins().brif(is_range, range, &[], sequence, &[]);
        self.b.switch_to_block(range);
        let start = self.field(desc, offset_of!(Descriptor, data));
        let step = self.field(desc, offset_of!(Descriptor, index));
        let start = self.b.ins().sextend(types::I128, start);
        let step = self.b.ins().sextend(types::I128, step);
        let wideindex = self.b.ins().uextend(types::I128, index);
        let product = self.b.ins().imul(step, wideindex);
        let value = self.b.ins().iadd(start, product);
        let small = self.b.ins().ireduce(types::I64, value);
        let wide = self.b.ins().sextend(types::I128, small);
        let valid = self.b.ins().icmp(IntCC::Equal, value, wide);
        self.guard(valid, exit)?;
        store_scalar_cached(self.b, self.scalars, self.registers, dst, INT, small)?;
        self.b.ins().jump(finish, &[]);
        self.b.switch_to_block(sequence);
        let is_float = self.b.ins().icmp_imm_s(IntCC::Equal, kind, FLOATS as i64);
        let floats = self.b.create_block();
        let values = self.b.create_block();
        let done = self.b.create_block();
        self.b.ins().brif(is_float, floats, &[], values, &[]);
        self.b.switch_to_block(floats);
        let data = self.field(desc, offset_of!(Descriptor, data));
        let off = self.b.ins().ishl_imm_u(index, 3);
        let p = self.b.ins().iadd(data, off);
        let bits = self.field(p, 0);
        store_scalar_cached(self.b, self.scalars, self.registers, dst, FLOAT, bits)?;
        self.b.ins().jump(done, &[]);
        self.b.switch_to_block(values);
        let data = self.field(desc, offset_of!(Descriptor, data));
        let off = self.b.ins().ishl_imm_u(index, 5);
        let p = self.b.ins().iadd(data, off);
        self.boxed_at(p, dst)?;
        self.b.ins().jump(done, &[]);
        self.b.switch_to_block(done);
        self.b.ins().jump(finish, &[]);
        self.b.switch_to_block(finish);
        Ok(())
    }
    fn sequence(
        &mut self,
        r: Register,
        kind: crate::sequence::SequenceKind,
        exit: lir::ExitId,
    ) -> Result<ir::Value> {
        let p = self.resolve(r, exit)?;
        let actual = self.field(p, offset_of!(Descriptor, kind));
        let valid = match kind {
            crate::sequence::SequenceKind::Tuple => {
                self.b.ins().icmp_imm_s(IntCC::Equal, actual, TUPLE as i64)
            }
            crate::sequence::SequenceKind::List => {
                let a = self.b.ins().icmp_imm_s(IntCC::Equal, actual, LIST as i64);
                let b = self.b.ins().icmp_imm_s(IntCC::Equal, actual, FLOATS as i64);
                self.b.ins().bor(a, b)
            }
        };
        if self.region.schema.is_none() {
            self.guard(valid, exit)?;
        }
        Ok(p)
    }
    pub fn reset_private(&mut self, value: Register) -> Result<()> {
        if let Some(state) = self
            .nest_vars
            .and_then(|vars| vars.iterators.iter().find(|s| s.iterator == value))
        {
            let zero = self.b.ins().iconst(types::I64, 0);
            self.b.def_var(state.index, zero);
            self.b.def_var(state.exhausted, zero);
            return Ok(());
        }
        // These references name freshly prepared, non-escaping iterator objects. No allocation, call or
        // collection can replace them while the closed nest executes.
        let index = load_cached(self.b, self.scalars, self.registers, value, 16, types::I64)?;
        let base = load(
            self.b,
            self.registers,
            Register(self.lir.registers.len() + 1),
            8,
            types::I64,
        )?;
        let off = self
            .b
            .ins()
            .imul_imm_s(index, size_of::<Descriptor>() as i64);
        let pointer = self.b.ins().iadd(base, off);
        let indexptr = self.field(pointer, offset_of!(Descriptor, index));
        let exhausted = self.field(pointer, offset_of!(Descriptor, exhausted));
        let zero = self.b.ins().iconst(types::I64, 0);
        self.b.ins().store(MemFlagsData::new(), zero, indexptr, 0);
        let zero = self.b.ins().iconst(types::I8, 0);
        self.b.ins().store(MemFlagsData::new(), zero, exhausted, 0);
        Ok(())
    }
    pub fn emit(&mut self, op: &Operation, dst: Option<Register>) -> Result<bool> {
        match op {
            Operation::Truth { value, .. } => {
                let bits =
                    load_cached(self.b, self.scalars, self.registers, *value, 8, types::I64)?;
                store_scalar_cached(
                    self.b,
                    self.scalars,
                    self.registers,
                    dst.unwrap(),
                    RAW_BOOL,
                    bits,
                )?;
            }
            Operation::GuardInt { .. } | Operation::GuardFloat { .. }
                if self.region.schema.is_some() => {}
            Operation::FloatListView {
                value,
                length,
                failure,
            } => {
                let p = self.resolve(*value, *failure)?;
                let kind = self.field(p, offset_of!(Descriptor, kind));
                let valid = self.b.ins().icmp_imm_s(IntCC::Equal, kind, FLOATS as i64);
                if self.region.schema.is_none() {
                    self.guard(valid, *failure)?;
                }
                let len = self.field(p, offset_of!(Descriptor, len));
                if let Some(n) = length {
                    let valid = self.b.ins().icmp_imm_s(IntCC::Equal, len, *n as i64);
                    if self.region.schema.is_none() {
                        self.guard(valid, *failure)?;
                    }
                }
                let data = self.field(p, offset_of!(Descriptor, data));
                let dst = dst.unwrap();
                store_scalar(self.b, self.registers, dst, FLOAT_VIEW_TAG, data)?;
                self.b
                    .ins()
                    .store(MemFlagsData::new(), len, self.registers, offset(dst, 16)?);
                let zero = self.b.ins().iconst(types::I64, 0);
                self.b
                    .ins()
                    .store(MemFlagsData::new(), zero, self.registers, offset(dst, 24)?);
            }
            Operation::SequenceUnpack {
                value,
                kind,
                count,
                failure,
            } => {
                let p = self.sequence(*value, *kind, *failure)?;
                let len = self.field(p, offset_of!(Descriptor, len));
                let valid = self.b.ins().icmp_imm_s(IntCC::Equal, len, *count as i64);
                if self.region.schema.is_none() {
                    self.guard(valid, *failure)?;
                }
                copy_slots(
                    self.b,
                    self.scalars,
                    self.registers,
                    &[(*value, dst.unwrap())],
                )?;
            }
            Operation::SequenceGet {
                value,
                kind,
                index: lir::SequenceIndex::Constant(index),
                failure,
            } => {
                let p = self.sequence(*value, *kind, *failure)?;
                let len = self.field(p, offset_of!(Descriptor, len));
                let index = if *index < 0 {
                    self.b.ins().iadd_imm_s(len, *index)
                } else {
                    self.b.ins().iconst(types::I64, *index)
                };
                let valid = self.b.ins().icmp(IntCC::UnsignedLessThan, index, len);
                if self.region.schema.is_none() {
                    self.guard(valid, *failure)?;
                }
                if *kind == crate::sequence::SequenceKind::Tuple {
                    let data = self.field(p, offset_of!(Descriptor, data));
                    let off = self.b.ins().ishl_imm_u(index, 5);
                    let pointer = self.b.ins().iadd(data, off);
                    self.boxed_at(pointer, dst.unwrap())?;
                } else {
                    self.element(p, index, dst.unwrap(), *failure)?;
                }
            }
            Operation::Call {
                helper: lir::Helper::Python(crate::hir::CoreOp::Next),
                arguments,
                ..
            } => {
                let exit = self.region.entry;
                let iterator = self.resolve(arguments[0], exit)?;
                let kind = self.field(iterator, offset_of!(Descriptor, kind));
                let valid = self.b.ins().icmp_imm_s(IntCC::Equal, kind, ITERATOR as i64);
                self.guard(valid, exit)?;
                let mut words = vec![];
                for field in [0, 8, 16, 24] {
                    words.push(self.field(iterator, offset_of!(Descriptor, source) + field));
                }
                let source = self.resolve_words(words.try_into().unwrap(), exit)?;
                let kind = self.field(source, offset_of!(Descriptor, kind));
                let a = self.b.ins().icmp_imm_s(IntCC::Equal, kind, TUPLE as i64);
                let b = self.b.ins().icmp_imm_s(IntCC::Equal, kind, LIST as i64);
                let c = self.b.ins().icmp_imm_s(IntCC::Equal, kind, FLOATS as i64);
                let ab = self.b.ins().bor(a, b);
                let abc = self.b.ins().bor(ab, c);
                let range = self.b.ins().icmp_imm_s(IntCC::Equal, kind, RANGE as i64);
                let valid = self.b.ins().bor(abc, range);
                self.guard(valid, exit)?;
                let virtual_state = self
                    .nest_vars
                    .and_then(|vars| vars.iterators.iter().find(|s| s.iterator == arguments[0]));
                let (index, exhausted, indexptr, exhaustedptr) = if let Some(state) = virtual_state
                {
                    (
                        self.b.use_var(state.index),
                        self.b.use_var(state.exhausted),
                        None,
                        None,
                    )
                } else {
                    let ip = self.field(iterator, offset_of!(Descriptor, index));
                    let index = self.field(ip, 0);
                    let ep = self.field(iterator, offset_of!(Descriptor, exhausted));
                    let exhausted = self.b.ins().load(types::I8, MemFlagsData::new(), ep, 0);
                    let exhausted = self.b.ins().uextend(types::I64, exhausted);
                    (index, exhausted, Some(ip), Some(ep))
                };
                let exhausted = self.b.ins().icmp_imm_s(IntCC::NotEqual, exhausted, 0);
                let len = self.field(source, offset_of!(Descriptor, len));
                let end = self
                    .b
                    .ins()
                    .icmp(IntCC::UnsignedGreaterThanOrEqual, index, len);
                let end = self.b.ins().bor(end, exhausted);
                let done = self.b.create_block();
                let item = self.b.create_block();
                let join = self.b.create_block();
                self.b.ins().brif(end, done, &[], item, &[]);
                self.b.switch_to_block(item);
                self.element(source, index, dst.unwrap(), exit)?;
                let next = self.b.ins().iadd_imm_s(index, 1);
                if let Some(state) = virtual_state {
                    self.b.def_var(state.index, next);
                } else {
                    self.b
                        .ins()
                        .store(MemFlagsData::new(), next, indexptr.unwrap(), 0);
                }
                self.b.ins().jump(join, &[]);
                self.b.switch_to_block(done);
                if let Some(state) = virtual_state {
                    let yes = self.b.ins().iconst(types::I64, 1);
                    self.b.def_var(state.exhausted, yes);
                } else {
                    let yes = self.b.ins().iconst(types::I8, 1);
                    self.b
                        .ins()
                        .store(MemFlagsData::new(), yes, exhaustedptr.unwrap(), 0);
                }
                copy_slots(
                    self.b,
                    self.scalars,
                    self.registers,
                    &[(Register(self.lir.registers.len() + 2), dst.unwrap())],
                )?;
                self.b.ins().jump(join, &[]);
                self.b.switch_to_block(join);
            }
            Operation::Call {
                helper: lir::Helper::Python(crate::hir::CoreOp::IsDone),
                arguments,
                ..
            } => {
                let words = self.words(arguments[0])?;
                let is_ref = self.b.ins().icmp_imm_s(IntCC::Equal, words[0], REF as i64);
                let reference = self.b.create_block();
                let scalar = self.b.create_block();
                let done = self.b.create_block();
                self.b.append_block_param(done, types::I64);
                self.b.ins().brif(is_ref, reference, &[], scalar, &[]);
                self.b.switch_to_block(scalar);
                let no = self.b.ins().iconst(types::I64, 0);
                self.b.ins().jump(done, &[no.into()]);
                self.b.switch_to_block(reference);
                let p = self.resolve_words(words, self.region.entry)?;
                let kind = self.field(p, offset_of!(Descriptor, kind));
                let yes = self.b.ins().icmp_imm_s(IntCC::Equal, kind, DONE as i64);
                let yes = self.b.ins().uextend(types::I64, yes);
                self.b.ins().jump(done, &[yes.into()]);
                self.b.switch_to_block(done);
                let value = self.b.block_params(done)[0];
                store_scalar_cached(
                    self.b,
                    self.scalars,
                    self.registers,
                    dst.unwrap(),
                    BOOL,
                    value,
                )?;
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
}
