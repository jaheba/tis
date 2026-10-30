mod analysis;
mod builtins;
mod comprehension;
mod promote;
mod view;
pub use analysis::Analysis;

use crate::profile::{self, FunctionId, FunctionProfile};
use std::{cell::RefCell, fmt, rc::Rc};

use crate::{Result, bytecode::Code};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResumePoint {
    pub block: BlockId,
    pub instruction: usize,
}

/// IDs remain stable when execution specializes an instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ValueId(pub usize);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlockId(pub usize);

#[derive(Debug, Clone)]
pub enum Constant {
    /// Internal marker carried through local-value joins, never a Python value.
    Unbound,
    None,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
}

impl Constant {
    // These values contain no heap identity and can be embedded by every execution backend.
    #[inline]
    pub(crate) fn immediate(&self) -> Option<crate::Value> {
        use crate::Value;
        Some(match self {
            Self::Unbound => Value::Unbound,
            Self::None => Value::None,
            Self::Bool(v) => Value::Bool(*v),
            Self::Int(v) => Value::Int(*v),
            Self::Float(v) => Value::Float(*v),
            Self::String(s) => Value::InlineString(crate::InlineString::new(s)?),
        })
    }
}

#[derive(Debug, Clone)]
pub enum Op {
    Constant(Constant),
    Add(ValueId, ValueId),
    Subtract(ValueId, ValueId),
    Less(ValueId, ValueId),
    List(Vec<ValueId>),
    Tuple(Vec<ValueId>),
    Dict(Vec<(ValueId, ValueId)>),
    GetItem(ValueId, ValueId),
    SetItem {
        object: ValueId,
        key: ValueId,
        value: ValueId,
    },
    Len(ValueId),
    Runtime(CoreOp, Vec<ValueId>),
}

#[derive(Debug, Clone)]
pub struct Edge {
    pub block: BlockId,
    pub arguments: Vec<ValueId>,
}

impl Edge {
    pub fn new(block: BlockId, arguments: Vec<ValueId>) -> Self {
        Self { block, arguments }
    }
}

#[derive(Debug, Clone)]
pub enum Terminator {
    Return(ValueId),
    Jump(Edge),
    Branch {
        condition: ValueId,
        yes: Edge,
        no: Edge,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AddStrategy {
    #[default]
    Generic,
    Integer,
}

/// Observations select a guarded execution strategy; they are not proofs of types.
#[derive(Debug, Clone, Default)]
pub struct AddFeedback {
    pub strategy: AddStrategy,
    pub executions: u64,
    pub fast_paths: u64,
    pub guard_failures: u64,
    pub(crate) consecutive_integers: u8,
}

impl AddFeedback {
    pub const WARMUP: u8 = 8;
}

#[derive(Debug, Clone)]
pub struct Instruction {
    pub result: ValueId,
    pub op: Op,
    pub feedback: Option<AddFeedback>,
}

#[derive(Debug, Clone)]
pub struct Block {
    pub parameters: Vec<ValueId>,
    pub instructions: Vec<Instruction>,
    pub terminator: Option<Terminator>,
}

/// Runtime operations retain their semantic identity through bytecode lowering.
#[derive(Debug, Clone)]
pub enum CoreOp {
    Missing,
    BytesLiteral(Vec<u8>),
    IntegerLiteral(num_bigint::BigInt),
    Import(String),
    UnpackStar(usize, usize),
    LoadName(String),
    StoreName(String),
    DeleteName(String),
    DeleteBinding(String),
    DeleteAttr(String),
    DeleteItem,
    ZeroSuper(Option<String>),
    LoadLocal(usize),
    StoreLocal(usize),
    CheckLocal(usize),
    Binary(String),
    Inplace(String),
    Compare(String),
    Unary(String),
    Call(Vec<Option<String>>),
    Function(Box<Function>),
    Class(String, Box<Function>),
    GetAttr(String),
    SetAttr(String),
    TryCall(Box<Function>),
    With(Box<Function>),
    PropagateException,
    ExceptionMatch,
    EnterExcept,
    ExitExcept,
    ListAppend,
    Generator(usize, Option<String>),
    Iter,
    Next,
    IsDone,
    Unpack(usize),
    UnpackItem(usize),
    Slice,
    Set,
    Format(String, bool),
    Join,
    Assert,
    Raise,
    Namespace,
}

/// A function owns semantic IR, lexical scope metadata and cached bytecode.
#[derive(Debug, Clone)]
pub struct Function {
    id: FunctionId,
    pub(crate) name: String,
    revision: u64,
    pub(crate) profile: Rc<RefCell<FunctionProfile>>,
    pub(crate) blocks: Vec<Block>,
    pub(crate) parameter_names: Vec<String>,
    pub(crate) positional_only: usize,
    pub(crate) keyword_only: Vec<String>,
    pub(crate) vararg: Option<String>,
    pub(crate) kwarg: Option<String>,
    pub(crate) local_names: Vec<String>,
    pub(crate) local_slots: Vec<String>,
    pub(crate) local_states: Vec<Vec<Vec<Option<ValueId>>>>,
    pub(crate) global_names: Vec<String>,
    pub(crate) nonlocal_names: Vec<String>,
    pub(crate) module: bool,
    pub(crate) value_count: usize,
    pub(crate) code: Option<Rc<Code>>,
    locations: Vec<Option<(usize, usize)>>,
}

impl Function {
    pub fn new(parameters: usize) -> Self {
        let mut function = Self {
            id: profile::function_id(),
            name: "<anonymous>".into(),
            revision: profile::revision(),
            profile: Rc::new(RefCell::new(FunctionProfile::default())),
            blocks: Vec::new(),
            parameter_names: Vec::new(),
            positional_only: 0,
            keyword_only: Vec::new(),
            vararg: None,
            kwarg: None,
            local_names: Vec::new(),
            local_slots: Vec::new(),
            local_states: Vec::new(),
            global_names: Vec::new(),
            nonlocal_names: Vec::new(),
            module: false,
            value_count: 0,
            code: None,
            locations: Vec::new(),
        };
        function.block(parameters);
        function
    }

    pub fn entry(&self) -> BlockId {
        BlockId(0)
    }

    pub(crate) fn environment_required(&self) -> bool {
        if let Some(code) = &self.code {
            return code.environment_required;
        }
        self.module
            || self.local_names.len() != self.local_slots.len()
            || !self.global_names.is_empty()
            || !self.nonlocal_names.is_empty()
            || self
                .blocks
                .iter()
                .flat_map(|block| &block.instructions)
                .any(|i| matches!(i.op, Op::Runtime(CoreOp::Namespace, _)))
    }

    pub fn local_slots(&self) -> &[String] {
        &self.local_slots
    }

    /// Bindings before an operation; None retains the original frame slot during the prologue.
    pub fn local_state(&self, point: ResumePoint) -> &[Option<ValueId>] {
        self.local_states
            .get(point.block.0)
            .and_then(|states| states.get(point.instruction))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub(crate) fn index_locals(&mut self, class_scope: bool) {
        fn references(function: &Function, names: &mut std::collections::BTreeSet<String>) {
            for instruction in function.blocks.iter().flat_map(|b| &b.instructions) {
                match &instruction.op {
                    Op::Runtime(
                        CoreOp::LoadName(name)
                        | CoreOp::StoreName(name)
                        | CoreOp::DeleteName(name)
                        | CoreOp::DeleteBinding(name)
                        | CoreOp::ZeroSuper(Some(name)),
                        _,
                    ) => {
                        names.insert(name.clone());
                    }
                    Op::Runtime(
                        CoreOp::Function(body)
                        | CoreOp::TryCall(body)
                        | CoreOp::With(body)
                        | CoreOp::Class(_, body),
                        _,
                    ) => {
                        references(body, names);
                    }
                    _ => {}
                }
            }
        }
        let mut captured = std::collections::BTreeSet::new();
        for instruction in self.blocks.iter().flat_map(|b| &b.instructions) {
            if let Op::Runtime(
                CoreOp::Function(body)
                | CoreOp::TryCall(body)
                | CoreOp::With(body)
                | CoreOp::Class(_, body),
                _,
            ) = &instruction.op
            {
                references(body, &mut captured);
            }
        }
        for instruction in self.blocks.iter().flat_map(|block| &block.instructions) {
            if let Op::Runtime(
                CoreOp::DeleteName(name)
                | CoreOp::DeleteBinding(name)
                | CoreOp::ZeroSuper(Some(name)),
                _,
            ) = &instruction.op
            {
                captured.insert(name.clone());
            }
        }
        // Overlapping names in nested scopes conservatively retain a shared environment binding.
        if !self.module && !class_scope {
            self.local_slots = self
                .local_names
                .iter()
                .filter(|name| !captured.contains(*name))
                .cloned()
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect();
        }
        for instruction in self.blocks.iter_mut().flat_map(|b| &mut b.instructions) {
            if let Op::Runtime(operation, _) = &mut instruction.op {
                match operation {
                    CoreOp::Function(body) | CoreOp::TryCall(body) | CoreOp::With(body) => {
                        body.index_locals(false)
                    }
                    CoreOp::Class(_, body) => body.index_locals(true),
                    CoreOp::LoadName(name) | CoreOp::StoreName(name) => {
                        if let Ok(slot) = self.local_slots.binary_search(name) {
                            *operation = if matches!(operation, CoreOp::LoadName(_)) {
                                CoreOp::LoadLocal(slot)
                            } else {
                                CoreOp::StoreLocal(slot)
                            };
                        }
                    }
                    _ => {}
                }
            }
        }
        self.invalidate();
    }

    pub fn block(&mut self, parameters: usize) -> BlockId {
        self.invalidate();
        let id = BlockId(self.blocks.len());
        let parameters = (0..parameters).map(|_| self.value()).collect();
        self.blocks.push(Block {
            parameters,
            instructions: Vec::new(),
            terminator: None,
        });
        if !self.local_states.is_empty() {
            self.local_states
                .push(vec![vec![None; self.local_slots.len()]]);
        }
        id
    }

    pub fn parameters(&self, block: BlockId) -> &[ValueId] {
        &self.blocks[block.0].parameters
    }

    pub fn append(&mut self, block: BlockId, op: Op) -> ValueId {
        if !self.local_states.is_empty() {
            let states = &mut self.local_states[block.0];
            states.push(states.last().unwrap().clone());
        }
        let result = self.value();
        let feedback = matches!(op, Op::Add(..)).then(AddFeedback::default);
        self.locations[result.0] = Some((block.0, self.blocks[block.0].instructions.len()));
        self.blocks[block.0].instructions.push(Instruction {
            result,
            op,
            feedback,
        });
        result
    }

    pub fn terminate(&mut self, block: BlockId, terminator: Terminator) {
        self.invalidate();
        self.blocks[block.0].terminator = Some(terminator);
    }

    pub fn feedback(&self, value: ValueId) -> Option<&AddFeedback> {
        let &(block, instruction) = self.locations.get(value.0)?.as_ref()?;
        self.blocks[block].instructions[instruction]
            .feedback
            .as_ref()
    }

    pub(crate) fn feedback_mut(&mut self, value: ValueId) -> Option<&mut AddFeedback> {
        let &(block, instruction) = self.locations.get(value.0)?.as_ref()?;
        self.blocks[block].instructions[instruction]
            .feedback
            .as_mut()
    }

    pub fn bytecode(&mut self) -> Result<&Code> {
        if self.code.is_none() {
            self.code = Some(Rc::new(Code::lower(self)?));
        }
        Ok(self.code.as_ref().unwrap())
    }

    pub fn id(&self) -> FunctionId {
        self.id
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn profile(&self) -> FunctionProfile {
        self.profile.borrow().clone()
    }
    pub fn analyze(&self) -> Result<Analysis> {
        analysis::analyze(self)
    }
    pub(crate) fn validated_backedges(&self) -> Result<std::collections::BTreeSet<(usize, usize)>> {
        analysis::validated_backedges(self)
    }

    pub fn lower_lir(&self, mode: crate::lir::LoweringMode) -> Result<crate::lir::Function> {
        crate::lir::Function::lower(self, mode)
    }
    pub(crate) fn snapshot(&self) -> Self {
        let mut snapshot = self.clone();
        snapshot.profile = Rc::new(RefCell::new(self.profile()));
        snapshot.code = None;
        for block in &mut snapshot.blocks {
            for instruction in &mut block.instructions {
                if let Op::Runtime(
                    CoreOp::Function(body)
                    | CoreOp::TryCall(body)
                    | CoreOp::With(body)
                    | CoreOp::Class(_, body),
                    _,
                ) = &mut instruction.op
                {
                    **body = body.snapshot();
                }
            }
        }
        snapshot
    }
    pub(crate) fn shared_bytecode(&mut self) -> Result<Rc<Code>> {
        self.bytecode()?;
        Ok(self.code.as_ref().unwrap().clone())
    }
    pub(crate) fn instruction(&self, value: ValueId) -> Option<&Instruction> {
        let &(block, index) = self.locations.get(value.0)?.as_ref()?;
        self.blocks.get(block)?.instructions.get(index)
    }
    fn invalidate(&mut self) {
        self.code = None;
        self.revision = profile::revision();
        self.profile = Rc::new(RefCell::new(FunctionProfile::default()));
    }

    fn value(&mut self) -> ValueId {
        let id = ValueId(self.value_count);
        self.invalidate();
        self.value_count += 1;
        self.locations.push(None);
        id
    }
}

impl fmt::Display for Function {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, block) in self.blocks.iter().enumerate() {
            write!(f, "b{index}(")?;
            operands(f, &block.parameters)?;
            writeln!(f, "):")?;
            for instruction in &block.instructions {
                writeln!(f, "    {instruction}")?;
            }
            write!(f, "    ")?;
            match &block.terminator {
                Some(terminator) => writeln!(f, "{terminator}")?,
                None => writeln!(f, "<unterminated>")?,
            }
        }
        Ok(())
    }
}

impl fmt::Display for Instruction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "%{} = ", self.result.0)?;
        if let Op::Add(lhs, rhs) = self.op {
            let name = match self.feedback.as_ref().map(|feedback| feedback.strategy) {
                Some(AddStrategy::Integer) => "add.int [fallback=add]",
                _ => "add",
            };
            write!(f, "{name} %{}, %{}", lhs.0, rhs.0)
        } else {
            write!(f, "{}", self.op)
        }
    }
}

impl fmt::Display for Terminator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Return(value) => write!(f, "return %{}", value.0),
            Self::Jump(edge) => write!(f, "jump {edge}"),
            Self::Branch { condition, yes, no } => {
                write!(f, "branch %{}, {yes}, {no}", condition.0)
            }
        }
    }
}

impl fmt::Display for Edge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "b{}(", self.block.0)?;
        operands(f, &self.arguments)?;
        write!(f, ")")
    }
}

impl fmt::Display for Op {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Constant(value) => write!(f, "const {value:?}"),
            Self::Add(lhs, rhs) => write!(f, "add %{}, %{}", lhs.0, rhs.0),
            Self::Subtract(lhs, rhs) => write!(f, "subtract %{}, %{}", lhs.0, rhs.0),
            Self::Less(lhs, rhs) => write!(f, "less %{}, %{}", lhs.0, rhs.0),
            Self::List(values) | Self::Tuple(values) => {
                write!(
                    f,
                    "{}(",
                    if matches!(self, Self::List(_)) {
                        "list"
                    } else {
                        "tuple"
                    }
                )?;
                operands(f, values)?;
                write!(f, ")")
            }
            Self::Dict(entries) => {
                write!(f, "dict(")?;
                for (index, (key, value)) in entries.iter().enumerate() {
                    if index != 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "%{}: %{}", key.0, value.0)?;
                }
                write!(f, ")")
            }
            Self::GetItem(object, key) => write!(f, "getitem %{}, %{}", object.0, key.0),
            Self::SetItem { object, key, value } => {
                write!(f, "setitem %{}, %{}, %{}", object.0, key.0, value.0)
            }
            Self::Len(object) => write!(f, "len %{}", object.0),
            Self::Runtime(operation, values) => {
                match operation {
                    CoreOp::Function(_) => write!(f, "function")?,
                    CoreOp::TryCall(_) => write!(f, "try expression")?,
                    CoreOp::Class(name, _) => write!(f, "class {name}")?,
                    other => write!(f, "{other:?}")?,
                }
                write!(f, "(")?;
                operands(f, values)?;
                write!(f, ")")
            }
        }
    }
}

fn operands(f: &mut fmt::Formatter<'_>, values: &[ValueId]) -> fmt::Result {
    for (index, value) in values.iter().enumerate() {
        if index != 0 {
            write!(f, ", ")?;
        }
        write!(f, "%{}", value.0)?;
    }
    Ok(())
}
