//! Relocate one audited AArch64 continuation suffix; unknown templates stay indirect.

const LOAD_NEXT: u32 = 0xa943_8843; // ldp x3, x2, [x2, #56]
const BR_NEXT: u32 = 0xd61f_0060; // br x3

#[cfg(all(target_arch = "aarch64", target_endian = "little"))]
const _: () = {
    assert!(std::mem::offset_of!(super::Patch, next) == 56);
    assert!(std::mem::offset_of!(super::Patch, next_patch) == 64);
};

pub(super) fn link(
    name: &str,
    code: &mut [u8],
    code_offset: usize,
    patch_offset: usize,
    next_code: usize,
    next_patch: usize,
) -> bool {
    // x2 remains the Patch argument at these exact suffixes. Conditional memory
    // templates may call feedback first, but restore the same tail ABI before here.
    if !matches!(
        name,
        "constant"
            | "copy"
            | "retag"
            | "bool_not"
            | "to_float"
            | "fadd"
            | "fsub"
            | "fmul"
            | "fadd_box"
            | "fsub_box"
            | "fmul_box"
            | "fdiv"
            | "fdiv_box"
            | "float_load_raw"
            | "float_store"
            | "guard"
            | "guard_unbox"
            | "fpair_aa"
            | "fpair_as"
            | "fpair_am"
            | "fpair_sa"
            | "fpair_ss"
            | "fpair_sm"
            | "fpair_ma"
            | "fpair_ms"
            | "fpair_mm"
    ) || code.len() < 8
        || !code.len().is_multiple_of(4)
    {
        return false;
    }
    let guarded = matches!(name, "guard" | "guard_unbox");
    let suffix = if guarded {
        // The failure block stays byte-for-byte intact: ldr x0,[x2,#88]; ret.
        if code.len() < 16
            || code[code.len() - 8..] != [0x40, 0x2c, 0x40, 0xf9, 0xc0, 0x03, 0x5f, 0xd6]
        {
            return false;
        }
        code.len() - 16
    } else {
        code.len() - 8
    };
    if code[suffix..suffix + 4] != LOAD_NEXT.to_le_bytes()
        || code[suffix + 4..suffix + 8] != BR_NEXT.to_le_bytes()
    {
        return false;
    }
    // A merged failure/callback tail must not enter after our rewritten Patch adjustment.
    if (guarded
        || name.starts_with("fpair_")
        || matches!(name, "fdiv" | "fdiv_box" | "float_load_raw" | "float_store"))
        && code
            .as_chunks::<4>()
            .0
            .iter()
            .enumerate()
            .any(|(i, bytes)| {
                let instruction = u32::from_le_bytes(*bytes);
                branch_target(i * 4, instruction) == Some((suffix + 4) as i128)
            })
    {
        return false;
    }
    let Some(branch_pc) = code_offset.checked_add(suffix + 4) else {
        return false;
    };
    if !branch_pc.is_multiple_of(4) || !next_code.is_multiple_of(4) {
        return false;
    }
    let branch_delta = next_code as i128 - branch_pc as i128;
    let patch_delta = next_patch as i128 - patch_offset as i128;
    if !(-(1i128 << 27)..(1i128 << 27)).contains(&branch_delta) || patch_delta.abs() > 4095 {
        return false;
    }

    // The tail ABI passes only x0/x1/x2. x3 is scratch, so its unused code address
    // can disappear; ADD/SUB preserves the exact next Patch argument without a load.
    let add = if patch_delta < 0 {
        0xd100_0042
    } else {
        0x9100_0042
    };
    let add = add | ((patch_delta.unsigned_abs() as u32) << 10);
    let branch = if branch_delta == 4 {
        0xd503_201f // nop: the next template begins immediately after this suffix.
    } else {
        0x1400_0000 | (((branch_delta / 4) as i32 as u32) & 0x03ff_ffff)
    };
    code[suffix..suffix + 4].copy_from_slice(&add.to_le_bytes());
    code[suffix + 4..suffix + 8].copy_from_slice(&branch.to_le_bytes());
    true
}

/// Direct AArch64 branches, including link, compare/test and conditional forms.
fn branch_target(pc: usize, instruction: u32) -> Option<i128> {
    let (bits, width) = if instruction & 0x7c00_0000 == 0x1400_0000 {
        (instruction & 0x03ff_ffff, 26)
    } else if instruction & 0xff00_0000 == 0x5400_0000 || instruction & 0x7e00_0000 == 0x3400_0000 {
        ((instruction >> 5) & 0x7ffff, 19)
    } else if instruction & 0x7e00_0000 == 0x3600_0000 {
        ((instruction >> 5) & 0x3fff, 14)
    } else {
        return None;
    };
    let signed = ((bits << (32 - width)) as i32 >> (32 - width)) as i128;
    Some(pc as i128 + signed * 4)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn suffix() -> Vec<u8> {
        [LOAD_NEXT.to_le_bytes(), BR_NEXT.to_le_bytes()].concat()
    }

    #[test]
    fn relocates_forward_backward_and_shared_patch_tails() {
        for delta in [-4095i64, -120, 0, 120, 4095] {
            for target in [0usize, 8192] {
                let mut code = suffix();
                assert!(link(
                    "fadd",
                    &mut code,
                    4096,
                    8192,
                    target,
                    (8192 + delta) as usize
                ));
                let add = u32::from_le_bytes(code[..4].try_into().unwrap());
                let amount = ((add >> 10) & 4095) as i64;
                let decoded = if add & (1 << 30) != 0 {
                    -amount
                } else {
                    amount
                };
                assert_eq!(decoded, delta);
                let branch = u32::from_le_bytes(code[4..].try_into().unwrap());
                let displacement = (((branch & 0x03ff_ffff) << 6) as i32 >> 6) as i64 * 4;
                assert_eq!(4100 + displacement, target as i64);
            }
        }
    }

    #[test]
    fn adjacent_template_falls_through_without_an_indirect_branch() {
        let mut code = suffix();
        assert!(link("retag", &mut code, 0, 0, 8, 120));
        assert_eq!(&code[4..], &0xd503_201fu32.to_le_bytes());
    }

    #[cfg(all(target_arch = "aarch64", target_endian = "little"))]
    #[test]
    fn current_arithmetic_templates_satisfy_the_suffix_contract() {
        for name in [
            "constant",
            "copy",
            "retag",
            "fadd",
            "fsub",
            "fmul",
            "fadd_box",
            "fsub_box",
            "fmul_box",
            "fdiv",
            "fdiv_box",
            "float_load_raw",
            "float_store",
            "fpair_aa",
            "fpair_as",
            "fpair_am",
            "fpair_sa",
            "fpair_ss",
            "fpair_sm",
            "fpair_ma",
            "fpair_ms",
            "fpair_mm",
        ] {
            let mut code = crate::copy_patch::template(name).unwrap().to_vec();
            let next = code.len();
            assert!(link(name, &mut code, 0, 0, next, 120), "{name}");
            assert_eq!(&code[next - 4..], &0xd503_201fu32.to_le_bytes());
        }
    }

    #[cfg(all(target_arch = "aarch64", target_endian = "little"))]
    #[test]
    fn guard_failure_blocks_remain_intact() {
        for name in ["guard", "guard_unbox"] {
            let mut code = crate::copy_patch::template(name).unwrap().to_vec();
            let original = code.clone();
            let next = code.len();
            assert!(link(name, &mut code, 0, 0, next, 120));
            assert_eq!(&code[next - 8..], &original[next - 8..]);
            let branch = u32::from_le_bytes(code[next - 12..next - 8].try_into().unwrap());
            assert_eq!(branch_target(next - 12, branch), Some(next as i128));
            let mut shared = original;
            let entry = 0x1400_0000u32 | ((next - 12) / 4) as u32;
            shared[..4].copy_from_slice(&entry.to_le_bytes());
            let before = shared.clone();
            assert!(!link(name, &mut shared, 0, 0, next, 120));
            assert_eq!(shared, before);
        }
    }

    #[test]
    fn conditional_tails_reject_entries_after_the_patch_adjustment() {
        for branch in [
            0x1400_0002u32,
            0x9400_0002,
            0x5400_0040,
            0x3400_0040,
            0xb500_0040,
            0x3600_0040,
            0xb700_0040,
        ] {
            let mut code = [branch.to_le_bytes().as_slice(), suffix().as_slice()].concat();
            let original = code.clone();
            assert!(!link("float_store", &mut code, 0, 0, 16, 120));
            assert_eq!(code, original);
        }
        assert_eq!(branch_target(16, 0x17ff_ffff), Some(12));
        assert_eq!(branch_target(16, 0x54ff_ffe0), Some(12));
        let mut code = [0x1400_0001u32.to_le_bytes().as_slice(), suffix().as_slice()].concat();
        assert!(link("float_store", &mut code, 0, 0, 16, 120));
    }

    #[test]
    fn rejects_unrecognized_or_unrepresentable_tails_without_mutation() {
        for (name, pc, patch, next_pc, next_patch) in [
            ("callback", 0, 0, 16, 120),
            ("fadd", 1, 0, 16, 120),
            ("fadd", 0, 0, 17, 120),
            ("fadd", 0, 0, 16, 4096),
            ("fadd", 0, 8192, 16, 0),
            ("fadd", 0, 0, (1 << 27) + 4, 120),
            ("fadd", usize::MAX, 0, 16, 120),
        ] {
            let mut code = suffix();
            let original = code.clone();
            assert!(!link(name, &mut code, pc, patch, next_pc, next_patch));
            assert_eq!(code, original);
        }
        let mut code = suffix();
        code[0] ^= 1;
        let original = code.clone();
        assert!(!link("fadd", &mut code, 0, 0, 16, 120));
        assert_eq!(code, original);
    }
}
