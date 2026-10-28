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
pub(crate) const ENTER: i32 = 1;
pub(crate) const HELPER: i32 = 2;
pub(crate) const FINISH: i32 = 3;
pub(crate) const SEQUENCE: i32 = 4;
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct Slot {
    pub(crate) tag: u64,
    pub(crate) a: u64,
    pub(crate) b: u64,
    pub(crate) c: u64,
}
impl Default for Slot {
    fn default() -> Self {
        Self {
            tag: EMPTY,
            a: 0,
            b: 0,
            c: 0,
        }
    }
}
