//! Fresh native entries copy invariant frame-local inputs before running generated code.
use super::*;
use std::mem::offset_of;

/// Fast reads reuse the seeded boxed register. OSR, unbound values and reopened feedback
/// retain the original LoadLocal helper, including its error and observation order.
pub(super) fn probe(
    b: &mut FunctionBuilder<'_>,
    scalars: &ScalarVars,
    host: ir::Value,
    registers: ir::Value,
    result: Register,
    metadata: Register,
    done: ir::Block,
) -> Result<()> {
    let ready = load(b, registers, metadata, 0, types::I64)?;
    let ready = b.ins().icmp_imm_s(IntCC::NotEqual, ready, 0);
    let collecting = b.ins().load(
        types::I8,
        MemFlagsData::new(),
        host,
        offset_of!(crate::native_runtime::Context<'_>, collecting) as i32,
    );
    let quiet = b.ins().icmp_imm_s(IntCC::Equal, collecting, 0);
    let enabled = b.ins().band(ready, quiet);
    let tag = load_cached(b, scalars, registers, result, 0, types::I64)?;
    let bound = b.ins().icmp_imm_s(IntCC::NotEqual, tag, UNBOUND as i64);
    let hit = b.ins().band(enabled, bound);
    let fast = b.create_block();
    let miss = b.create_block();
    b.ins().brif(hit, fast, &[], miss, &[]);
    b.switch_to_block(fast);
    b.ins().jump(done, &[]);
    b.switch_to_block(miss);
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::{
        Tiering, Vm,
        hir::{Constant, CoreOp, Function, Op, Terminator},
    };

    #[test]
    fn an_unbound_invariant_load_raises_after_prior_effects() {
        let mut f = Function::new(0);
        f.local_names = vec!["missing".into(), "later".into()];
        f.local_slots = f.local_names.clone();
        let block = f.entry();
        let print = f.append(block, Op::Runtime(CoreOp::LoadName("print".into()), vec![]));
        let message = f.append(block, Op::Constant(Constant::String("before".into())));
        f.append(
            block,
            Op::Runtime(CoreOp::Call(vec![None]), vec![print, message]),
        );
        let value = f.append(block, Op::Runtime(CoreOp::LoadLocal(0), vec![]));
        f.append(block, Op::Runtime(CoreOp::LoadLocal(1), vec![]));
        f.terminate(block, Terminator::Return(value));
        let mut vm = Vm::new();
        vm.tiering = Tiering::Native;
        vm.thresholds.function_entries = 1;
        vm.profiling.window_ticks = 2;
        for _ in 0..20 {
            let error = vm.run(&mut f, &[]).unwrap_err();
            assert!(
                error.to_string().contains("UnboundLocalError: missing"),
                "{error}"
            );
        }
        assert!(vm.tier_stats.native_entries > 0);
        assert!(
            vm.native_artifacts
                .values()
                .any(|code| code.entry_locals.len() >= 2)
        );
        assert_eq!(vm.output, "before\n".repeat(20));
        assert_eq!(vm.stack_len(), 0);
    }
}
