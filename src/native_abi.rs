//! Fixed C-layout representations shared by native backends and offline stencils.
use crate::Value;
use std::cell::Cell;
pub(crate) const NONE: u64 = 0;
pub(crate) const BOOL: u64 = 1;
pub(crate) const INT: u64 = 2;
pub(crate) const FLOAT: u64 = 3;
pub(crate) const REF: u64 = 4;
pub(crate) const RAW_INT: u64 = 5;
pub(crate) const RAW_FLOAT: u64 = 6;
pub(crate) const RAW_BOOL: u64 = 7;
pub(crate) const EMPTY: u64 = 8;
pub(crate) const UNBOUND: u64 = 9;
pub(crate) const FLOAT_VIEW_TAG: u64 = 10;
pub(crate) const INLINE_STRING: u64 = 11;
pub(crate) const ENTER: i32 = 1;
pub(crate) const HELPER: i32 = 2;
pub(crate) const FINISH: i32 = 3;
pub(crate) const SEQUENCE: i32 = 4;
pub(crate) const UNPACK_BATCH: i32 = 5;
pub(crate) const FLOAT_VIEW: i32 = 6;
pub(crate) const REGION_ENTER: i32 = 7;
pub(crate) const NEST_ENTER: i32 = 8;
pub(crate) const SCALAR_CALL: i32 = 9;
pub(crate) const TUPLE_ALLOC: i32 = 10;
pub(crate) const NAME_LOAD: i32 = 11;
pub(crate) const LEAF_ENTER: i32 = 12;
pub(crate) const NAME_STORE: i32 = 13;
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(crate) struct Slot {
    pub(crate) tag: u64,
    pub(crate) a: u64,
}
impl Default for Slot {
    fn default() -> Self {
        Self { tag: EMPTY, a: 0 }
    }
}

// Reference tags carry the 32-bit heap identity in their upper half; the payload
// carries the slot index and generation. Float views pack length/feedback above their kind.
pub(crate) const KIND_MASK: u64 = 0xff;
pub(crate) const VIEW_LENGTH_SHIFT: u32 = 9;
pub(crate) const VIEW_FEEDBACK: u64 = 1 << 8;
const _: () = assert!(std::mem::size_of::<Slot>() == 16);

pub(crate) const NATIVE_LEN: u64 = 1;
pub(crate) const NATIVE_SQRT: u64 = 2;

pub(crate) const STRING_NEXT: i32 = 14;

pub(crate) const RANGE_NEXT: i32 = 15;
pub(crate) const LIST_SET: i32 = 16;

pub(crate) const CLOSED_BUILTIN: i32 = 17;

/// Read-only native view, refreshed after allocation or any mutable object borrow.
/// Values remain Rust-owned; native code may use a pointer only until its next callback.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct NativeFields {
    /// Zero rejects access; 1 = instance, 2 = boxed list, 3 = tuple, 4 = float list, 5 = range, 6 = iterator, 7 = cached range iterator, 8 = byte boolean list, 9 = ASCII string, 10 = audited scalar builtin (class_tag identifies it).
    pub(crate) kind: u64,
    pub(crate) class_tag: u64,
    pub(crate) class_payload: u64,
    pub(crate) generation: u64,
    /// Instance shape ID, iterator cursor pointer, or list snapshot-write permission token.
    pub(crate) shape: u64,
    /// Boxed sequences address Values; kinds 4/8/9 address f64/u8/u8 storage. Kind guards precede decoding.
    pub(crate) values: *const Value,
    /// Sequence writes require an exclusive borrow; iterator rows address their exhausted Cell<bool>.
    pub(crate) writable_values: *mut Value,
    pub(crate) len: usize,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct NativeFieldDirectory {
    pub(crate) entries: *const NativeFields,
    pub(crate) len: usize,
    pub(crate) heap: u64,
}

// Zero is the empty-cache sentinel; real shape identities always start at one.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct FieldSlot {
    pub(crate) shape: u64,
    pub(crate) index: usize,
}

/// First member of the invocation Context. Pointers borrow the live VM; the directory
/// Cell is reloaded after callbacks, and no payload pointer survives a runtime boundary.
#[repr(C)]
pub(crate) struct NativeHeader {
    pub(crate) directory: *const Cell<NativeFieldDirectory>,
    pub(crate) attribute_protocols: *const bool,
    pub(crate) inline_field_hits: u64,
    pub(crate) inline_block_hits: u64,
    pub(crate) quiet_counts: *mut u64,
    pub(crate) previous: usize,
    pub(crate) skip_probe: bool,
    pub(crate) collecting: bool,
    /// Lazily cached exhaustion sentinel, rooted for this native invocation.
    pub(crate) done: Slot,
}

impl Slot {
    // Offline code must contain the conversion; runtime callers retain normal inlining decisions.
    #[cfg_attr(tis_offline_stencils, inline(always))]
    pub(crate) fn boxed(value: Value) -> Self {
        match value {
            Value::Unbound => Self { tag: UNBOUND, a: 0 },
            Value::None => Self { tag: NONE, a: 0 },
            Value::Bool(v) => Self {
                tag: BOOL,
                a: v as u64,
            },
            Value::Int(v) => Self {
                tag: INT,
                a: v as u64,
            },
            Value::Float(v) => Self {
                tag: FLOAT,
                a: v.to_bits(),
            },
            Value::InlineString(text) => {
                let (high, a) = text.words();
                Self {
                    tag: INLINE_STRING | ((high as u64) << 32),
                    a,
                }
            }
            Value::Ref(id) => Self {
                tag: REF | ((id.heap as u64) << 32),
                a: id.index as u64 | ((id.generation as u64) << 32),
            },
        }
    }
}

/// Byte offsets into the verified canonical image; usize::MAX marks an absent output.
#[repr(C)]
#[derive(Debug)]
pub(crate) struct RangeFill {
    pub counter_limit: usize,
    pub counter_add: i64,
    pub counter_step: usize,
    pub counter_update: usize,
    pub inclusive: bool,
    pub descending: bool,
    pub last: usize,
    pub constant: usize,
    pub result: usize,
    pub backedge: usize,
    pub vm_offset: usize,
    pub entries_offset: usize,
    pub iterations_offset: usize,
    pub value: u8,
}

/// At most four guarded reduction outputs; kinds: invariant, index, last-selected, count, sum-index.
#[repr(C)]
#[derive(Debug)]
pub(crate) struct RangeReduce {
    pub outputs: [usize; 4],
    pub kinds: [u8; 4],
    pub len: usize,
    pub backedge: usize,
    pub true_blocks: u64,
    pub false_blocks: u64,
    pub vm_offset: usize,
    pub entries_offset: usize,
    pub iterations_offset: usize,
}

/// Exact arithmetic span; every value including the final counter must stay inline.
/// Empty, non-progressing and overflowing spans retain the original loop.
#[inline(always)]
#[allow(dead_code)] // Also compiled into the separate stencil crate.
pub(crate) fn counter_span(
    first: i64,
    limit: i64,
    step: i64,
    inclusive: bool,
    descending: bool,
) -> Option<(usize, i64, i64)> {
    let (first, limit, step) = (first as i128, limit as i128, step as i128);
    let distance = if descending {
        if step >= 0 {
            return None;
        }
        first - limit
    } else {
        if step <= 0 {
            return None;
        }
        limit - first
    };
    let distance = distance - i128::from(!inclusive);
    if distance < 0 {
        return None;
    }
    let distance = i64::try_from(distance).ok()? as u64;
    let count = distance
        .checked_div((step as i64).unsigned_abs())?
        .checked_add(1)?;
    let count = i64::try_from(count).ok()? as i128;
    let end = first.checked_add(count.checked_mul(step)?)?;
    let last = end - step;
    Some((
        usize::try_from(count).ok()?,
        i64::try_from(last).ok()?,
        i64::try_from(end).ok()?,
    ))
}
