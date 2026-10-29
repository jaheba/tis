//! Guarded native operations with read forwarding and an ordered, private write buffer.
use super::*;
use cranelift_codegen::ir::{StackSlotData, StackSlotKind};

type GuardKey = (
    Option<lir::StateId>,
    Option<(usize, [ir::Value; 2])>,
    lir::ExitReason,
);

pub(super) struct Emitter<'a, 'b> {
    b: &'a mut FunctionBuilder<'b>,
    plan: &'a Plan,
    host: ir::Value,
    output: ir::Value,
    depth: ir::Value,
    values: Vec<[Variable; 2]>,
    locals: Vec<[Variable; 2]>,
    blocks: Vec<ir::Block>,
    fail: ir::Block,
    self_ref: ir::FuncRef,
    pending: Vec<([ir::Value; 2], ir::Value, [ir::Value; 2])>,
    resume: Option<lir::StateId>,
    field_miss: Option<(usize, [ir::Value; 2])>,
    guard_exits: BTreeMap<GuardKey, ir::Block>,
}
impl<'a, 'b> Emitter<'a, 'b> {
    pub(super) fn emit(
        b: &'a mut FunctionBuilder<'b>,
        self_ref: ir::FuncRef,
        plan: &'a Plan,
    ) -> Result<()> {
        let entry = b.create_block();
        b.append_block_params_for_function_params(entry);
        b.switch_to_block(entry);
        let parameters = b.block_params(entry).to_vec();
        let count = plan
            .tuples
            .values()
            .flatten()
            .map(|r| r.0 + 1)
            .max()
            .unwrap_or(0)
            .max(plan.lir.registers.len());
        let values = (0..count)
            .map(|_| std::array::from_fn(|_| b.declare_var(types::I64)))
            .collect::<Vec<_>>();
        let locals = (0..plan.lir.local_count)
            .map(|_| std::array::from_fn(|_| b.declare_var(types::I64)))
            .collect::<Vec<_>>();
        let blocks = (0..plan.lir.blocks.len())
            .map(|_| b.create_block())
            .collect();
        let fail = b.create_block();
        b.set_cold_block(fail);
        let mut e = Self {
            b,
            plan,
            host: parameters[0],
            output: parameters[2],
            depth: parameters[3],
            values,
            locals,
            blocks,
            fail,
            self_ref,
            pending: vec![],
            resume: None,
            field_miss: None,
            guard_exits: BTreeMap::new(),
        };
        let unbound = e.constant(UNBOUND, 0);
        for vars in e.values.iter().chain(&e.locals) {
            for (var, value) in vars.iter().zip(unbound) {
                e.b.def_var(*var, value);
            }
        }
        for (index, &slot) in plan.parameters.iter().enumerate() {
            for field in 0..2 {
                let v = e.b.ins().load(
                    types::I64,
                    MemFlagsData::new(),
                    parameters[1],
                    (index * 16 + field * 8) as i32,
                );
                e.b.def_var(e.locals[slot][field], v);
            }
        }
        let available = e.b.ins().icmp_imm_s(IntCC::UnsignedGreaterThan, e.depth, 0);
        e.guard(available);
        e.b.ins().jump(e.blocks[0], &[]);
        for (index, block) in plan.lir.blocks.iter().enumerate() {
            e.b.switch_to_block(e.blocks[index]);
            for instruction in &block.instructions {
                if plan.mutation {
                    e.resume = match instruction.operation {
                        Operation::Call { state, .. } | Operation::Truth { state, .. } => {
                            Some(state)
                        }
                        Operation::NumberCompare { failure, .. } => {
                            Some(plan.lir.exits[failure.0].state)
                        }
                        _ => None,
                    };
                }
                e.instruction(instruction)?;
            }
            match &block.terminator {
                Terminator::Return(r) => {
                    for (index, field) in plan.tuples.get(r).into_iter().flatten().enumerate() {
                        let words = e.read(*field);
                        let tag = words[0];
                        let primitive =
                            e.b.ins()
                                .icmp_imm_s(IntCC::UnsignedLessThanOrEqual, tag, FLOAT as i64);
                        e.guard(primitive);
                        for (field, value) in words.into_iter().enumerate() {
                            e.b.ins().store(
                                MemFlagsData::new(),
                                value,
                                e.output,
                                (index * 16 + field * 8) as i32,
                            );
                        }
                    }
                    let zero = e.b.ins().iconst(types::I64, 0);
                    e.b.ins().return_(&[zero]);
                }
                Terminator::Jump(edge) => e.edge(edge),
                Terminator::Branch { condition, yes, no } => {
                    let v = e.read(*condition);
                    let yes_block = e.b.create_block();
                    let no_block = e.b.create_block();
                    e.b.ins().brif(v[1], yes_block, &[], no_block, &[]);
                    e.b.switch_to_block(yes_block);
                    e.edge(yes);
                    e.b.switch_to_block(no_block);
                    e.edge(no);
                }
            }
        }
        e.b.switch_to_block(fail);
        let one = e.b.ins().iconst(types::I64, 1);
        e.b.ins().return_(&[one]);
        e.b.seal_all_blocks();
        Ok(())
    }
    fn constant(&mut self, tag: u64, bits: u64) -> [ir::Value; 2] {
        [
            self.b.ins().iconst(types::I64, tag as i64),
            self.b.ins().iconst(types::I64, bits as i64),
        ]
    }
    fn read(&mut self, r: Register) -> [ir::Value; 2] {
        self.values[r.0].map(|v| self.b.use_var(v))
    }
    fn write(&mut self, r: Register, words: [ir::Value; 2]) {
        for (var, value) in self.values[r.0].into_iter().zip(words) {
            self.b.def_var(var, value);
        }
    }
    fn guard(&mut self, condition: ir::Value) {
        self.guard_reason(condition, lir::ExitReason::TypeGuard);
    }
    fn guard_reason(&mut self, condition: ir::Value, reason: lir::ExitReason) {
        let next = self.b.create_block();
        if self.resume.is_some() || self.field_miss.is_some() {
            let key = (self.resume, self.field_miss, reason);
            // Guards within one HIR operation share the same pre-effect state and cache miss data.
            if let Some(&failed) = self.guard_exits.get(&key) {
                self.b.ins().brif(condition, next, &[], failed, &[]);
                self.b.switch_to_block(next);
                return;
            }
            let failed = self.b.create_block();
            self.b.set_cold_block(failed);
            self.guard_exits.insert(key, failed);
            self.b.ins().brif(condition, next, &[], failed, &[]);
            self.b.switch_to_block(failed);
            if let Some(state) = self.resume {
                let registers = self.field(self.host, offset_of!(Host, registers));
                let snapshot = &self.plan.lir.states[state.0];
                let values = snapshot
                    .values
                    .iter()
                    .map(|(_, r)| *r)
                    .chain(snapshot.local_values.iter().flatten().copied())
                    .collect::<std::collections::BTreeSet<_>>();
                for register in values {
                    let words = self.read(register);
                    for (field, value) in words.into_iter().enumerate() {
                        self.b.ins().store(
                            MemFlagsData::new(),
                            value,
                            registers,
                            (register.0 * 16 + field * 8) as i32,
                        );
                    }
                }
                let reason = self.b.ins().iconst(types::I64, reason as i64);
                self.b.ins().store(
                    MemFlagsData::new(),
                    reason,
                    self.host,
                    offset_of!(Host, exit_reason) as i32,
                );
                let state = self.b.ins().iconst(types::I64, state.0 as i64);
                self.b.ins().store(
                    MemFlagsData::new(),
                    state,
                    self.host,
                    offset_of!(Host, state) as i32,
                );
            }
            if let Some((index, receiver)) = self.field_miss {
                let index = self.b.ins().iconst(types::I64, index as i64);
                self.b.ins().store(
                    MemFlagsData::new(),
                    index,
                    self.host,
                    offset_of!(Host, miss_field) as i32,
                );
                for (field, word) in receiver.into_iter().enumerate() {
                    self.b.ins().store(
                        MemFlagsData::new(),
                        word,
                        self.host,
                        (offset_of!(Host, receiver) + field * 8) as i32,
                    );
                }
            }
            self.b.ins().jump(self.fail, &[]);
        } else {
            self.b.ins().brif(condition, next, &[], self.fail, &[]);
        }
        self.b.switch_to_block(next);
    }
    fn field(&mut self, p: ir::Value, offset: usize) -> ir::Value {
        self.b
            .ins()
            .load(types::I64, MemFlagsData::new(), p, offset as i32)
    }
    // Heap contents and weak caches cannot change until this invocation returns. Buffered writes
    // leave the backing heap untouched, so Cranelift can reuse these reads across buffer stores.
    fn frozen_field(&mut self, p: ir::Value, offset: usize) -> ir::Value {
        self.b.ins().load(
            types::I64,
            MemFlagsData::new().with_readonly(),
            p,
            offset as i32,
        )
    }
    fn tag(&mut self, words: [ir::Value; 2], tag: u64) {
        let valid = self.b.ins().icmp_imm_s(IntCC::Equal, words[0], tag as i64);
        self.guard(valid);
    }
    fn resolve(&mut self, words: [ir::Value; 2]) -> ir::Value {
        let kind = self.b.ins().band_imm_s(words[0], KIND_MASK as i64);
        let is_ref = self.b.ins().icmp_imm_s(IntCC::Equal, kind, REF as i64);
        self.guard(is_ref);
        let owner = self.b.ins().ushr_imm_u(words[0], 32);
        let heap = self.frozen_field(
            self.host,
            offset_of!(Host, directory) + offset_of!(NativeFieldDirectory, heap),
        );
        let valid = self.b.ins().icmp(IntCC::Equal, owner, heap);
        self.guard(valid);
        let index = self.b.ins().band_imm_s(words[1], u32::MAX as i64);
        let len = self.frozen_field(
            self.host,
            offset_of!(Host, directory) + offset_of!(NativeFieldDirectory, len),
        );
        let valid = self.b.ins().icmp(IntCC::UnsignedLessThan, index, len);
        self.guard(valid);
        let base = self.frozen_field(
            self.host,
            offset_of!(Host, directory) + offset_of!(NativeFieldDirectory, entries),
        );
        let offset = self
            .b
            .ins()
            .imul_imm_s(index, size_of::<NativeFields>() as i64);
        let row = self.b.ins().iadd(base, offset);
        let actual = self.frozen_field(row, offset_of!(NativeFields, generation));
        let generation = self.b.ins().ushr_imm_u(words[1], 32);
        let valid = self.b.ins().icmp(IntCC::Equal, actual, generation);
        self.guard(valid);
        row
    }
    fn decode(&mut self, p: ir::Value) -> [ir::Value; 2] {
        let kind = self
            .b
            .ins()
            .load(types::I32, MemFlagsData::new().with_readonly(), p, 0);
        let kind = self.b.ins().uextend(types::I64, kind);
        let numeric = self
            .b
            .ins()
            .icmp_imm_s(IntCC::UnsignedGreaterThanOrEqual, kind, 3);
        let not_ref = self.b.ins().icmp_imm_s(IntCC::UnsignedLessThan, kind, 5);
        let numeric = self.b.ins().band(numeric, not_ref);
        let scalar = self.b.create_block();
        let other = self.b.create_block();
        let done = self.b.create_block();
        self.b.append_block_param(done, types::I64);
        self.b.append_block_param(done, types::I64);
        self.b.ins().brif(numeric, scalar, &[], other, &[]);
        self.b.switch_to_block(scalar);
        let bits = self.frozen_field(p, 8);
        let tag = self.b.ins().iadd_imm_s(kind, -1);
        self.b.ins().jump(done, &[tag.into(), bits.into()]);
        self.b.switch_to_block(other);
        let is_ref = self.b.ins().icmp_imm_s(IntCC::Equal, kind, 5);
        let reference = self.b.create_block();
        let primitive = self.b.create_block();
        self.b.ins().brif(is_ref, reference, &[], primitive, &[]);
        self.b.switch_to_block(reference);
        let heap = self
            .b
            .ins()
            .load(types::I32, MemFlagsData::new().with_readonly(), p, 4);
        let index = self
            .b
            .ins()
            .load(types::I32, MemFlagsData::new().with_readonly(), p, 8);
        let generation = self
            .b
            .ins()
            .load(types::I32, MemFlagsData::new().with_readonly(), p, 12);
        let heap = self.b.ins().uextend(types::I64, heap);
        let heap = self.b.ins().ishl_imm_u(heap, 32);
        let tag = self.b.ins().bor_imm_s(heap, REF as i64);
        let index = self.b.ins().uextend(types::I64, index);
        let generation = self.b.ins().uextend(types::I64, generation);
        let generation = self.b.ins().ishl_imm_u(generation, 32);
        let bits = self.b.ins().bor(index, generation);
        self.b.ins().jump(done, &[tag.into(), bits.into()]);
        self.b.switch_to_block(primitive);
        let boolean = self.b.create_block();
        let none = self.b.create_block();
        let is_bool = self.b.ins().icmp_imm_s(IntCC::Equal, kind, 2);
        self.b.ins().brif(is_bool, boolean, &[], none, &[]);
        self.b.switch_to_block(boolean);
        let bits = self
            .b
            .ins()
            .load(types::I8, MemFlagsData::new().with_readonly(), p, 4);
        let bits = self.b.ins().uextend(types::I64, bits);
        let tag = self.b.ins().iconst(types::I64, BOOL as i64);
        self.b.ins().jump(done, &[tag.into(), bits.into()]);
        self.b.switch_to_block(none);
        let is_none = self.b.ins().icmp_imm_s(IntCC::Equal, kind, 1);
        self.guard(is_none);
        let words = self.constant(NONE, 0);
        self.b.ins().jump(done, &[words[0].into(), words[1].into()]);
        self.b.switch_to_block(done);
        let params = self.b.block_params(done);
        [params[0], params[1]]
    }
    fn number(&mut self, words: [ir::Value; 2]) -> ir::Value {
        let int = self.b.ins().icmp_imm_s(IntCC::Equal, words[0], INT as i64);
        let float = self
            .b
            .ins()
            .icmp_imm_s(IntCC::Equal, words[0], FLOAT as i64);
        let valid = self.b.ins().bor(int, float);
        self.guard(valid);
        // Large mixed integers retain the VM's exact comparison/conversion semantics via replay.
        let low = self.b.ins().icmp_imm_s(
            IntCC::SignedGreaterThanOrEqual,
            words[1],
            -9_007_199_254_740_992,
        );
        let high = self.b.ins().icmp_imm_s(
            IntCC::SignedLessThanOrEqual,
            words[1],
            9_007_199_254_740_992,
        );
        let exact = self.b.ins().band(low, high);
        let valid = self.b.ins().bor(float, exact);
        self.guard(valid);
        let converted = self.b.ins().fcvt_from_sint(types::F64, words[1]);
        let raw = self
            .b
            .ins()
            .bitcast(types::F64, MemFlagsData::new(), words[1]);
        self.b.ins().select(int, converted, raw)
    }
    fn truth(&mut self, words: [ir::Value; 2]) -> ir::Value {
        let primitive =
            self.b
                .ins()
                .icmp_imm_s(IntCC::UnsignedLessThanOrEqual, words[0], FLOAT as i64);
        let fast = self.b.create_block();
        let heap = self.b.create_block();
        let done = self.b.create_block();
        self.b.append_block_param(done, types::I8);
        self.b.ins().brif(primitive, fast, &[], heap, &[]);
        self.b.switch_to_block(fast);
        let float = self
            .b
            .ins()
            .icmp_imm_s(IntCC::Equal, words[0], FLOAT as i64);
        let magnitude = self.b.ins().ishl_imm_u(words[1], 1);
        let bits = self.b.ins().select(float, magnitude, words[1]);
        let truth = self.b.ins().icmp_imm_s(IntCC::NotEqual, bits, 0);
        let not_none = self
            .b
            .ins()
            .icmp_imm_s(IntCC::NotEqual, words[0], NONE as i64);
        let truth = self.b.ins().band(truth, not_none);
        self.b.ins().jump(done, &[truth.into()]);
        self.b.switch_to_block(heap);
        let row = self.resolve(words);
        let kind = self.frozen_field(row, offset_of!(NativeFields, kind));
        let instance = self.b.ins().icmp_imm_s(IntCC::Equal, kind, 1);
        self.guard(instance);
        let truth = self.b.ins().iconst(types::I8, 1);
        self.b.ins().jump(done, &[truth.into()]);
        self.b.switch_to_block(done);
        self.b.block_params(done)[0]
    }
    fn arithmetic(
        &mut self,
        op: &str,
        left: [ir::Value; 2],
        right: [ir::Value; 2],
    ) -> [ir::Value; 2] {
        let left_int = self.b.ins().icmp_imm_s(IntCC::Equal, left[0], INT as i64);
        let right_int = self.b.ins().icmp_imm_s(IntCC::Equal, right[0], INT as i64);
        let both_int = self.b.ins().band(left_int, right_int);
        let integer = self.b.create_block();
        let float = self.b.create_block();
        let done = self.b.create_block();
        self.b.append_block_param(done, types::I64);
        self.b.append_block_param(done, types::I64);
        if op != "/" {
            self.b.ins().brif(both_int, integer, &[], float, &[]);
        } else {
            self.b.ins().jump(float, &[]);
        }
        self.b.switch_to_block(integer);
        if op != "/" {
            let a = self.b.ins().sextend(types::I128, left[1]);
            let b = self.b.ins().sextend(types::I128, right[1]);
            let wide = match op {
                "+" => self.b.ins().iadd(a, b),
                "-" => self.b.ins().isub(a, b),
                _ => self.b.ins().imul(a, b),
            };
            let bits = self.b.ins().ireduce(types::I64, wide);
            let check = self.b.ins().sextend(types::I128, bits);
            let valid = self.b.ins().icmp(IntCC::Equal, wide, check);
            self.guard_reason(valid, lir::ExitReason::IntegerOverflow);
            let tag = self.b.ins().iconst(types::I64, INT as i64);
            self.b.ins().jump(done, &[tag.into(), bits.into()]);
        } else {
            self.b.ins().jump(self.fail, &[]);
        }
        self.b.switch_to_block(float);
        let a = self.number(left);
        let b = self.number(right);
        let value = match op {
            "+" => self.b.ins().fadd(a, b),
            "-" => self.b.ins().fsub(a, b),
            "*" => self.b.ins().fmul(a, b),
            _ => {
                let zero = self.b.ins().f64const(0.0);
                let valid = self.b.ins().fcmp(FloatCC::NotEqual, b, zero);
                self.guard_reason(valid, lir::ExitReason::FloatArithmetic);
                self.b.ins().fdiv(a, b)
            }
        };
        let bits = self.b.ins().bitcast(types::I64, MemFlagsData::new(), value);
        let tag = self.b.ins().iconst(types::I64, FLOAT as i64);
        self.b.ins().jump(done, &[tag.into(), bits.into()]);
        self.b.switch_to_block(done);
        let p = self.b.block_params(done);
        [p[0], p[1]]
    }
    fn compare(&mut self, op: &str, left: [ir::Value; 2], right: [ir::Value; 2]) -> [ir::Value; 2] {
        let done = self.b.create_block();
        self.b.append_block_param(done, types::I64);
        let integer = self.b.create_block();
        let floating = self.b.create_block();
        let objects = self.b.create_block();
        let a = self.b.ins().icmp_imm_s(IntCC::Equal, left[0], INT as i64);
        let b = self.b.ins().icmp_imm_s(IntCC::Equal, right[0], INT as i64);
        let ints = self.b.ins().band(a, b);
        if ["is", "is not"].contains(&op) {
            self.b.ins().jump(objects, &[]);
        } else {
            self.b.ins().brif(ints, integer, &[], floating, &[]);
        }
        self.b.switch_to_block(integer);
        let cc = match op {
            "==" => IntCC::Equal,
            "!=" => IntCC::NotEqual,
            "<" => IntCC::SignedLessThan,
            ">" => IntCC::SignedGreaterThan,
            "<=" => IntCC::SignedLessThanOrEqual,
            _ => IntCC::SignedGreaterThanOrEqual,
        };
        let v = self.b.ins().icmp(cc, left[1], right[1]);
        let v = self.b.ins().uextend(types::I64, v);
        self.b.ins().jump(done, &[v.into()]);
        self.b.switch_to_block(floating);
        let refkind = self.b.ins().band_imm_s(left[0], KIND_MASK as i64);
        let reference = self.b.ins().icmp_imm_s(IntCC::Equal, refkind, REF as i64);
        let numbers = self.b.create_block();
        self.b.ins().brif(reference, objects, &[], numbers, &[]);
        self.b.switch_to_block(numbers);
        let a = self.number(left);
        let b = self.number(right);
        let cc = match op {
            "==" => FloatCC::Equal,
            "!=" => FloatCC::NotEqual,
            "<" => FloatCC::LessThan,
            ">" => FloatCC::GreaterThan,
            "<=" => FloatCC::LessThanOrEqual,
            _ => FloatCC::GreaterThanOrEqual,
        };
        let v = self.b.ins().fcmp(cc, a, b);
        let v = self.b.ins().uextend(types::I64, v);
        self.b.ins().jump(done, &[v.into()]);
        self.b.switch_to_block(objects);
        if !["==", "!=", "is", "is not"].contains(&op) {
            self.b.ins().jump(self.fail, &[]);
        } else {
            // Instance equality in this VM is identity; containers require recursive VM equality.
            let a = self.resolve(left);
            let b = self.resolve(right);
            for row in [a, b] {
                let kind = self.frozen_field(row, offset_of!(NativeFields, kind));
                let valid = self.b.ins().icmp_imm_s(IntCC::Equal, kind, 1);
                self.guard(valid);
            }
            let a = self.b.ins().icmp(IntCC::Equal, left[0], right[0]);
            let b = self.b.ins().icmp(IntCC::Equal, left[1], right[1]);
            let mut v = self.b.ins().band(a, b);
            if ["!=", "is not"].contains(&op) {
                v = self.b.ins().icmp_imm_s(IntCC::Equal, v, 0);
            }
            let v = self.b.ins().uextend(types::I64, v);
            self.b.ins().jump(done, &[v.into()]);
        }
        self.b.switch_to_block(done);
        let bits = self.b.block_params(done)[0];
        [self.b.ins().iconst(types::I64, BOOL as i64), bits]
    }
    fn edge(&mut self, edge: &lir::Edge) {
        let values = edge
            .arguments
            .iter()
            .map(|r| self.read(*r))
            .collect::<Vec<_>>();
        for (&r, value) in self.plan.lir.blocks[edge.block.0]
            .parameters
            .iter()
            .zip(values)
        {
            self.write(r, value);
        }
        self.b.ins().jump(self.blocks[edge.block.0], &[]);
    }
    fn instruction(&mut self, instruction: &lir::Instruction) -> Result<()> {
        let result = instruction.result.unwrap();
        let words = match &instruction.operation {
            Operation::Constant(value) => match value {
                hir::Constant::None => self.constant(NONE, 0),
                hir::Constant::Unbound => self.constant(UNBOUND, 0),
                hir::Constant::Bool(v) => self.constant(BOOL, *v as u64),
                hir::Constant::Int(v) => self.constant(INT, *v as u64),
                hir::Constant::Float(v) => self.constant(FLOAT, v.to_bits()),
                _ => unreachable!(),
            },
            Operation::BoolNot(value) => {
                let words = self.read(*value);
                let inverted = self.b.ins().icmp_imm_s(IntCC::Equal, words[1], 0);
                let bits = self.b.ins().uextend(types::I64, inverted);
                [self.b.ins().iconst(types::I64, BOOL as i64), bits]
            }
            Operation::BoxBool(value) => self.read(*value),
            Operation::NumberCompare {
                value,
                integer,
                operator,
                ..
            } => {
                let left = self.read(*value);
                let right = self.constant(INT, *integer as u64);
                self.compare(operator, left, right)
            }
            Operation::SequenceUnpack { .. } => return Ok(()),
            Operation::SequenceGet {
                value,
                index: lir::SequenceIndex::Constant(index),
                ..
            } => {
                let source = self.plan.tuples[value][*index as usize];
                self.read(source)
            }
            Operation::Truth { value, .. } => {
                let words = self.read(*value);
                let truth = self.truth(words);
                let bits = self.b.ins().uextend(types::I64, truth);
                [self.b.ins().iconst(types::I64, BOOL as i64), bits]
            }
            Operation::Call {
                helper, arguments, ..
            } => match helper {
                Helper::LoadName(_) => {
                    let index = self
                        .plan
                        .globals
                        .iter()
                        .position(|(r, _)| *r == result)
                        .unwrap();
                    let globals = self.field(self.host, offset_of!(Host, globals));
                    [
                        self.frozen_field(globals, index * 16),
                        self.frozen_field(globals, index * 16 + 8),
                    ]
                }
                Helper::GetAttr(attribute) => {
                    let receiver = self.read(arguments[0]);
                    if self.plan.methods.contains_key(&result) {
                        let row = self.resolve(receiver);
                        let kind = self.frozen_field(row, offset_of!(NativeFields, kind));
                        let valid = self.b.ins().icmp_imm_s(IntCC::Equal, kind, 1);
                        self.guard(valid);
                        for (offset, expected) in [
                            (offset_of!(NativeFields, class_tag), offset_of!(Host, class)),
                            (
                                offset_of!(NativeFields, class_payload),
                                offset_of!(Host, class) + 8,
                            ),
                            (offset_of!(NativeFields, shape), offset_of!(Host, shape)),
                        ] {
                            let actual = self.frozen_field(row, offset);
                            let expected = self.field(self.host, expected);
                            let valid = self.b.ins().icmp(IntCC::Equal, actual, expected);
                            self.guard(valid);
                        }
                        receiver
                    } else {
                        let done = self.b.create_block();
                        self.b.append_block_param(done, types::I64);
                        self.b.append_block_param(done, types::I64);
                        if attribute.text == "sqrt" {
                            let module =
                                self.b
                                    .ins()
                                    .icmp_imm_s(IntCC::Equal, receiver[0], MATH as i64);
                            let fast = self.b.create_block();
                            let fields = self.b.create_block();
                            self.b.ins().brif(module, fast, &[], fields, &[]);
                            self.b.switch_to_block(fast);
                            let words = self.constant(SQRT, 0);
                            self.b.ins().jump(done, &[words[0].into(), words[1].into()]);
                            self.b.switch_to_block(fields);
                        }
                        let index = self
                            .plan
                            .fields
                            .iter()
                            .position(|(r, _)| *r == result)
                            .unwrap();
                        let attribute = &self.plan.fields[index].1;
                        self.field_miss = Some((index, receiver));
                        let row = self.resolve(receiver);
                        let shape = self.frozen_field(row, offset_of!(NativeFields, shape));
                        let cache = self
                            .b
                            .ins()
                            .iconst(types::I64, attribute as *const crate::Attribute as i64);
                        let expected = self.frozen_field(cache, 0);
                        let valid = self.b.ins().icmp(IntCC::Equal, shape, expected);
                        self.guard(valid);
                        let nonzero = self.b.ins().icmp_imm_s(IntCC::NotEqual, shape, 0);
                        self.guard(nonzero);
                        let slot = self.frozen_field(cache, 8);
                        let len = self.frozen_field(row, offset_of!(NativeFields, len));
                        let valid = self.b.ins().icmp(IntCC::UnsignedLessThan, slot, len);
                        self.guard(valid);
                        let values = self.frozen_field(row, offset_of!(NativeFields, values));
                        let offset = self.b.ins().ishl_imm_u(slot, 4);
                        let pointer = self.b.ins().iadd(values, offset);
                        let mut words = self.decode(pointer);
                        // Multi-block regions forward from the dynamic prefix, not compile-time block order.
                        if self.plan.mutation && self.plan.lir.blocks.len() > 1 {
                            words = self.forward(receiver, slot, words);
                        } else {
                            for (owner, field, value) in &self.pending {
                                let tag = self.b.ins().icmp(IntCC::Equal, receiver[0], owner[0]);
                                let payload =
                                    self.b.ins().icmp(IntCC::Equal, receiver[1], owner[1]);
                                let same = self.b.ins().band(tag, payload);
                                let field = self.b.ins().icmp(IntCC::Equal, slot, *field);
                                let same = self.b.ins().band(same, field);
                                words = [
                                    self.b.ins().select(same, value[0], words[0]),
                                    self.b.ins().select(same, value[1], words[1]),
                                ];
                            }
                        }
                        self.field_miss = None;
                        self.b.ins().jump(done, &[words[0].into(), words[1].into()]);
                        self.b.switch_to_block(done);
                        let p = self.b.block_params(done);
                        [p[0], p[1]]
                    }
                }
                Helper::SetAttr(_) => {
                    let receiver = self.read(arguments[0]);
                    let value = self.read(arguments[1]);
                    let integer = self.b.ins().icmp_imm_s(IntCC::Equal, value[0], INT as i64);
                    let float = self
                        .b
                        .ins()
                        .icmp_imm_s(IntCC::Equal, value[0], FLOAT as i64);
                    let numeric = self.b.ins().bor(integer, float);
                    self.guard(numeric);
                    let index = self
                        .plan
                        .fields
                        .iter()
                        .position(|(r, _)| *r == result)
                        .unwrap();
                    let attribute = &self.plan.fields[index].1;
                    self.field_miss = Some((index, receiver));
                    let row = self.resolve(receiver);
                    let shape = self.frozen_field(row, offset_of!(NativeFields, shape));
                    let cache = self
                        .b
                        .ins()
                        .iconst(types::I64, attribute as *const crate::Attribute as i64);
                    let expected = self.frozen_field(cache, 0);
                    let valid = self.b.ins().icmp(IntCC::Equal, shape, expected);
                    self.guard(valid);
                    let valid = self.b.ins().icmp_imm_s(IntCC::NotEqual, shape, 0);
                    self.guard(valid);
                    let slot = self.frozen_field(cache, 8);
                    let len = self.frozen_field(row, offset_of!(NativeFields, len));
                    let valid = self.b.ins().icmp(IntCC::UnsignedLessThan, slot, len);
                    self.guard(valid);
                    let count = self.field(self.host, offset_of!(Host, write_count));
                    let valid = self.b.ins().icmp_imm_s(
                        IntCC::UnsignedLessThan,
                        count,
                        self.plan.write_capacity as i64,
                    );
                    self.guard(valid);
                    let writes = self.field(self.host, offset_of!(Host, writes));
                    let offset = self
                        .b
                        .ins()
                        .imul_imm_s(count, size_of::<mutation::Write>() as i64);
                    let writes = self.b.ins().iadd(writes, offset);
                    let offset = 0;
                    for (field, word) in receiver.into_iter().enumerate() {
                        self.b.ins().store(
                            MemFlagsData::new(),
                            word,
                            writes,
                            (offset + offset_of!(mutation::Write, receiver) + field * 8) as i32,
                        );
                    }
                    self.b.ins().store(
                        MemFlagsData::new(),
                        slot,
                        writes,
                        (offset + offset_of!(mutation::Write, slot)) as i32,
                    );
                    for (field, word) in value.into_iter().enumerate() {
                        self.b.ins().store(
                            MemFlagsData::new(),
                            word,
                            writes,
                            (offset + offset_of!(mutation::Write, value) + field * 8) as i32,
                        );
                    }
                    if self.plan.lir.blocks.len() == 1 {
                        self.pending.push((receiver, slot, value));
                    }
                    let count = self.b.ins().iadd_imm_s(count, 1);
                    self.b.ins().store(
                        MemFlagsData::new(),
                        count,
                        self.host,
                        offset_of!(Host, write_count) as i32,
                    );
                    self.field_miss = None;
                    self.constant(NONE, 0)
                }
                Helper::Python(CoreOp::LoadLocal(slot) | CoreOp::CheckLocal(slot)) => {
                    let words = if matches!(helper, Helper::Python(CoreOp::CheckLocal(_))) {
                        self.read(arguments[0])
                    } else {
                        self.locals[*slot].map(|v| self.b.use_var(v))
                    };
                    let valid = self
                        .b
                        .ins()
                        .icmp_imm_s(IntCC::NotEqual, words[0], UNBOUND as i64);
                    self.guard(valid);
                    words
                }
                Helper::Python(CoreOp::StoreLocal(slot)) => {
                    let words = self.read(arguments[0]);
                    for (var, value) in self.locals[*slot].into_iter().zip(words) {
                        self.b.def_var(var, value);
                    }
                    self.constant(NONE, 0)
                }
                Helper::Add | Helper::Subtract => {
                    let a = self.read(arguments[0]);
                    let b = self.read(arguments[1]);
                    self.arithmetic(
                        if matches!(helper, Helper::Add) {
                            "+"
                        } else {
                            "-"
                        },
                        a,
                        b,
                    )
                }
                Helper::Less => {
                    let a = self.read(arguments[0]);
                    let b = self.read(arguments[1]);
                    self.compare("<", a, b)
                }
                Helper::Python(CoreOp::Binary(op) | CoreOp::Inplace(op)) => {
                    let a = self.read(arguments[0]);
                    let b = self.read(arguments[1]);
                    self.arithmetic(op, a, b)
                }
                Helper::Python(CoreOp::Compare(op)) => {
                    let a = self.read(arguments[0]);
                    let b = self.read(arguments[1]);
                    self.compare(op, a, b)
                }
                Helper::Python(CoreOp::Unary(op)) => {
                    if op != "not" {
                        return Err(Error::InvalidIr("unsupported readonly unary"));
                    }
                    let words = self.read(arguments[0]);
                    let truth = self.truth(words);
                    let truth = self.b.ins().icmp_imm_s(IntCC::Equal, truth, 0);
                    let bits = self.b.ins().uextend(types::I64, truth);
                    [self.b.ins().iconst(types::I64, BOOL as i64), bits]
                }
                Helper::Len => {
                    let words = self.read(arguments[0]);
                    self.length(words)
                }
                Helper::GetItem => {
                    let words = self.read(arguments[0]);
                    let index = self.read(arguments[1]);
                    self.tag(index, INT);
                    let row = self.resolve(words);
                    let kind = self.frozen_field(row, offset_of!(NativeFields, kind));
                    let list = self.b.ins().icmp_imm_s(IntCC::Equal, kind, 2);
                    let tuple = self.b.ins().icmp_imm_s(IntCC::Equal, kind, 3);
                    let valid = self.b.ins().bor(list, tuple);
                    self.guard(valid);
                    let len = self.frozen_field(row, offset_of!(NativeFields, len));
                    let negative = self.b.ins().icmp_imm_s(IntCC::SignedLessThan, index[1], 0);
                    let adjusted = self.b.ins().iadd(index[1], len);
                    let index = self.b.ins().select(negative, adjusted, index[1]);
                    let valid = self.b.ins().icmp(IntCC::UnsignedLessThan, index, len);
                    self.guard(valid);
                    let values = self.frozen_field(row, offset_of!(NativeFields, values));
                    let offset = self.b.ins().ishl_imm_u(index, 4);
                    let pointer = self.b.ins().iadd(values, offset);
                    self.decode(pointer)
                }
                Helper::Tuple | Helper::Python(CoreOp::Unpack(_)) => return Ok(()),
                Helper::Python(CoreOp::UnpackItem(index)) => {
                    let source = self.plan.tuples[&arguments[0]][*index];
                    self.read(source)
                }
                Helper::Python(CoreOp::Call(_)) => {
                    if let Some(receiver) = self.plan.recursive.get(&result) {
                        let receiver = self.read(*receiver);
                        let mut inputs = vec![receiver];
                        inputs.extend(arguments[1..].iter().map(|r| self.read(*r)));
                        let args = self.b.create_sized_stack_slot(StackSlotData::new(
                            StackSlotKind::ExplicitSlot,
                            (inputs.len() * 16) as u32,
                            3,
                        ));
                        let outputs = self.b.create_sized_stack_slot(StackSlotData::new(
                            StackSlotKind::ExplicitSlot,
                            (self.plan.arity * 16) as u32,
                            3,
                        ));
                        for (index, words) in inputs.into_iter().enumerate() {
                            for (field, value) in words.into_iter().enumerate() {
                                self.b.ins().stack_store(
                                    types::I64,
                                    value,
                                    args,
                                    (index * 16 + field * 8) as i32,
                                );
                            }
                        }
                        let args = self.b.ins().stack_addr(types::I64, args, 0);
                        let out = self.b.ins().stack_addr(types::I64, outputs, 0);
                        let depth = self.b.ins().iadd_imm_s(self.depth, -1);
                        let call = self
                            .b
                            .ins()
                            .call(self.self_ref, &[self.host, args, out, depth]);
                        let status = self.b.inst_results(call)[0];
                        let valid = self.b.ins().icmp_imm_s(IntCC::Equal, status, 0);
                        self.guard(valid);
                        for (index, &r) in self.plan.tuples[&result].iter().enumerate() {
                            let words =
                                [self.field(out, index * 16), self.field(out, index * 16 + 8)];
                            self.write(r, words);
                        }
                        return Ok(());
                    }
                    let callable = self.read(arguments[0]);
                    let argument = self.read(arguments[1]);
                    let len = self
                        .b
                        .ins()
                        .icmp_imm_s(IntCC::Equal, callable[0], LEN as i64);
                    let length = self.b.create_block();
                    let square = self.b.create_block();
                    let done = self.b.create_block();
                    self.b.append_block_param(done, types::I64);
                    self.b.append_block_param(done, types::I64);
                    self.b.ins().brif(len, length, &[], square, &[]);
                    self.b.switch_to_block(length);
                    let words = self.length(argument);
                    self.b.ins().jump(done, &[words[0].into(), words[1].into()]);
                    self.b.switch_to_block(square);
                    self.tag(callable, SQRT);
                    let number = self.number(argument);
                    let zero = self.b.ins().f64const(0.0);
                    let negative = self.b.ins().fcmp(FloatCC::LessThan, number, zero);
                    let valid = self.b.ins().icmp_imm_s(IntCC::Equal, negative, 0);
                    self.guard(valid);
                    let number = self.b.ins().sqrt(number);
                    let bits = self
                        .b
                        .ins()
                        .bitcast(types::I64, MemFlagsData::new(), number);
                    let tag = self.b.ins().iconst(types::I64, FLOAT as i64);
                    self.b.ins().jump(done, &[tag.into(), bits.into()]);
                    self.b.switch_to_block(done);
                    let p = self.b.block_params(done);
                    [p[0], p[1]]
                }
                _ => unreachable!("verified readonly helper"),
            },
            _ => unreachable!("generic readonly LIR"),
        };
        self.write(result, words);
        Ok(())
    }
    fn forward(
        &mut self,
        receiver: [ir::Value; 2],
        slot: ir::Value,
        initial: [ir::Value; 2],
    ) -> [ir::Value; 2] {
        let count = self.field(self.host, offset_of!(Host, write_count));
        let valid = self.b.ins().icmp_imm_s(
            IntCC::UnsignedLessThanOrEqual,
            count,
            self.plan.write_capacity as i64,
        );
        self.guard(valid);
        let writes = self.field(self.host, offset_of!(Host, writes));
        let header = self.b.create_block();
        let body = self.b.create_block();
        let done = self.b.create_block();
        for _ in 0..3 {
            self.b.append_block_param(header, types::I64);
        }
        for _ in 0..2 {
            self.b.append_block_param(done, types::I64);
        }
        let zero = self.b.ins().iconst(types::I64, 0);
        self.b
            .ins()
            .jump(header, &[zero.into(), initial[0].into(), initial[1].into()]);
        self.b.switch_to_block(header);
        let params = self.b.block_params(header).to_vec();
        let index = params[0];
        let words = [params[1], params[2]];
        let more = self.b.ins().icmp(IntCC::UnsignedLessThan, index, count);
        self.b
            .ins()
            .brif(more, body, &[], done, &[words[0].into(), words[1].into()]);
        self.b.switch_to_block(body);
        let offset = self
            .b
            .ins()
            .imul_imm_s(index, size_of::<mutation::Write>() as i64);
        let write = self.b.ins().iadd(writes, offset);
        let owner = [
            self.field(write, offset_of!(mutation::Write, receiver)),
            self.field(write, offset_of!(mutation::Write, receiver) + 8),
        ];
        let field = self.field(write, offset_of!(mutation::Write, slot));
        let value = [
            self.field(write, offset_of!(mutation::Write, value)),
            self.field(write, offset_of!(mutation::Write, value) + 8),
        ];
        let tag = self.b.ins().icmp(IntCC::Equal, receiver[0], owner[0]);
        let payload = self.b.ins().icmp(IntCC::Equal, receiver[1], owner[1]);
        let same = self.b.ins().band(tag, payload);
        let field = self.b.ins().icmp(IntCC::Equal, slot, field);
        let same = self.b.ins().band(same, field);
        let words = [
            self.b.ins().select(same, value[0], words[0]),
            self.b.ins().select(same, value[1], words[1]),
        ];
        let next = self.b.ins().iadd_imm_s(index, 1);
        self.b
            .ins()
            .jump(header, &[next.into(), words[0].into(), words[1].into()]);
        self.b.switch_to_block(done);
        let p = self.b.block_params(done);
        [p[0], p[1]]
    }
    fn length(&mut self, words: [ir::Value; 2]) -> [ir::Value; 2] {
        let row = self.resolve(words);
        let kind = self.frozen_field(row, offset_of!(NativeFields, kind));
        let low = self
            .b
            .ins()
            .icmp_imm_s(IntCC::UnsignedGreaterThanOrEqual, kind, 2);
        let high = self
            .b
            .ins()
            .icmp_imm_s(IntCC::UnsignedLessThanOrEqual, kind, 4);
        let valid = self.b.ins().band(low, high);
        self.guard(valid);
        let len = self.frozen_field(row, offset_of!(NativeFields, len));
        [self.b.ins().iconst(types::I64, INT as i64), len]
    }
}
