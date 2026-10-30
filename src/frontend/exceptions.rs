//! Protected suites return an explicit control outcome through ordinary HIR frames.
use super::*;

// Outcomes are (normal/return/break/continue/raise, value). They stay rooted like any tuple.
impl Builder {
    fn fragment_builder(&self) -> Result<Self> {
        let mut fragment = Self::new(&[], false)?;
        fragment.control = true;
        fragment.super_parameter = self.super_parameter.clone();
        fragment.external_loop = self.external_loop || !self.loops.is_empty();
        Ok(fragment)
    }

    fn flow(&mut self, block: BlockId, kind: i64, value: ValueId) -> ValueId {
        let kind = self.constant(block, Constant::Int(kind));
        self.function.append(block, Op::Tuple(vec![kind, value]))
    }

    fn field(&mut self, block: BlockId, value: ValueId, index: i64) -> ValueId {
        let index = self.constant(block, Constant::Int(index));
        self.function.append(block, Op::GetItem(value, index))
    }

    fn is_flow(&mut self, block: BlockId, kind: ValueId, expected: i64) -> ValueId {
        let expected = self.constant(block, Constant::Int(expected));
        self.runtime(block, CoreOp::Compare("==".into()), vec![kind, expected])
    }

    fn finish_fragment(&mut self, block: Option<BlockId>) {
        if let Some(block) = block {
            let value = self.none(block);
            let flow = self.flow(block, 0, value);
            self.function.terminate(block, Terminator::Return(flow));
        }
    }

    fn protect_suite(&mut self, block: BlockId, body: &[Stmt]) -> Result<ValueId> {
        let mut fragment = self.fragment_builder()?;
        let end = fragment.statements(body, Some(fragment.function.entry()))?;
        fragment.finish_fragment(end);
        Ok(self.runtime(block, CoreOp::TryCall(Box::new(fragment.function)), vec![]))
    }

    fn edge_value(&mut self, from: BlockId, to: BlockId, value: ValueId) {
        self.function
            .terminate(from, Terminator::Jump(Edge::new(to, vec![value])));
    }

    fn normalize_outcome(&mut self, block: BlockId, outcome: ValueId) -> (BlockId, ValueId) {
        let success = self.field(block, outcome, 0);
        let value = self.field(block, outcome, 1);
        let ok = self.function.block(0);
        let failed = self.function.block(0);
        let join = self.function.block(1);
        self.branch(block, success, ok, failed);
        self.edge_value(ok, join, value);
        let failure = self.flow(failed, 4, value);
        self.edge_value(failed, join, failure);
        (join, self.function.parameters(join)[0])
    }

    pub(super) fn return_value(&mut self, block: BlockId, value: ValueId) {
        self.close_except(block, 0);
        let value = if self.control {
            self.flow(block, 1, value)
        } else {
            value
        };
        self.function.terminate(block, Terminator::Return(value));
    }

    pub(super) fn loop_control(&mut self, block: BlockId, continuing: bool) -> Result<()> {
        if let Some(&(head, exit, depth)) = self.loops.last() {
            self.close_except(block, depth);
            self.jump(block, if continuing { head } else { exit });
        } else if self.external_loop {
            let none = self.none(block);
            let flow = self.flow(block, if continuing { 3 } else { 2 }, none);
            self.function.terminate(block, Terminator::Return(flow));
        } else {
            return Err(Error::Syntax("loop control outside loop".into()));
        }
        Ok(())
    }

    fn dispatch_flow(&mut self, block: BlockId, flow: ValueId) -> Result<Option<BlockId>> {
        let kind = self.field(block, flow, 0);
        let normal = self.is_flow(block, kind, 0);
        let next = self.function.block(0);
        let mut transfer = self.function.block(0);
        self.branch(block, normal, next, transfer);
        if self.control {
            self.function.terminate(transfer, Terminator::Return(flow));
            return Ok(Some(next));
        }
        let value = self.field(transfer, flow, 1);
        let returning = self.is_flow(transfer, kind, 1);
        let returned = self.function.block(0);
        let other = self.function.block(0);
        self.branch(transfer, returning, returned, other);
        self.return_value(returned, value);
        transfer = other;
        if !self.loops.is_empty() {
            for (tag, continuing) in [(2, false), (3, true)] {
                let matches = self.is_flow(transfer, kind, tag);
                let matched = self.function.block(0);
                let other = self.function.block(0);
                self.branch(transfer, matches, matched, other);
                self.loop_control(matched, continuing)?;
                transfer = other;
            }
        }
        self.runtime(transfer, CoreOp::PropagateException, vec![value]);
        let none = self.none(transfer);
        self.function.terminate(transfer, Terminator::Return(none));
        Ok(Some(next))
    }

    fn protected_try_except(&mut self, stmt: &ast::StmtTry) -> Result<()> {
        let entry = self.function.entry();
        let outcome = self.protect_suite(entry, &stmt.body)?;
        let (block, flow) = self.normalize_outcome(entry, outcome);
        let kind = self.field(block, flow, 0);
        let failed = self.is_flow(block, kind, 4);
        let error = self.function.block(0);
        let success = self.function.block(0);
        self.branch(block, failed, error, success);
        let normal = self.is_flow(success, kind, 0);
        let otherwise = self.function.block(0);
        let transferred = self.function.block(0);
        self.branch(success, normal, otherwise, transferred);
        self.function
            .terminate(transferred, Terminator::Return(flow));
        let end = self.statements(&stmt.orelse, Some(otherwise))?;
        self.finish_fragment(end);
        if stmt.handlers.is_empty() {
            self.function.terminate(error, Terminator::Return(flow));
            return Ok(());
        }
        let exception = self.field(error, flow, 1);
        self.runtime(error, CoreOp::EnterExcept, vec![exception]);
        let mut next = Some(error);
        for handler in &stmt.handlers {
            let ast::ExceptHandler::ExceptHandler(handler) = handler;
            let block = next
                .take()
                .ok_or_else(|| Error::Syntax("bare except must be last".into()))?;
            let matched = if let Some(class) = &handler.type_ {
                let (block, class) = self.expression(class, block)?;
                let matches = self.runtime(block, CoreOp::ExceptionMatch, vec![exception, class]);
                let yes = self.function.block(0);
                let no = self.function.block(0);
                self.branch(block, matches, yes, no);
                next = Some(no);
                yes
            } else {
                block
            };
            if let Some(name) = &handler.name {
                self.runtime(
                    matched,
                    CoreOp::StoreName(name.to_string()),
                    vec![exception],
                );
            }
            let outcome = self.protect_suite(matched, &handler.body)?;
            if let Some(name) = &handler.name {
                self.runtime(matched, CoreOp::DeleteName(name.to_string()), vec![]);
            }
            self.runtime(matched, CoreOp::ExitExcept, vec![]);
            let (end, flow) = self.normalize_outcome(matched, outcome);
            self.function.terminate(end, Terminator::Return(flow));
        }
        if let Some(next) = next {
            self.runtime(next, CoreOp::ExitExcept, vec![]);
            self.function.terminate(next, Terminator::Return(flow));
        }
        Ok(())
    }

    pub(super) fn general_try(
        &mut self,
        stmt: &ast::StmtTry,
        block: BlockId,
    ) -> Result<Option<BlockId>> {
        if stmt.is_star {
            return Err(Error::Unsupported("except star"));
        }
        let mut protected = self.fragment_builder()?;
        protected.protected_try_except(stmt)?;
        let outcome = self.runtime(block, CoreOp::TryCall(Box::new(protected.function)), vec![]);
        let (mut block, mut flow) = self.normalize_outcome(block, outcome);
        if !stmt.finalbody.is_empty() {
            let kind = self.field(block, flow, 0);
            let failed = self.is_flow(block, kind, 4);
            let exceptional = self.function.block(0);
            let ordinary = self.function.block(0);
            let run = self.function.block(0);
            self.branch(block, failed, exceptional, ordinary);
            let exception = self.field(exceptional, flow, 1);
            self.runtime(exceptional, CoreOp::EnterExcept, vec![exception]);
            self.jump(exceptional, run);
            self.jump(ordinary, run);
            let final_outcome = self.protect_suite(run, &stmt.finalbody)?;
            let cleanup = self.function.block(0);
            let skip = self.function.block(0);
            let joined = self.function.block(0);
            self.branch(run, failed, cleanup, skip);
            self.runtime(cleanup, CoreOp::ExitExcept, vec![]);
            self.jump(cleanup, joined);
            self.jump(skip, joined);
            let (end, final_flow) = self.normalize_outcome(joined, final_outcome);
            let kind = self.field(end, final_flow, 0);
            let normal = self.is_flow(end, kind, 0);
            let original = self.function.block(0);
            let override_ = self.function.block(0);
            let chosen = self.function.block(1);
            self.branch(end, normal, original, override_);
            self.edge_value(original, chosen, flow);
            self.edge_value(override_, chosen, final_flow);
            block = chosen;
            flow = self.function.parameters(chosen)[0];
        }
        self.dispatch_flow(block, flow)
    }
}

impl Builder {
    pub(super) fn delete_target(&mut self, target: &Expr, block: BlockId) -> Result<BlockId> {
        match target {
            Expr::Name(name) => {
                self.runtime(block, CoreOp::DeleteBinding(name.id.to_string()), vec![]);
                Ok(block)
            }
            Expr::Attribute(attr) => {
                let (block, value) = self.expression(&attr.value, block)?;
                self.runtime(
                    block,
                    CoreOp::DeleteAttr(attr.attr.to_string()),
                    vec![value],
                );
                Ok(block)
            }
            Expr::Subscript(item) => {
                let (block, value) = self.expression(&item.value, block)?;
                let (block, key) = self.expression(&item.slice, block)?;
                self.runtime(block, CoreOp::DeleteItem, vec![value, key]);
                Ok(block)
            }
            Expr::Tuple(items) => {
                let mut block = block;
                for target in &items.elts {
                    block = self.delete_target(target, block)?;
                }
                Ok(block)
            }
            Expr::List(items) => {
                let mut block = block;
                for target in &items.elts {
                    block = self.delete_target(target, block)?;
                }
                Ok(block)
            }
            _ => Err(Error::Unsupported("deletion target")),
        }
    }
}

impl Builder {
    pub(super) fn with_items(
        &mut self,
        items: &[ast::WithItem],
        body: &[Stmt],
        block: BlockId,
    ) -> Result<Option<BlockId>> {
        let Some((item, rest)) = items.split_first() else {
            return self.statements(body, Some(block));
        };
        let (block, manager) = self.expression(&item.context_expr, block)?;
        let mut fragment = self.fragment_builder()?;
        let name = format!(".with_value_{}", fragment.function.id().0);
        fragment.function.parameter_names.push(name.clone());
        fragment.function.local_names.push(name.clone());
        let entry = fragment.function.entry();
        let entered = fragment.runtime(entry, CoreOp::LoadName(name), vec![]);
        let start = if let Some(target) = &item.optional_vars {
            fragment.assign(target, entered, entry)?
        } else {
            entry
        };
        let end = fragment.with_items(rest, body, start)?;
        fragment.finish_fragment(end);
        let flow = self.runtime(
            block,
            CoreOp::With(Box::new(fragment.function)),
            vec![manager],
        );
        self.dispatch_flow(block, flow)
    }
}
