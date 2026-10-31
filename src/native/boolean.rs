//! Whole Boolean-list loops selected by the same LIR plans as copy-and-patch.
use super::*;
use crate::region::{fill::BoolFill, reduce::BoolReduce};
use std::mem::offset_of;

pub(super) struct Emitter<'a, 'b> {
    pub b: &'a mut FunctionBuilder<'b>,
    pub scalars: &'a ScalarVars,
    pub host: ir::Value,
    pub registers: ir::Value,
}

struct Input {
    vm: ir::Value,
    cursor: ir::Value,
    size: ir::Value,
    count: ir::Value,
    first: ir::Value,
    last: ir::Value,
    step: ir::Value,
    length: ir::Value,
    data: ir::Value,
    counter: Option<(Register, Register, ir::Value)>,
}

impl Emitter<'_, '_> {
    fn guard(&mut self, condition: ir::Value, miss: ir::Block) {
        let next = self.b.create_block();
        self.b.ins().brif(condition, next, &[], miss, &[]);
        self.b.switch_to_block(next);
    }

    fn field(&mut self, pointer: ir::Value, offset: usize) -> ir::Value {
        self.b
            .ins()
            .load(types::I64, MemFlagsData::new(), pointer, offset as i32)
    }

    fn small(&mut self, wide: ir::Value, miss: ir::Block) -> ir::Value {
        let value = self.b.ins().ireduce(types::I64, wide);
        let restored = self.b.ins().sextend(types::I128, value);
        let valid = self.b.ins().icmp(IntCC::Equal, wide, restored);
        self.guard(valid, miss);
        value
    }

    fn input(
        &mut self,
        iterator: Register,
        list: Register,
        writable: bool,
        miss: ir::Block,
    ) -> Result<Input> {
        let skip = self.b.ins().load(
            types::I8,
            MemFlagsData::new(),
            self.host,
            offset_of!(NativeHeader, skip_probe) as i32,
        );
        let enabled = self.b.ins().icmp_imm_s(IntCC::Equal, skip, 0);
        self.guard(enabled, miss);
        // checked_row also excludes reopened profiling and validates heap identity/generation.
        let (vm, row) = fields::checked_row(
            self.b,
            self.scalars,
            self.host,
            self.registers,
            iterator,
            miss,
        )?;
        let kind = self.field(row, offset_of!(NativeFields, kind));
        let cached_range = self.b.ins().icmp_imm_s(IntCC::Equal, kind, 7);
        self.guard(cached_range, miss);
        let cursor = self.field(row, offset_of!(NativeFields, shape));
        let nonnull = self.b.ins().icmp_imm_s(IntCC::NotEqual, cursor, 0);
        self.guard(nonnull, miss);
        let position = self.field(cursor, 0);
        let size = self.field(row, offset_of!(NativeFields, len));
        let remaining = self.b.ins().icmp(IntCC::UnsignedLessThan, position, size);
        self.guard(remaining, miss);
        let start = self.field(row, offset_of!(NativeFields, class_tag));
        let step = self.field(row, offset_of!(NativeFields, class_payload));
        let start = self.b.ins().sextend(types::I128, start);
        let wide_step = self.b.ins().sextend(types::I128, step);
        let wide_position = self.b.ins().uextend(types::I128, position);
        let product = self.b.ins().imul(wide_step, wide_position);
        let first_wide = self.b.ins().iadd(start, product);
        let last_position = self.b.ins().iadd_imm_s(size, -1);
        let last_position = self.b.ins().uextend(types::I128, last_position);
        let product = self.b.ins().imul(wide_step, last_position);
        let last_wide = self.b.ins().iadd(start, product);
        let first = self.small(first_wide, miss);
        let last = self.small(last_wide, miss);
        let (_, list_row) =
            fields::checked_row(self.b, self.scalars, self.host, self.registers, list, miss)?;
        let kind = self.field(list_row, offset_of!(NativeFields, kind));
        let boolean = self.b.ins().icmp_imm_s(IntCC::Equal, kind, 8);
        self.guard(boolean, miss);
        let length = self.field(list_row, offset_of!(NativeFields, len));
        let wide_length = self.b.ins().uextend(types::I128, length);
        let negative_length = self.b.ins().ineg(wide_length);
        for endpoint in [first_wide, last_wide] {
            let lower =
                self.b
                    .ins()
                    .icmp(IntCC::SignedGreaterThanOrEqual, endpoint, negative_length);
            self.guard(lower, miss);
            let upper = self
                .b
                .ins()
                .icmp(IntCC::SignedLessThan, endpoint, wide_length);
            self.guard(upper, miss);
        }
        let data = self.field(
            list_row,
            if writable {
                offset_of!(NativeFields, writable_values)
            } else {
                offset_of!(NativeFields, values)
            },
        );
        let nonnull = self.b.ins().icmp_imm_s(IntCC::NotEqual, data, 0);
        self.guard(nonnull, miss);
        if writable {
            let token = self.field(list_row, offset_of!(NativeFields, shape));
            let nonnull = self.b.ins().icmp_imm_s(IntCC::NotEqual, token, 0);
            self.guard(nonnull, miss);
            let permission = self.b.ins().load(types::I8, MemFlagsData::new(), token, 0);
            let allowed = self.b.ins().icmp_imm_s(IntCC::NotEqual, permission, 0);
            self.guard(allowed, miss);
        }
        let count = self.b.ins().isub(size, position);
        Ok(Input {
            vm,
            cursor,
            size,
            count,
            first,
            last,
            step,
            length,
            data,
            counter: None,
        })
    }

    fn counter_input(
        &mut self,
        counter: Register,
        list: Register,
        plan: crate::region::fill::Counter,
        miss: ir::Block,
    ) -> Result<Input> {
        let skip = self.b.ins().load(
            types::I8,
            MemFlagsData::new(),
            self.host,
            offset_of!(NativeHeader, skip_probe) as i32,
        );
        let enabled = self.b.ins().icmp_imm_s(IntCC::Equal, skip, 0);
        self.guard(enabled, miss);
        let mut values = vec![];
        for register in [counter, plan.limit, plan.step] {
            let tag = load_cached(
                self.b,
                self.scalars,
                self.registers,
                register,
                0,
                types::I64,
            )?;
            let valid = self.b.ins().icmp_imm_s(IntCC::Equal, tag, INT as i64);
            self.guard(valid, miss);
            values.push(load_cached(
                self.b,
                self.scalars,
                self.registers,
                register,
                8,
                types::I64,
            )?);
        }
        let [first, limit, step] = values.as_slice() else {
            unreachable!()
        };
        let (first, limit, step) = (*first, *limit, *step);
        // Evaluate affine bounds only after exact integer guards, before any write.
        // Overflow retains the original arbitrary-precision header evaluation.
        let limit = if plan.limit_add != 0 {
            let wide = self.b.ins().sextend(types::I128, limit);
            let offset = self.b.ins().iconst(types::I64, plan.limit_add);
            let offset = self.b.ins().sextend(types::I128, offset);
            let sum = self.b.ins().iadd(wide, offset);
            self.small(sum, miss)
        } else {
            limit
        };
        let sign = self.b.ins().icmp_imm_s(
            if plan.descending {
                IntCC::SignedLessThan
            } else {
                IntCC::SignedGreaterThan
            },
            step,
            0,
        );
        self.guard(sign, miss);
        let relation = match (plan.descending, plan.inclusive) {
            (false, false) => IntCC::SignedLessThan,
            (false, true) => IntCC::SignedLessThanOrEqual,
            (true, false) => IntCC::SignedGreaterThan,
            (true, true) => IntCC::SignedGreaterThanOrEqual,
        };
        let nonempty = self.b.ins().icmp(relation, first, limit);
        self.guard(nonempty, miss);
        let first_wide = self.b.ins().sextend(types::I128, first);
        let limit_wide = self.b.ins().sextend(types::I128, limit);
        let step_wide = self.b.ins().sextend(types::I128, step);
        let distance = if plan.descending {
            self.b.ins().isub(first_wide, limit_wide)
        } else {
            self.b.ins().isub(limit_wide, first_wide)
        };
        let distance = if plan.inclusive {
            distance
        } else {
            self.b.ins().iadd_imm_s(distance, -1)
        };
        let distance = self.small(distance, miss);
        let magnitude = if plan.descending {
            self.b.ins().ineg(step)
        } else {
            step
        };
        let count = self.b.ins().udiv(distance, magnitude);
        let count = self.b.ins().iadd_imm_s(count, 1);
        let positive = self.b.ins().icmp_imm_s(IntCC::SignedGreaterThan, count, 0);
        self.guard(positive, miss);
        let wide_count = self.b.ins().uextend(types::I128, count);
        let product = self.b.ins().imul(wide_count, step_wide);
        let end_wide = self.b.ins().iadd(first_wide, product);
        let end = self.small(end_wide, miss);
        let last_wide = self.b.ins().isub(end_wide, step_wide);
        let last = self.small(last_wide, miss);
        // Row checks reject profiling and stale identities before exposing any mutable pointer.
        let (vm, row) =
            fields::checked_row(self.b, self.scalars, self.host, self.registers, list, miss)?;
        let kind = self.field(row, offset_of!(NativeFields, kind));
        let boolean = self.b.ins().icmp_imm_s(IntCC::Equal, kind, 8);
        self.guard(boolean, miss);
        let length = self.field(row, offset_of!(NativeFields, len));
        let wide_length = self.b.ins().uextend(types::I128, length);
        let negative_length = self.b.ins().ineg(wide_length);
        for endpoint in [first_wide, last_wide] {
            let lower =
                self.b
                    .ins()
                    .icmp(IntCC::SignedGreaterThanOrEqual, endpoint, negative_length);
            self.guard(lower, miss);
            let upper = self
                .b
                .ins()
                .icmp(IntCC::SignedLessThan, endpoint, wide_length);
            self.guard(upper, miss);
        }
        let data = self.field(row, offset_of!(NativeFields, writable_values));
        let nonnull = self.b.ins().icmp_imm_s(IntCC::NotEqual, data, 0);
        self.guard(nonnull, miss);
        let token = self.field(row, offset_of!(NativeFields, shape));
        let nonnull = self.b.ins().icmp_imm_s(IntCC::NotEqual, token, 0);
        self.guard(nonnull, miss);
        let permission = self.b.ins().load(types::I8, MemFlagsData::new(), token, 0);
        let allowed = self.b.ins().icmp_imm_s(IntCC::NotEqual, permission, 0);
        self.guard(allowed, miss);
        let zero = self.b.ins().iconst(types::I64, 0);
        Ok(Input {
            vm,
            cursor: zero,
            size: count,
            count,
            first,
            last,
            step,
            length,
            data,
            counter: Some((counter, plan.update, end)),
        })
    }

    fn pointer(&mut self, input: &Input, index: ir::Value) -> ir::Value {
        let negative = self.b.ins().icmp_imm_s(IntCC::SignedLessThan, index, 0);
        let adjusted = self.b.ins().iadd(index, input.length);
        let offset = self.b.ins().select(negative, adjusted, index);
        self.b.ins().iadd(input.data, offset)
    }

    fn account(&mut self, input: &Input, edge: usize, blocks: ir::Value) -> Result<()> {
        let edge_offset = edge.checked_mul(8).ok_or(Error::IntegerOverflow)?;
        let edge_offset = i32::try_from(edge_offset).map_err(error)?;
        if let Some((counter, update, end)) = input.counter {
            store_scalar_cached(self.b, self.scalars, self.registers, counter, INT, end)?;
            store_scalar_cached(self.b, self.scalars, self.registers, update, INT, end)?;
        } else {
            self.b
                .ins()
                .store(MemFlagsData::new(), input.size, input.cursor, 0);
        }
        let counts = self.field(self.host, offset_of!(NativeHeader, quiet_counts));
        let count = self.field(counts, edge_offset as usize);
        let next = self.b.ins().iadd(count, input.count);
        let overflow = self.b.ins().icmp(IntCC::UnsignedLessThan, next, count);
        let full = self.b.ins().iconst(types::I64, -1);
        let next = self.b.ins().select(overflow, full, next);
        self.b
            .ins()
            .store(MemFlagsData::new(), next, counts, edge_offset);
        let previous = self.field(self.host, offset_of!(NativeHeader, inline_block_hits));
        let total = self.b.ins().iadd(previous, blocks);
        self.b.ins().store(
            MemFlagsData::new(),
            total,
            self.host,
            offset_of!(NativeHeader, inline_block_hits) as i32,
        );
        for (offset, increment) in [
            (
                offset_of!(crate::tier::TierStats, native_region_entries),
                self.b.ins().iconst(types::I64, 1),
            ),
            (
                offset_of!(crate::tier::TierStats, native_region_iterations),
                input.count,
            ),
        ] {
            let offset = offset_of!(Vm, tier_stats) + offset;
            let old = self.field(input.vm, offset);
            let new = self.b.ins().iadd(old, increment);
            self.b
                .ins()
                .store(MemFlagsData::new(), new, input.vm, offset as i32);
        }
        Ok(())
    }

    pub fn fill(&mut self, plan: &BoolFill, edge: usize) -> Result<()> {
        for (register, constant) in &plan.literals {
            let value = Slot::boxed(
                constant
                    .immediate()
                    .ok_or(Error::InvalidIr("fill literal"))?,
            );
            let payload = self.b.ins().iconst(types::I64, value.a as i64);
            store_scalar_cached(
                self.b,
                self.scalars,
                self.registers,
                *register,
                value.tag,
                payload,
            )?;
        }
        let miss = self.b.create_block();
        let finish = self.b.create_block();
        let input = if let Some(counter) = plan.counter {
            self.counter_input(plan.iterator, plan.list, counter, miss)?
        } else {
            self.input(plan.iterator, plan.list, true, miss)?
        };
        let body = self.b.create_block();
        self.b.append_block_param(body, types::I64);
        self.b.append_block_param(body, types::I64);
        self.b
            .ins()
            .jump(body, &[input.first.into(), input.count.into()]);
        self.b.switch_to_block(body);
        let index = self.b.block_params(body)[0];
        let remaining = self.b.block_params(body)[1];
        let pointer = self.pointer(&input, index);
        let fill = self.b.ins().iconst(types::I8, i64::from(plan.value));
        // Exact bounds/storage were guarded before the first write; no call can invalidate them.
        self.b.ins().store(MemFlagsData::new(), fill, pointer, 0);
        let remaining = self.b.ins().iadd_imm_s(remaining, -1);
        let next = self.b.ins().iadd(index, input.step);
        let commit = self.b.create_block();
        self.b.ins().brif(
            remaining,
            body,
            &[next.into(), remaining.into()],
            commit,
            &[],
        );
        self.b.switch_to_block(commit);
        if let Some(last) = plan.last {
            store_scalar_cached(self.b, self.scalars, self.registers, last, INT, input.last)?;
        }
        let value = self.b.ins().iconst(types::I64, i64::from(plan.value));
        store_scalar_cached(
            self.b,
            self.scalars,
            self.registers,
            plan.constant,
            BOOL,
            value,
        )?;
        if let Some(result) = plan.result {
            let zero = self.b.ins().iconst(types::I64, 0);
            store_scalar_cached(self.b, self.scalars, self.registers, result, NONE, zero)?;
        }
        let blocks = self.b.ins().imul_imm_s(input.count, 2);
        self.account(&input, edge, blocks)?;
        self.b.ins().jump(finish, &[]);
        self.b.switch_to_block(miss);
        self.b.ins().jump(finish, &[]);
        self.b.switch_to_block(finish);
        Ok(())
    }

    pub fn reduce(&mut self, plan: &BoolReduce, edge: usize) -> Result<()> {
        let miss = self.b.create_block();
        let finish = self.b.create_block();
        let input = self.input(plan.iterator, plan.list, false, miss)?;
        let zero = self.b.ins().iconst(types::I64, 0);
        let minimum = self.b.ins().smin(input.first, input.last);
        let minimum = self.b.ins().smin(minimum, zero);
        let maximum = self.b.ins().smax(input.first, input.last);
        let maximum = self.b.ins().smax(maximum, zero);
        let wide_count = self.b.ins().uextend(types::I128, input.count);
        let low = self.b.ins().sextend(types::I128, minimum);
        let low = self.b.ins().imul(low, wide_count);
        let high = self.b.ins().sextend(types::I128, maximum);
        let high = self.b.ins().imul(high, wide_count);
        let mut initial = Vec::new();
        for &(register, kind) in &plan.outputs {
            let value = if kind >= 3 {
                let tag = load_cached(
                    self.b,
                    self.scalars,
                    self.registers,
                    register,
                    0,
                    types::I64,
                )?;
                let tag = self.b.ins().band_imm_s(tag, KIND_MASK as i64);
                let integer = self.b.ins().icmp_imm_s(IntCC::Equal, tag, INT as i64);
                self.guard(integer, miss);
                let value = load_cached(
                    self.b,
                    self.scalars,
                    self.registers,
                    register,
                    8,
                    types::I64,
                )?;
                let wide = self.b.ins().sextend(types::I128, value);
                let lower = if kind == 3 {
                    wide
                } else {
                    self.b.ins().iadd(wide, low)
                };
                self.small(lower, miss);
                let bound = if kind == 3 { wide_count } else { high };
                let upper = self.b.ins().iadd(wide, bound);
                self.small(upper, miss);
                value
            } else {
                zero
            };
            initial.push(value);
        }
        let body = self.b.create_block();
        for _ in 0..4 + initial.len() {
            self.b.append_block_param(body, types::I64);
        }
        let mut arguments = vec![input.first, input.count, zero, zero];
        arguments.extend(initial);
        self.b.ins().jump(
            body,
            &arguments
                .iter()
                .copied()
                .map(Into::into)
                .collect::<Vec<_>>(),
        );
        self.b.switch_to_block(body);
        let values = self.b.block_params(body).to_vec();
        let (index, remaining, matches, selected) = (values[0], values[1], values[2], values[3]);
        let pointer = self.pointer(&input, index);
        let byte = self
            .b
            .ins()
            .load(types::I8, MemFlagsData::new(), pointer, 0);
        let present = self.b.ins().icmp_imm_s(IntCC::NotEqual, byte, 0);
        let chosen = self.b.create_block();
        let advance = self.b.create_block();
        for _ in 0..2 + plan.outputs.len() {
            self.b.append_block_param(advance, types::I64);
        }
        // Most predicates can be false. Keep additions and last-selected updates on the
        // selected arm, rather than carrying checked add-zero work through every element.
        let mut unchanged = vec![matches, selected];
        unchanged.extend_from_slice(&values[4..]);
        self.b.ins().brif(
            present,
            chosen,
            &[],
            advance,
            &unchanged
                .iter()
                .copied()
                .map(Into::into)
                .collect::<Vec<_>>(),
        );
        self.b.switch_to_block(chosen);
        let matches_next = self.b.ins().iadd_imm_s(matches, 1);
        let mut updated = vec![matches_next, index];
        for (n, &(_, kind)) in plan.outputs.iter().enumerate() {
            let value = values[n + 4];
            let updated_value = match kind {
                3 | 4 => {
                    let amount = if kind == 3 {
                        self.b.ins().iconst(types::I64, 1)
                    } else {
                        index
                    };
                    let (sum, overflow) = self.b.ins().sadd_overflow(value, amount);
                    let valid = self.b.ins().icmp_imm_s(IntCC::Equal, overflow, 0);
                    self.guard(valid, miss);
                    sum
                }
                _ => value,
            };
            updated.push(updated_value);
        }
        self.b.ins().jump(
            advance,
            &updated.iter().copied().map(Into::into).collect::<Vec<_>>(),
        );
        self.b.switch_to_block(advance);
        let accumulated = self.b.block_params(advance).to_vec();
        let matches_next = accumulated[0];
        let selected_next = accumulated[1];
        let mut next = vec![
            self.b.ins().iadd(index, input.step),
            self.b.ins().iadd_imm_s(remaining, -1),
            matches_next,
            selected_next,
        ];
        next.extend_from_slice(&accumulated[2..]);
        let commit = self.b.create_block();
        self.b.ins().brif(
            next[1],
            body,
            &next.iter().copied().map(Into::into).collect::<Vec<_>>(),
            commit,
            &[],
        );
        self.b.switch_to_block(commit);
        // The scan is read-only. Overflow/type misses above replay the untouched original loop.
        for (n, &(register, kind)) in plan.outputs.iter().enumerate() {
            let value = match kind {
                1 => input.last,
                2 => selected_next,
                3 | 4 => next[n + 4],
                _ => continue,
            };
            if kind == 2 {
                let publish = self.b.create_block();
                let keep = self.b.create_block();
                self.b.ins().brif(matches_next, publish, &[], keep, &[]);
                self.b.switch_to_block(publish);
                store_scalar_cached(self.b, self.scalars, self.registers, register, INT, value)?;
                self.b.ins().jump(keep, &[]);
                self.b.switch_to_block(keep);
            } else {
                store_scalar_cached(self.b, self.scalars, self.registers, register, INT, value)?;
            }
        }
        let selected_blocks = self
            .b
            .ins()
            .imul_imm_s(matches_next, plan.true_blocks as i64);
        let others = self.b.ins().isub(input.count, matches_next);
        let other_blocks = self.b.ins().imul_imm_s(others, plan.false_blocks as i64);
        let blocks = self.b.ins().iadd(selected_blocks, other_blocks);
        self.account(&input, edge, blocks)?;
        self.b.ins().jump(finish, &[]);
        self.b.switch_to_block(miss);
        self.b.ins().jump(finish, &[]);
        self.b.switch_to_block(finish);
        Ok(())
    }
}
