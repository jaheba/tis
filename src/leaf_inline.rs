//! Bounded HIR bodies share caller registers and retain logical frames at safepoints.
mod tuple;

use crate::{Error, Heap, Object, Result, Value, hir, lir};
use hir::{CoreOp, Op, ValueId};
use lir::{Helper, Operation, Register};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};

#[derive(Clone)]
pub struct Plan {
    pub source: Rc<hir::Function>,
    pub bound: bool,
    pub(crate) live_profile: std::rc::Weak<RefCell<crate::profile::FunctionProfile>>,
}

impl std::fmt::Debug for Plan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LeafPlan")
            .field("source", &self.source.id())
            .field("revision", &self.source.revision())
            .field("name", &self.source.name)
            .field("bound", &self.bound)
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct Scope {
    pub id: crate::inline_frames::ScopeId,
    pub source: Rc<hir::Function>,
    pub callsite: hir::ResumePoint,
    pub destination: ValueId,
    pub registers: std::ops::Range<usize>,
}

impl Plan {
    pub(crate) fn observe(heap: &Heap, mut value: Value, allow_cfg: bool) -> Option<Rc<Self>> {
        let bound = if let Object::Method(function, _) = heap.get(value).ok()? {
            value = *function;
            true
        } else {
            false
        };
        let Object::Function(closure) = heap.get(value).ok()? else {
            return None;
        };
        let function = closure.function.try_borrow().ok()?;
        if !closure.defaults.is_empty()
            || function.environment_required()
            || (function.local_states.is_empty() && !function.local_slots.is_empty())
            || (!allow_cfg
                && (function.blocks.len() != 1 || function.blocks[0].instructions.len() > 96))
            || function.blocks.len() > 48
            || function
                .blocks
                .iter()
                .map(|b| b.instructions.len())
                .sum::<usize>()
                > 384
            || !function.keyword_only.is_empty()
            || function.vararg.is_some()
            || function.kwarg.is_some()
            || function.parameter_names.len() > 4
            || function.parameter_names.is_empty()
        {
            return None;
        }
        for instruction in function.blocks.iter().flat_map(|b| &b.instructions) {
            match &instruction.op {
                Op::Constant(_)
                | Op::Add(..)
                | Op::Subtract(..)
                | Op::Less(..)
                | Op::List(_)
                | Op::Tuple(_)
                | Op::Dict(_)
                | Op::GetItem(..)
                | Op::SetItem { .. }
                | Op::Len(_) => {}
                Op::Runtime(CoreOp::LoadName(_) | CoreOp::Call(_), _) if !allow_cfg => return None,
                Op::Runtime(CoreOp::LoadLocal(slot), _)
                    if function
                        .parameter_names
                        .contains(function.local_slots.get(*slot)?) => {}
                // Shared operations retain their effects. Calls and lexical reads receive
                // explicit logical-frame context when their bodies are incorporated.
                Op::Runtime(
                    CoreOp::GetAttr(_)
                    | CoreOp::SetAttr(_)
                    | CoreOp::Binary(_)
                    | CoreOp::Inplace(_)
                    | CoreOp::Compare(_)
                    | CoreOp::Unary(_)
                    | CoreOp::LoadName(_)
                    | CoreOp::Call(_)
                    | CoreOp::Unpack(_)
                    | CoreOp::UnpackItem(_),
                    _,
                ) => {}
                _ => return None,
            }
        }
        if !function.analyze().ok()?.backedges.is_empty() {
            return None;
        }
        let mut source = function.clone();
        let mut profile = source.profile.try_borrow().ok()?.clone();
        for site in &mut profile.sites {
            site.leaf_body = None;
        }
        // A frozen copy must not create a profile -> plan -> profile ownership cycle.
        source.profile = Rc::new(RefCell::new(profile));
        Some(Rc::new(Self {
            live_profile: Rc::downgrade(&function.profile),
            source: Rc::new(source),
            bound,
        }))
    }

    pub(crate) fn matches(&self, heap: &Heap, arguments: &[Value]) -> bool {
        let Some((&target, args)) = arguments.split_first() else {
            return false;
        };
        let (target, receiver) = match heap.get(target) {
            Ok(Object::Method(function, receiver)) if self.bound => (*function, Some(*receiver)),
            Ok(Object::Function(_)) if !self.bound => (target, None),
            _ => return false,
        };
        let Ok(Object::Function(closure)) = heap.get(target) else {
            return false;
        };
        let Ok(function) = closure.function.try_borrow() else {
            return false;
        };
        function.id() == self.source.id()
            && function.revision() == self.source.revision()
            && closure.defaults.is_empty()
            && args.len() + usize::from(self.bound) == self.source.parameter_names.len()
            && arguments.iter().copied().chain(receiver).all(|value| {
                value != Value::Unbound
                    && (!matches!(value, Value::Ref(_)) || heap.get(value).is_ok())
            })
    }
}

pub(crate) fn merge(
    source: &hir::Function,
    output: &mut lir::Function,
    allow_cfg: bool,
) -> Result<()> {
    // Preserve the specialized readonly expression inliner, and cap total growth per artifact.
    let profile = source.profile.borrow();
    let mut fused_any = false;
    let mut budget = if allow_cfg { 768_usize } else { 192 };
    for block in 0..output.blocks.len() {
        let instructions = std::mem::take(&mut output.blocks[block].instructions);
        let mut merged = Vec::new();
        let mut current = block;
        let mut instructions = std::collections::VecDeque::from(instructions);
        while let Some(instruction) = instructions.pop_front() {
            let candidate = if let Operation::Call {
                helper: Helper::Python(CoreOp::Call(names)),
                arguments,
                ..
            } = &instruction.operation
            {
                if names.iter().all(Option::is_none) {
                    profile
                        .site(instruction.origin)
                        .filter(|site| {
                            site.executions >= 8
                                && site.other_targets == 0
                                && site.call_targets.len() == 1
                        })
                        .and_then(|site| site.leaf_body.as_ref())
                        .filter(|plan| {
                            allow_cfg
                                || (plan.source.blocks.len() == 1
                                    && !plan.source.blocks[0].instructions.iter().any(|i| {
                                        matches!(
                                            i.op,
                                            Op::Runtime(CoreOp::LoadName(_) | CoreOp::Call(_), _)
                                        )
                                    }))
                        })
                        .filter(|plan| {
                            arguments.len() + usize::from(plan.bound)
                                == plan.source.parameter_names.len() + 1
                        })
                        .cloned()
                } else {
                    None
                }
            } else {
                None
            };
            if let Some(plan) = candidate
                && plan
                    .source
                    .blocks
                    .iter()
                    .map(|b| b.instructions.len())
                    .sum::<usize>()
                    <= budget
                && source
                    .local_state(hir::ResumePoint {
                        block: instruction.point.block,
                        instruction: instruction.point.instruction + 1,
                    })
                    .len()
                    == source.local_slots.len()
            {
                budget -= plan
                    .source
                    .blocks
                    .iter()
                    .map(|b| b.instructions.len())
                    .sum::<usize>();
                let fusion = allow_cfg
                    .then(|| {
                        tuple::Fusion::find(
                            source,
                            &instruction,
                            instructions.make_contiguous(),
                            &plan.source,
                        )
                    })
                    .flatten();
                let cfg = fusion.is_some()
                    || plan.source.blocks.len() > 1
                    || plan
                        .source
                        .blocks
                        .iter()
                        .flat_map(|b| &b.instructions)
                        .any(|i| {
                            matches!(i.op, Op::Runtime(CoreOp::LoadName(_) | CoreOp::Call(_), _))
                        });
                let (next, fused) = expand(
                    source,
                    output,
                    &mut merged,
                    instruction,
                    plan,
                    cfg.then_some(current),
                    fusion.as_ref(),
                )?;
                if fused {
                    for _ in 0..fusion.as_ref().unwrap().consumers {
                        instructions.pop_front();
                    }
                    fused_any = true;
                }
                if let Some(next) = next {
                    output.blocks[current].instructions = std::mem::take(&mut merged);
                    current = next;
                }
            } else {
                merged.push(instruction);
            }
        }
        output.blocks[current].instructions = merged;
    }
    if fused_any {
        output.prune_inline_metadata();
    }
    Ok(())
}

fn register(output: &mut lir::Function) -> Register {
    let result = Register(output.registers.len());
    output.registers.push(lir::Type::Boxed);
    output.scoped_results.insert(result);
    output.shadow_slots = output.registers.len();
    result
}

fn expand(
    caller: &hir::Function,
    output: &mut lir::Function,
    body: &mut Vec<lir::Instruction>,
    call: lir::Instruction,
    plan: Rc<Plan>,
    cfg_block: Option<usize>,
    fusion: Option<&tuple::Fusion>,
) -> Result<(Option<usize>, bool)> {
    let Operation::Call {
        arguments, state, ..
    } = &call.operation
    else {
        unreachable!()
    };
    let arguments = arguments.clone();
    let state = *state;
    let physical = output.states[state.0].clone();
    let destination = call.result.ok_or(Error::InvalidIr("inline call result"))?;
    let emit = |body: &mut Vec<lir::Instruction>, result, operation| {
        body.push(lir::Instruction {
            result,
            operation,
            origin: call.origin,
            point: call.point,
        });
    };
    let helper = |helper: Helper, arguments: Vec<Register>, state| {
        let effects = helper.effects();
        Operation::Call {
            helper,
            arguments,
            state,
            roots: vec![],
            effects,
        }
    };
    let first_register = output.registers.len();
    let scope = crate::inline_frames::ScopeId(output.inline_scopes.len() + 1);
    let guard = register(output);
    emit(
        body,
        Some(guard),
        helper(Helper::LeafGuard(plan.clone()), arguments.clone(), state),
    );
    let failure = lir::ExitId(output.exits.len());
    output.exits.push(lir::SideExit {
        inline: None,
        state,
        reason: lir::ExitReason::TypeGuard,
    });
    emit(
        body,
        None,
        Operation::GuardInt {
            value: guard,
            exit: failure,
        },
    );
    // The entry guard validates binding and language depth before effects. Nested calls
    // charge this logical frame while executing through a scoped runtime helper.
    let caller_environment = register(output);
    emit(
        body,
        Some(caller_environment),
        helper(Helper::CurrentEnvironment, vec![], state),
    );
    let environment = register(output);
    emit(
        body,
        Some(environment),
        helper(Helper::LeafEnvironment, vec![arguments[0]], state),
    );
    let mut parameters = arguments[1..].to_vec();
    if plan.bound {
        let receiver = register(output);
        emit(
            body,
            Some(receiver),
            helper(Helper::LeafReceiver, vec![arguments[0]], state),
        );
        parameters.insert(0, receiver);
    }
    let mut source = (*plan.source).clone();
    if let Some(profile) = plan.live_profile.upgrade() {
        let mut profile = profile.borrow().clone();
        for site in &mut profile.sites {
            site.leaf_body = None;
        }
        // A loop can finish its site samples before the callee opens a profiling window.
        // Refresh feedback at compilation; the captured semantic body/revision stays fixed.
        source.profile = Rc::new(RefCell::new(profile));
    }
    let source = Rc::new(source);
    let mut resumed_source = (*source).clone();
    if let Some(profile) = plan.live_profile.upgrade() {
        resumed_source.profile = profile;
    }
    let resumed_source = Rc::new(resumed_source);
    let parameter_slot = |slot: usize| {
        source
            .parameter_names
            .iter()
            .position(|name| *name == source.local_slots[slot])
    };
    let continuation = crate::inline_frames::ReturnContinuation::after_call(caller, call.origin)?;
    let analysis = caller.analyze()?;
    let outer_block = &caller.blocks[continuation.resume.block.0];
    let live = if continuation.resume.instruction == outer_block.instructions.len() {
        &analysis.live_terminator[continuation.resume.block.0]
    } else {
        &analysis.live_before[continuation.resume.block.0][continuation.resume.instruction]
    };
    let outer = crate::inline_frames::FrameSnapshot {
        scope: crate::inline_frames::ScopeId(0),
        parent: None,
        source: Rc::new(caller.clone()),
        resume: continuation.resume,
        environment: caller_environment,
        values: live
            .iter()
            .filter(|value| **value != call.origin)
            .map(|value| (*value, Register(value.0)))
            .collect(),
        locals: caller
            .local_state(continuation.resume)
            .iter()
            .map(|value| {
                value
                    .filter(|value| *value != call.origin)
                    .map(|value| Register(value.0))
            })
            .collect(),
        pending: Some(continuation),
    };
    // Lower once through the ordinary optimizer. Only graph locations and frame context
    // change below; scalar, comparison and sequence selection stay in lir::Function.
    let lowered =
        lir::Function::lower_with_inline(&source, lir::LoweringMode::Profiled, false, false)?;
    let fusion = fusion.filter(|fusion| fusion.matches_lowering(&lowered));
    let mut aliases = vec![None; lowered.registers.len()];
    for (&value, &parameter) in source.parameters(source.entry()).iter().zip(&parameters) {
        aliases[value.0] = Some(parameter);
    }
    for instruction in source.blocks.iter().flat_map(|block| &block.instructions) {
        if let Op::Runtime(CoreOp::LoadLocal(slot), _) = instruction.op {
            aliases[instruction.result.0] = Some(
                parameters
                    [parameter_slot(slot).ok_or(Error::InvalidIr("inline local prologue"))?],
            );
        }
    }
    let registers: Vec<_> = lowered
        .registers
        .iter()
        .enumerate()
        .map(|(index, &kind)| {
            aliases[index].unwrap_or_else(|| {
                let result = Register(output.registers.len());
                output.registers.push(kind);
                if kind == lir::Type::Boxed {
                    output.scoped_results.insert(result);
                }
                result
            })
        })
        .collect();
    for (index, &target) in registers.iter().enumerate().take(source.value_count) {
        if aliases[index].is_none() {
            output
                .inline_origins
                .insert(target, (scope, ValueId(index)));
        }
    }
    for (&raw, &boxed) in &lowered.scalar_sources {
        output
            .scalar_sources
            .insert(registers[raw.0], registers[boxed.0]);
    }
    // Entry-guard failure still owns the untouched pre-call state. Once entered, only
    // the suspended caller continuation and logical callee are needed for reconstruction.
    let mut physical = physical;
    let mut outer_values: BTreeMap<_, _> = outer.values.iter().copied().collect();
    for register in outer.locals.iter().flatten() {
        outer_values.insert(ValueId(register.0), *register);
    }
    physical.values = outer_values.into_iter().collect();
    physical.local_values = outer.locals.clone();
    let mut needed: BTreeSet<_> = lowered.exits.iter().map(|exit| exit.state).collect();
    for (block, body) in lowered.blocks.iter().enumerate() {
        for instruction in &body.instructions {
            if fusion.is_some_and(|fusion| fusion.producer(block, instruction)) {
                continue;
            }
            match &instruction.operation {
                Operation::Call {
                    helper: Helper::Python(CoreOp::LoadLocal(_)),
                    ..
                } => {}
                Operation::Call { state, .. } | Operation::Truth { state, .. } => {
                    needed.insert(*state);
                }
                _ => {}
            }
        }
    }
    let mut snapshots = vec![None; lowered.states.len()];
    let mut states = vec![lir::StateId(usize::MAX); lowered.states.len()];
    for (index, state) in lowered.states.iter().enumerate() {
        if needed.contains(&lir::StateId(index)) {
            let (id, snapshot) = inline_state(
                output,
                &physical,
                &outer,
                &resumed_source,
                state,
                &registers,
                &parameters,
                environment,
                scope,
            )?;
            states[index] = id;
            snapshots[index] = Some(snapshot);
        }
    }
    let exits: Vec<_> = lowered
        .exits
        .iter()
        .map(|exit| {
            let id = lir::ExitId(output.exits.len());
            output.exits.push(lir::SideExit {
                state: states[exit.state.0],
                reason: exit.reason,
                inline: Some(snapshots[exit.state.0].as_ref().unwrap().clone()),
            });
            id
        })
        .collect();
    let cfg_base = output.blocks.len();
    let continuation_block = cfg_base + lowered.blocks.len();
    if let Some(block) = cfg_block {
        for (index, source_block) in lowered.blocks.iter().enumerate() {
            output.blocks.push(lir::Block {
                parameters: if index == 0 {
                    vec![]
                } else {
                    source_block
                        .parameters
                        .iter()
                        .map(|r| registers[r.0])
                        .collect()
                },
                instructions: vec![],
                terminator: lir::Terminator::Return(destination),
            });
            output.block_origins.push(None);
        }
        let terminator = std::mem::replace(
            &mut output.blocks[block].terminator,
            lir::Terminator::Jump(lir::Edge {
                block: hir::BlockId(cfg_base),
                arguments: vec![],
            }),
        );
        output.blocks.push(lir::Block {
            parameters: fusion.map_or_else(|| vec![destination], |fusion| fusion.fields.clone()),
            instructions: vec![],
            terminator,
        });
        output.block_origins.push(None);
    }
    let analysis = source.analyze()?;
    for (block_index, source_block) in lowered.blocks.iter().enumerate() {
        let mut copied = Vec::new();
        let body = if cfg_block.is_some() {
            &mut copied
        } else {
            &mut *body
        };
        for instruction in &source_block.instructions {
            if fusion.is_some_and(|fusion| fusion.producer(block_index, instruction)) {
                continue;
            }
            if matches!(
                instruction.operation,
                Operation::Call {
                    helper: Helper::Python(CoreOp::LoadLocal(_)),
                    ..
                }
            ) {
                continue;
            }
            let mut operation = instruction.operation.remap(&registers, &states, &exits);
            match &mut operation {
                Operation::Call {
                    helper,
                    arguments,
                    state,
                    roots,
                    effects,
                } => {
                    // Expression-call plans have a physical-frame fallback. Keep that fallback
                    // scoped until their native preflight can charge this logical frame too.
                    if let Helper::InlineCall { names, .. } = helper {
                        *helper = Helper::Python(CoreOp::Call(names.clone()));
                    }
                    if matches!(
                        helper,
                        Helper::LoadName(_)
                            | Helper::ScalarCall(_)
                            | Helper::Python(CoreOp::Call(_))
                    ) {
                        *helper = Helper::Scoped(Box::new(helper.clone()));
                        arguments.insert(0, environment);
                    }
                    *effects = helper.effects();
                    *roots = if effects.needs_safepoint() {
                        state_roots(output, *state)
                    } else {
                        vec![]
                    };
                }
                Operation::Truth {
                    state,
                    roots,
                    effects,
                    ..
                } => {
                    *roots = if effects.needs_safepoint() {
                        state_roots(output, *state)
                    } else {
                        vec![]
                    };
                }
                _ => {}
            }
            emit(body, instruction.result.map(|r| registers[r.0]), operation);
        }
        let edge = |edge: &lir::Edge| lir::Edge {
            block: hir::BlockId(cfg_base + edge.block.0),
            arguments: edge.arguments.iter().map(|r| registers[r.0]).collect(),
        };
        let target = match &source_block.terminator {
            lir::Terminator::Return(value) => {
                let fields = fusion.map(|fusion| &fusion.returns[&block_index].1);
                let resume = hir::ResumePoint {
                    block: hir::BlockId(block_index),
                    instruction: source.blocks[block_index].instructions.len()
                        - usize::from(fields.is_some()),
                };
                let state = lir::FrameState {
                    resume,
                    environment_root: lir::FrameEnvironment::Current,
                    values: if fields.is_some() {
                        &analysis.live_before[block_index][resume.instruction]
                    } else {
                        &analysis.live_terminator[block_index]
                    }
                    .iter()
                    .map(|v| (*v, Register(v.0)))
                    .collect(),
                    local_count: source.local_slots.len(),
                    local_values: source
                        .local_state(resume)
                        .iter()
                        .map(|v| v.map(|v| Register(v.0)))
                        .collect(),
                };
                let (state, _) = inline_state(
                    output,
                    &physical,
                    &outer,
                    &resumed_source,
                    &state,
                    &registers,
                    &parameters,
                    environment,
                    scope,
                )?;
                let returned = if cfg_block.is_some() {
                    register(output)
                } else {
                    destination
                };
                output.scoped_results.insert(returned);
                output.inline_origins.insert(
                    returned,
                    (scope, fields.map_or(ValueId(value.0), |fields| fields[0])),
                );
                emit(
                    body,
                    Some(returned),
                    helper(
                        if fields.is_some() {
                            Helper::TupleReturn(plan.clone())
                        } else {
                            Helper::LeafReturn(plan.clone())
                        },
                        vec![registers[fields.map_or(value.0, |fields| fields[0].0)]],
                        state,
                    ),
                );
                lir::Terminator::Jump(lir::Edge {
                    block: hir::BlockId(continuation_block),
                    arguments: fields.map_or_else(
                        || vec![returned],
                        |fields| fields.iter().map(|v| registers[v.0]).collect(),
                    ),
                })
            }
            lir::Terminator::Jump(target) => lir::Terminator::Jump(edge(target)),
            lir::Terminator::Branch { condition, yes, no } => lir::Terminator::Branch {
                condition: registers[condition.0],
                yes: edge(yes),
                no: edge(no),
            },
        };
        if cfg_block.is_some() {
            output.blocks[cfg_base + block_index].instructions = copied;
            output.blocks[cfg_base + block_index].terminator = target;
        }
    }
    output.shadow_slots = output.registers.len();
    output.inline_scopes.push(Scope {
        id: scope,
        source: resumed_source,
        callsite: call.point,
        destination: call.origin,
        registers: first_register..output.registers.len(),
    });
    Ok((cfg_block.map(|_| continuation_block), fusion.is_some()))
}

fn state_roots(output: &lir::Function, id: lir::StateId) -> Vec<Register> {
    output.states[id.0]
        .values
        .iter()
        .map(|(_, r)| *r)
        .chain(output.inline_roots(id).iter().copied())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn inline_state(
    output: &mut lir::Function,
    physical: &lir::FrameState,
    outer: &crate::inline_frames::FrameSnapshot,
    source: &Rc<hir::Function>,
    state: &lir::FrameState,
    registers: &[Register],
    parameters: &[Register],
    environment: Register,
    scope: crate::inline_frames::ScopeId,
) -> Result<(lir::StateId, Rc<crate::inline_frames::Snapshot>)> {
    let inner = crate::inline_frames::FrameSnapshot {
        scope,
        parent: Some(crate::inline_frames::ScopeId(0)),
        source: source.clone(),
        resume: state.resume,
        environment,
        values: state
            .values
            .iter()
            .map(|(value, register)| (*value, registers[register.0]))
            .collect(),
        locals: state
            .local_values
            .iter()
            .enumerate()
            .map(|(slot, value)| {
                value.map(|r| registers[r.0]).or_else(|| {
                    source
                        .parameter_names
                        .iter()
                        .position(|name| *name == source.local_slots[slot])
                        .map(|index| parameters[index])
                })
            })
            .collect(),
        pending: None,
    };
    let snapshot = Rc::new(crate::inline_frames::Snapshot {
        frames: vec![outer.clone(), inner],
    });
    let mut state = physical.clone();
    state.environment_root = lir::FrameEnvironment::Inline(Rc::new(lir::InlineState::new(
        snapshot.clone(),
        &output.registers,
    )?));
    let id = lir::StateId(output.states.len());
    output.states.push(state);
    Ok((id, snapshot))
}

/// Recognize only the contiguous, read-only setup emitted by this pass.
/// Keeping the guard in LIR preserves its original side exit and dominance checks.
pub(crate) fn bootstrap_end(body: &[lir::Instruction], index: usize) -> Option<usize> {
    let guard = body.get(index)?;
    let Operation::Call {
        helper: Helper::LeafGuard(plan),
        arguments,
        state,
        effects,
        ..
    } = &guard.operation
    else {
        return None;
    };
    if *effects != Helper::LeafGuard(plan.clone()).effects() {
        return None;
    }
    let result = guard.result?;
    if !matches!(body.get(index + 1)?.operation, Operation::GuardInt { value, .. } if value == result)
    {
        return None;
    }
    let callable = *arguments.first()?;
    let count = if plan.bound { 3 } else { 2 };
    for offset in 0..count {
        let instruction = body.get(index + 2 + offset)?;
        let Operation::Call {
            helper,
            arguments,
            state: next_state,
            effects,
            ..
        } = &instruction.operation
        else {
            return None;
        };
        let expected = match offset {
            0 => Helper::CurrentEnvironment,
            1 => Helper::LeafEnvironment,
            _ => Helper::LeafReceiver,
        };
        let inputs = if offset == 0 {
            &[][..]
        } else {
            std::slice::from_ref(&callable)
        };
        if instruction.result.is_none()
            || !matches!(
                (offset, helper),
                (0, Helper::CurrentEnvironment)
                    | (1, Helper::LeafEnvironment)
                    | (2, Helper::LeafReceiver)
            )
            || arguments != inputs
            || next_state != state
            || *effects != expected.effects()
        {
            return None;
        }
    }
    Some(index + 1 + count)
}

pub(crate) fn bootstrap_reads(body: &[lir::Instruction]) -> BTreeSet<usize> {
    body.iter()
        .enumerate()
        .filter_map(|(index, _)| bootstrap_end(body, index).map(|end| index + 2..=end))
        .flatten()
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::{BytecodeMode, Tiering, Value, Vm, compile};

    #[test]
    fn fused_entries_preserve_reopened_feedback_for_free_and_bound_calls() {
        for tier in [Tiering::Native, Tiering::CopyPatch] {
            for bound in [false, true] {
                let source = if bound {
                    "class Box:\n    def leaf(self,x):\n        return [x]\nb=Box()\ndef caller(x):\n    return b.leaf(x)\nfor i in range(500):\n    caller(i)\n"
                } else {
                    "def leaf(x):\n    return [x]\ndef caller(x):\n    return leaf(x)\nfor i in range(500):\n    caller(i)\n"
                };
                let mut vm = Vm::new();
                vm.tiering = tier;
                vm.bytecode = BytecodeMode::Register;
                vm.thresholds.function_entries = 16;
                vm.thresholds.loop_backedges = 32;
                vm.run_program(&mut compile(source).unwrap()).unwrap();
                let caller = vm.get_global("caller").unwrap();
                let before = vm.tier_stats.clone();
                let value = vm.call(caller, &[Value::Int(501)]).unwrap();
                assert_eq!(
                    vm.heap.get_item(value, Value::Int(0)).unwrap(),
                    Value::Int(501)
                );
                assert_eq!(
                    vm.tier_stats.native_leaf_entry_batches - before.native_leaf_entry_batches,
                    1
                );
                let hir = vm.function_hir(caller).unwrap();
                hir.profile.borrow_mut().phase = crate::profile::ProfilePhase::Profiling;
                let before = vm.tier_stats.clone();
                let value = vm.call(caller, &[Value::Int(502)]).unwrap();
                assert_eq!(
                    vm.heap.get_item(value, Value::Int(0)).unwrap(),
                    Value::Int(502)
                );
                assert_eq!(
                    vm.tier_stats.leaf_body_inlines - before.leaf_body_inlines,
                    1
                );
                assert_eq!(
                    vm.tier_stats.native_leaf_entry_batches,
                    before.native_leaf_entry_batches
                );
                assert!(vm.tier_stats.native_feedback_probes > before.native_feedback_probes);
                assert_eq!(vm.stack_len(), 0);
            }
        }
    }
    #[test]
    fn copied_comparisons_do_not_pollute_caller_feedback() {
        for tier in [Tiering::Native, Tiering::CopyPatch] {
            let mut vm = Vm::new();
            vm.tiering = tier;
            vm.cfg_inlining = true;
            vm.thresholds.function_entries = 16;
            vm.thresholds.loop_backedges = 32;
            vm.run_program(&mut compile("def leaf(x):\n    if x>0:\n        return [x]\n    return [x+1]\ndef caller(x):\n    return leaf(x)\nfor i in range(500):\n    caller(1)\n").unwrap()).unwrap();
            let caller = vm.get_global("caller").unwrap();
            let hir = vm.function_hir(caller).unwrap();
            let call = hir
                .blocks
                .iter()
                .flat_map(|b| &b.instructions)
                .find(|i| {
                    matches!(
                        i.op,
                        crate::hir::Op::Runtime(crate::hir::CoreOp::Call(_), _)
                    )
                })
                .unwrap()
                .result;
            let before = hir.profile().sites[call.0]
                .result
                .count(crate::profile::ValueKind::Bool);
            hir.profile.borrow_mut().phase = crate::profile::ProfilePhase::Profiling;
            let hits = vm.tier_stats.leaf_body_inlines;
            let result = vm.call(caller, &[Value::Int(2)]).unwrap();
            assert_eq!(
                vm.heap.get_item(result, Value::Int(0)).unwrap(),
                Value::Int(2)
            );
            assert!(vm.tier_stats.leaf_body_inlines > hits);
            assert_eq!(
                hir.profile().sites[call.0]
                    .result
                    .count(crate::profile::ValueKind::Bool),
                before
            );
            assert_eq!(vm.stack_len(), 0);
        }
    }
}
