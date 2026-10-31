//! Suspended bytecode frames retain lexical state without retaining a Rust stack.
use crate::{
    Error, Object, Result, Value, Vm,
    bytecode::Code,
    hir::{Function, ValueId},
};
use std::{cell::RefCell, rc::Rc};
#[derive(Debug, Clone)]
pub struct SavedFrame {
    pub slots: Vec<Option<Value>>,
    pub locals: Vec<Option<Value>>,
    pub ip: usize,
    pub previous: Option<usize>,
}
#[derive(Debug, Clone)]
pub enum Frame {
    Fresh {
        arguments: Vec<Value>,
        defaults: Option<(Vec<Value>, usize)>,
    },
    Saved(SavedFrame),
}
#[derive(Debug, Clone)]
pub struct Generator {
    pub function: Rc<RefCell<Function>>,
    pub code: Rc<Code>,
    pub environment: Value,
    pub mode: crate::BytecodeMode,
    pub frame: Option<Frame>,
    pub running: bool,
    pub result: Value,
}
impl Generator {
    pub(crate) fn visit(&self, visit: &mut impl FnMut(Value)) {
        visit(self.environment);
        visit(self.result);
        match &self.frame {
            Some(Frame::Fresh {
                arguments,
                defaults,
            }) => {
                arguments.iter().copied().for_each(&mut *visit);
                if let Some((defaults, _)) = defaults {
                    defaults.iter().copied().for_each(visit);
                }
            }
            Some(Frame::Saved(frame)) => frame
                .slots
                .iter()
                .chain(&frame.locals)
                .flatten()
                .copied()
                .for_each(visit),
            None => {}
        }
    }
}
#[derive(Debug)]
pub(crate) struct YieldPoint {
    pub depth: usize,
    pub ip: usize,
    pub previous: Option<usize>,
    pub destination: Option<ValueId>,
}
impl Vm {
    pub(crate) fn next_generator(&mut self, receiver: Value) -> Result<Value> {
        self.protocol_roots(&[receiver], |vm| {
            let Object::Suspended(generator) = vm.heap.get(receiver)? else {
                return Err(Error::Type("expected generator"));
            };
            if generator.running {
                return Err(Error::Runtime(
                    "ValueError: generator already executing".into(),
                ));
            }
            let Some(frame) = generator.frame.clone() else {
                return Ok(vm.heap.allocate(Object::Done));
            };
            let (function, code, environment, mode) = (
                generator.function.clone(),
                generator.code.clone(),
                generator.environment,
                generator.mode,
            );
            if let Object::Suspended(g) = &mut *vm.heap.get_mut(receiver)? {
                g.running = true;
            }
            let old_mode = vm.bytecode;
            vm.bytecode = mode;
            let result = vm.resume_generator_frame(function, code, environment, frame);
            vm.bytecode = old_mode;
            let saved = vm.suspended_frame.take();
            let yielded = {
                let mut object = vm.heap.get_mut(receiver)?;
                let Object::Suspended(generator) = &mut *object else {
                    return Err(Error::InvalidIr("generator owner"));
                };
                generator.running = false;
                generator.frame = saved.map(Frame::Saved);
                if let Ok(value) = result
                    && generator.frame.is_none()
                {
                    generator.result = value;
                }
                generator.frame.is_some()
            };
            match result {
                Ok(value) if yielded => Ok(value),
                Ok(_) => Ok(vm.heap.allocate(Object::Done)),
                Err(Error::Runtime(message))
                    if message == "StopIteration" || message.starts_with("StopIteration:") =>
                {
                    vm.pending_exception = None;
                    Err(Error::Runtime(
                        "RuntimeError: generator raised StopIteration".into(),
                    ))
                }
                Err(err) => Err(err),
            }
        })
    }
}
