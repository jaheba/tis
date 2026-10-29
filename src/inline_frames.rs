//! Logical inline scopes have independent HIR identities and explicit return continuations.
//! Their register maps may share machine values, but never alias semantic frame slots.
use crate::{
    Error, Object, Result, Value, hir,
    lir::{self, Register},
};
use std::{collections::BTreeSet, rc::Rc};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ScopeId(pub usize);

#[derive(Debug, Clone)]
pub struct ReturnContinuation {
    pub destination: hir::ValueId,
    pub resume: hir::ResumePoint,
    pub local_destinations: Vec<usize>,
}

impl ReturnContinuation {
    pub fn after_call(source: &hir::Function, destination: hir::ValueId) -> Result<Self> {
        for (block, body) in source.blocks.iter().enumerate() {
            for (index, instruction) in body.instructions.iter().enumerate() {
                if instruction.result == destination {
                    if !matches!(instruction.op, hir::Op::Runtime(hir::CoreOp::Call(_), _)) {
                        return Err(Error::InvalidIr("inline continuation is not a call"));
                    }
                    let resume = hir::ResumePoint {
                        block: hir::BlockId(block),
                        instruction: index + 1,
                    };
                    let local_destinations = source
                        .local_states
                        .get(block)
                        .and_then(|states| states.get(index + 1))
                        .map(|locals| {
                            locals
                                .iter()
                                .enumerate()
                                .filter_map(|(slot, value)| {
                                    (*value == Some(destination)).then_some(slot)
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    return Ok(Self {
                        destination,
                        resume,
                        local_destinations,
                    });
                }
            }
        }
        Err(Error::InvalidIr("unknown inline call destination"))
    }

    fn verify(&self, source: &hir::Function) -> Result<()> {
        let expected = Self::after_call(source, self.destination)?;
        if self.resume != expected.resume || self.local_destinations != expected.local_destinations
        {
            return Err(Error::InvalidIr("invalid inline return continuation"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct FrameSnapshot {
    pub scope: ScopeId,
    pub parent: Option<ScopeId>,
    pub source: Rc<hir::Function>,
    pub resume: hir::ResumePoint,
    pub environment: Register,
    pub values: Vec<(hir::ValueId, Register)>,
    pub locals: Vec<Option<Register>>,
    pub pending: Option<ReturnContinuation>,
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    /// Outermost caller first; the last frame owns the current operation.
    pub frames: Vec<FrameSnapshot>,
}

impl Snapshot {
    pub fn verify(&self, registers: &[lir::Type]) -> Result<()> {
        if self.frames.is_empty() {
            return Err(Error::InvalidIr("empty inline snapshot"));
        }
        let mut scopes = BTreeSet::new();
        let mut parent = None;
        for (index, frame) in self.frames.iter().enumerate() {
            let source = &frame.source;
            let analysis = source.analyze()?;
            if !scopes.insert(frame.scope) || frame.parent != parent {
                return Err(Error::InvalidIr("invalid inline scope ancestry"));
            }
            parent = Some(frame.scope);
            let block = source
                .blocks
                .get(frame.resume.block.0)
                .ok_or(Error::InvalidIr("inline resume block"))?;
            if frame.resume.instruction > block.instructions.len()
                || frame.locals.len() != source.local_slots.len()
            {
                return Err(Error::InvalidIr("inline frame layout"));
            }
            if registers.get(frame.environment.0) != Some(&lir::Type::Boxed) {
                return Err(Error::InvalidIr("inline environment register"));
            }
            let mut values = BTreeSet::new();
            for (value, register) in &frame.values {
                if value.0 >= source.value_count || !values.insert(*value) {
                    return Err(Error::InvalidIr("inline value map"));
                }
                Self::materializable(registers, *register)?;
            }
            for register in frame.locals.iter().flatten() {
                Self::materializable(registers, *register)?;
            }
            let pending = if index + 1 < self.frames.len() {
                let pending = frame
                    .pending
                    .as_ref()
                    .ok_or(Error::InvalidIr("missing inline return continuation"))?;
                pending.verify(source)?;
                if frame.resume != pending.resume || values.contains(&pending.destination) {
                    return Err(Error::InvalidIr("pending inline return state"));
                }
                Some(pending.destination)
            } else {
                if frame.pending.is_some() {
                    return Err(Error::InvalidIr("innermost inline frame is suspended"));
                }
                None
            };
            let live = if frame.resume.instruction == block.instructions.len() {
                &analysis.live_terminator[frame.resume.block.0]
            } else {
                &analysis.live_before[frame.resume.block.0][frame.resume.instruction]
            };
            if live
                .iter()
                .any(|value| Some(*value) != pending && !values.contains(value))
            {
                return Err(Error::InvalidIr("incomplete inline live values"));
            }
        }
        Ok(())
    }

    fn materializable(types: &[lir::Type], register: Register) -> Result<()> {
        if !matches!(
            types.get(register.0),
            Some(lir::Type::Boxed | lir::Type::I64 | lir::Type::F64 | lir::Type::Bool)
        ) {
            return Err(Error::InvalidIr("unmaterializable inline register"));
        }
        Ok(())
    }

    pub fn roots(&self, types: &[lir::Type]) -> Result<Vec<Register>> {
        self.verify(types)?;
        Ok(self
            .frames
            .iter()
            .flat_map(|frame| {
                std::iter::once(frame.environment)
                    .chain(frame.values.iter().map(|(_, r)| *r))
                    .chain(frame.locals.iter().flatten().copied())
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect())
    }

    pub fn reconstruct(
        &self,
        types: &[lir::Type],
        mut read: impl FnMut(Register) -> Result<Value>,
    ) -> Result<DeoptStack> {
        self.verify(types)?;
        let mut frames = Vec::with_capacity(self.frames.len());
        for frame in &self.frames {
            let mut slots = vec![None; frame.source.value_count];
            for (value, register) in &frame.values {
                slots[value.0] = Some(read(*register)?);
            }
            let mut locals = frame
                .locals
                .iter()
                .map(|register| {
                    register
                        .map(&mut read)
                        .transpose()
                        .map(|value| value.filter(|value| *value != Value::Unbound))
                })
                .collect::<Result<Vec<_>>>()?;
            if let Some(pending) = &frame.pending {
                for &slot in &pending.local_destinations {
                    locals[slot] = None;
                }
            }
            frames.push(ResumeFrame {
                source: frame.source.clone(),
                state: lir::DeoptFrame {
                    function: frame.source.id(),
                    revision: frame.source.revision(),
                    resume: frame.resume,
                    environment: read(frame.environment)?,
                    slots,
                    locals,
                },
                pending: frame.pending.clone(),
            });
        }
        Ok(DeoptStack { frames })
    }
}

#[derive(Debug, Clone)]
pub struct ResumeFrame {
    pub source: Rc<hir::Function>,
    pub state: lir::DeoptFrame,
    pub pending: Option<ReturnContinuation>,
}

#[derive(Debug, Clone)]
pub struct DeoptStack {
    pub frames: Vec<ResumeFrame>,
}

impl DeoptStack {
    pub(crate) fn validate(&self, heap: &crate::Heap) -> Result<()> {
        if self.frames.is_empty() {
            return Err(Error::InvalidIr("empty deoptimization stack"));
        }
        for (index, frame) in self.frames.iter().enumerate() {
            let source = &frame.source;
            let state = &frame.state;
            let analysis = source.analyze()?;
            let block = source
                .blocks
                .get(state.resume.block.0)
                .ok_or(Error::InvalidIr("deoptimization block"))?;
            if source.id() != state.function
                || source.revision() != state.revision
                || state.slots.len() != source.value_count
                || state.locals.len() != source.local_slots.len()
                || state.resume.instruction > block.instructions.len()
            {
                return Err(Error::InvalidIr("stale inline deoptimization frame"));
            }
            if !matches!(heap.get(state.environment)?, Object::Environment(_)) {
                return Err(Error::InvalidIr("inline environment"));
            }
            match (&frame.pending, index + 1 < self.frames.len()) {
                (Some(pending), true) => {
                    pending.verify(source)?;
                    if pending.resume != state.resume
                        || state.slots[pending.destination.0].is_some()
                    {
                        return Err(Error::InvalidIr("pending inline return"));
                    }
                }
                (None, false) => {}
                _ => return Err(Error::InvalidIr("inline continuation chain")),
            }
            let live = if state.resume.instruction == block.instructions.len() {
                &analysis.live_terminator[state.resume.block.0]
            } else {
                &analysis.live_before[state.resume.block.0][state.resume.instruction]
            };
            let pending = frame.pending.as_ref().map(|pending| pending.destination);
            if live
                .iter()
                .any(|value| Some(*value) != pending && state.slots[value.0].is_none())
            {
                return Err(Error::InvalidIr("incomplete deoptimization live values"));
            }
            for value in state.slots.iter().chain(&state.locals).flatten() {
                if matches!(value, Value::Ref(_)) {
                    heap.get(*value)?;
                }
            }
        }
        Ok(())
    }

    pub(crate) fn roots(&self) -> impl Iterator<Item = Value> + '_ {
        self.frames.iter().flat_map(|frame| {
            std::iter::once(frame.state.environment).chain(
                frame
                    .state
                    .slots
                    .iter()
                    .chain(&frame.state.locals)
                    .flatten()
                    .copied(),
            )
        })
    }
}

pub(crate) struct PendingResume {
    pub continuation: ReturnContinuation,
    pub frames: Vec<ResumeFrame>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BytecodeMode, Vm, compile};
    use hir::{Constant, CoreOp, Function, Op, Terminator};

    fn scenario(vm: &mut Vm) -> (Snapshot, Vec<Value>, Value, Value) {
        vm.run_program(
            &mut compile("def churn():\n    for i in range(12000):\n        trash=[i]\n").unwrap(),
        )
        .unwrap();
        let churn = vm.get_global("churn").unwrap();
        let environment = match vm.heap.get(churn).unwrap() {
            Object::Function(closure) => closure.environment,
            _ => unreachable!(),
        };
        let keep = vm.heap.allocate(Object::List(vec![Value::Int(42)].into()));
        let child = vm
            .heap
            .allocate(Object::List(vec![Value::Int(99), Value::Int(0)].into()));

        let mut caller = Function::new(5);
        let block = caller.entry();
        let args = caller.parameters(block).to_vec();
        let call = caller.append(block, Op::Runtime(CoreOp::Call(vec![]), vec![args[0]]));
        let result = caller.append(block, Op::Tuple(vec![call, args[1]]));
        caller.terminate(block, Terminator::Return(result));
        let continuation = ReturnContinuation::after_call(&caller, call).unwrap();

        let mut callee = Function::new(2);
        let block = callee.entry();
        let args = callee.parameters(block).to_vec();
        let zero = callee.append(block, Op::Constant(Constant::Int(0)));
        // The exit occurs after this effect; replay would replace 99 with zero.
        callee.append(
            block,
            Op::SetItem {
                object: args[0],
                key: zero,
                value: zero,
            },
        );
        let resume = hir::ResumePoint {
            block,
            instruction: 2,
        };
        let one = callee.append(block, Op::Constant(Constant::Int(1)));
        let seven = callee.append(block, Op::Constant(Constant::Int(7)));
        callee.append(
            block,
            Op::SetItem {
                object: args[0],
                key: one,
                value: seven,
            },
        );
        callee.append(block, Op::Runtime(CoreOp::Call(vec![]), vec![args[1]]));
        callee.terminate(block, Terminator::Return(args[0]));

        let snapshot = Snapshot {
            frames: vec![
                FrameSnapshot {
                    scope: ScopeId(0),
                    parent: None,
                    source: Rc::new(caller),
                    resume: continuation.resume,
                    environment: Register(0),
                    values: vec![(hir::ValueId(1), Register(1))],
                    locals: vec![],
                    pending: Some(continuation),
                },
                FrameSnapshot {
                    scope: ScopeId(1),
                    parent: Some(ScopeId(0)),
                    source: Rc::new(callee),
                    resume,
                    environment: Register(0),
                    values: vec![
                        (hir::ValueId(0), Register(2)),
                        (hir::ValueId(1), Register(3)),
                    ],
                    locals: vec![],
                    pending: None,
                },
            ],
        };
        (snapshot, vec![environment, keep, child, churn], keep, child)
    }

    #[test]
    fn nested_resume_preserves_effects_and_all_frame_roots() {
        for mode in [BytecodeMode::Stack, BytecodeMode::Register] {
            let mut vm = Vm::new();
            vm.bytecode = mode;
            let (snapshot, registers, keep, child) = scenario(&mut vm);
            let types = vec![lir::Type::Boxed; registers.len()];
            assert_eq!(snapshot.roots(&types).unwrap().len(), 4);
            let stack = snapshot
                .reconstruct(&types, |r| Ok(registers[r.0]))
                .unwrap();
            let epoch = vm.heap.collection_epoch();
            let result = vm.resume_inline_stack(stack).unwrap();
            assert!(vm.heap.collection_epoch() > epoch);
            assert_eq!(vm.heap.get_item(result, Value::Int(0)).unwrap(), child);
            assert_eq!(vm.heap.get_item(result, Value::Int(1)).unwrap(), keep);
            assert_eq!(
                vm.heap.get_item(child, Value::Int(0)).unwrap(),
                Value::Int(99)
            );
            assert_eq!(
                vm.heap.get_item(child, Value::Int(1)).unwrap(),
                Value::Int(7)
            );
            assert_eq!(
                vm.heap.get_item(keep, Value::Int(0)).unwrap(),
                Value::Int(42)
            );
            assert_eq!(vm.stack_len(), 0);
            vm.collect_garbage(&[]).unwrap();
            assert!(vm.heap.get(keep).is_err());
            assert!(vm.heap.get(child).is_err());
        }
    }

    #[test]
    fn tier_side_exits_resume_nested_frames_in_the_existing_caller() {
        for tier in [
            crate::Tiering::Lir,
            crate::Tiering::Native,
            crate::Tiering::CopyPatch,
        ] {
            for mode in [BytecodeMode::Stack, BytecodeMode::Register] {
                let mut vm = Vm::new();
                vm.tiering = tier;
                vm.bytecode = mode;
                let (snapshot, mut arguments, keep, child) = scenario(&mut vm);
                arguments.push(Value::Float(0.5));
                let mut source = (*snapshot.frames[0].source).clone();
                let mut lir = source.lower_lir(lir::LoweringMode::Generic).unwrap();
                let point = hir::ResumePoint {
                    block: source.entry(),
                    instruction: 0,
                };
                let state = lir::StateId(lir.states.len());
                lir.states.push(lir::FrameState {
                    resume: point,
                    values: (0..5).map(|i| (hir::ValueId(i), Register(i))).collect(),
                    environment_root: lir::FrameEnvironment::Current,
                    local_count: 0,
                    local_values: vec![],
                });
                let exit = lir::ExitId(lir.exits.len());
                lir.exits.push(lir::SideExit {
                    inline: Some(Rc::new(snapshot)),
                    state,
                    reason: lir::ExitReason::TypeGuard,
                });
                let origin = source.blocks[0].instructions[0].result;
                lir.blocks[0].instructions.insert(
                    0,
                    lir::Instruction {
                        result: None,
                        operation: lir::Operation::GuardInt {
                            value: Register(4),
                            exit,
                        },
                        origin,
                        point,
                    },
                );
                let mut invalid = lir.clone();
                invalid.states[state.0]
                    .values
                    .retain(|(_, register)| register.0 != 3);
                assert!(vm.install_lir(&source, invalid).is_err());
                assert!(lir.dump().contains("scope1"));
                vm.install_lir(&source, lir).unwrap();
                let result = vm.run(&mut source, &arguments).unwrap();
                assert_eq!(vm.tier_stats.deoptimizations, 1);
                assert_eq!(source.profile.borrow().sites[origin.0].deoptimizations, 1);
                if tier == crate::Tiering::Native {
                    assert!(vm.tier_stats.native_entries > 0);
                }
                if tier == crate::Tiering::CopyPatch {
                    assert!(vm.tier_stats.copy_patch_entries > 0);
                }
                assert_eq!(vm.heap.get_item(result, Value::Int(0)).unwrap(), child);
                assert_eq!(vm.heap.get_item(result, Value::Int(1)).unwrap(), keep);
                assert_eq!(
                    vm.heap.get_item(child, Value::Int(0)).unwrap(),
                    Value::Int(99)
                );
                assert_eq!(
                    vm.heap.get_item(child, Value::Int(1)).unwrap(),
                    Value::Int(7)
                );
                assert_eq!(vm.stack_len(), 0);
            }
        }
    }

    #[test]
    fn invalid_outer_state_is_rejected_before_child_effects() {
        let mut vm = Vm::new();
        let (snapshot, registers, _, child) = scenario(&mut vm);
        let types = vec![lir::Type::Boxed; registers.len()];
        let mut stack = snapshot
            .reconstruct(&types, |r| Ok(registers[r.0]))
            .unwrap();
        stack.frames[0].state.slots[1] = None;
        assert!(vm.resume_inline_stack(stack).is_err());
        assert_eq!(
            vm.heap.get_item(child, Value::Int(1)).unwrap(),
            Value::Int(0)
        );
        assert_eq!(vm.stack_len(), 0);
    }

    #[test]
    fn recursive_scopes_resume_at_terminators_and_restore_pending_locals() {
        for mode in [BytecodeMode::Stack, BytecodeMode::Register] {
            let mut vm = Vm::new();
            vm.bytecode = mode;
            let (_, registers, _, _) = scenario(&mut vm);
            let mut source = Function::new(1);
            let block = source.entry();
            let callable = source.parameters(block)[0];
            let call = source.append(block, Op::Runtime(CoreOp::Call(vec![]), vec![callable]));
            source.terminate(block, Terminator::Return(call));
            source.local_names = vec!["saved".into()];
            source.local_slots = vec!["saved".into()];
            source.local_states = vec![vec![vec![None], vec![Some(call)]]];
            let continuation = ReturnContinuation::after_call(&source, call).unwrap();
            assert_eq!(continuation.local_destinations, vec![0]);
            let source = Rc::new(source);
            let snapshot = Snapshot {
                frames: (0..3)
                    .map(|index| FrameSnapshot {
                        scope: ScopeId(index),
                        parent: index.checked_sub(1).map(ScopeId),
                        source: source.clone(),
                        resume: continuation.resume,
                        environment: Register(0),
                        values: if index == 2 {
                            vec![(call, Register(1))]
                        } else {
                            vec![]
                        },
                        locals: vec![None],
                        pending: (index < 2).then(|| continuation.clone()),
                    })
                    .collect(),
            };
            let stack = snapshot
                .reconstruct(&[lir::Type::Boxed, lir::Type::I64], |r| {
                    Ok(if r.0 == 0 {
                        registers[0]
                    } else {
                        Value::Int(42)
                    })
                })
                .unwrap();
            assert_eq!(vm.resume_inline_stack(stack).unwrap(), Value::Int(42));
            assert_eq!(vm.stack_len(), 0);
        }
    }

    #[test]
    fn child_errors_unwind_without_running_parent_continuations() {
        let mut vm = Vm::new();
        let (snapshot, mut registers, keep, child) = scenario(&mut vm);
        vm.run_program(&mut compile("def fail():\n    return 1//0\n").unwrap())
            .unwrap();
        registers[3] = vm.get_global("fail").unwrap();
        let stack = snapshot
            .reconstruct(&vec![lir::Type::Boxed; registers.len()], |r| {
                Ok(registers[r.0])
            })
            .unwrap();
        assert!(vm.resume_inline_stack(stack).is_err());
        assert_eq!(vm.stack_len(), 0);
        assert_eq!(
            vm.heap.get_item(child, Value::Int(1)).unwrap(),
            Value::Int(7)
        );
        vm.collect_garbage(&[]).unwrap();
        assert!(vm.heap.get(keep).is_err());
        assert!(vm.heap.get(child).is_err());
        assert!(vm.call(registers[3], &[]).is_err());
        assert_eq!(vm.stack_len(), 0);
    }

    #[test]
    fn scope_ancestry_and_materialization_are_verified() {
        let mut vm = Vm::new();
        let (mut snapshot, registers, _, _) = scenario(&mut vm);
        let mut types = vec![lir::Type::Boxed; registers.len()];
        snapshot.frames[1].parent = None;
        assert!(snapshot.verify(&types).is_err());
        snapshot.frames[1].parent = Some(ScopeId(0));
        types[1] = lir::Type::FloatView;
        assert!(snapshot.verify(&types).is_err());
        types[1] = lir::Type::Boxed;
        snapshot.frames[0].values.clear();
        assert!(snapshot.verify(&types).is_err());
    }
}
