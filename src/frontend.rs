//! Ruff AST lowering. Every executable construct becomes value IR before bytecode.

use crate::{
    Error, Result,
    hir::{BlockId, Constant, CoreOp, Edge, Function, Op, Terminator, ValueId},
};
use ruff_python_ast::{self as ast, Expr, Stmt};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug)]
pub struct Program {
    functions: BTreeMap<String, Function>,
}
impl Program {
    pub fn function(&self, name: &str) -> Option<&Function> {
        self.functions.get(name)
    }
    pub fn function_mut(&mut self, name: &str) -> Option<&mut Function> {
        self.functions.get_mut(name)
    }
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.functions.keys().map(String::as_str)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CompileOptions {
    pub promote_locals: bool,
}
impl Default for CompileOptions {
    fn default() -> Self {
        Self {
            promote_locals: true,
        }
    }
}

pub fn compile(source: &str) -> Result<Program> {
    compile_with_options(source, CompileOptions::default())
}

pub fn compile_with_options(source: &str, options: CompileOptions) -> Result<Program> {
    let module = ruff_python_parser::parse_module(source)
        .map_err(|e| Error::Syntax(e.to_string()))?
        .into_syntax();
    validate_scope(&module.body, &[], false, false)?;
    let mut builder = Builder::new(&module.body, true)?;
    builder.function.name = "<module>".into();
    let mut block = Some(builder.function.entry());
    for (index, statement) in module.body.iter().enumerate() {
        let Some(current) = block else {
            break;
        };
        if index + 1 == module.body.len()
            && let Stmt::Expr(expr) = statement
        {
            let (current, value) = builder.expression(&expr.value, current)?;
            builder
                .function
                .terminate(current, Terminator::Return(value));
            block = None;
        } else {
            block = builder.statement(statement, current)?;
        }
    }
    builder.finish(block)?;
    builder.function.index_locals(false);
    if options.promote_locals {
        builder.function.promote_locals()?;
    }
    builder.function.specialize_lists()?;
    let mut functions = BTreeMap::new();
    for block in &builder.function.blocks {
        for instruction in &block.instructions {
            if let Op::Runtime(CoreOp::Function(function), _) = &instruction.op {
                // Named functions remain inspectable before module execution.
                let next=block.instructions.iter().find(|candidate|matches!(&candidate.op,Op::Runtime(CoreOp::StoreName(_),values) if values==&[instruction.result]));
                if let Some(next) = next
                    && let Op::Runtime(CoreOp::StoreName(name), _) = &next.op
                {
                    functions.insert(name.clone(), (**function).clone());
                }
            }
        }
    }
    functions.insert("<module>".into(), builder.function);
    Ok(Program { functions })
}

struct Builder {
    function: Function,
    loops: Vec<(BlockId, BlockId, usize)>,
    except_depth: usize,
}
impl Builder {
    fn new(body: &[Stmt], module: bool) -> Result<Self> {
        let mut function = Function::new(0);
        function.module = module;
        let mut locals = BTreeSet::new();
        let mut globals = BTreeSet::new();
        let mut nonlocals = BTreeSet::new();
        bindings(body, &mut locals, &mut globals, &mut nonlocals);
        if !globals.is_disjoint(&nonlocals) {
            return Err(Error::Syntax("name declared global and nonlocal".into()));
        }
        function.local_names = locals
            .difference(&globals)
            .filter(|n| !nonlocals.contains(*n))
            .cloned()
            .collect();
        function.global_names = globals.into_iter().collect();
        function.nonlocal_names = nonlocals.into_iter().collect();
        Ok(Self {
            function,
            loops: vec![],
            except_depth: 0,
        })
    }
    fn runtime(&mut self, block: BlockId, op: CoreOp, values: Vec<ValueId>) -> ValueId {
        self.function.append(block, Op::Runtime(op, values))
    }
    fn constant(&mut self, block: BlockId, value: Constant) -> ValueId {
        self.function.append(block, Op::Constant(value))
    }
    fn none(&mut self, block: BlockId) -> ValueId {
        self.constant(block, Constant::None)
    }
    fn jump(&mut self, from: BlockId, to: BlockId) {
        self.function
            .terminate(from, Terminator::Jump(Edge::new(to, vec![])));
    }
    fn branch(&mut self, block: BlockId, condition: ValueId, yes: BlockId, no: BlockId) {
        self.function.terminate(
            block,
            Terminator::Branch {
                condition,
                yes: Edge::new(yes, vec![]),
                no: Edge::new(no, vec![]),
            },
        );
    }
    fn finish(&mut self, block: Option<BlockId>) -> Result<()> {
        if let Some(block) = block {
            let value = self.none(block);
            self.function.terminate(block, Terminator::Return(value));
        }
        // Lower bytecode only after lexical indexing and HIR transformations, when requested.
        Ok(())
    }
    fn statements(&mut self, body: &[Stmt], mut block: Option<BlockId>) -> Result<Option<BlockId>> {
        for statement in body {
            if let Some(current) = block {
                block = self.statement(statement, current)?;
            } else {
                break;
            }
        }
        Ok(block)
    }

    fn function(parameters: &ast::Parameters, body: &[Stmt]) -> Result<Function> {
        let mut builder = Self::new(body, false)?;
        builder.parameters(parameters)?;
        let block = builder.statements(body, Some(builder.function.entry()))?;
        builder.finish(block)?;
        Ok(builder.function)
    }
    fn parameters(&mut self, parameters: &ast::Parameters) -> Result<()> {
        self.function.positional_only = parameters.posonlyargs.len();
        self.function.parameter_names = parameters
            .posonlyargs
            .iter()
            .chain(&parameters.args)
            .map(|p| p.parameter.name.to_string())
            .collect();
        self.function.keyword_only = parameters
            .kwonlyargs
            .iter()
            .map(|p| p.parameter.name.to_string())
            .collect();
        self.function.vararg = parameters.vararg.as_ref().map(|p| p.name.to_string());
        self.function.kwarg = parameters.kwarg.as_ref().map(|p| p.name.to_string());
        let mut names = BTreeSet::new();
        for name in self
            .function
            .parameter_names
            .iter()
            .chain(&self.function.keyword_only)
            .chain(self.function.vararg.iter())
            .chain(self.function.kwarg.iter())
        {
            if !names.insert(name.clone())
                || self.function.global_names.contains(name)
                || self.function.nonlocal_names.contains(name)
            {
                return Err(Error::Syntax(format!("invalid parameter {name}")));
            }
        }
        self.function.local_names.extend(names);
        Ok(())
    }
    fn defaults(
        &mut self,
        parameters: &ast::Parameters,
        mut block: BlockId,
    ) -> Result<(BlockId, Vec<ValueId>)> {
        let mut values = vec![];
        for parameter in parameters.posonlyargs.iter().chain(&parameters.args) {
            if let Some(default) = &parameter.default {
                let (next, value) = self.expression(default, block)?;
                block = next;
                values.push(value);
            }
        }
        for parameter in &parameters.kwonlyargs {
            let value = if let Some(default) = &parameter.default {
                let (next, value) = self.expression(default, block)?;
                block = next;
                value
            } else {
                self.runtime(block, CoreOp::Missing, vec![])
            };
            values.push(value);
        }
        Ok((block, values))
    }

    fn statement(&mut self, statement: &Stmt, mut block: BlockId) -> Result<Option<BlockId>> {
        match statement {
            Stmt::Import(stmt) => {
                for alias in &stmt.names {
                    if !matches!(alias.name.as_str(), "math" | "json" | "bisect") {
                        return Err(Error::Unsupported(
                            "only builtin math, json and bisect imports",
                        ));
                    }
                    let value = self.runtime(block, CoreOp::Import(alias.name.to_string()), vec![]);
                    let name = alias.asname.as_ref().unwrap_or(&alias.name).to_string();
                    self.store_target(Target::Name(name), value, block)?;
                }
            }
            Stmt::ImportFrom(stmt) => {
                let module = stmt.module.as_ref().map(|name| name.as_str());
                if stmt.level != 0 || !matches!(module, Some("math" | "json" | "bisect")) {
                    return Err(Error::Unsupported(
                        "only builtin math, json and bisect imports",
                    ));
                }
                let module = self.runtime(block, CoreOp::Import(module.unwrap().into()), vec![]);
                for alias in &stmt.names {
                    if alias.name.as_str() == "*" {
                        return Err(Error::Unsupported("star imports"));
                    }
                    let value =
                        self.runtime(block, CoreOp::GetAttr(alias.name.to_string()), vec![module]);
                    let name = alias.asname.as_ref().unwrap_or(&alias.name).to_string();
                    self.store_target(Target::Name(name), value, block)?;
                }
            }
            Stmt::Pass(_) | Stmt::Global(_) | Stmt::Nonlocal(_) => {}
            Stmt::Expr(expr) => {
                block = self.expression(&expr.value, block)?.0;
            }
            Stmt::Return(stmt) => {
                let value = if let Some(value) = &stmt.value {
                    let (next, value) = self.expression(value, block)?;
                    block = next;
                    value
                } else {
                    self.none(block)
                };
                self.close_except(block, 0);
                self.function.terminate(block, Terminator::Return(value));
                return Ok(None);
            }
            Stmt::Assign(stmt) => {
                let (next, value) = self.expression(&stmt.value, block)?;
                block = next;
                for target in &stmt.targets {
                    block = self.assign(target, value, block)?;
                }
            }
            Stmt::AnnAssign(stmt) => {
                if let Some(value) = &stmt.value {
                    let (next, value) = self.expression(value, block)?;
                    block = self.assign(&stmt.target, value, next)?;
                }
            }
            Stmt::AugAssign(stmt) => {
                // Evaluate a subscript or attribute target once, before the RHS.
                let (next, old, target) = self.target(&stmt.target, block)?;
                let (next, rhs) = self.expression(&stmt.value, next)?;
                let value = self.runtime(
                    next,
                    CoreOp::Inplace(operator_name(stmt.op)?.into()),
                    vec![old, rhs],
                );
                block = self.store_target(target, value, next)?;
            }
            Stmt::FunctionDef(stmt) => {
                if stmt.is_async || stmt.type_params.is_some() {
                    return Err(Error::Unsupported("async or generic function"));
                }
                if !stmt.decorator_list.is_empty() {
                    return Err(Error::Unsupported("function decorators"));
                }
                let function =
                    Self::function(&stmt.parameters, &stmt.body)?.with_name(stmt.name.to_string());
                let (next, defaults) = self.defaults(&stmt.parameters, block)?;
                block = next;
                let value = self.runtime(block, CoreOp::Function(Box::new(function)), defaults);
                self.runtime(block, CoreOp::StoreName(stmt.name.to_string()), vec![value]);
            }
            Stmt::ClassDef(stmt) => {
                if !stmt.decorator_list.is_empty() || stmt.type_params.is_some() {
                    return Err(Error::Unsupported("decorated or generic class"));
                }
                let mut bases = vec![];
                if let Some(arguments) = &stmt.arguments {
                    if !arguments.keywords.is_empty() {
                        return Err(Error::Unsupported("class keywords"));
                    }
                    for expr in &arguments.args {
                        if matches!(expr,Expr::Name(name) if name.id.as_str()=="object") {
                            continue;
                        }
                        let (next, value) = self.expression(expr, block)?;
                        block = next;
                        bases.push(value);
                    }
                }
                let mut body = Self::new(&stmt.body, false)?;
                body.function.name = format!("<class {}>", stmt.name);
                let end = body.statements(&stmt.body, Some(body.function.entry()))?;
                body.finish(end)?;
                let value = self.runtime(
                    block,
                    CoreOp::Class(stmt.name.to_string(), Box::new(body.function)),
                    bases,
                );
                self.runtime(block, CoreOp::StoreName(stmt.name.to_string()), vec![value]);
            }
            Stmt::If(stmt) => {
                return self.conditional(&stmt.test, &stmt.body, &stmt.elif_else_clauses, block);
            }
            Stmt::While(stmt) => {
                let head = self.function.block(0);
                let body = self.function.block(0);
                let exhausted = self.function.block(0);
                let exit = self.function.block(0);
                self.jump(block, head);
                self.condition(&stmt.test, head, body, exhausted)?;
                self.loops.push((head, exit, self.except_depth));
                let end = self.statements(&stmt.body, Some(body))?;
                self.loops.pop();
                if let Some(end) = end {
                    self.jump(end, head);
                }
                if let Some(end) = self.statements(&stmt.orelse, Some(exhausted))? {
                    self.jump(end, exit);
                }
                block = exit;
            }
            Stmt::For(stmt) => {
                if stmt.is_async {
                    return Err(Error::Unsupported("async for"));
                }
                let (next, source) = self.expression(&stmt.iter, block)?;
                let iter = self.runtime(next, CoreOp::Iter, vec![source]);
                let head = self.function.block(0);
                let body = self.function.block(0);
                let exhausted = self.function.block(0);
                let exit = self.function.block(0);
                self.jump(next, head);
                let value = self.runtime(head, CoreOp::Next, vec![iter]);
                let done = self.runtime(head, CoreOp::IsDone, vec![value]);
                self.branch(head, done, exhausted, body);
                let body = self.assign(&stmt.target, value, body)?;
                self.loops.push((head, exit, self.except_depth));
                let end = self.statements(&stmt.body, Some(body))?;
                self.loops.pop();
                if let Some(end) = end {
                    self.jump(end, head);
                }
                if let Some(end) = self.statements(&stmt.orelse, Some(exhausted))? {
                    self.jump(end, exit);
                }
                block = exit;
            }
            Stmt::Break(_) => {
                let &(_, exit, depth) = self
                    .loops
                    .last()
                    .ok_or_else(|| Error::Syntax("break outside loop".into()))?;
                self.close_except(block, depth);
                self.jump(block, exit);
                return Ok(None);
            }
            Stmt::Continue(_) => {
                let &(head, _, depth) = self
                    .loops
                    .last()
                    .ok_or_else(|| Error::Syntax("continue outside loop".into()))?;
                self.close_except(block, depth);
                self.jump(block, head);
                return Ok(None);
            }
            Stmt::Try(stmt) => return self.try_statement(stmt, block),
            Stmt::Raise(stmt) => {
                if stmt.cause.is_some() {
                    return Err(Error::Unsupported("raise from"));
                }
                let values = if let Some(exception) = &stmt.exc {
                    let (next, value) = self.expression(exception, block)?;
                    block = next;
                    vec![value]
                } else {
                    vec![]
                };
                self.runtime(block, CoreOp::Raise, values);
                let value = self.none(block);
                self.function.terminate(block, Terminator::Return(value));
                return Ok(None);
            }
            Stmt::Assert(stmt) => {
                let (next, value) = self.expression(&stmt.test, block)?;
                block = next;
                self.runtime(block, CoreOp::Assert, vec![value]);
            }
            _ => return Err(Error::Unsupported("statement outside core subset")),
        }
        Ok(Some(block))
    }
    fn conditional(
        &mut self,
        test: &Expr,
        body: &[Stmt],
        rest: &[ast::ElifElseClause],
        block: BlockId,
    ) -> Result<Option<BlockId>> {
        // All arms of one if/elif chain share a continuation, rather than a ladder of joins.
        let arms = std::iter::once((Some(test), body)).chain(
            rest.iter()
                .map(|clause| (clause.test.as_ref(), clause.body.as_slice())),
        );
        let mut pending = Some(block);
        let mut exits = Vec::new();
        for (test, body) in arms {
            let Some(current) = pending.take() else { break };
            let arm = if let Some(test) = test {
                let yes = self.function.block(0);
                let no = self.function.block(0);
                self.condition(test, current, yes, no)?;
                pending = Some(no);
                yes
            } else {
                current
            };
            if let Some(end) = self.statements(body, Some(arm))? {
                exits.push(end);
            }
        }
        if let Some(no_else) = pending {
            exits.push(no_else);
        }
        match exits.as_slice() {
            [] => Ok(None),
            [only] => Ok(Some(*only)),
            _ => {
                let join = self.function.block(0);
                for end in exits {
                    self.jump(end, join);
                }
                Ok(Some(join))
            }
        }
    }

    fn condition(
        &mut self,
        expr: &Expr,
        mut block: BlockId,
        yes: BlockId,
        no: BlockId,
    ) -> Result<()> {
        // Conditions need control flow, not the operand-valued merge required by an and/or expression.
        match expr {
            Expr::BoolOp(expr) => {
                for (index, operand) in expr.values.iter().enumerate() {
                    if index + 1 == expr.values.len() {
                        self.condition(operand, block, yes, no)?;
                    } else {
                        let next = self.function.block(0);
                        if expr.op == ast::BoolOp::And {
                            self.condition(operand, block, next, no)?;
                        } else {
                            self.condition(operand, block, yes, next)?;
                        }
                        block = next;
                    }
                }
            }
            Expr::UnaryOp(expr) if expr.op == ast::UnaryOp::Not => {
                self.condition(&expr.operand, block, no, yes)?;
            }
            _ => {
                let (block, value) = self.expression(expr, block)?;
                self.branch(block, value, yes, no);
            }
        }
        Ok(())
    }

    fn expression(&mut self, expr: &Expr, mut block: BlockId) -> Result<(BlockId, ValueId)> {
        let value = match expr {
            Expr::Name(expr) => self.runtime(block, CoreOp::LoadName(expr.id.to_string()), vec![]),
            Expr::NoneLiteral(_) => self.none(block),
            Expr::BooleanLiteral(expr) => self.constant(block, Constant::Bool(expr.value)),
            Expr::NumberLiteral(expr) => self.constant(
                block,
                match &expr.value {
                    ast::Number::Int(n) => Constant::Int(n.as_i64().ok_or(Error::IntegerOverflow)?),
                    ast::Number::Float(n) => Constant::Float(*n),
                    _ => return Err(Error::Unsupported("complex numbers")),
                },
            ),
            Expr::StringLiteral(expr) => {
                self.constant(block, Constant::String(expr.value.to_string()))
            }
            Expr::BinOp(expr) => {
                let (next, left) = self.expression(&expr.left, block)?;
                let (next, right) = self.expression(&expr.right, next)?;
                block = next;
                self.binary(expr.op, left, right, block)?
            }
            Expr::UnaryOp(expr) => {
                // The positive magnitude of i64::MIN cannot be represented as an i64 constant.
                if expr.op == ast::UnaryOp::USub
                    && let Expr::NumberLiteral(literal) = &*expr.operand
                    && let ast::Number::Int(integer) = &literal.value
                    && integer.to_string() == "9223372036854775808"
                {
                    return Ok((block, self.constant(block, Constant::Int(i64::MIN))));
                }
                let (next, value) = self.expression(&expr.operand, block)?;
                block = next;
                self.runtime(
                    block,
                    CoreOp::Unary(
                        match expr.op {
                            ast::UnaryOp::Not => "not",
                            ast::UnaryOp::USub => "-",
                            ast::UnaryOp::UAdd => "+",
                            ast::UnaryOp::Invert => "~",
                        }
                        .into(),
                    ),
                    vec![value],
                )
            }
            Expr::BoolOp(expr) => {
                let join = self.function.block(1);
                let result = self.function.parameters(join)[0];
                for (index, operand) in expr.values.iter().enumerate() {
                    let (next, value) = self.expression(operand, block)?;
                    block = next;
                    if index + 1 == expr.values.len() {
                        self.function
                            .terminate(block, Terminator::Jump(Edge::new(join, vec![value])));
                    } else {
                        let next = self.function.block(0);
                        let edge = Edge::new(join, vec![value]);
                        let advance = Edge::new(next, vec![]);
                        let (yes, no) = if expr.op == ast::BoolOp::And {
                            (advance, edge)
                        } else {
                            (edge, advance)
                        };
                        self.function.terminate(
                            block,
                            Terminator::Branch {
                                condition: value,
                                yes,
                                no,
                            },
                        );
                        block = next;
                    }
                }
                block = join;
                result
            }
            Expr::Compare(expr) => {
                let (next, mut left) = self.expression(&expr.operands[0], block)?;
                block = next;
                if expr.ops.len() == 1 {
                    // A single comparison has no short-circuit join or value merge.
                    let (next, right) = self.expression(&expr.operands[1], block)?;
                    block = next;
                    self.runtime(
                        block,
                        CoreOp::Compare(compare_name(expr.ops[0]).into()),
                        vec![left, right],
                    )
                } else {
                    let join = self.function.block(1);
                    let result = self.function.parameters(join)[0];
                    for (index, op) in expr.ops.iter().enumerate() {
                        let (next, right) = self.expression(&expr.operands[index + 1], block)?;
                        block = next;
                        let condition = self.runtime(
                            block,
                            CoreOp::Compare(compare_name(*op).into()),
                            vec![left, right],
                        );
                        left = right;
                        if index + 1 == expr.ops.len() {
                            self.function.terminate(
                                block,
                                Terminator::Jump(Edge::new(join, vec![condition])),
                            );
                        } else {
                            let next = self.function.block(0);
                            self.function.terminate(
                                block,
                                Terminator::Branch {
                                    condition,
                                    yes: Edge::new(next, vec![]),
                                    no: Edge::new(join, vec![condition]),
                                },
                            );
                            block = next;
                        }
                    }
                    block = join;
                    result
                }
            }
            Expr::If(expr) => {
                let (next, condition) = self.expression(&expr.test, block)?;
                let yes = self.function.block(0);
                let no = self.function.block(0);
                let join = self.function.block(1);
                self.branch(next, condition, yes, no);
                for (start, expr) in [(yes, &expr.body), (no, &expr.orelse)] {
                    let (end, value) = self.expression(expr, start)?;
                    self.function
                        .terminate(end, Terminator::Jump(Edge::new(join, vec![value])));
                }
                block = join;
                self.function.parameters(join)[0]
            }
            Expr::List(expr) => {
                let (next, values) = self.expressions(&expr.elts, block)?;
                block = next;
                self.function.append(block, Op::List(values))
            }
            Expr::Tuple(expr) => {
                let (next, values) = self.expressions(&expr.elts, block)?;
                block = next;
                self.function.append(block, Op::Tuple(values))
            }
            Expr::Set(expr) => {
                let (next, values) = self.expressions(&expr.elts, block)?;
                block = next;
                self.runtime(block, CoreOp::Set, values)
            }
            Expr::Dict(expr) => {
                // Insert each pair after evaluating it, preserving side effects and duplicate-key order.
                let dict = self.function.append(block, Op::Dict(vec![]));
                for item in &expr.items {
                    if let Some(key) = &item.key {
                        let (next, key) = self.expression(key, block)?;
                        let (next, value) = self.expression(&item.value, next)?;
                        block = next;
                        self.function.append(
                            block,
                            Op::SetItem {
                                object: dict,
                                key,
                                value,
                            },
                        );
                    } else {
                        let (next, source) = self.expression(&item.value, block)?;
                        block = next;
                        let update =
                            self.runtime(block, CoreOp::GetAttr("update".into()), vec![dict]);
                        self.runtime(block, CoreOp::Call(vec![None]), vec![update, source]);
                    }
                }
                dict
            }
            Expr::Subscript(expr) => {
                let (next, object) = self.expression(&expr.value, block)?;
                let (next, key) = self.expression(&expr.slice, next)?;
                block = next;
                self.function.append(block, Op::GetItem(object, key))
            }
            Expr::Slice(expr) => {
                let mut values = vec![];
                for part in [&expr.lower, &expr.upper, &expr.step] {
                    let value = if let Some(expr) = part {
                        let (next, value) = self.expression(expr, block)?;
                        block = next;
                        value
                    } else {
                        self.none(block)
                    };
                    values.push(value);
                }
                self.runtime(block, CoreOp::Slice, values)
            }
            Expr::Attribute(expr) => {
                let (next, object) = self.expression(&expr.value, block)?;
                block = next;
                self.runtime(block, CoreOp::GetAttr(expr.attr.to_string()), vec![object])
            }
            Expr::Call(expr) => {
                let (next, callable) = self.expression(&expr.func, block)?;
                block = next;
                let mut values = vec![callable];
                let mut names = vec![];
                for expr in &expr.arguments.args {
                    let (expr, name) = if let Expr::Starred(expr) = expr {
                        (&*expr.value, Some("*".into()))
                    } else {
                        (expr, None)
                    };
                    let (next, value) = self.expression(expr, block)?;
                    block = next;
                    values.push(value);
                    names.push(name);
                }
                for keyword in &expr.arguments.keywords {
                    let (next, value) = self.expression(&keyword.value, block)?;
                    block = next;
                    values.push(value);
                    names.push(Some(
                        keyword
                            .arg
                            .as_ref()
                            .map(|s| s.to_string())
                            .unwrap_or("**".into()),
                    ));
                }
                self.runtime(block, CoreOp::Call(names), values)
            }
            Expr::Lambda(expr) => {
                let mut body = Self::new(&[], false)?;
                if let Some(parameters) = &expr.parameters {
                    body.parameters(parameters)?;
                }
                let (end, value) = body.expression(&expr.body, body.function.entry())?;
                body.function.terminate(end, Terminator::Return(value));
                let (next, defaults) = if let Some(parameters) = &expr.parameters {
                    self.defaults(parameters, block)?
                } else {
                    (block, vec![])
                };
                block = next;
                self.runtime(block, CoreOp::Function(Box::new(body.function)), defaults)
            }
            Expr::Generator(expr) => return self.generator(&expr.generators, &expr.elt, block),
            Expr::ListComp(expr) => {
                let (next, value) =
                    self.comprehension(&expr.generators, &expr.elt, None, "list", block)?;
                block = next;
                value
            }
            Expr::SetComp(expr) => {
                let (next, value) =
                    self.comprehension(&expr.generators, &expr.elt, None, "set", block)?;
                block = next;
                value
            }
            Expr::DictComp(expr) => {
                let (next, value) = self.comprehension(
                    &expr.generators,
                    expr.key
                        .as_deref()
                        .ok_or(Error::Unsupported("dictionary unpack comprehension"))?,
                    Some(&expr.value),
                    "dict",
                    block,
                )?;
                block = next;
                value
            }
            Expr::FString(expr) => {
                let mut values = vec![];
                for part in expr.value.iter() {
                    match part {
                        ast::FStringPartRef::Literal(literal) => values.push(
                            self.constant(block, Constant::String(literal.value.to_string())),
                        ),
                        ast::FStringPartRef::FString(fstring) => {
                            for element in &fstring.elements {
                                match element {
                                    ast::InterpolatedStringElement::Literal(literal) => values
                                        .push(self.constant(
                                            block,
                                            Constant::String(literal.value.to_string()),
                                        )),
                                    ast::InterpolatedStringElement::Interpolation(
                                        interpolation,
                                    ) => {
                                        if interpolation.debug_text.is_some() {
                                            return Err(Error::Unsupported("debug f-string"));
                                        }
                                        let (next, value) =
                                            self.expression(&interpolation.expression, block)?;
                                        block = next;
                                        let mut spec = String::new();
                                        if let Some(format) = &interpolation.format_spec {
                                            for element in &format.elements {
                                                if let ast::InterpolatedStringElement::Literal(
                                                    literal,
                                                ) = element
                                                {
                                                    spec.push_str(&literal.value);
                                                } else {
                                                    return Err(Error::Unsupported(
                                                        "dynamic format spec",
                                                    ));
                                                }
                                            }
                                        }
                                        let repr = matches!(
                                            interpolation.conversion,
                                            ast::ConversionFlag::Repr | ast::ConversionFlag::Ascii
                                        );
                                        values.push(self.runtime(
                                            block,
                                            CoreOp::Format(spec, repr),
                                            vec![value],
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
                self.runtime(block, CoreOp::Join, values)
            }
            _ => return Err(Error::Unsupported("expression outside core subset")),
        };
        Ok((block, value))
    }
    fn expressions(
        &mut self,
        exprs: &[Expr],
        mut block: BlockId,
    ) -> Result<(BlockId, Vec<ValueId>)> {
        let mut values = vec![];
        for expr in exprs {
            let (next, value) = self.expression(expr, block)?;
            block = next;
            values.push(value);
        }
        Ok((block, values))
    }
    fn binary(
        &mut self,
        op: ast::Operator,
        left: ValueId,
        right: ValueId,
        block: BlockId,
    ) -> Result<ValueId> {
        Ok(match op {
            ast::Operator::Add => self.function.append(block, Op::Add(left, right)),
            ast::Operator::Sub => self.function.append(block, Op::Subtract(left, right)),
            op => self.runtime(
                block,
                CoreOp::Binary(
                    match op {
                        ast::Operator::Mult => "*",
                        ast::Operator::Div => "/",
                        ast::Operator::FloorDiv => "//",
                        ast::Operator::Mod => "%",
                        ast::Operator::Pow => "**",
                        ast::Operator::BitAnd => "&",
                        ast::Operator::BitOr => "|",
                        ast::Operator::BitXor => "^",
                        _ => return Err(Error::Unsupported("binary operator")),
                    }
                    .into(),
                ),
                vec![left, right],
            ),
        })
    }

    fn assign(&mut self, target: &Expr, value: ValueId, mut block: BlockId) -> Result<BlockId> {
        match target {
            Expr::Name(name) => {
                self.runtime(block, CoreOp::StoreName(name.id.to_string()), vec![value]);
            }
            Expr::Tuple(expr) => {
                block = self.unpack(&expr.elts, value, block)?;
            }
            Expr::List(expr) => {
                block = self.unpack(&expr.elts, value, block)?;
            }
            Expr::Subscript(expr) => {
                let (next, object) = self.expression(&expr.value, block)?;
                let (next, key) = self.expression(&expr.slice, next)?;
                block = next;
                self.function
                    .append(block, Op::SetItem { object, key, value });
            }
            Expr::Attribute(expr) => {
                let (next, object) = self.expression(&expr.value, block)?;
                block = next;
                self.runtime(
                    block,
                    CoreOp::SetAttr(expr.attr.to_string()),
                    vec![object, value],
                );
            }
            _ => return Err(Error::Unsupported("assignment target")),
        }
        Ok(block)
    }
    fn unpack(&mut self, targets: &[Expr], value: ValueId, mut block: BlockId) -> Result<BlockId> {
        let star = targets
            .iter()
            .position(|target| matches!(target, Expr::Starred(_)));
        if targets
            .iter()
            .filter(|target| matches!(target, Expr::Starred(_)))
            .count()
            > 1
        {
            return Err(Error::Syntax("multiple starred targets".into()));
        }
        let unpack = self.runtime(
            block,
            if let Some(index) = star {
                CoreOp::UnpackStar(index, targets.len() - index - 1)
            } else {
                CoreOp::Unpack(targets.len())
            },
            vec![value],
        );
        // Snapshot all outer elements before targets can mutate an aliased source.
        let elements = (0..targets.len())
            .map(|index| self.runtime(block, CoreOp::UnpackItem(index), vec![unpack]))
            .collect::<Vec<_>>();
        for (target, value) in targets.iter().zip(elements) {
            let target = if let Expr::Starred(expr) = target {
                &expr.value
            } else {
                target
            };
            block = self.assign(target, value, block)?;
        }
        Ok(block)
    }
    fn target(&mut self, target: &Expr, block: BlockId) -> Result<(BlockId, ValueId, Target)> {
        match target {
            Expr::Name(name) => {
                let value = self.runtime(block, CoreOp::LoadName(name.id.to_string()), vec![]);
                Ok((block, value, Target::Name(name.id.to_string())))
            }
            Expr::Attribute(expr) => {
                let (block, object) = self.expression(&expr.value, block)?;
                let value =
                    self.runtime(block, CoreOp::GetAttr(expr.attr.to_string()), vec![object]);
                Ok((block, value, Target::Attr(object, expr.attr.to_string())))
            }
            Expr::Subscript(expr) => {
                let (block, object) = self.expression(&expr.value, block)?;
                let (block, key) = self.expression(&expr.slice, block)?;
                let value = self.function.append(block, Op::GetItem(object, key));
                Ok((block, value, Target::Item(object, key)))
            }
            _ => Err(Error::Unsupported("augmented assignment target")),
        }
    }
    fn store_target(&mut self, target: Target, value: ValueId, block: BlockId) -> Result<BlockId> {
        match target {
            Target::Name(name) => {
                self.runtime(block, CoreOp::StoreName(name), vec![value]);
            }
            Target::Attr(object, name) => {
                self.runtime(block, CoreOp::SetAttr(name), vec![object, value]);
            }
            Target::Item(object, key) => {
                self.function
                    .append(block, Op::SetItem { object, key, value });
            }
        }
        Ok(block)
    }

    fn close_except(&mut self, block: BlockId, depth: usize) {
        for _ in depth..self.except_depth {
            self.runtime(block, CoreOp::ExitExcept, vec![]);
        }
    }

    fn try_statement(&mut self, stmt: &ast::StmtTry, block: BlockId) -> Result<Option<BlockId>> {
        if stmt.is_star || !stmt.finalbody.is_empty() || stmt.handlers.is_empty() {
            return Err(Error::Unsupported("try finally or except star"));
        }
        for handler in &stmt.handlers {
            let ast::ExceptHandler::ExceptHandler(handler) = handler;
            if handler.name.is_some() {
                return Err(Error::Unsupported("except as binding"));
            }
        }
        let [statement] = stmt.body.as_slice() else {
            return Err(Error::Unsupported("multi-statement try body"));
        };
        let expression = match statement {
            Stmt::Assign(assign)
                if assign
                    .targets
                    .iter()
                    .all(|target| matches!(target, Expr::Name(_))) =>
            {
                &assign.value
            }
            Stmt::Expr(expr) => &expr.value,
            _ => {
                return Err(Error::Unsupported(
                    "try body requires expression or name assignment",
                ));
            }
        };
        let mut callback = Self::new(&[], false)?;
        let entry = callback.function.entry();
        let (end, value) = callback.expression(expression, entry)?;
        callback.function.terminate(end, Terminator::Return(value));
        let outcome = self.runtime(block, CoreOp::TryCall(Box::new(callback.function)), vec![]);
        let zero = self.constant(block, Constant::Int(0));
        let success = self.function.append(block, Op::GetItem(outcome, zero));
        let one = self.constant(block, Constant::Int(1));
        let value = self.function.append(block, Op::GetItem(outcome, one));
        let ok = self.function.block(0);
        let failed = self.function.block(0);
        let join = self.function.block(0);
        self.branch(block, success, ok, failed);
        let mut ok = ok;
        if let Stmt::Assign(assign) = statement {
            for target in &assign.targets {
                ok = self.assign(target, value, ok)?;
            }
        }
        let mut continues = false;
        if let Some(end) = self.statements(&stmt.orelse, Some(ok))? {
            self.jump(end, join);
            continues = true;
        }
        self.runtime(failed, CoreOp::EnterExcept, vec![value]);
        self.except_depth += 1;
        let mut next = Some(failed);
        for handler in &stmt.handlers {
            let Some(block) = next else {
                return Err(Error::Syntax("bare except must be last".into()));
            };
            let ast::ExceptHandler::ExceptHandler(handler) = handler;
            let body = if let Some(class) = &handler.type_ {
                let (block, class) = self.expression(class, block)?;
                let matches = self.runtime(block, CoreOp::ExceptionMatch, vec![value, class]);
                let body = self.function.block(0);
                let no = self.function.block(0);
                self.branch(block, matches, body, no);
                next = Some(no);
                body
            } else {
                next = None;
                block
            };
            if let Some(end) = self.statements(&handler.body, Some(body))? {
                self.runtime(end, CoreOp::ExitExcept, vec![]);
                self.jump(end, join);
                continues = true;
            }
        }
        self.except_depth -= 1;
        if let Some(block) = next {
            self.runtime(block, CoreOp::ExitExcept, vec![]);
            self.runtime(block, CoreOp::Raise, vec![value]);
            let none = self.none(block);
            self.function.terminate(block, Terminator::Return(none));
        }
        if continues {
            Ok(Some(join))
        } else {
            let none = self.none(join);
            self.function.terminate(join, Terminator::Return(none));
            Ok(None)
        }
    }

    fn generator(
        &mut self,
        generators: &[ast::Comprehension],
        element: &Expr,
        block: BlockId,
    ) -> Result<(BlockId, ValueId)> {
        let [generator] = generators else {
            return Err(Error::Unsupported("nested generator clauses"));
        };
        if generator.is_async {
            return Err(Error::Unsupported("async generator"));
        }
        let (block, source) = self.expression(&generator.iter, block)?;
        let iterator = self.runtime(block, CoreOp::Iter, vec![source]);
        // A normal factory frame supplies one shared scope, retained by all lazy callbacks.
        let mut factory = Self::new(&[], false)?;
        factory.function.parameter_names = vec![".iter".into()];
        let mut names = BTreeSet::new();
        target_names(&generator.target, &mut names);
        factory.function.local_names = std::iter::once(".iter".into())
            .chain(names.iter().cloned())
            .collect();
        let entry = factory.function.entry();
        let iter = factory.runtime(entry, CoreOp::LoadName(".iter".into()), vec![]);
        let mut assign = Self::new(&[], false)?;
        assign.function.parameter_names = vec![".value".into()];
        assign.function.local_names = vec![".value".into()];
        assign.function.nonlocal_names = names.into_iter().collect();
        let start = assign.function.entry();
        let value = assign.runtime(start, CoreOp::LoadName(".value".into()), vec![]);
        let end = assign.assign(&generator.target, value, start)?;
        let none = assign.none(end);
        assign.function.terminate(end, Terminator::Return(none));
        let assign = factory.runtime(entry, CoreOp::Function(Box::new(assign.function)), vec![]);
        let mut values = vec![iter, assign];
        for expression in std::iter::once(element).chain(generator.ifs.iter()) {
            let mut callback = Self::new(&[], false)?;
            let start = callback.function.entry();
            let (end, value) = callback.expression(expression, start)?;
            callback.function.terminate(end, Terminator::Return(value));
            values.push(factory.runtime(
                entry,
                CoreOp::Function(Box::new(callback.function)),
                vec![],
            ));
        }
        let result = factory.runtime(
            entry,
            CoreOp::Generator(
                generator.ifs.len(),
                match &generator.target {
                    Expr::Name(name) => Some(name.id.to_string()),
                    _ => None,
                },
            ),
            values,
        );
        factory
            .function
            .terminate(entry, Terminator::Return(result));
        let factory = self.runtime(block, CoreOp::Function(Box::new(factory.function)), vec![]);
        let result = self.runtime(block, CoreOp::Call(vec![None]), vec![factory, iterator]);
        Ok((block, result))
    }

    fn comprehension(
        &mut self,
        generators: &[ast::Comprehension],
        element: &Expr,
        value: Option<&Expr>,
        kind: &str,
        block: BlockId,
    ) -> Result<(BlockId, ValueId)> {
        let first = generators
            .first()
            .ok_or(Error::InvalidIr("empty comprehension"))?;
        let (block, source) = self.expression(&first.iter, block)?;
        let source = self.runtime(block, CoreOp::Iter, vec![source]);
        let mut body = Self::new(&[], false)?;
        body.function.parameter_names = vec![".iter".into()];
        body.function.local_names.push(".iter".into());
        let mut locals = BTreeSet::new();
        for generator in generators {
            target_names(&generator.target, &mut locals);
        }
        body.function.local_names.extend(locals);
        let entry = body.function.entry();
        let result = match kind {
            "dict" => body.function.append(entry, Op::Dict(vec![])),
            "set" => body.runtime(entry, CoreOp::Set, vec![]),
            _ => body.function.append(entry, Op::List(vec![])),
        };
        let end = body.comp_loop(generators, 0, element, value, kind, result, entry)?;
        body.function.terminate(end, Terminator::Return(result));
        let function = self.runtime(block, CoreOp::Function(Box::new(body.function)), vec![]);
        let value = self.runtime(block, CoreOp::Call(vec![None]), vec![function, source]);
        Ok((block, value))
    }
    #[allow(clippy::too_many_arguments)]
    fn comp_loop(
        &mut self,
        generators: &[ast::Comprehension],
        index: usize,
        element: &Expr,
        value: Option<&Expr>,
        kind: &str,
        result: ValueId,
        block: BlockId,
    ) -> Result<BlockId> {
        if index == generators.len() {
            let (block, key) = self.expression(element, block)?;
            return if let Some(value) = value {
                let (block, value) = self.expression(value, block)?;
                self.function.append(
                    block,
                    Op::SetItem {
                        object: result,
                        key,
                        value,
                    },
                );
                Ok(block)
            } else {
                let method = self.runtime(
                    block,
                    CoreOp::GetAttr(if kind == "set" { "add" } else { "append" }.into()),
                    vec![result],
                );
                self.runtime(block, CoreOp::Call(vec![None]), vec![method, key]);
                Ok(block)
            };
        }
        let generator = &generators[index];
        if generator.is_async {
            return Err(Error::Unsupported("async comprehension"));
        }
        let (block, iterator) = if index == 0 {
            let iterator = self.runtime(block, CoreOp::LoadName(".iter".into()), vec![]);
            (block, iterator)
        } else {
            let (block, source) = self.expression(&generator.iter, block)?;
            let iterator = self.runtime(block, CoreOp::Iter, vec![source]);
            (block, iterator)
        };
        let head = self.function.block(0);
        let body = self.function.block(0);
        let exit = self.function.block(0);
        self.jump(block, head);
        let next = self.runtime(head, CoreOp::Next, vec![iterator]);
        let done = self.runtime(head, CoreOp::IsDone, vec![next]);
        self.branch(head, done, exit, body);
        let mut body = self.assign(&generator.target, next, body)?;
        for filter in &generator.ifs {
            let (block, condition) = self.expression(filter, body)?;
            body = self.function.block(0);
            self.branch(block, condition, body, head);
        }
        let end = self.comp_loop(generators, index + 1, element, value, kind, result, body)?;
        self.jump(end, head);
        Ok(exit)
    }
}

enum Target {
    Name(String),
    Attr(ValueId, String),
    Item(ValueId, ValueId),
}
fn compare_name(op: ast::CmpOp) -> &'static str {
    match op {
        ast::CmpOp::Eq => "==",
        ast::CmpOp::NotEq => "!=",
        ast::CmpOp::Lt => "<",
        ast::CmpOp::LtE => "<=",
        ast::CmpOp::Gt => ">",
        ast::CmpOp::GtE => ">=",
        ast::CmpOp::Is => "is",
        ast::CmpOp::IsNot => "is not",
        ast::CmpOp::In => "in",
        ast::CmpOp::NotIn => "not in",
    }
}
fn target_names(expr: &Expr, names: &mut BTreeSet<String>) {
    match expr {
        Expr::Name(expr) => {
            names.insert(expr.id.to_string());
        }
        Expr::List(expr) => {
            for expr in &expr.elts {
                target_names(expr, names);
            }
        }
        Expr::Tuple(expr) => {
            for expr in &expr.elts {
                target_names(expr, names);
            }
        }
        Expr::Starred(expr) => target_names(&expr.value, names),
        _ => {}
    }
}
fn bindings(
    body: &[Stmt],
    locals: &mut BTreeSet<String>,
    globals: &mut BTreeSet<String>,
    nonlocals: &mut BTreeSet<String>,
) {
    for statement in body {
        match statement {
            Stmt::Import(stmt) => {
                for alias in &stmt.names {
                    locals.insert(alias.asname.as_ref().unwrap_or(&alias.name).to_string());
                }
            }
            Stmt::ImportFrom(stmt) => {
                for alias in &stmt.names {
                    locals.insert(alias.asname.as_ref().unwrap_or(&alias.name).to_string());
                }
            }
            Stmt::Assign(stmt) => {
                for target in &stmt.targets {
                    target_names(target, locals);
                }
            }
            Stmt::AnnAssign(stmt) => target_names(&stmt.target, locals),
            Stmt::AugAssign(stmt) => target_names(&stmt.target, locals),
            Stmt::FunctionDef(stmt) => {
                locals.insert(stmt.name.to_string());
            }
            Stmt::ClassDef(stmt) => {
                locals.insert(stmt.name.to_string());
            }
            Stmt::For(stmt) => {
                target_names(&stmt.target, locals);
                bindings(&stmt.body, locals, globals, nonlocals);
                bindings(&stmt.orelse, locals, globals, nonlocals);
            }
            Stmt::While(stmt) => {
                bindings(&stmt.body, locals, globals, nonlocals);
                bindings(&stmt.orelse, locals, globals, nonlocals);
            }
            Stmt::If(stmt) => {
                bindings(&stmt.body, locals, globals, nonlocals);
                for clause in &stmt.elif_else_clauses {
                    bindings(&clause.body, locals, globals, nonlocals);
                }
            }
            Stmt::Try(stmt) => {
                bindings(&stmt.body, locals, globals, nonlocals);
                bindings(&stmt.orelse, locals, globals, nonlocals);
                bindings(&stmt.finalbody, locals, globals, nonlocals);
                for handler in &stmt.handlers {
                    let ast::ExceptHandler::ExceptHandler(handler) = handler;
                    bindings(&handler.body, locals, globals, nonlocals);
                }
            }
            Stmt::Global(stmt) => globals.extend(stmt.names.iter().map(ToString::to_string)),
            Stmt::Nonlocal(stmt) => nonlocals.extend(stmt.names.iter().map(ToString::to_string)),
            _ => {}
        }
    }
}

fn operator_name(op: ast::Operator) -> Result<&'static str> {
    Ok(match op {
        ast::Operator::Add => "+",
        ast::Operator::Sub => "-",
        ast::Operator::Mult => "*",
        ast::Operator::Div => "/",
        ast::Operator::FloorDiv => "//",
        ast::Operator::Mod => "%",
        ast::Operator::Pow => "**",
        ast::Operator::BitAnd => "&",
        ast::Operator::BitOr => "|",
        ast::Operator::BitXor => "^",
        _ => return Err(Error::Unsupported("binary operator")),
    })
}

fn validate_scope(
    body: &[Stmt],
    parents: &[BTreeSet<String>],
    in_function: bool,
    in_loop: bool,
) -> Result<()> {
    for statement in body {
        match statement {
            Stmt::FunctionDef(definition) => {
                let mut locals = BTreeSet::new();
                let mut globals = BTreeSet::new();
                let mut nonlocals = BTreeSet::new();
                bindings(&definition.body, &mut locals, &mut globals, &mut nonlocals);
                locals.extend(
                    definition
                        .parameters
                        .posonlyargs
                        .iter()
                        .chain(&definition.parameters.args)
                        .chain(&definition.parameters.kwonlyargs)
                        .map(|p| p.parameter.name.to_string()),
                );
                locals.extend(
                    definition
                        .parameters
                        .vararg
                        .iter()
                        .chain(definition.parameters.kwarg.iter())
                        .map(|p| p.name.to_string()),
                );
                locals.retain(|name| !globals.contains(name) && !nonlocals.contains(name));
                let mut scopes = parents.to_vec();
                scopes.push(locals);
                validate_scope(&definition.body, &scopes, true, false)?;
            }
            Stmt::ClassDef(definition) => validate_scope(&definition.body, parents, false, false)?,
            Stmt::Nonlocal(declaration) => {
                for name in &declaration.names {
                    // The final scope is the current function; a nonlocal needs an enclosing function.
                    let enclosing = if in_function {
                        &parents[..parents.len().saturating_sub(1)]
                    } else {
                        parents
                    };
                    if !enclosing
                        .iter()
                        .rev()
                        .any(|scope| scope.contains(name.as_str()))
                    {
                        return Err(Error::Syntax(format!("no binding for nonlocal {name}")));
                    }
                }
            }
            Stmt::Return(_) if !in_function => {
                return Err(Error::Syntax("return outside function".into()));
            }
            Stmt::Break(_) | Stmt::Continue(_) if !in_loop => {
                return Err(Error::Syntax("loop control outside loop".into()));
            }
            Stmt::For(stmt) => {
                validate_scope(&stmt.body, parents, in_function, true)?;
                validate_scope(&stmt.orelse, parents, in_function, in_loop)?;
            }
            Stmt::While(stmt) => {
                validate_scope(&stmt.body, parents, in_function, true)?;
                validate_scope(&stmt.orelse, parents, in_function, in_loop)?;
            }
            Stmt::Try(stmt) => {
                validate_scope(&stmt.body, parents, in_function, in_loop)?;
                validate_scope(&stmt.orelse, parents, in_function, in_loop)?;
                for handler in &stmt.handlers {
                    let ast::ExceptHandler::ExceptHandler(handler) = handler;
                    validate_scope(&handler.body, parents, in_function, in_loop)?;
                }
            }
            Stmt::If(stmt) => {
                validate_scope(&stmt.body, parents, in_function, in_loop)?;
                for clause in &stmt.elif_else_clauses {
                    validate_scope(&clause.body, parents, in_function, in_loop)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}
