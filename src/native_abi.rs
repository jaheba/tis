//! Fixed C-layout register representation shared by native backends and offline stencils.
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
pub(crate) const ENTER: i32 = 1;
pub(crate) const HELPER: i32 = 2;
pub(crate) const FINISH: i32 = 3;
pub(crate) const SEQUENCE: i32 = 4;
pub(crate) const UNPACK_BATCH: i32 = 5;
pub(crate) const FLOAT_VIEW: i32 = 6;
pub(crate) const REGION_ENTER: i32 = 7;
pub(crate) const NEST_ENTER: i32 = 8;
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
