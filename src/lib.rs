//! A Python subset compiled through IR to bytecode, with an independent runtime.

pub mod bytecode;
mod error;
mod execution;
pub mod frontend;
mod heap;
mod list;
pub mod region;
pub use list::List;
pub mod hir;
pub mod lir;
pub mod profile;

// Compatibility for existing IR-builder users.
pub use hir as ir;
mod copy_patch;
mod native;
mod native_abi;
mod native_runtime;
mod register_vm;
mod runtime;
mod scalar;
mod sequence;
mod tier;
mod value;
mod vm;
pub use bytecode::{BytecodeMode, NumericMode};
pub use tier::{TierStats, Tiering};

pub use error::{Error, Result};
pub use frontend::{CompileOptions, Program, compile, compile_with_options};
pub use heap::{Heap, Key, Object};
pub use value::{ObjectId, Value};
pub use vm::Vm;
