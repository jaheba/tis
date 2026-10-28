//! A Python subset compiled through IR to bytecode, with an independent runtime.

pub mod bytecode;
mod error;
mod execution;
pub mod frontend;
mod heap;
pub mod hir;
pub mod lir;
pub mod profile;

// Compatibility for existing IR-builder users.
pub use hir as ir;
mod native;
mod runtime;
mod tier;
mod value;
mod vm;
pub use tier::{TierStats, Tiering};

pub use error::{Error, Result};
pub use frontend::{Program, compile};
pub use heap::{Heap, Key, Object};
pub use value::{ObjectId, Value};
pub use vm::Vm;
