/// A non-owning, generation-checked identity. Keep it rooted across explicit collection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ObjectId {
    pub(crate) heap: u32,
    pub(crate) index: u32,
    pub(crate) generation: u32,
}

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value {
    #[doc(hidden)]
    Unbound,
    None,
    Bool(bool),
    Int(i64),
    Float(f64),
    Ref(ObjectId),
}

// The 32-bit discriminant leaves twelve bytes for the reference identity.
const _: () = assert!(std::mem::size_of::<Value>() == 16);
const _: () = assert!(std::mem::size_of::<Option<Value>>() == 16);

/// Immutable source literal with a weak, heap-checked site cache. The VM owns its interned value.
#[derive(Clone)]
pub struct StringLiteral {
    pub(crate) text: String,
    pub(crate) cache: std::cell::Cell<Option<Value>>,
}
impl From<String> for StringLiteral {
    fn from(text: String) -> Self {
        Self {
            text,
            cache: std::cell::Cell::new(None),
        }
    }
}
impl std::fmt::Debug for StringLiteral {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.text.fmt(f)
    }
}
