//! Bounded callee HIR incorporation. Plans contain semantic identities and numeric expressions,
//! never heap addresses. Native callers emit the expression body after a closed preflight guard.
use crate::{Heap, Object, Value, hir, profile::FunctionId};
use std::rc::Rc;

#[derive(Debug, Clone)]
pub struct Plan {
    pub(crate) source: FunctionId,
    pub(crate) revision: u64,
    pub(crate) bound: bool,
    pub(crate) parameters: usize,
    pub(crate) nodes: Vec<Node>,
    pub(crate) result: usize,
    pub(crate) boolean: bool,
}

#[derive(Debug, Clone)]
pub(crate) enum Node {
    Argument(usize),
    Field(usize, crate::Attribute),
    Constant(f64),
    Add(usize, usize),
    Subtract(usize, usize),
    Multiply(usize, usize),
    Less(usize, usize),
    Compare(String, usize, usize),
    Not(usize),
    Select(usize, usize, usize),
}

#[derive(Clone, Copy)]
enum Source {
    Argument(usize),
    Number(usize),
    Boolean(usize),
}

impl Plan {
    pub(crate) fn observe(heap: &Heap, mut callee: Value) -> Option<Rc<Self>> {
        let bound = if let Object::Method(function, _) = heap.get(callee).ok()? {
            callee = *function;
            true
        } else {
            false
        };
        let Object::Function(closure) = heap.get(callee).ok()? else {
            return None;
        };
        if !closure.defaults.is_empty() {
            return None;
        }
        let function = closure.function.try_borrow().ok()?;
        // No lexical reads, argument packing/defaults, writes, calls, loops or nested exits.
        if function.environment_required()
            || function.blocks.len() > 12
            || !function.keyword_only.is_empty()
            || function.vararg.is_some()
            || function.kwarg.is_some()
            || function.parameter_names.is_empty()
            || function.parameter_names.len() > 4
            || function
                .blocks
                .iter()
                .map(|b| b.instructions.len())
                .sum::<usize>()
                > 64
        {
            return None;
        }
        // Profiles can see host-edited HIR before the callee's bytecode is rebuilt. Validate
        // graph indices/dominance first; malformed bodies must miss, never panic in profiling.
        function.analyze().ok()?;
        let mut plan = Self {
            source: function.id(),
            revision: function.revision(),
            bound,
            parameters: function.parameter_names.len(),
            nodes: vec![],
            result: 0,
            boolean: false,
        };
        let source = plan.copy_block(
            &function,
            hir::BlockId(0),
            vec![None; function.value_count],
            &mut vec![],
        )?;
        match source {
            Source::Boolean(n) => {
                plan.result = n;
                plan.boolean = true;
            }
            source => plan.result = plan.number(source)?,
        }
        if plan.nodes.is_empty() || plan.nodes.len() > 128 {
            return None;
        }
        Some(Rc::new(plan))
    }

    fn number(&mut self, source: Source) -> Option<usize> {
        match source {
            Source::Argument(index) => {
                if let Some(n) = self
                    .nodes
                    .iter()
                    .position(|node| matches!(node,Node::Argument(existing) if *existing==index))
                {
                    return Some(n);
                }
                let n = self.nodes.len();
                self.nodes.push(Node::Argument(index));
                Some(n)
            }
            Source::Number(index) => Some(index),
            Source::Boolean(_) => None,
        }
    }

    /// Acyclic readonly branches become typed selects. Speculating floating arithmetic and
    /// guarded field reads is harmless; missing/non-float leaves reject the entire inline first.
    fn copy_block(
        &mut self,
        function: &hir::Function,
        block: hir::BlockId,
        mut values: Vec<Option<Source>>,
        path: &mut Vec<hir::BlockId>,
    ) -> Option<Source> {
        if path.contains(&block) || self.nodes.len() > 128 {
            return None;
        }
        path.push(block);
        for instruction in &function.blocks[block.0].instructions {
            if self.nodes.len() > 128 {
                return None;
            }
            let source = match &instruction.op {
                hir::Op::Constant(hir::Constant::Unbound | hir::Constant::None) => continue,
                hir::Op::Constant(hir::Constant::Float(value)) => {
                    let n = self.nodes.len();
                    self.nodes.push(Node::Constant(*value));
                    Source::Number(n)
                }
                hir::Op::Runtime(hir::CoreOp::LoadLocal(slot), _) => {
                    // Physical locals are sorted by name, independently of positional binding order.
                    let name = function.local_slots.get(*slot)?;
                    Source::Argument(
                        function
                            .parameter_names
                            .iter()
                            .position(|parameter| parameter == name)?,
                    )
                }
                hir::Op::Runtime(hir::CoreOp::GetAttr(name), inputs) => {
                    let [input] = inputs.as_slice() else {
                        return None;
                    };
                    let Source::Argument(arg) = values[input.0]? else {
                        return None;
                    };
                    // The entire copied body is readonly/non-reentrant, so equal argument-field
                    // reads cannot change between source operations, including across branches.
                    if let Some(n)=self.nodes.iter().position(|node| matches!(node,Node::Field(existing,attribute) if *existing==arg && attribute.text==*name)) {
                        Source::Number(n)
                    } else {
                        let n = self.nodes.len();
                        self.nodes.push(Node::Field(arg, name.clone().into()));
                        Source::Number(n)
                    }
                }
                hir::Op::Add(a, b) | hir::Op::Subtract(a, b) | hir::Op::Less(a, b) => {
                    let a = self.number(values[a.0]?)?;
                    let b = self.number(values[b.0]?)?;
                    let n = self.nodes.len();
                    let boolean = matches!(instruction.op, hir::Op::Less(..));
                    self.nodes.push(match instruction.op {
                        hir::Op::Add(..) => Node::Add(a, b),
                        hir::Op::Subtract(..) => Node::Subtract(a, b),
                        _ => Node::Less(a, b),
                    });
                    if boolean {
                        Source::Boolean(n)
                    } else {
                        Source::Number(n)
                    }
                }
                hir::Op::Runtime(hir::CoreOp::Binary(operator), inputs) if operator == "*" => {
                    let [a, b] = inputs.as_slice() else {
                        return None;
                    };
                    let a = self.number(values[a.0]?)?;
                    let b = self.number(values[b.0]?)?;
                    let n = self.nodes.len();
                    self.nodes.push(Node::Multiply(a, b));
                    Source::Number(n)
                }
                hir::Op::Runtime(hir::CoreOp::Compare(operator), inputs)
                    if matches!(operator.as_str(), "<" | "<=" | ">" | ">=" | "==" | "!=") =>
                {
                    let [a, b] = inputs.as_slice() else {
                        return None;
                    };
                    let a = self.number(values[a.0]?)?;
                    let b = self.number(values[b.0]?)?;
                    let n = self.nodes.len();
                    self.nodes.push(Node::Compare(operator.clone(), a, b));
                    Source::Boolean(n)
                }
                hir::Op::Runtime(hir::CoreOp::Unary(operator), inputs) if operator == "not" => {
                    let [a] = inputs.as_slice() else { return None };
                    let Source::Boolean(a) = values[a.0]? else {
                        return None;
                    };
                    let n = self.nodes.len();
                    self.nodes.push(Node::Not(a));
                    Source::Boolean(n)
                }
                _ => return None,
            };
            values[instruction.result.0] = Some(source);
        }
        let edge = |this: &mut Self, edge: &hir::Edge, path: &mut Vec<hir::BlockId>| {
            let mut next = values.clone();
            for (&value, &parameter) in edge
                .arguments
                .iter()
                .zip(&function.blocks[edge.block.0].parameters)
            {
                next[parameter.0] = values[value.0];
            }
            this.copy_block(function, edge.block, next, path)
        };
        let result = match function.blocks[block.0].terminator.as_ref()? {
            hir::Terminator::Return(value) => values[value.0]?,
            hir::Terminator::Jump(next) => edge(self, next, path)?,
            hir::Terminator::Branch { condition, yes, no } => {
                let Source::Boolean(condition) = values[condition.0]? else {
                    return None;
                };
                let yes = edge(self, yes, path)?;
                let no = edge(self, no, path)?;
                let n = self.nodes.len();
                match (yes, no) {
                    (Source::Boolean(yes), Source::Boolean(no)) => {
                        self.nodes.push(Node::Select(condition, yes, no));
                        Source::Boolean(n)
                    }
                    (yes, no) => {
                        let yes = self.number(yes)?;
                        let no = self.number(no)?;
                        let n = self.nodes.len();
                        self.nodes.push(Node::Select(condition, yes, no));
                        Source::Number(n)
                    }
                }
            }
        };
        path.pop();
        Some(result)
    }

    /// All guards precede body execution. This reads existing fields only, never attribute methods,
    /// VM allocation, Python or collection. Misses leave the original caller Call untouched.
    pub(crate) fn prepare(
        &self,
        heap: &Heap,
        arguments: &[Value],
        output: &mut [crate::native_abi::Slot],
    ) -> Option<()> {
        let (&callee, arguments) = arguments.split_first()?;
        let (callee, receiver) = match heap.get(callee).ok()? {
            Object::Method(function, receiver) if self.bound => (*function, Some(*receiver)),
            Object::Function(_) if !self.bound => (callee, None),
            _ => return None,
        };
        let Object::Function(closure) = heap.get(callee).ok()? else {
            return None;
        };
        let function = closure.function.try_borrow().ok()?;
        if function.id() != self.source
            || function.revision() != self.revision
            || !closure.defaults.is_empty()
            || arguments.len() + usize::from(receiver.is_some()) != self.parameters
        {
            return None;
        }
        // Binding validates even unused arguments; do not hide stale handles or internal markers.
        for value in arguments.iter().copied().chain(receiver) {
            if value == Value::Unbound {
                return None;
            }
            if matches!(value, Value::Ref(_)) {
                heap.get(value).ok()?;
            }
        }
        let argument = |i: usize| {
            if let Some(receiver) = receiver {
                if i == 0 {
                    Some(receiver)
                } else {
                    arguments.get(i - 1).copied()
                }
            } else {
                arguments.get(i).copied()
            }
        };
        // Borrow each existing instance once. No VM allocation, collection or reentrancy can
        // invalidate these shared arena borrows before preflight returns.
        let mut fields: [Option<&crate::Attributes>; 4] = [None; 4];
        for (index, node) in self.nodes.iter().enumerate() {
            let value = match node {
                Node::Argument(i) => argument(*i)?,
                Node::Field(i, name) => {
                    let cached = fields.get_mut(*i)?;
                    let attributes = if let Some(attributes) = *cached {
                        attributes
                    } else {
                        let Object::Instance { attributes, .. } = heap.get(argument(*i)?).ok()?
                        else {
                            return None;
                        };
                        *cached = Some(attributes);
                        attributes
                    };
                    // Shape identity and slot bounds guard the existing field cache; mutation
                    // changes the shape or refreshes the value, never a retained object pointer.
                    attributes.read(name).0?
                }
                _ => continue,
            };
            let Value::Float(value) = value else {
                return None;
            };
            output[index] = crate::native_abi::Slot {
                tag: crate::native_abi::RAW_FLOAT,
                a: value.to_bits(),
            };
        }
        Some(())
    }
}

/// Private native scratch contains only primitive leaf operands and is never a GC root.
pub(crate) fn scratch_slots(lir: &crate::lir::Function) -> usize {
    lir.blocks
        .iter()
        .flat_map(|b| &b.instructions)
        .filter_map(|i| {
            if let crate::lir::Operation::Call {
                helper: crate::lir::Helper::InlineCall { body, .. },
                ..
            } = &i.operation
            {
                Some(body.nodes.len())
            } else {
                None
            }
        })
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use crate::{Tiering, Value, Vm, compile};

    #[test]
    fn malformed_host_edited_hir_is_rejected_before_copying_indices() {
        let mut vm = Vm::new();
        vm.run_program(&mut compile("def f(x):\n    return x*x\n").unwrap())
            .unwrap();
        let callee = vm.get_global("f").unwrap();
        let crate::Object::Function(closure) = vm.heap.get(callee).unwrap() else {
            panic!("not function")
        };
        {
            let mut function = closure.function.borrow_mut();
            let entry = function.entry();
            let invalid = function.append(
                entry,
                crate::hir::Op::Add(
                    crate::hir::ValueId(usize::MAX),
                    crate::hir::ValueId(usize::MAX),
                ),
            );
            function.terminate(entry, crate::hir::Terminator::Return(invalid));
            assert!(function.analyze().is_err());
        }
        assert!(super::Plan::observe(&vm.heap, callee).is_none());
        assert!(vm.call(callee, &[Value::Float(2.0)]).is_err());
        assert_eq!(vm.stack_len(), 0);
    }

    #[test]
    fn inline_miss_preserves_callee_recursion_limit_checkpoint() {
        for tier in [Tiering::Native, Tiering::CopyPatch] {
            let mut vm = Vm::new();
            vm.tiering = tier;
            vm.thresholds.function_entries = 32;
            vm.run_program(&mut compile("def square(x):\n    return x*x\ndef caller(x):\n    return square(x)\nfor i in range(500):\n    caller(3.0)\n").unwrap()).unwrap();
            let caller = vm.get_global("caller").unwrap();
            let hits = vm.tier_stats.native_body_inlines;
            vm.depth = 255;
            let error = vm.call(caller, &[Value::Float(4.0)]).unwrap_err();
            assert!(error.to_string().contains("RecursionError"));
            assert_eq!(vm.tier_stats.native_body_inlines, hits);
            assert_eq!(vm.depth, 255);
            assert_eq!(vm.stack_len(), 0);
            vm.depth = 0;
        }
    }
}
