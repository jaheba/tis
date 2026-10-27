//! Bounded runtime observations keyed by semantic code identity, never by heap address.

use crate::{
    Heap, Object, Value,
    hir::{BlockId, Function, ValueId},
};
use std::{
    collections::BTreeMap,
    fmt,
    sync::atomic::{AtomicU64, Ordering},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FunctionId(pub u64);
static NEXT_FUNCTION: AtomicU64 = AtomicU64::new(1);
static NEXT_REVISION: AtomicU64 = AtomicU64::new(1);
pub(crate) fn function_id() -> FunctionId {
    FunctionId(NEXT_FUNCTION.fetch_add(1, Ordering::Relaxed))
}
pub(crate) fn revision() -> u64 {
    NEXT_REVISION.fetch_add(1, Ordering::Relaxed)
}

#[repr(usize)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ValueKind {
    None,
    Bool,
    Int,
    Float,
    String,
    List,
    Tuple,
    Dict,
    Set,
    Function,
    Native,
    Method,
    Class,
    Instance,
    Range,
    Iterator,
    Slice,
    Environment,
    Internal,
}
impl ValueKind {
    pub const ALL: [Self; 19] = [
        Self::None,
        Self::Bool,
        Self::Int,
        Self::Float,
        Self::String,
        Self::List,
        Self::Tuple,
        Self::Dict,
        Self::Set,
        Self::Function,
        Self::Native,
        Self::Method,
        Self::Class,
        Self::Instance,
        Self::Range,
        Self::Iterator,
        Self::Slice,
        Self::Environment,
        Self::Internal,
    ];
    pub(crate) fn of(heap: &Heap, value: Value) -> Self {
        match value {
            Value::None => Self::None,
            Value::Bool(_) => Self::Bool,
            Value::Int(_) => Self::Int,
            Value::Float(_) => Self::Float,
            Value::Ref(_) => match heap.get(value) {
                Ok(Object::String(_)) => Self::String,
                Ok(Object::List(_)) => Self::List,
                Ok(Object::Tuple(_)) => Self::Tuple,
                Ok(Object::Dict(_)) => Self::Dict,
                Ok(Object::Set(_)) => Self::Set,
                Ok(Object::Function(_)) => Self::Function,
                Ok(Object::Native(_)) => Self::Native,
                Ok(Object::Bound(..) | Object::Method(..)) => Self::Method,
                Ok(Object::Class { .. }) => Self::Class,
                Ok(Object::Instance { .. }) => Self::Instance,
                Ok(Object::Range { .. }) => Self::Range,
                Ok(
                    Object::Iterator { .. }
                    | Object::Enumerate { .. }
                    | Object::Zip { .. }
                    | Object::View { .. },
                ) => Self::Iterator,
                Ok(Object::Slice { .. }) => Self::Slice,
                Ok(Object::Environment(_)) => Self::Environment,
                _ => Self::Internal,
            },
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct TypeProfile {
    counts: [u64; 19],
}
impl TypeProfile {
    pub fn count(&self, kind: ValueKind) -> u64 {
        self.counts[kind as usize]
    }
    pub fn observations(&self) -> u64 {
        self.counts
            .iter()
            .fold(0_u64, |sum, n| sum.saturating_add(*n))
    }
    pub fn monomorphic(&self) -> Option<ValueKind> {
        let mut kinds = ValueKind::ALL
            .into_iter()
            .filter(|kind| self.count(*kind) > 0);
        let first = kinds.next()?;
        if kinds.next().is_none() {
            Some(first)
        } else {
            None
        }
    }
    pub(crate) fn observe(&mut self, kind: ValueKind) {
        let count = &mut self.counts[kind as usize];
        *count = count.saturating_add(1);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum CallTarget {
    Function(FunctionId),
    Native(String),
    Method { receiver: ValueKind, name: String },
    Class { code: FunctionId, name: String },
    Other,
}
impl CallTarget {
    pub(crate) fn of(heap: &Heap, value: Value) -> Self {
        match heap.get(value) {
            Ok(Object::Function(closure)) => Self::Function(closure.function.borrow().id()),
            Ok(Object::Method(function, _)) => Self::of(heap, *function),
            Ok(Object::Native(name)) => Self::Native(name.clone()),
            Ok(Object::Bound(receiver, name)) => Self::Method {
                receiver: ValueKind::of(heap, *receiver),
                name: name.clone(),
            },
            Ok(Object::Class { code, name, .. }) => Self::Class {
                code: *code,
                name: name.clone(),
            },
            _ => Self::Other,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SiteProfile {
    pub executions: u64,
    pub fast_paths: u64,
    pub guard_failures: u64,
    pub deoptimizations: u64,
    pub operands: Vec<TypeProfile>,
    pub result: TypeProfile,
    pub call_targets: BTreeMap<CallTarget, u64>,
    pub other_targets: u64,
}
impl SiteProfile {
    pub(crate) fn call(&mut self, target: CallTarget) {
        if self.call_targets.contains_key(&target) || self.call_targets.len() < 4 {
            let count = self.call_targets.entry(target).or_default();
            *count = count.saturating_add(1);
        } else {
            self.other_targets = self.other_targets.saturating_add(1);
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct FunctionProfile {
    pub entries: u64,
    pub returns: u64,
    pub errors: u64,
    pub deoptimizations: u64,
    pub arguments: Vec<TypeProfile>,
    pub return_types: TypeProfile,
    pub blocks: Vec<u64>,
    pub edges: BTreeMap<(usize, usize), u64>,
    pub backedges: BTreeMap<(usize, usize), u64>,
    pub sites: Vec<SiteProfile>,
}
impl FunctionProfile {
    pub(crate) fn layout(&mut self, blocks: usize, values: usize) {
        self.blocks.resize(blocks, 0);
        self.sites.resize_with(values, SiteProfile::default);
    }
    pub fn site(&self, value: ValueId) -> Option<&SiteProfile> {
        self.sites.get(value.0)
    }
    pub fn loop_count(&self, header: BlockId) -> u64 {
        self.backedges
            .iter()
            .filter(|((_, to), _)| *to == header.0)
            .fold(0_u64, |sum, (_, n)| sum.saturating_add(*n))
    }
}
impl fmt::Display for FunctionProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "entries={} returns={} errors={}",
            self.entries, self.returns, self.errors
        )?;
        for (block, count) in self.blocks.iter().enumerate() {
            writeln!(f, "b{block}: {count} visits")?;
        }
        for ((from, to), count) in &self.edges {
            writeln!(
                f,
                "b{from} -> b{to}: {count}{}",
                if self.backedges.contains_key(&(*from, *to)) {
                    " backedge"
                } else {
                    ""
                }
            )?;
        }
        for (id, site) in self
            .sites
            .iter()
            .enumerate()
            .filter(|(_, site)| site.executions > 0)
        {
            write!(f, "%{id}: {} executions; result", site.executions)?;
            for kind in ValueKind::ALL {
                if site.result.count(kind) > 0 {
                    write!(f, " {kind:?}={}", site.result.count(kind))?;
                }
            }
            if !site.call_targets.is_empty() {
                write!(f, "; calls {:?}", site.call_targets)?;
            }
            writeln!(f)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
pub struct HotThresholds {
    pub function_entries: u64,
    pub loop_backedges: u64,
}
impl Default for HotThresholds {
    fn default() -> Self {
        Self {
            function_entries: 1000,
            loop_backedges: 1000,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Hotspot {
    Function,
    Loop { header: usize },
}

/// A detached HIR/profile snapshot. Later observations cannot change this compilation input.
#[derive(Debug, Clone)]
pub struct CompilationRequest {
    pub function: FunctionId,
    pub revision: u64,
    pub hotspot: Hotspot,
    pub count: u64,
    pub(crate) hir: Function,
}
impl CompilationRequest {
    pub fn hir(&self) -> &Function {
        &self.hir
    }
    pub fn is_current(&self, hir: &Function) -> bool {
        self.function == hir.id() && self.revision == hir.revision()
    }
    pub fn lower(&self) -> crate::Result<crate::lir::Function> {
        self.hir.lower_lir(crate::lir::LoweringMode::Profiled)
    }
}
