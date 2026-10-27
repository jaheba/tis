//! A Python subset compiled through IR to bytecode, with an independent runtime.

pub mod bytecode;
mod error;
mod execution;
pub mod frontend;
mod heap;
pub mod ir;
mod runtime;
mod value;
mod vm;

pub use error::{Error, Result};
pub use frontend::{Program, compile};
pub use heap::{Heap, Key, Object};
pub use value::{ObjectId, Value};
pub use vm::Vm;
