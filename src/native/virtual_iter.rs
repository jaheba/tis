//! Non-escaping iterator state stays in SSA until a nest leaves generated code.
use super::*;
use crate::region::{Descriptor, nest::LoopNest};
use std::mem::{offset_of, size_of};
pub(super) struct IteratorVars {
    pub iterator: Register,
    pub index: Variable,
    pub exhausted: Variable,
    pub count: Variable,
}
pub(super) struct NestVars {
    pub active: Variable,
    pub iterators: Vec<IteratorVars>,
}
impl NestVars {
    pub fn new(
        b: &mut FunctionBuilder<'_>,
        plan: &LoopNest,
        lir: &lir::Function,
        active: Variable,
    ) -> Self {
        let iterators = std::iter::once(plan.iterator)
            .chain(plan.children.iter().map(|&i| lir.regions[i].iterator))
            .map(|iterator| {
                let mut var = || {
                    let v = b.declare_var(types::I64);
                    let zero = b.ins().iconst(types::I64, 0);
                    b.def_var(v, zero);
                    v
                };
                IteratorVars {
                    iterator,
                    index: var(),
                    exhausted: var(),
                    count: var(),
                }
            })
            .collect();
        Self { active, iterators }
    }
    fn pointers(
        b: &mut FunctionBuilder<'_>,
        registers: ir::Value,
        lir: &lir::Function,
        iterator: Register,
    ) -> Result<(ir::Value, ir::Value)> {
        // Physical slots contain entry-prepared references, even before their logical Iter definitions.
        let index = load(b, registers, iterator, 16, types::I64)?;
        let base = load(
            b,
            registers,
            Register(lir.registers.len() + 1),
            8,
            types::I64,
        )?;
        let off = b.ins().imul_imm_s(index, size_of::<Descriptor>() as i64);
        let descriptor = b.ins().iadd(base, off);
        let index = b.ins().load(
            types::I64,
            MemFlagsData::new(),
            descriptor,
            offset_of!(Descriptor, index) as i32,
        );
        let exhausted = b.ins().load(
            types::I64,
            MemFlagsData::new(),
            descriptor,
            offset_of!(Descriptor, exhausted) as i32,
        );
        Ok((index, exhausted))
    }
    pub fn initialize(
        &self,
        b: &mut FunctionBuilder<'_>,
        registers: ir::Value,
        lir: &lir::Function,
    ) -> Result<()> {
        for state in &self.iterators {
            let (index, exhausted) = Self::pointers(b, registers, lir, state.iterator)?;
            let index = b.ins().load(types::I64, MemFlagsData::new(), index, 0);
            b.def_var(state.index, index);
            let exhausted = b.ins().load(types::I8, MemFlagsData::new(), exhausted, 0);
            let exhausted = b.ins().uextend(types::I64, exhausted);
            b.def_var(state.exhausted, exhausted);
            let zero = b.ins().iconst(types::I64, 0);
            b.def_var(state.count, zero);
        }
        Ok(())
    }
    pub fn publish(
        &self,
        b: &mut FunctionBuilder<'_>,
        registers: ir::Value,
        lir: &lir::Function,
    ) -> Result<()> {
        let active = b.use_var(self.active);
        let write = b.create_block();
        let done = b.create_block();
        b.ins().brif(active, write, &[], done, &[]);
        b.switch_to_block(write);
        for (i, state) in self.iterators.iter().enumerate() {
            let (indexptr, exhaustedptr) = Self::pointers(b, registers, lir, state.iterator)?;
            let index = b.use_var(state.index);
            let exhausted = b.use_var(state.exhausted);
            let exhausted = b.ins().ireduce(types::I8, exhausted);
            b.ins().store(MemFlagsData::new(), index, indexptr, 0);
            b.ins()
                .store(MemFlagsData::new(), exhausted, exhaustedptr, 0);
            let count = b.use_var(state.count);
            b.ins().store(
                MemFlagsData::new(),
                count,
                registers,
                offset(Register(lir.registers.len() + 3 + i), 8)?,
            );
        }
        b.ins().jump(done, &[]);
        b.switch_to_block(done);
        Ok(())
    }
}
