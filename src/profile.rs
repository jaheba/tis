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
            Value::Unbound => Self::Internal,
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
            Ok(Object::Native(name)) => Self::Native(name.to_string()),
            Ok(Object::Bound(receiver, name)) => Self::Method {
                receiver: ValueKind::of(heap, *receiver),
                name: name.to_string(),
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
    /// Executions sampled during profiling, not the site's lifetime execution count.
    pub executions: u64,
    pub fast_paths: u64,
    pub guard_failures: u64,
    pub deoptimizations: u64,
    pub operands: Vec<TypeProfile>,
    pub result: TypeProfile,
    pub call_targets: BTreeMap<CallTarget, u64>,
    pub other_targets: u64,
    pub exit_reasons: BTreeMap<crate::lir::ExitReason, u64>,
    window: u32,
    window_samples: u32,
}
impl SiteProfile {
    pub(crate) fn sample(&mut self, window: u32, config: ProfileConfig) -> bool {
        if config.mode == ProfileMode::Eager {
            self.executions = self.executions.saturating_add(1);
            return true;
        }
        if self.window != window {
            self.window = window;
            self.window_samples = 0;
        }
        if self.window_samples >= config.site_samples.max(1) {
            return false;
        }
        self.window_samples += 1;
        self.executions = self.executions.saturating_add(1);
        true
    }
    pub(crate) fn call(&mut self, target: CallTarget) {
        if self.call_targets.contains_key(&target) || self.call_targets.len() < 4 {
            let count = self.call_targets.entry(target).or_default();
            *count = count.saturating_add(1);
        } else {
            self.other_targets = self.other_targets.saturating_add(1);
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ProfilePhase {
    #[default]
    Cold,
    Profiling,
    Ready,
    Compiled,
    Retired,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ProfileMode {
    #[default]
    Staged,
    /// Collect every observation for diagnostics and explicit lowering experiments.
    Eager,
}

#[derive(Debug, Clone, Copy)]
pub struct ProfileConfig {
    pub mode: ProfileMode,
    /// Completed calls or backedges in each function-wide observation window.
    pub window_ticks: u32,
    pub site_samples: u32,
    /// Includes the first window; repeated guard failures eventually retire automatic compilation.
    pub max_windows: u32,
}
impl Default for ProfileConfig {
    fn default() -> Self {
        Self {
            mode: ProfileMode::Staged,
            window_ticks: 128,
            site_samples: 32,
            max_windows: 3,
        }
    }
}
impl ProfileConfig {
    pub fn eager() -> Self {
        Self {
            mode: ProfileMode::Eager,
            ..Self::default()
        }
    }
    pub(crate) fn collecting(self, phase: ProfilePhase) -> bool {
        self.mode == ProfileMode::Eager || phase == ProfilePhase::Profiling
    }
}

#[derive(Debug, Clone, Default)]
pub struct FunctionProfile {
    pub phase: ProfilePhase,
    pub windows: u32,
    ticks: u32,
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
    pub(crate) fn start_window(&mut self, config: ProfileConfig) -> bool {
        if self.phase == ProfilePhase::Profiling {
            return true;
        }
        if self.windows >= config.max_windows.max(1) {
            self.phase = ProfilePhase::Retired;
            return false;
        }
        self.windows += 1;
        self.ticks = 0;
        self.phase = ProfilePhase::Profiling;
        true
    }

    pub(crate) fn advance(&mut self, config: ProfileConfig) {
        if self.phase == ProfilePhase::Profiling && config.mode == ProfileMode::Staged {
            self.ticks = self.ticks.saturating_add(1);
            if self.ticks >= config.window_ticks.max(1) {
                self.phase = ProfilePhase::Ready;
            }
        }
    }

    pub(crate) fn probe(&mut self, hot: bool, backedge: bool, config: ProfileConfig) -> bool {
        if config.mode == ProfileMode::Eager {
            return hot;
        }
        match self.phase {
            ProfilePhase::Cold if hot => {
                self.start_window(config);
            }
            ProfilePhase::Profiling if backedge => self.advance(config),
            _ => {}
        }
        self.phase == ProfilePhase::Ready
    }
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
            "entries={} returns={} errors={} phase={:?} windows={}",
            self.entries, self.returns, self.errors, self.phase, self.windows
        )?;
        for (block, count) in self.blocks.iter().enumerate() {
            writeln!(f, "b{block}: {count} profiled visits")?;
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
            write!(f, "%{id}: {} sampled executions; result", site.executions)?;
            for kind in ValueKind::ALL {
                if site.result.count(kind) > 0 {
                    write!(f, " {kind:?}={}", site.result.count(kind))?;
                }
            }
            if !site.call_targets.is_empty() {
                write!(f, "; calls {:?}", site.call_targets)?;
            }
            if !site.exit_reasons.is_empty() {
                write!(f, "; exits {:?}", site.exit_reasons)?;
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
