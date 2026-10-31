//! Compiled by build.rs, not as part of the runtime crate.
//! Slot and patch pointers are valid for a synchronous call. Offsets are bounds checked by the
//! linker; patches remain immutable and continuations stay alive until the last active call returns.
#![allow(dead_code, unused_imports, unsafe_op_in_unsafe_fn, private_interfaces)]
use std::ffi::c_void;
#[path = "../native_abi.rs"]
mod abi;
#[path = "patch.rs"]
mod patch;
#[path = "../scalar.rs"]
mod scalar;
#[path = "../value.rs"]
mod value;
use abi::*;
use patch::Patch;
pub(crate) use value::Value;
type Next = unsafe extern "C" fn(*mut c_void, *mut Slot, *const Patch) -> u64;
#[inline(always)]
unsafe fn read(slots: *mut Slot, offset: usize) -> Slot {
    unsafe { slots.cast::<u8>().add(offset).cast::<Slot>().read() }
}
#[inline(always)]
unsafe fn write(slots: *mut Slot, offset: usize, value: Slot) {
    unsafe {
        slots.cast::<u8>().add(offset).cast::<Slot>().write(value);
    }
}
// Each alternate points at a patch with the same synchronous entry ABI.
#[inline(always)]
unsafe fn fallback(ctx: *mut c_void, slots: *mut Slot, p: &Patch) -> u64 {
    unsafe {
        let f: Next = std::mem::transmute(p.alternate);
        f(ctx, slots, p.alternate_patch as *const Patch)
    }
}
#[inline(always)]
unsafe fn next(ctx: *mut c_void, slots: *mut Slot, p: &Patch) -> u64 {
    unsafe {
        let f: Next = std::mem::transmute(p.next);
        f(ctx, slots, p.next_patch as *const Patch)
    }
}
#[inline(always)]
fn scalar(tag: u64, a: u64) -> Slot {
    Slot { tag, a }
}

/// Fresh-entry imports stay rooted in their original local frame. OSR, observing windows and
/// unbound values use the original helper so errors and feedback occur at the original load.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tis_stencil_local_load(
    ctx: *mut c_void,
    slots: *mut Slot,
    patch: *const Patch,
) -> u64 {
    unsafe {
        let p = &*patch;
        if (*ctx.cast::<NativeHeader>()).collecting
            || read(slots, p.left).tag == 0
            || read(slots, p.dst).tag == UNBOUND
        {
            return fallback(ctx, slots, p);
        }
        next(ctx, slots, p)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tis_stencil_constant(
    ctx: *mut c_void,
    slots: *mut Slot,
    patch: *const Patch,
) -> u64 {
    unsafe {
        let p = &*patch;
        write(slots, p.dst, Slot { tag: p.tag, a: p.a });
        next(ctx, slots, p)
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tis_stencil_copy(
    ctx: *mut c_void,
    slots: *mut Slot,
    patch: *const Patch,
) -> u64 {
    unsafe {
        let p = &*patch;
        write(slots, p.dst, read(slots, p.left));
        next(ctx, slots, p)
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tis_stencil_guard(
    ctx: *mut c_void,
    slots: *mut Slot,
    patch: *const Patch,
) -> u64 {
    unsafe {
        let p = &*patch;
        if read(slots, p.left).tag != p.tag {
            return p.status;
        }
        next(ctx, slots, p)
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tis_stencil_retag(
    ctx: *mut c_void,
    slots: *mut Slot,
    patch: *const Patch,
) -> u64 {
    unsafe {
        let p = &*patch;
        write(slots, p.dst, scalar(p.tag, read(slots, p.left).a));
        next(ctx, slots, p)
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tis_stencil_iadd(
    ctx: *mut c_void,
    slots: *mut Slot,
    patch: *const Patch,
) -> u64 {
    unsafe {
        let p = &*patch;
        let Some(value) = (read(slots, p.left).a as i64).checked_add(read(slots, p.right).a as i64)
        else {
            return p.status;
        };
        write(slots, p.dst, scalar(RAW_INT, value as u64));
        next(ctx, slots, p)
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tis_stencil_iadd_box(
    ctx: *mut c_void,
    slots: *mut Slot,
    patch: *const Patch,
) -> u64 {
    unsafe {
        let p = &*patch;
        let Some(value) = (read(slots, p.left).a as i64).checked_add(read(slots, p.right).a as i64)
        else {
            return p.status;
        };
        write(slots, p.dst, scalar(RAW_INT, value as u64));
        write(slots, p.a as usize, scalar(INT, value as u64));
        next(ctx, slots, p)
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tis_stencil_to_float(
    ctx: *mut c_void,
    slots: *mut Slot,
    patch: *const Patch,
) -> u64 {
    unsafe {
        let p = &*patch;
        let value = read(slots, p.left).a as i64 as f64;
        write(slots, p.dst, scalar(RAW_FLOAT, value.to_bits()));
        next(ctx, slots, p)
    }
}
macro_rules! binary {
    ($name:ident,$op:ident,$boxed:expr) => {
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(
            ctx: *mut c_void,
            slots: *mut Slot,
            patch: *const Patch,
        ) -> u64 {
            unsafe {
                let p = &*patch;
                let left = f64::from_bits(read(slots, p.left).a);
                let right = f64::from_bits(read(slots, p.right).a);
                let Some(value) = scalar::FloatOperator::$op.evaluate(left, right) else {
                    return p.status;
                };
                write(slots, p.dst, scalar(RAW_FLOAT, value.to_bits()));
                if $boxed {
                    write(slots, p.a as usize, scalar(FLOAT, value.to_bits()));
                }
                next(ctx, slots, p)
            }
        }
    };
}
binary!(tis_stencil_fadd, Add, false);
binary!(tis_stencil_fadd_box, Add, true);
binary!(tis_stencil_fsub, Subtract, false);
binary!(tis_stencil_fsub_box, Subtract, true);
binary!(tis_stencil_fmul, Multiply, false);
binary!(tis_stencil_fmul_box, Multiply, true);
binary!(tis_stencil_fdiv, Divide, false);
binary!(tis_stencil_fdiv_box, Divide, true);
// Both operations are total Float add/subtract/multiply. Every original image
// result stays canonical; a dependent second operand can reuse the first FP value.
macro_rules! float_pair {
    ($name:ident, $first:tt, $second:tt) => {
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(ctx: *mut c_void, slots: *mut Slot, patch: *const Patch) -> u64 {
            unsafe {
                let p = &*patch;
                let first = f64::from_bits(read(slots, p.left).a) $first f64::from_bits(read(slots, p.right).a);
                write(slots, p.dst, scalar(RAW_FLOAT, first.to_bits()));
                write(slots, p.a as usize, scalar(FLOAT, first.to_bits()));
                let left = if p.event & 1 != 0 { first } else { f64::from_bits(read(slots, p.tag as usize).a) };
                let right = if p.event & 2 != 0 { first } else { f64::from_bits(read(slots, p.status as usize).a) };
                let second = left $second right;
                write(slots, p.b as usize, scalar(RAW_FLOAT, second.to_bits()));
                if p.c != u64::MAX {
                    write(slots, p.c as usize, scalar(FLOAT, second.to_bits()));
                }
                next(ctx, slots, p)
            }
        }
    };
}
float_pair!(tis_stencil_fpair_aa, +, +);
float_pair!(tis_stencil_fpair_as, +, -);
float_pair!(tis_stencil_fpair_am, +, *);
float_pair!(tis_stencil_fpair_sa, -, +);
float_pair!(tis_stencil_fpair_ss, -, -);
float_pair!(tis_stencil_fpair_sm, -, *);
float_pair!(tis_stencil_fpair_ma, *, +);
float_pair!(tis_stencil_fpair_ms, *, -);
float_pair!(tis_stencil_fpair_mm, *, *);

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tis_stencil_fpow(
    ctx: *mut c_void,
    slots: *mut Slot,
    patch: *const Patch,
) -> u64 {
    unsafe {
        let p = &*patch;
        let left = f64::from_bits(read(slots, p.left).a);
        let right = f64::from_bits(read(slots, p.right).a);
        let power: extern "C" fn(f64, f64) -> f64 = std::mem::transmute(p.power);
        let Some(value) = scalar::power_with(left, right, |a, b| power(a, b)) else {
            return p.status;
        };
        write(slots, p.dst, scalar(RAW_FLOAT, value.to_bits()));
        next(ctx, slots, p)
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tis_stencil_fpow_box(
    ctx: *mut c_void,
    slots: *mut Slot,
    patch: *const Patch,
) -> u64 {
    unsafe {
        let p = &*patch;
        let left = f64::from_bits(read(slots, p.left).a);
        let right = f64::from_bits(read(slots, p.right).a);
        let power: extern "C" fn(f64, f64) -> f64 = std::mem::transmute(p.power);
        let Some(value) = scalar::power_with(left, right, |a, b| power(a, b)) else {
            return p.status;
        };
        write(slots, p.dst, scalar(RAW_FLOAT, value.to_bits()));
        write(slots, p.a as usize, scalar(FLOAT, value.to_bits()));
        next(ctx, slots, p)
    }
}
macro_rules! compare {
    ($name:ident,$op:ident) => {
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(
            ctx: *mut c_void,
            slots: *mut Slot,
            patch: *const Patch,
        ) -> u64 {
            unsafe {
                let p = &*patch;
                let left = f64::from_bits(read(slots, p.left).a);
                let right = f64::from_bits(read(slots, p.right).a);
                let value = scalar::FloatCondition::$op.evaluate(left, right);
                write(slots, p.dst, scalar(RAW_BOOL, value as u64));
                next(ctx, slots, p)
            }
        }
    };
}
compare!(tis_stencil_feq, Eq);
compare!(tis_stencil_fne, Ne);
compare!(tis_stencil_flt, Lt);
compare!(tis_stencil_fle, Le);
compare!(tis_stencil_fgt, Gt);
compare!(tis_stencil_fge, Ge);
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tis_stencil_callback(
    ctx: *mut c_void,
    slots: *mut Slot,
    patch: *const Patch,
) -> u64 {
    unsafe {
        let p = &*patch;
        let callback: extern "C" fn(*mut c_void, *mut Slot, i32, i32) -> i32 =
            std::mem::transmute(p.callback);
        if callback(ctx, slots, p.event, p.location) != 0 {
            return u64::MAX;
        }
        next(ctx, slots, p)
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tis_stencil_guarded_callback(
    ctx: *mut c_void,
    slots: *mut Slot,
    patch: *const Patch,
) -> u64 {
    unsafe {
        let p = &*patch;
        // This stencil's secondary pointer is a closed probe, not floating power.
        // Both functions borrow the invocation's fixed image synchronously; only
        // the full helper may collect or execute Python, and a miss has no effects.
        let probe: extern "C" fn(*mut c_void, *mut Slot, i32, i32) -> i32 =
            std::mem::transmute(p.power);
        let status = probe(ctx, slots, p.event, p.location);
        if status < 0 {
            return u64::MAX;
        }
        if status > 0 {
            let callback: extern "C" fn(*mut c_void, *mut Slot, i32, i32) -> i32 =
                std::mem::transmute(p.callback);
            if callback(ctx, slots, HELPER, p.location) != 0 {
                return u64::MAX;
            }
        }
        next(ctx, slots, p)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tis_stencil_truth(
    ctx: *mut c_void,
    slots: *mut Slot,
    patch: *const Patch,
) -> u64 {
    unsafe {
        let p = &*patch;
        let value = read(slots, p.left);
        let truth = match value.tag {
            NONE => Some(false),
            BOOL | INT => Some(value.a != 0),
            // Clearing the sign bit makes positive and negative zero false; NaNs are true.
            FLOAT => Some(value.a << 1 != 0),
            _ => None,
        };
        if let Some(truth) = truth {
            write(slots, p.dst, scalar(RAW_BOOL, truth as u64));
        } else {
            let callback: extern "C" fn(*mut c_void, *mut Slot, i32, i32) -> i32 =
                std::mem::transmute(p.callback);
            if callback(ctx, slots, p.event, p.location) != 0 {
                return u64::MAX;
            }
        }
        next(ctx, slots, p)
    }
}
#[inline(always)]
unsafe fn float_element(slots: *mut Slot, p: &Patch) -> Option<*mut f64> {
    let view = unsafe { read(slots, p.left) };
    let index = if (p.a as i64) < 0 {
        (view.tag >> VIEW_LENGTH_SHIFT).wrapping_add(p.a)
    } else {
        p.a
    };
    if index >= (view.tag >> VIEW_LENGTH_SHIFT) {
        return None;
    }
    // View creation validates contiguous float storage; verification bounds its lifetime to a call-free block region.
    Some(unsafe { (view.a as *mut f64).add(index as usize) })
}
#[inline(always)]
unsafe fn float_feedback(ctx: *mut c_void, slots: *mut Slot, p: &Patch) -> bool {
    if p.tag != 0 && unsafe { read(slots, p.left) }.tag & VIEW_FEEDBACK != 0 {
        let callback: extern "C" fn(*mut c_void, *mut Slot, i32, i32) -> i32 =
            unsafe { std::mem::transmute(p.callback) };
        return callback(ctx, slots, p.event, p.location) == 0;
    }
    true
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tis_stencil_float_load(
    ctx: *mut c_void,
    slots: *mut Slot,
    patch: *const Patch,
) -> u64 {
    unsafe {
        let p = &*patch;
        let Some(element) = float_element(slots, p) else {
            return p.status;
        };
        write(slots, p.dst, scalar(FLOAT, element.read().to_bits()));
        if !float_feedback(ctx, slots, p) {
            return u64::MAX;
        }
        next(ctx, slots, p)
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tis_stencil_float_store(
    ctx: *mut c_void,
    slots: *mut Slot,
    patch: *const Patch,
) -> u64 {
    unsafe {
        let p = &*patch;
        let Some(element) = float_element(slots, p) else {
            return p.status;
        };
        element.write(f64::from_bits(read(slots, p.right).a));
        write(slots, p.dst, scalar(NONE, 0));
        if !float_feedback(ctx, slots, p) {
            return u64::MAX;
        }
        next(ctx, slots, p)
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tis_stencil_branch(
    ctx: *mut c_void,
    slots: *mut Slot,
    patch: *const Patch,
) -> u64 {
    unsafe {
        let p = &*patch;
        if read(slots, p.left).a != 0 {
            next(ctx, slots, p)
        } else {
            let f: Next = std::mem::transmute(p.alternate);
            f(ctx, slots, p.alternate_patch as *const Patch)
        }
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tis_stencil_jump(
    ctx: *mut c_void,
    slots: *mut Slot,
    patch: *const Patch,
) -> u64 {
    unsafe { next(ctx, slots, &*patch) }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tis_stencil_return(
    _ctx: *mut c_void,
    slots: *mut Slot,
    patch: *const Patch,
) -> u64 {
    unsafe {
        let p = &*patch;
        write(slots, p.dst, read(slots, p.left));
        0
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tis_stencil_float_unpack(
    ctx: *mut c_void,
    slots: *mut Slot,
    patch: *const Patch,
) -> u64 {
    unsafe {
        let p = &*patch;
        write(slots, p.dst, read(slots, p.left));
        if p.tag != 0 && read(slots, p.right).tag & VIEW_FEEDBACK != 0 {
            let callback: extern "C" fn(*mut c_void, *mut Slot, i32, i32) -> i32 =
                std::mem::transmute(p.callback);
            if callback(ctx, slots, p.event, p.location) != 0 {
                return u64::MAX;
            }
        }
        next(ctx, slots, p)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tis_stencil_bool_not(
    ctx: *mut c_void,
    slots: *mut Slot,
    patch: *const Patch,
) -> u64 {
    unsafe {
        let p = &*patch;
        // The LIR verifier requires a raw Bool operand; normalization also keeps the output canonical.
        write(
            slots,
            p.dst,
            scalar(RAW_BOOL, u64::from(read(slots, p.left).a == 0)),
        );
        next(ctx, slots, p)
    }
}

// These offsets are patched from repr(C) Context, which remains exclusively owned
// by this synchronous invocation. No Rust reference to it is live between callbacks.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tis_stencil_quiet_edge(
    ctx: *mut c_void,
    slots: *mut Slot,
    patch: *const Patch,
) -> u64 {
    unsafe {
        let p = &*patch;
        let bytes = ctx.cast::<u8>();
        if !*bytes.add(p.a as usize).cast::<bool>()
            && !*bytes.add(p.c as usize).cast::<bool>()
            && *bytes.add(p.b as usize).cast::<usize>() == p.tag as usize
        {
            let counts = *bytes.add(p.power).cast::<*mut u64>();
            let count = &mut *counts.add(p.status as usize);
            *count = count.saturating_add(1);
        }
        next(ctx, slots, p)
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tis_stencil_quiet_enter(
    ctx: *mut c_void,
    slots: *mut Slot,
    patch: *const Patch,
) -> u64 {
    unsafe {
        let p = &*patch;
        let bytes = ctx.cast::<u8>();
        if *bytes.add(p.a as usize).cast::<bool>() {
            return fallback(ctx, slots, p);
        } else {
            *bytes.add(p.b as usize).cast::<usize>() = p.tag as usize;
            *bytes.add(p.c as usize).cast::<bool>() = false;
            let hits = &mut *bytes.add(p.power).cast::<u64>();
            *hits = hits.wrapping_add(1);
        }
        next(ctx, slots, p)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tis_stencil_field_probe(
    ctx: *mut c_void,
    slots: *mut Slot,
    patch: *const Patch,
) -> u64 {
    unsafe {
        let p = &*patch;
        // Field probes use 0=hit, 1=miss, 2=error; errors must not replay the operation.
        // Both functions borrow the invocation's fixed image synchronously; only
        // the full helper may collect or execute Python, and a miss has no effects.
        let probe: extern "C" fn(*mut c_void, *mut Slot, i32, i32) -> i32 =
            std::mem::transmute(p.power);
        let status = probe(ctx, slots, p.event, p.location);
        if status > 1 {
            return u64::MAX;
        }
        if status == 1 {
            let callback: extern "C" fn(*mut c_void, *mut Slot, i32, i32) -> i32 =
                std::mem::transmute(p.callback);
            if callback(ctx, slots, HELPER, p.location) != 0 {
                return u64::MAX;
            }
        }
        next(ctx, slots, p)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tis_stencil_is_done(
    ctx: *mut c_void,
    slots: *mut Slot,
    patch: *const Patch,
) -> u64 {
    unsafe {
        let p = &*patch;
        if !*ctx.cast::<u8>().add(p.a as usize).cast::<bool>()
            && (read(slots, p.left).tag & KIND_MASK != REF
                || checked_native_row(ctx, read(slots, p.left))
                    .is_some_and(|row| matches!((*row).kind, 1..=10)))
        {
            let dst = slots.cast::<u8>().add(p.dst).cast::<Slot>();
            std::ptr::addr_of_mut!((*dst).tag).write_volatile(BOOL);
            std::ptr::addr_of_mut!((*dst).a).write_volatile(0);
        } else {
            return fallback(ctx, slots, p);
        }
        next(ctx, slots, p)
    }
}

/// All row/cache pointers are consumed before the fallback or tail continuation.
#[inline(always)]
unsafe fn direct_field(ctx: *mut c_void, slots: *mut Slot, p: &Patch) -> bool {
    unsafe {
        let header = &mut *ctx.cast::<NativeHeader>();
        if header.collecting {
            return false;
        }
        let receiver = read(slots, p.left);
        if receiver.tag & KIND_MASK != REF {
            return false;
        }
        let directory = (*header.directory).get();
        if receiver.tag >> 32 != directory.heap {
            return false;
        }
        let index = receiver.a as u32 as usize;
        if index >= directory.len {
            return false;
        }
        let row = &*directory.entries.add(index);
        if row.generation != receiver.a >> 32 || row.kind != 1 || *header.attribute_protocols {
            return false;
        }
        // The cache is owned by the immutable Rc-held LIR, not an instance.
        let cache = (*(p.a as *const std::cell::Cell<FieldSlot>)).get();
        if cache.shape != row.shape || cache.index >= row.len {
            return false;
        }
        let value = Slot::boxed(row.values.add(cache.index).read());
        write(slots, p.dst, value);
        header.inline_field_hits = header.inline_field_hits.wrapping_add(1);
        true
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tis_stencil_direct_field(
    ctx: *mut c_void,
    slots: *mut Slot,
    patch: *const Patch,
) -> u64 {
    unsafe {
        let p = &*patch;
        if !direct_field(ctx, slots, p) {
            return fallback(ctx, slots, p);
        }
        next(ctx, slots, p)
    }
}

/// The directory is reloaded each time; only the synchronous caller owns these rows.
#[inline(always)]
unsafe fn checked_native_row(ctx: *mut c_void, value: Slot) -> Option<*const NativeFields> {
    unsafe {
        if value.tag & KIND_MASK != REF {
            return None;
        }
        let directory = (*(*ctx.cast::<NativeHeader>()).directory).get();
        let index = value.a as u32 as usize;
        if value.tag >> 32 != directory.heap || index >= directory.len {
            return None;
        }
        let row = directory.entries.add(index);
        if (*row).generation != value.a >> 32 {
            return None;
        }
        Some(row)
    }
}
#[inline(always)]
unsafe fn count_inline(ctx: *mut c_void, offset: usize) {
    unsafe {
        let counter = ctx.cast::<u8>().add(offset).cast::<u64>();
        *counter = (*counter).wrapping_add(1);
    }
}
#[inline(always)]
unsafe fn direct_float_view(ctx: *mut c_void, slots: *mut Slot, p: &Patch) -> bool {
    unsafe {
        let Some(row) = checked_native_row(ctx, read(slots, p.left)) else {
            return false;
        };
        let row = &*row;
        if row.kind != 4 || row.writable_values.is_null() || (p.a != 0 && row.len as u64 != p.a - 1)
        {
            return false;
        }
        // A shared generic borrow may have cached a boxed snapshot. Renew permission
        // through the exclusive fallback before any generated write can make it stale.
        if row.shape == 0 || *(row.shape as *const u8) == 0 {
            return false;
        }
        let collecting = (*ctx.cast::<NativeHeader>()).collecting;
        write(
            slots,
            p.dst,
            Slot {
                tag: FLOAT_VIEW_TAG
                    | ((row.len as u64) << VIEW_LENGTH_SHIFT)
                    | if collecting { VIEW_FEEDBACK } else { 0 },
                a: row.writable_values as u64,
            },
        );
        count_inline(ctx, p.power);
        true
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tis_stencil_direct_float_view(
    ctx: *mut c_void,
    slots: *mut Slot,
    patch: *const Patch,
) -> u64 {
    unsafe {
        let p = &*patch;
        if !direct_float_view(ctx, slots, p) {
            return fallback(ctx, slots, p);
        }
        next(ctx, slots, p)
    }
}
#[inline(always)]
unsafe fn direct_unpack(ctx: *mut c_void, slots: *mut Slot, p: &Patch) -> bool {
    unsafe {
        if (*ctx.cast::<NativeHeader>()).collecting {
            return false;
        }
        let source = read(slots, p.left);
        let Some(row) = checked_native_row(ctx, source) else {
            return false;
        };
        let row = &*row;
        let kind_ok = if p.status == 3 {
            row.kind == 3
        } else {
            matches!(row.kind, 2 | 4 | 8)
        };
        if !kind_ok || row.len != p.tag as usize || (row.len != 0 && row.values.is_null()) {
            return false;
        }
        let outputs = [p.a, p.b, p.c];
        for (i, &offset) in outputs.iter().take(row.len).enumerate() {
            let value = match row.kind {
                4 => Slot {
                    tag: FLOAT,
                    a: row.values.cast::<f64>().add(i).read().to_bits(),
                },
                8 => Slot {
                    tag: BOOL,
                    a: (row.values.cast::<u8>().add(i).read() != 0) as u64,
                },
                _ => Slot::boxed(row.values.add(i).read()),
            };
            write(slots, offset as usize, value);
        }
        write(slots, p.dst, source);
        count_inline(ctx, p.power);
        true
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tis_stencil_direct_unpack(
    ctx: *mut c_void,
    slots: *mut Slot,
    patch: *const Patch,
) -> u64 {
    unsafe {
        let p = &*patch;
        if !direct_unpack(ctx, slots, p) {
            return fallback(ctx, slots, p);
        }
        next(ctx, slots, p)
    }
}

#[inline(always)]
fn inline_byte(source: Slot, index: usize) -> u8 {
    // Volatile reads keep LLVM from building a vector-literal packing loop with external relocations.
    // These addresses name initialized local words and never escape this call-free stencil.
    if index < 4 {
        unsafe { std::ptr::addr_of!(source.tag).read_volatile() }
            .wrapping_shr((32 + index * 8) as u32) as u8
    } else {
        unsafe { std::ptr::addr_of!(source.a).read_volatile() }
            .wrapping_shr(((index - 4) * 8) as u32) as u8
    }
}

/// Exact builtins and primitive operands cannot reenter Python or collect. Every miss happens
/// before publishing a result or charging the call; custom protocols retain the original helper.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tis_stencil_direct_scalar(
    ctx: *mut c_void,
    slots: *mut Slot,
    patch: *const Patch,
) -> u64 {
    unsafe {
        let p = &*patch;
        if (*ctx.cast::<NativeHeader>()).collecting {
            return fallback(ctx, slots, p);
        }
        let Some(callable) = checked_native_row(ctx, read(slots, p.right)) else {
            return fallback(ctx, slots, p);
        };
        if (*callable).kind != 10 || (*callable).class_tag != p.tag {
            return fallback(ctx, slots, p);
        }
        let input = read(slots, p.left);
        let output = if p.tag == NATIVE_SQRT {
            let number = match input.tag {
                FLOAT => f64::from_bits(input.a),
                INT => (input.a as i64) as f64,
                BOOL => input.a as f64,
                _ => return fallback(ctx, slots, p),
            };
            if number < 0.0 {
                return fallback(ctx, slots, p);
            }
            scalar(FLOAT, number.sqrt().to_bits())
        } else if p.tag == NATIVE_LEN {
            let length = if input.tag & KIND_MASK == INLINE_STRING {
                let bytes = (input.a >> 56) as usize;
                if bytes > 11 {
                    return fallback(ctx, slots, p);
                }
                let mut count = 0;
                for index in 0..bytes {
                    count += usize::from(inline_byte(input, index) & 0xc0 != 0x80);
                }
                count
            } else {
                let Some(row) = checked_native_row(ctx, input) else {
                    return fallback(ctx, slots, p);
                };
                if !matches!((*row).kind, 2 | 3 | 4 | 8 | 9) {
                    return fallback(ctx, slots, p);
                }
                (*row).len
            };
            if length > i64::MAX as usize {
                return fallback(ctx, slots, p);
            }
            scalar(INT, length as u64)
        } else {
            return fallback(ctx, slots, p);
        };
        write(slots, p.dst, output);
        let vm = ctx.cast::<u8>().add(p.c as usize).cast::<*mut u8>().read();
        for offset in [p.a, p.b] {
            let counter = vm.add(offset as usize).cast::<u64>();
            *counter = (*counter).wrapping_add(1);
        }
        next(ctx, slots, p)
    }
}

/// Only exact ASCII and primitive unit-step bounds can produce an inline result here.
#[inline(always)]
unsafe fn string_slice(ctx: *mut c_void, slots: *mut Slot, p: &Patch) -> bool {
    unsafe {
        if (*ctx.cast::<NativeHeader>()).collecting {
            return false;
        }
        let source = read(slots, p.left);
        let inline = source.tag & KIND_MASK == INLINE_STRING;
        let (length, bytes) = if inline {
            let length = (source.a >> 56) as usize;
            if length > value::InlineString::CAPACITY {
                return false;
            }
            for index in 0..length {
                if inline_byte(source, index) >= 128 {
                    return false;
                }
            }
            (length, std::ptr::null::<u8>())
        } else {
            let Some(row) = checked_native_row(ctx, source) else {
                return false;
            };
            if (*row).kind != 9 || (*row).values.is_null() {
                return false;
            }
            ((*row).len, (*row).values.cast::<u8>())
        };
        let step = read(slots, p.b as usize);
        if step.tag & KIND_MASK != NONE && !(step.tag & KIND_MASK == INT && step.a == 1) {
            return false;
        }
        let bound = |value: Slot, default: usize| match value.tag & KIND_MASK {
            NONE => Some(default),
            INT => {
                let index = value.a as i64 as i128;
                let index = if index < 0 {
                    index + length as i128
                } else {
                    index
                };
                Some(index.max(0).min(length as i128) as usize)
            }
            _ => None,
        };
        let (Some(start), Some(stop)) = (
            bound(read(slots, p.right), 0),
            bound(read(slots, p.a as usize), length),
        ) else {
            return false;
        };
        let count = stop.max(start) - start;
        if count > value::InlineString::CAPACITY {
            return false;
        }
        let mut high = 0u64;
        let mut low = (count as u64) << 56;
        for index in 0..count {
            let byte = if inline {
                inline_byte(source, start + index)
            } else {
                bytes.add(start + index).read_volatile()
            } as u64;
            if index < 4 {
                high |= byte.wrapping_shl((index * 8) as u32);
            } else {
                low |= byte.wrapping_shl(((index - 4) * 8) as u32);
            }
        }
        // ASCII is valid UTF-8; the packed length and zero padding match InlineString::words.
        write(
            slots,
            p.dst,
            Slot {
                tag: INLINE_STRING | (high << 32),
                a: low,
            },
        );
        let vm = ctx.cast::<u8>().add(p.c as usize).cast::<*mut u8>().read();
        let counter = vm.add(p.status as usize).cast::<u64>();
        *counter = (*counter).wrapping_add(1);
        true
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tis_stencil_direct_string_slice(
    ctx: *mut c_void,
    slots: *mut Slot,
    patch: *const Patch,
) -> u64 {
    unsafe {
        let p = &*patch;
        if !string_slice(ctx, slots, p) {
            return fallback(ctx, slots, p);
        }
        next(ctx, slots, p)
    }
}

/// An unsuccessful element probe leaves the cursor and sticky exhaustion unchanged.
#[inline(always)]
unsafe fn direct_next(ctx: *mut c_void, slots: *mut Slot, p: &Patch) -> bool {
    unsafe {
        if (*ctx.cast::<NativeHeader>()).collecting {
            return false;
        }
        let Some(row) = checked_native_row(ctx, read(slots, p.left)) else {
            return false;
        };
        let row = &*row;
        if !matches!(row.kind, 6 | 7)
            || row.shape == 0
            || row.writable_values.is_null()
            || row.writable_values.cast::<bool>().read()
        {
            return false;
        }
        let cursor = row.shape as *mut usize;
        let index = cursor.read();
        let value = if row.kind == 7 {
            if index >= row.len {
                return false;
            }
            let result = (row.class_tag as i64 as i128)
                + (row.class_payload as i64 as i128) * (index as i128);
            let Ok(result) = i64::try_from(result) else {
                return false;
            };
            Slot {
                tag: INT,
                a: result as u64,
            }
        } else {
            let source = Slot {
                tag: row.class_tag,
                a: row.class_payload,
            };
            let Some(source) = checked_native_row(ctx, source) else {
                return false;
            };
            let source = &*source;
            if index >= source.len || source.values.is_null() {
                return false;
            }
            match source.kind {
                2 | 3 => Slot::boxed(source.values.add(index).read()),
                4 => Slot {
                    tag: FLOAT,
                    a: source.values.cast::<f64>().add(index).read().to_bits(),
                },
                8 => Slot {
                    tag: BOOL,
                    a: (source.values.cast::<u8>().add(index).read() != 0) as u64,
                },
                _ => return false,
            }
        };
        let Some(next_index) = index.checked_add(1) else {
            return false;
        };
        cursor.write(next_index);
        write(slots, p.dst, value);
        count_inline(ctx, p.a as usize);
        true
    }
}
/// The sentinel has an invocation-scoped GC root; no heap handle is fabricated here.
#[inline(always)]
unsafe fn direct_done(ctx: *mut c_void, slots: *mut Slot, p: &Patch) -> bool {
    unsafe {
        let header = ctx.cast::<NativeHeader>();
        let done = (*header).done;
        if (*header).collecting || done.tag & KIND_MASK != REF {
            return false;
        }
        let Some(row) = checked_native_row(ctx, read(slots, p.left)) else {
            return false;
        };
        let row = &*row;
        if !matches!(row.kind, 6 | 7) || row.shape == 0 || row.writable_values.is_null() {
            return false;
        }
        // Iterator rows publish Cell storage; only that exact kind permits this interior write.
        let exhausted = row.writable_values.cast::<bool>();
        if !exhausted.read() {
            let index = (row.shape as *const usize).read();
            let len = if row.kind == 7 {
                row.len
            } else {
                let Some(source) = checked_native_row(
                    ctx,
                    Slot {
                        tag: row.class_tag,
                        a: row.class_payload,
                    },
                ) else {
                    return false;
                };
                if !matches!((*source).kind, 2 | 3 | 4 | 8) {
                    return false;
                }
                (*source).len
            };
            if index < len {
                return false;
            }
            exhausted.write(true);
        }
        write(slots, p.dst, done);
        count_inline(ctx, p.a as usize);
        true
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tis_stencil_direct_next(
    ctx: *mut c_void,
    slots: *mut Slot,
    patch: *const Patch,
) -> u64 {
    unsafe {
        let p = &*patch;
        if !direct_next(ctx, slots, p) && !direct_done(ctx, slots, p) {
            return fallback(ctx, slots, p);
        }
        next(ctx, slots, p)
    }
}

/// Primitive index normalization cannot execute Python; custom indices miss before effects.
#[inline(always)]
fn item_index(key: Slot, len: usize) -> Option<usize> {
    let index = match key.tag & KIND_MASK {
        INT => key.a as i64 as i128,
        BOOL => (key.a != 0) as i128,
        _ => return None,
    };
    let index = if index < 0 {
        index + len as i128
    } else {
        index
    };
    (index >= 0 && index < len as i128).then_some(index as usize)
}
#[inline(always)]
unsafe fn direct_item(ctx: *mut c_void, slots: *mut Slot, p: &Patch, store: bool) -> bool {
    unsafe {
        if (*ctx.cast::<NativeHeader>()).collecting {
            return false;
        }
        // Test the primitive key first so mapping/string lookups retain their cheap miss.
        let key = read(slots, p.right);
        if !matches!(key.tag & KIND_MASK, INT | BOOL) {
            return false;
        }
        let Some(row) = checked_native_row(ctx, read(slots, p.left)) else {
            return false;
        };
        let row = &*row;
        if !matches!(row.kind, 2 | 3 | 4 | 8) {
            return false;
        }
        let Some(index) = item_index(key, row.len) else {
            return false;
        };
        if store {
            if row.kind == 3 || row.writable_values.is_null() {
                return false;
            }
            let input = read(slots, p.a as usize);
            if matches!(row.kind, 4 | 8) {
                // Shared generic reads can cache boxed snapshots. Renew exclusive permission
                // through the runtime before writing compact storage after such a read.
                if row.shape == 0 || *(row.shape as *const u8) == 0 {
                    return false;
                }
                if row.kind == 8 {
                    if input.tag & KIND_MASK != BOOL {
                        return false;
                    }
                    row.writable_values
                        .cast::<u8>()
                        .add(index)
                        .write((input.a != 0) as u8);
                } else {
                    if input.tag & KIND_MASK != FLOAT {
                        return false;
                    }
                    row.writable_values
                        .cast::<f64>()
                        .add(index)
                        .write(f64::from_bits(input.a));
                }
            } else {
                let value = match input.tag & KIND_MASK {
                    NONE => Value::None,
                    BOOL => Value::Bool(input.a != 0),
                    INT => Value::Int(input.a as i64),
                    FLOAT => Value::Float(f64::from_bits(input.a)),
                    REF => Value::Ref(value::ObjectId {
                        heap: (input.tag >> 32) as u32,
                        index: input.a as u32,
                        generation: (input.a >> 32) as u32,
                    }),
                    _ => return false,
                };
                // No collection occurs here. The source and owner remain rooted in the
                // verified image until the next runtime safepoint; no refcount barrier exists.
                row.writable_values.add(index).write(value);
            }
            // SetItem has the Python None result even when the caller ignores it.
            if p.tag != 0 {
                let output = slots.cast::<u8>().add(p.dst).cast::<Slot>();
                std::ptr::addr_of_mut!((*output).tag).write_volatile(NONE);
                std::ptr::addr_of_mut!((*output).a).write_volatile(0);
            }
        } else {
            if row.values.is_null() {
                return false;
            }
            let value = match row.kind {
                4 => Slot {
                    tag: FLOAT,
                    a: row.values.cast::<f64>().add(index).read().to_bits(),
                },
                8 => Slot {
                    tag: BOOL,
                    a: (row.values.cast::<u8>().add(index).read() != 0) as u64,
                },
                _ => Slot::boxed(row.values.add(index).read()),
            };
            write(slots, p.dst, value);
        }
        let vm = ctx.cast::<u8>().add(p.c as usize).cast::<*mut u8>().read();
        let count = vm.add(p.b as usize).cast::<u64>();
        *count = (*count).wrapping_add(1);
        true
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tis_stencil_direct_getitem(
    ctx: *mut c_void,
    slots: *mut Slot,
    patch: *const Patch,
) -> u64 {
    unsafe {
        let p = &*patch;
        if !direct_item(ctx, slots, p, false) {
            return fallback(ctx, slots, p);
        }
        next(ctx, slots, p)
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tis_stencil_direct_setitem(
    ctx: *mut c_void,
    slots: *mut Slot,
    patch: *const Patch,
) -> u64 {
    unsafe {
        let p = &*patch;
        if !direct_item(ctx, slots, p, true) {
            return fallback(ctx, slots, p);
        }
        next(ctx, slots, p)
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tis_stencil_guard_unbox(
    ctx: *mut c_void,
    slots: *mut Slot,
    patch: *const Patch,
) -> u64 {
    unsafe {
        let p = &*patch;
        let value = read(slots, p.left);
        if value.tag != p.tag {
            return p.status;
        }
        write(slots, p.dst, scalar(p.b, value.a));
        next(ctx, slots, p)
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tis_stencil_float_load_raw(
    ctx: *mut c_void,
    slots: *mut Slot,
    patch: *const Patch,
) -> u64 {
    unsafe {
        let p = &*patch;
        let Some(element) = float_element(slots, p) else {
            return p.status;
        };
        write(slots, p.dst, scalar(FLOAT, element.read().to_bits()));
        if !float_feedback(ctx, slots, p) {
            return u64::MAX;
        }
        // Preserve the original observation point before publishing the raw scalar.
        write(slots, p.b as usize, scalar(RAW_FLOAT, read(slots, p.dst).a));
        next(ctx, slots, p)
    }
}

/// Slow paths are tail continuations, so successful steps need no C-call stack frame.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tis_stencil_next_test(
    ctx: *mut c_void,
    slots: *mut Slot,
    patch: *const Patch,
) -> u64 {
    unsafe {
        let p = &*patch;
        let finished = if direct_next(ctx, slots, p) {
            false
        } else if direct_done(ctx, slots, p) {
            true
        } else {
            let slow: Next = std::mem::transmute(p.alternate);
            return slow(ctx, slots, p.alternate_patch as *const Patch);
        };
        let value = read(slots, p.dst);
        let ordinary = value.tag & KIND_MASK != REF
            || checked_native_row(ctx, value).is_some_and(|row| matches!((*row).kind, 1..=10));
        if !finished && !ordinary {
            // The cursor already advanced: continue at IsDone, never replay Next.
            let slow: Next = std::mem::transmute(p.callback);
            return slow(ctx, slots, p.power as *const Patch);
        }
        let done = slots.cast::<u8>().add(p.b as usize).cast::<Slot>();
        std::ptr::addr_of_mut!((*done).tag).write_volatile(BOOL);
        std::ptr::addr_of_mut!((*done).a).write_volatile(u64::from(finished));
        let condition = slots.cast::<u8>().add(p.c as usize).cast::<Slot>();
        std::ptr::addr_of_mut!((*condition).tag).write_volatile(RAW_BOOL);
        std::ptr::addr_of_mut!((*condition).a).write_volatile(u64::from(finished));
        next(ctx, slots, p)
    }
}

/// A guarded, call-free loop; misses leave the original iterator/body untouched.
#[inline(always)]
unsafe fn range_fill(ctx: *mut c_void, slots: *mut Slot, p: &Patch) -> bool {
    unsafe {
        let header = ctx.cast::<NativeHeader>();
        if (*header).collecting || (*header).skip_probe {
            return false;
        }
        let info = &*(p.a as *const RangeFill);
        let Some(iterator) = checked_native_row(ctx, read(slots, p.left)) else {
            return false;
        };
        let iterator = &*iterator;
        if iterator.kind != 7 || iterator.shape == 0 {
            return false;
        }
        let cursor = iterator.shape as *mut usize;
        let position = cursor.read();
        if position >= iterator.len {
            return false;
        }
        let Some(list) = checked_native_row(ctx, read(slots, p.right)) else {
            return false;
        };
        let list = &*list;
        if list.kind != 8
            || list.writable_values.is_null()
            || list.shape == 0
            || *(list.shape as *const u8) == 0
        {
            return false;
        }
        let start = iterator.class_tag as i64 as i128;
        let step = iterator.class_payload as i64;
        let first = start + step as i128 * position as i128;
        let last = start + step as i128 * (iterator.len - 1) as i128;
        let (Ok(mut value), Ok(last)) = (i64::try_from(first), i64::try_from(last)) else {
            return false;
        };
        let length = list.len as i128;
        if first < -length || first >= length || (last as i128) < -length || last as i128 >= length
        {
            return false;
        }
        let count = iterator.len - position;
        let mut remaining = count;
        let fill = u8::from(info.value != 0);
        let data = list.writable_values.cast::<u8>();
        while remaining != 0 {
            let index = if value < 0 {
                (value as i128 + length) as usize
            } else {
                value as usize
            };
            // Volatile stores prevent lowering this relocatable stencil to an external memset call.
            data.add(index).write_volatile(fill);
            remaining -= 1;
            if remaining != 0 {
                // Both endpoints fit i64 and the progression is monotonic, so every intermediate fits.
                value = value.wrapping_add(step);
            }
        }
        cursor.write(iterator.len);
        if info.last != usize::MAX {
            write(slots, info.last, scalar(INT, last as u64));
        }
        write(slots, info.constant, scalar(BOOL, fill as u64));
        if info.result != usize::MAX {
            write(slots, info.result, scalar(NONE, 0));
        }
        let edge = (*header).quiet_counts.add(info.backedge);
        *edge = (*edge).saturating_add(count as u64);
        (*header).inline_block_hits = (*header)
            .inline_block_hits
            .wrapping_add((count as u64).wrapping_mul(2));
        let vm = ctx
            .cast::<u8>()
            .add(info.vm_offset)
            .cast::<*mut u8>()
            .read();
        let entries = vm.add(info.entries_offset).cast::<u64>();
        *entries = (*entries).wrapping_add(1);
        let iterations = vm.add(info.iterations_offset).cast::<u64>();
        *iterations = (*iterations).wrapping_add(count as u64);
        true
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tis_stencil_range_fill(
    ctx: *mut c_void,
    slots: *mut Slot,
    patch: *const Patch,
) -> u64 {
    unsafe {
        let p = &*patch;
        range_fill(ctx, slots, p);
        // At an exhausted cursor the original Next owns sticky exhaustion and Done allocation.
        next(ctx, slots, p)
    }
}

/// Read-only speculative work is committed only after every guard and addition succeeds.
#[inline(always)]
unsafe fn range_reduce(ctx: *mut c_void, slots: *mut Slot, p: &Patch) -> bool {
    unsafe {
        let header = ctx.cast::<NativeHeader>();
        if (*header).collecting || (*header).skip_probe {
            return false;
        }
        let info = &*(p.a as *const RangeReduce);
        if info.len > 4 {
            return false;
        }
        let Some(iterator) = checked_native_row(ctx, read(slots, p.left)) else {
            return false;
        };
        let iterator = &*iterator;
        if iterator.kind != 7 || iterator.shape == 0 {
            return false;
        }
        let cursor = iterator.shape as *mut usize;
        let position = cursor.read();
        if position >= iterator.len {
            return false;
        }
        let Some(list) = checked_native_row(ctx, read(slots, p.right)) else {
            return false;
        };
        let list = &*list;
        if list.kind != 8 || list.values.is_null() {
            return false;
        }
        let start = iterator.class_tag as i64 as i128;
        let step = iterator.class_payload as i64;
        let first = start + step as i128 * position as i128;
        let last = start + step as i128 * (iterator.len - 1) as i128;
        let (Ok(mut index), Ok(last)) = (i64::try_from(first), i64::try_from(last)) else {
            return false;
        };
        let length = list.len as i128;
        if first < -length || first >= length || (last as i128) < -length || last as i128 >= length
        {
            return false;
        }
        let count = iterator.len - position;
        let low = (index.min(last).min(0) as i128) * count as i128;
        let high = (index.max(last).max(0) as i128) * count as i128;
        let mut accum = [0i64; 4];
        for (n, value) in accum.iter_mut().enumerate().take(info.len) {
            match info.kinds[n] {
                0..=2 => {}
                3 | 4 => {
                    let input = read(slots, info.outputs[n]);
                    if input.tag & KIND_MASK != INT {
                        return false;
                    }
                    *value = input.a as i64;
                    // Sufficient bounds avoid repeatedly rescanning a suffix that must overflow.
                    let (lower, upper) = if info.kinds[n] == 3 {
                        (0, count as i128)
                    } else {
                        (low, high)
                    };
                    if (*value as i128) + lower < i64::MIN as i128
                        || (*value as i128) + upper > i64::MAX as i128
                    {
                        return false;
                    }
                }
                _ => return false,
            }
        }
        let mut remaining = count;
        let mut matches = 0u64;
        let mut selected = 0i64;
        while remaining != 0 {
            let offset = if index < 0 {
                (index as i128 + length) as usize
            } else {
                index as usize
            };
            if list.values.cast::<u8>().add(offset).read_volatile() != 0 {
                matches = matches.wrapping_add(1);
                selected = index;
                for (n, value) in accum.iter_mut().enumerate().take(info.len) {
                    let amount = match info.kinds[n] {
                        3 => 1,
                        4 => index,
                        _ => continue,
                    };
                    let Some(sum) = value.checked_add(amount) else {
                        return false;
                    };
                    *value = sum;
                }
            }
            remaining -= 1;
            if remaining != 0 {
                index = index.wrapping_add(step);
            }
        }
        for (n, value) in accum.iter().enumerate().take(info.len) {
            let value = match info.kinds[n] {
                1 => last,
                2 if matches != 0 => selected,
                3 | 4 => *value,
                _ => continue,
            };
            write(slots, info.outputs[n], scalar(INT, value as u64));
        }
        cursor.write(iterator.len);
        let edge = (*header).quiet_counts.add(info.backedge);
        *edge = (*edge).saturating_add(count as u64);
        let blocks = matches.wrapping_mul(info.true_blocks).wrapping_add(
            (count as u64)
                .wrapping_sub(matches)
                .wrapping_mul(info.false_blocks),
        );
        (*header).inline_block_hits = (*header).inline_block_hits.wrapping_add(blocks);
        let vm = ctx
            .cast::<u8>()
            .add(info.vm_offset)
            .cast::<*mut u8>()
            .read();
        let entries = vm.add(info.entries_offset).cast::<u64>();
        *entries = (*entries).wrapping_add(1);
        let iterations = vm.add(info.iterations_offset).cast::<u64>();
        *iterations = (*iterations).wrapping_add(count as u64);
        true
    }
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tis_stencil_range_reduce(
    ctx: *mut c_void,
    slots: *mut Slot,
    patch: *const Patch,
) -> u64 {
    unsafe {
        let p = &*patch;
        range_reduce(ctx, slots, p);
        next(ctx, slots, p)
    }
}
