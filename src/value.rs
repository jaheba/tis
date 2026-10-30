/// A non-owning, generation-checked identity. Keep it rooted across explicit collection.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ObjectId {
    pub(crate) heap: u32,
    pub(crate) index: u32,
    pub(crate) generation: u32,
}

// Native field loads use these explicit discriminants and the repr(u32) C-union layout.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value {
    #[doc(hidden)]
    Unbound = 0,
    None = 1,
    Bool(bool) = 2,
    Int(i64) = 3,
    Float(f64) = 4,
    Ref(ObjectId) = 5,
    InlineString(InlineString) = 6,
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

/// UTF-8 stored in the twelve-byte payload available beside Value's tag.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct InlineString {
    bytes: [u8; 11],
    len: u8,
}
impl InlineString {
    pub const CAPACITY: usize = 11;

    pub fn new(text: &str) -> Option<Self> {
        if text.len() > Self::CAPACITY {
            return None;
        }
        let mut bytes = [0; 11];
        bytes[..text.len()].copy_from_slice(text.as_bytes());
        Some(Self {
            bytes,
            len: text.len() as u8,
        })
    }

    pub fn as_str(&self) -> &str {
        // Constructors accept UTF-8 and keep the length within the initialized payload.
        std::str::from_utf8(&self.bytes[..self.len as usize]).expect("inline UTF-8 invariant")
    }

    pub(crate) fn words(self) -> (u32, u64) {
        let mut bytes = [0; 12];
        bytes[..11].copy_from_slice(&self.bytes);
        bytes[11] = self.len;
        (
            u32::from_le_bytes(bytes[..4].try_into().unwrap()),
            u64::from_le_bytes(bytes[4..].try_into().unwrap()),
        )
    }

    pub(crate) fn from_words(high: u32, low: u64) -> Option<Self> {
        let mut bytes = [0; 12];
        bytes[..4].copy_from_slice(&high.to_le_bytes());
        bytes[4..].copy_from_slice(&low.to_le_bytes());
        let len = bytes[11] as usize;
        Self::new(std::str::from_utf8(bytes.get(..len)?).ok()?)
    }
}
impl std::fmt::Debug for InlineString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.as_str().fmt(f)
    }
}
