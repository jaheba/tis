//! A Python subset compiled through IR to bytecode, with an independent runtime.

mod bindings;
mod bisect;
mod body_inline;
pub mod inline_frames;
mod leaf_inline;
mod shape;
pub use bindings::Name;
pub use shape::{Attribute, Attributes, Shape};
pub mod bytecode;
mod dict;
mod error;
mod integer;
mod json;
mod string_index;
mod unicode;
pub use dict::Dict;
mod execution;
pub mod frontend;
mod heap;
pub mod library;
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
pub use native::NativeDiagnostics;
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
pub use heap::{ExceptionData, Heap, Key, Object};
pub use value::{InlineString, ObjectId, StringLiteral, Value};
pub use vm::Vm;

pub mod generator;
