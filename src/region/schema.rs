//! Shape/type guards for versioning an entire sequence loop before any of its iterations execute.
use super::*;
use std::collections::{BTreeMap, BTreeSet};
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Root {
    Item,
    Import(Register),
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Path {
    pub root: Root,
    pub indices: Vec<i64>,
}
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Constraint {
    Float(Path),
    Int(Path),
    Sequence(Path, crate::sequence::SequenceKind, Option<usize>),
    Floats(Path, Option<usize>),
    Bounds(Path, i64),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schema {
    pub constraints: Vec<Constraint>,
}
#[derive(Clone)]
enum Expr {
    Path(Path),
    Scalar(crate::profile::ValueKind),
}
impl Schema {
    pub(crate) fn build(lir: &lir::Function, header: BlockId, body: BlockId) -> Option<Self> {
        let imports = LoopRegion::imports(lir, header, body);
        let mut values = imports
            .iter()
            .copied()
            .map(|r| {
                (
                    r,
                    Expr::Path(Path {
                        root: Root::Import(r),
                        indices: vec![],
                    }),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let mut views = BTreeMap::new();
        let mut constraints = BTreeSet::new();
        let next = lir.blocks[header.0].instructions.first()?.result?;
        values.insert(
            next,
            Expr::Path(Path {
                root: Root::Item,
                indices: vec![],
            }),
        );
        for i in &lir.blocks[body.0].instructions {
            let op = &i.operation;
            let path = |r: &Register| match values.get(r) {
                Some(Expr::Path(p)) => Some(p.clone()),
                _ => None,
            };
            let expr = match op {
                Operation::Call {
                    helper: lir::Helper::LoadName(_),
                    ..
                } => Some(Expr::Path(Path {
                    root: Root::Import(i.result?),
                    indices: vec![],
                })),
                Operation::Call {
                    helper: lir::Helper::InlineCall { body, .. },
                    arguments,
                    ..
                } if body.loop_safe() => {
                    for argument in &arguments[1..] {
                        match values.get(argument)? {
                            Expr::Path(path) => {
                                constraints.insert(Constraint::Float(path.clone()));
                            }
                            Expr::Scalar(crate::profile::ValueKind::Float) => {}
                            _ => return None,
                        }
                    }
                    Some(Expr::Scalar(crate::profile::ValueKind::Float))
                }
                Operation::Copy(source) => values.get(source).cloned(),
                Operation::SequenceUnpack {
                    value, kind, count, ..
                } => {
                    let p = path(value)?;
                    constraints.insert(Constraint::Sequence(p.clone(), *kind, Some(*count)));
                    Some(Expr::Path(p))
                }
                Operation::SequenceGet {
                    value,
                    kind,
                    index: lir::SequenceIndex::Constant(index),
                    ..
                } => {
                    let mut p = path(value)?;
                    constraints.insert(Constraint::Sequence(p.clone(), *kind, None));
                    constraints.insert(Constraint::Bounds(p.clone(), *index));
                    p.indices.push(*index);
                    Some(Expr::Path(p))
                }
                Operation::FloatListView { value, length, .. } => {
                    let p = path(value)?;
                    constraints.insert(Constraint::Floats(p.clone(), *length));
                    views.insert(i.result?, p);
                    None
                }
                Operation::FloatListUnpack { value, .. } => Some(Expr::Path(path(value)?)),
                Operation::FloatListLoad { view, index, .. } => {
                    constraints.insert(Constraint::Bounds(views.get(view)?.clone(), *index));
                    Some(Expr::Scalar(crate::profile::ValueKind::Float))
                }
                Operation::FloatListStore { view, index, .. } => {
                    constraints.insert(Constraint::Bounds(views.get(view)?.clone(), *index));
                    Some(Expr::Scalar(crate::profile::ValueKind::None))
                }
                Operation::GuardFloat { value, .. } | Operation::GuardInt { value, .. } => {
                    let expected = if matches!(op, Operation::GuardFloat { .. }) {
                        crate::profile::ValueKind::Float
                    } else {
                        crate::profile::ValueKind::Int
                    };
                    match values.get(value)? {
                        Expr::Path(p) => {
                            constraints.insert(if expected == crate::profile::ValueKind::Float {
                                Constraint::Float(p.clone())
                            } else {
                                Constraint::Int(p.clone())
                            });
                        }
                        Expr::Scalar(kind) if *kind == expected => {}
                        _ => return None,
                    }
                    None
                }
                Operation::BoxFloat(_) => Some(Expr::Scalar(crate::profile::ValueKind::Float)),
                Operation::BoxInt(_) => Some(Expr::Scalar(crate::profile::ValueKind::Int)),
                Operation::BoxBool(_) => Some(Expr::Scalar(crate::profile::ValueKind::Bool)),
                // Region scalar signatures currently describe numeric values, not inline strings.
                Operation::Constant(crate::hir::Constant::String(_)) => return None,
                Operation::Constant(c) => Some(Expr::Scalar(match c {
                    crate::hir::Constant::Float(_) => crate::profile::ValueKind::Float,
                    crate::hir::Constant::Int(_) => crate::profile::ValueKind::Int,
                    crate::hir::Constant::Bool(_) => crate::profile::ValueKind::Bool,
                    _ => crate::profile::ValueKind::None,
                })),
                _ => None,
            };
            if let Some(expr) = expr {
                values.insert(i.result?, expr);
            }
        }
        // Changing phis require an inductive scalar proof: entry guard plus a same-typed backedge.
        let Terminator::Jump(edge) = &lir.blocks[body.0].terminator else {
            return None;
        };
        for (parameter, argument) in lir.blocks[header.0].parameters.iter().zip(&edge.arguments) {
            if !imports.contains(parameter) {
                continue;
            }
            let path = Path {
                root: Root::Import(*parameter),
                indices: vec![],
            };
            if let Some(Expr::Path(same)) = values.get(argument)
                && same == &path
            {
                continue;
            }
            let proven = match values.get(argument) {
                Some(Expr::Scalar(crate::profile::ValueKind::Float)) => {
                    constraints.contains(&Constraint::Float(path))
                }
                Some(Expr::Scalar(crate::profile::ValueKind::Int)) => {
                    constraints.contains(&Constraint::Int(path))
                }
                _ => false,
            };
            if !proven {
                return None;
            }
        }
        Some(Self {
            constraints: constraints.into_iter().collect(),
        })
    }
    fn value(
        heap: &Heap,
        path: &Path,
        item: Value,
        imports: &BTreeMap<Register, Value>,
    ) -> Option<Value> {
        let mut value = match path.root {
            Root::Item => item,
            Root::Import(r) => *imports.get(&r)?,
        };
        for &index in &path.indices {
            // Preparation inspects storage only; generic indexing could allocate strings or run protocols.
            value = match heap.get(value).ok()? {
                Object::List(values) => {
                    let i = if index < 0 {
                        values.len() as i128 + i128::from(index)
                    } else {
                        i128::from(index)
                    };
                    values.value(usize::try_from(i).ok()?)?
                }
                Object::Tuple(values) => {
                    let i = if index < 0 {
                        values.len() as i128 + i128::from(index)
                    } else {
                        i128::from(index)
                    };
                    *values.get(usize::try_from(i).ok()?)?
                }
                _ => return None,
            };
        }
        Some(value)
    }
    fn check(&self, heap: &Heap, item: Value, imports: &BTreeMap<Register, Value>) -> bool {
        self.constraints.iter().all(|constraint| {
            let path=match constraint {Constraint::Float(p)|Constraint::Int(p)|Constraint::Sequence(p,..)|Constraint::Floats(p,..)|Constraint::Bounds(p,_)=>p};
            let Some(value)=Self::value(heap,path,item,imports) else {return false};
            match constraint {
                Constraint::Float(_)=>matches!(value,Value::Float(_)),Constraint::Int(_)=>matches!(value,Value::Int(_)),
                Constraint::Sequence(_,kind,len)=>match (kind,heap.get(value)) {
                    (crate::sequence::SequenceKind::List,Ok(Object::List(v)))=>len.is_none_or(|n|n==v.len()),
                    (crate::sequence::SequenceKind::Tuple,Ok(Object::Tuple(v)))=>len.is_none_or(|n|n==v.len()),_=>false,
                },
                Constraint::Floats(_,len)=>matches!(heap.get(value),Ok(Object::List(v)) if v.is_float()&&len.is_none_or(|n|n==v.len())),
                Constraint::Bounds(_,index)=>heap.len(value).is_ok_and(|len| {let index=if *index<0 {len as i128+i128::from(*index)}else{i128::from(*index)};index>=0&&index<len as i128}),
            }
        })
    }
    pub(crate) fn validate(
        &self,
        heap: &Heap,
        iterator: Value,
        imports: &BTreeMap<Register, Value>,
    ) -> Result<bool> {
        let Object::Iterator { source, .. } = heap.get(iterator)? else {
            return Ok(false);
        };
        match heap.get(*source)? {
            Object::Range { start, .. } => {
                Ok(heap.len(*source)? == 0 || self.check(heap, Value::Int(*start), imports))
            }
            Object::List(v) => {
                Ok((0..v.len()).all(|i| self.check(heap, v.value(i).unwrap(), imports)))
            }
            Object::Tuple(v) => Ok(v.iter().all(|v| self.check(heap, *v, imports))),
            _ => Ok(false),
        }
    }
}
