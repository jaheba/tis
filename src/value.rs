/// A non-owning, generation-checked identity. Keep it rooted across explicit collection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ObjectId {
    pub(crate) heap: usize,
    pub(crate) index: usize,
    pub(crate) generation: usize,
}

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
