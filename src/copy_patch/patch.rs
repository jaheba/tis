//! Read-only per-stencil operands and continuation relocations in the executable image.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct Patch {
    pub dst: usize,
    pub left: usize,
    pub right: usize,
    pub tag: u64,
    pub a: u64,
    pub b: u64,
    pub c: u64,
    pub next: usize,
    pub next_patch: usize,
    pub alternate: usize,
    pub alternate_patch: usize,
    pub status: u64,
    pub callback: usize,
    // Stencil-specific secondary pointer or Context field offset.
    pub power: usize,
    pub event: i32,
    pub location: i32,
}
