//! Ruff AST to value IR. Unsupported Python constructs are rejected explicitly.

use std::collections::BTreeMap;

use ruff_python_ast::{self as ast, Expr, Stmt};

use crate::{
    Error, Result,
    ir::{BlockId, Constant, Edge, Function, Op, Terminator, ValueId},
};

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

/// Parse Python with Ruff, build IR, then lower every function to bytecode.
pub fn compile(source: &str) -> Result<Program> {
    let module = ruff_python_parser::parse_module(source)
        .map_err(|error| Error::Syntax(error.to_string()))?
        .into_syntax();
    let mut functions = BTreeMap::new();
    let mut statements = Vec::new();
    for statement in &module.body {
        if let Stmt::FunctionDef(definition) = statement {
            if definition.is_async
                || !definition.decorator_list.is_empty()
                || definition.type_params.is_some()
            {
                return Err(Error::Unsupported("async, decorated or generic functions"));
            }
            let parameters = &definition.parameters;
            if parameters.vararg.is_some()
                || parameters.kwarg.is_some()
                || !parameters.kwonlyargs.is_empty()
            {
                return Err(Error::Unsupported("variadic or keyword-only parameters"));
            }
            let parameters = parameters
                .posonlyargs
                .iter()
                .chain(&parameters.args)
                .map(|parameter| {
                    if parameter.default.is_some() {
                        return Err(Error::Unsupported("default arguments"));
                    }
                    Ok(parameter.parameter.name.to_string())
                })
                .collect::<Result<Vec<_>>>()?;
            let body = definition.body.iter().collect::<Vec<_>>();
            let function = Builder::compile(&body, &parameters, false)?;
            if functions
                .insert(definition.name.to_string(), function)
                .is_some()
            {
                return Err(Error::Unsupported("redefining a function"));
            }
        } else {
            statements.push(statement);
        }
    }
    functions.insert("<module>".into(), Builder::compile(&statements, &[], true)?);
    Ok(Program { functions })
}

#[derive(Clone)]
struct State {
    block: BlockId,
    locals: BTreeMap<String, ValueId>,
}

struct Builder {
    function: Function,
}

impl Builder {
    fn compile(statements: &[&Stmt], parameters: &[String], module: bool) -> Result<Function> {
        let mut builder = Self {
            function: Function::new(parameters.len()),
        };
        let entry = builder.function.entry();
        let locals = parameters
            .iter()
            .cloned()
            .zip(builder.function.parameters(entry).iter().copied())
            .collect();
        let mut state = Some(State {
            block: entry,
            locals,
        });
        for (index, statement) in statements.iter().enumerate() {
            let Some(current) = state else { break };
            if module
                && index + 1 == statements.len()
                && let Stmt::Expr(expr) = statement
            {
                let value = builder.expression(&expr.value, &current)?;
                builder
                    .function
                    .terminate(current.block, Terminator::Return(value));
                state = None;
            } else {
                state = builder.statement(statement, current)?;
            }
        }
        if let Some(state) = state {
            let none = builder
                .function
                .append(state.block, Op::Constant(Constant::None));
            builder
                .function
                .terminate(state.block, Terminator::Return(none));
        }
        builder.function.bytecode()?;
        Ok(builder.function)
    }

    fn statements(&mut self, statements: &[Stmt], mut state: State) -> Result<Option<State>> {
        for statement in statements {
            match self.statement(statement, state)? {
                Some(next) => state = next,
                None => return Ok(None),
            }
        }
        Ok(Some(state))
    }

    fn statement(&mut self, statement: &Stmt, mut state: State) -> Result<Option<State>> {
        match statement {
            Stmt::Pass(_) => {}
            Stmt::Expr(expr) => {
                self.expression(&expr.value, &state)?;
            }
            Stmt::Return(statement) => {
                let value = match &statement.value {
                    Some(value) => self.expression(value, &state)?,
                    None => self
                        .function
                        .append(state.block, Op::Constant(Constant::None)),
                };
                self.function
                    .terminate(state.block, Terminator::Return(value));
                return Ok(None);
            }
            Stmt::Assign(statement) => {
                let value = self.expression(&statement.value, &state)?;
                for target in &statement.targets {
                    self.assign(target, value, &mut state)?;
                }
            }
            Stmt::AnnAssign(statement) => {
                if let Some(value) = &statement.value {
                    let value = self.expression(value, &state)?;
                    self.assign(&statement.target, value, &mut state)?;
                }
            }
            Stmt::If(statement) => {
                return self.conditional(
                    &statement.test,
                    &statement.body,
                    &statement.elif_else_clauses,
                    state,
                );
            }
            Stmt::While(statement) => {
                let names = state.locals.keys().cloned().collect::<Vec<_>>();
                let head = self.function.block(names.len());
                self.function.terminate(
                    state.block,
                    Terminator::Jump(Edge::new(
                        head,
                        names.iter().map(|name| state.locals[name]).collect(),
                    )),
                );
                let locals = names
                    .iter()
                    .cloned()
                    .zip(self.function.parameters(head).iter().copied())
                    .collect();
                let head_state = State {
                    block: head,
                    locals,
                };
                let condition = self.expression(&statement.test, &head_state)?;
                let body = self.function.block(0);
                let exit = self.function.block(0);
                self.function.terminate(
                    head,
                    Terminator::Branch {
                        condition,
                        yes: Edge::new(body, vec![]),
                        no: Edge::new(exit, vec![]),
                    },
                );
                if let Some(body_state) = self.statements(
                    &statement.body,
                    State {
                        block: body,
                        locals: head_state.locals.clone(),
                    },
                )? {
                    let arguments = names
                        .iter()
                        .map(|name| {
                            body_state.locals.get(name).copied().ok_or_else(|| {
                                Error::Syntax(format!("possibly unbound local {name:?}"))
                            })
                        })
                        .collect::<Result<_>>()?;
                    self.function.terminate(
                        body_state.block,
                        Terminator::Jump(Edge::new(head, arguments)),
                    );
                }
                return self.statements(
                    &statement.orelse,
                    State {
                        block: exit,
                        locals: head_state.locals,
                    },
                );
            }
            _ => return Err(Error::Unsupported("statement outside the tis core subset")),
        }
        Ok(Some(state))
    }

    fn conditional(
        &mut self,
        test: &Expr,
        body: &[Stmt],
        remaining: &[ast::ElifElseClause],
        state: State,
    ) -> Result<Option<State>> {
        let condition = self.expression(test, &state)?;
        let yes = self.function.block(0);
        let no = self.function.block(0);
        self.function.terminate(
            state.block,
            Terminator::Branch {
                condition,
                yes: Edge::new(yes, vec![]),
                no: Edge::new(no, vec![]),
            },
        );
        let yes = self.statements(
            body,
            State {
                block: yes,
                locals: state.locals.clone(),
            },
        )?;
        let no_state = State {
            block: no,
            locals: state.locals,
        };
        let no = if let Some((clause, rest)) = remaining.split_first() {
            if let Some(test) = &clause.test {
                self.conditional(test, &clause.body, rest, no_state)?
            } else {
                self.statements(&clause.body, no_state)?
            }
        } else {
            Some(no_state)
        };
        match (yes, no) {
            (None, other) | (other, None) => Ok(other),
            (Some(yes), Some(no)) => {
                let shared = yes
                    .locals
                    .keys()
                    .filter(|name| no.locals.contains_key(*name))
                    .cloned()
                    .collect::<Vec<_>>();
                let changed = shared
                    .iter()
                    .filter(|name| yes.locals[*name] != no.locals[*name])
                    .cloned()
                    .collect::<Vec<_>>();
                let join = self.function.block(changed.len());
                self.function.terminate(
                    yes.block,
                    Terminator::Jump(Edge::new(
                        join,
                        changed.iter().map(|name| yes.locals[name]).collect(),
                    )),
                );
                self.function.terminate(
                    no.block,
                    Terminator::Jump(Edge::new(
                        join,
                        changed.iter().map(|name| no.locals[name]).collect(),
                    )),
                );
                let mut locals = shared
                    .into_iter()
                    .map(|name| {
                        let value = yes.locals[&name];
                        (name, value)
                    })
                    .collect::<BTreeMap<_, _>>();
                for (name, &value) in changed.into_iter().zip(self.function.parameters(join)) {
                    locals.insert(name, value);
                }
                Ok(Some(State {
                    block: join,
                    locals,
                }))
            }
        }
    }

    fn assign(&mut self, target: &Expr, value: ValueId, state: &mut State) -> Result<()> {
        match target {
            Expr::Name(name) => {
                state.locals.insert(name.id.to_string(), value);
            }
            Expr::Subscript(target) => {
                let object = self.expression(&target.value, state)?;
                let key = self.expression(&target.slice, state)?;
                self.function
                    .append(state.block, Op::SetItem { object, key, value });
            }
            _ => return Err(Error::Unsupported("assignment target")),
        }
        Ok(())
    }

    fn expression(&mut self, expr: &Expr, state: &State) -> Result<ValueId> {
        let op = match expr {
            Expr::Name(name) => {
                return state.locals.get(name.id.as_str()).copied().ok_or_else(|| {
                    Error::Syntax(format!(
                        "undefined or possibly unbound name {:?}",
                        name.id.as_str()
                    ))
                });
            }
            Expr::NoneLiteral(_) => Op::Constant(Constant::None),
            Expr::BooleanLiteral(value) => Op::Constant(Constant::Bool(value.value)),
            Expr::NumberLiteral(value) => Op::Constant(match &value.value {
                ast::Number::Int(value) => {
                    Constant::Int(value.as_i64().ok_or(Error::IntegerOverflow)?)
                }
                ast::Number::Float(value) => Constant::Float(*value),
                _ => return Err(Error::Unsupported("complex literals")),
            }),
            Expr::StringLiteral(value) => Op::Constant(Constant::String(value.value.to_string())),
            Expr::BinOp(value) => {
                let lhs = self.expression(&value.left, state)?;
                let rhs = self.expression(&value.right, state)?;
                match value.op {
                    ast::Operator::Add => Op::Add(lhs, rhs),
                    ast::Operator::Sub => Op::Subtract(lhs, rhs),
                    _ => return Err(Error::Unsupported("binary operator")),
                }
            }
            Expr::UnaryOp(value) => {
                let zero = self
                    .function
                    .append(state.block, Op::Constant(Constant::Int(0)));
                let value_id = self.expression(&value.operand, state)?;
                match value.op {
                    ast::UnaryOp::USub => Op::Subtract(zero, value_id),
                    ast::UnaryOp::UAdd => Op::Add(zero, value_id),
                    _ => return Err(Error::Unsupported("unary operator")),
                }
            }
            Expr::Compare(value) if value.ops.len() == 1 => {
                let lhs = self.expression(&value.operands[0], state)?;
                let rhs = self.expression(&value.operands[1], state)?;
                match value.ops[0] {
                    ast::CmpOp::Lt => Op::Less(lhs, rhs),
                    ast::CmpOp::Gt => Op::Less(rhs, lhs),
                    _ => return Err(Error::Unsupported("comparison operator")),
                }
            }
            Expr::List(value) => Op::List(
                value
                    .elts
                    .iter()
                    .map(|expr| self.expression(expr, state))
                    .collect::<Result<_>>()?,
            ),
            Expr::Tuple(value) => Op::Tuple(
                value
                    .elts
                    .iter()
                    .map(|expr| self.expression(expr, state))
                    .collect::<Result<_>>()?,
            ),
            Expr::Dict(value) => Op::Dict(
                value
                    .items
                    .iter()
                    .map(|item| {
                        Ok((
                            self.expression(
                                item.key
                                    .as_ref()
                                    .ok_or(Error::Unsupported("dictionary unpacking"))?,
                                state,
                            )?,
                            self.expression(&item.value, state)?,
                        ))
                    })
                    .collect::<Result<_>>()?,
            ),
            Expr::Subscript(value) => {
                let object = self.expression(&value.value, state)?;
                let key = self.expression(&value.slice, state)?;
                Op::GetItem(object, key)
            }
            Expr::Call(value)
                if matches!(&*value.func, Expr::Name(name) if name.id.as_str() == "len" && !state.locals.contains_key("len"))
                    && value.arguments.args.len() == 1
                    && value.arguments.keywords.is_empty() =>
            {
                Op::Len(self.expression(&value.arguments.args[0], state)?)
            }
            _ => return Err(Error::Unsupported("expression outside the tis core subset")),
        };
        Ok(self.function.append(state.block, op))
    }
}
