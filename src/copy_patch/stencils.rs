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
use abi::*;
use patch::Patch;
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
#[inline(always)]
unsafe fn next(ctx: *mut c_void, slots: *mut Slot, p: &Patch) -> u64 {
    unsafe {
        let f: Next = std::mem::transmute(p.next);
        f(ctx, slots, p.next_patch as *const Patch)
    }
}
#[inline(always)]
fn scalar(tag: u64, a: u64) -> Slot {
    Slot { tag, a, b: 0, c: 0 }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tis_stencil_constant(
    ctx: *mut c_void,
    slots: *mut Slot,
    patch: *const Patch,
) -> u64 {
    unsafe {
        let p = &*patch;
        write(
            slots,
            p.dst,
            Slot {
                tag: p.tag,
                a: p.a,
                b: p.b,
                c: p.c,
            },
        );
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
                let Some(value) = scalar::FloatOperator::$op.evaluate(left, right) else {
                    return p.status;
                };
                write(slots, p.dst, scalar(RAW_FLOAT, value.to_bits()));
                next(ctx, slots, p)
            }
        }
    };
}
binary!(tis_stencil_fadd, Add);
binary!(tis_stencil_fsub, Subtract);
binary!(tis_stencil_fmul, Multiply);
binary!(tis_stencil_fdiv, Divide);
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
