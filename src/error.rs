use std::fmt;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Syntax(String),
    Arguments { expected: usize, actual: usize },
    InvalidIr(&'static str),
    Uninitialized(usize),
    InvalidObject,
    Type(&'static str),
    Index,
    MissingKey,
    IntegerOverflow,
    Unsupported(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Syntax(message) => write!(f, "syntax error: {message}"),
            Self::Arguments { expected, actual } => {
                write!(f, "expected {expected} arguments, got {actual}")
            }
            Self::InvalidIr(reason) => write!(f, "invalid IR: {reason}"),
            Self::Uninitialized(id) => write!(f, "value %{id} is uninitialized"),
            Self::InvalidObject => f.write_str("object does not belong to this heap"),
            Self::Type(reason) => write!(f, "type error: {reason}"),
            Self::Index => f.write_str("index out of range"),
            Self::MissingKey => f.write_str("key not found"),
            Self::IntegerOverflow => f.write_str("integer overflow"),
            Self::Unsupported(feature) => write!(f, "unsupported: {feature}"),
        }
    }
}

impl std::error::Error for Error {}
