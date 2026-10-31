use object::{Object, ObjectSection, ObjectSymbol};
use std::{env, fs, path::PathBuf, process::Command};
fn main() {
    println!("cargo:rustc-check-cfg=cfg(tis_offline_stencils)");
    for path in [
        "build.rs",
        "src/copy_patch/stencils.rs",
        "src/copy_patch/patch.rs",
        "src/native_abi.rs",
        "src/value.rs",
        "src/scalar.rs",
    ] {
        println!("cargo:rerun-if-changed={path}");
    }
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let object = out.join("stencils.o");
    let llvm = out.join("stencils.ll");
    let target = env::var("TARGET").unwrap();
    let supported = target.starts_with("aarch64-") || target.starts_with("x86_64-");
    if !supported {
        fs::write(
            out.join("stencils.rs"),
            "pub const STENCILS: &[(&str,&[u8])] = &[];",
        )
        .unwrap();
        return;
    }
    let source = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .join("src/copy_patch/stencils.rs");
    let status = Command::new(env::var_os("RUSTC").unwrap())
        .current_dir(&out)
        .arg(source)
        .args([
            "--cfg",
            "tis_offline_stencils",
            "--crate-name",
            "tis_stencils",
            "--crate-type",
            "lib",
            "--edition=2024",
            "--target",
            &target,
            "-Copt-level=3",
            "-Cpanic=abort",
            "-Cdebug-assertions=no",
            "-Coverflow-checks=no",
            "-Ccodegen-units=1",
            "-Clto=off",
            "--emit",
        ])
        .arg(format!(
            "obj={},llvm-ir={}",
            object.display(),
            llvm.display()
        ))
        .status()
        .expect("run rustc for stencils");
    assert!(status.success(), "stencil compilation failed");
    let bytes = fs::read(object).unwrap();
    let file = object::File::parse(bytes.as_slice()).unwrap();
    let ir = fs::read_to_string(llvm).unwrap();
    let mut functions = file
        .symbols()
        .filter(|s| s.is_definition() && s.kind() == object::SymbolKind::Text)
        .collect::<Vec<_>>();
    functions.sort_by_key(|s| (s.section_index().map(|index| index.0), s.address()));
    let mut generated = String::from(
        "// Generated from optimized Rust object code.\npub const STENCILS: &[(&str,&[u8])] = &[\n",
    );
    let mut count = 0;
    for symbol in &functions {
        let name = symbol.name().unwrap_or("").trim_start_matches('_');
        let Some(name) = name.strip_prefix("tis_stencil_") else {
            continue;
        };
        let section = file
            .section_by_index(symbol.section_index().unwrap())
            .unwrap();
        let end = if symbol.size() > 0 {
            symbol.address() + symbol.size()
        } else {
            functions
                .iter()
                .filter(|next| {
                    next.section_index() == symbol.section_index()
                        && next.address() > symbol.address()
                })
                .map(|next| next.address())
                .min()
                .unwrap_or(section.address() + section.size())
        };
        let start = symbol.address() - section.address();
        let size = end - symbol.address();
        assert!(size > 0, "empty stencil {name}");
        for (offset, relocation) in section.relocations() {
            assert!(
                offset < start || offset >= start + size,
                "stencil {name} has unsupported external relocation {relocation:?}"
            );
        }
        if name != "return" {
            let marker = format!("@tis_stencil_{name}(");
            let begin = ir.find(&marker).expect("stencil LLVM definition");
            let body = &ir[begin..];
            let body = &body[..body.find("\n}").unwrap()];
            if name.starts_with("fpair_")
                || matches!(
                    name,
                    "range_reduce"
                        | "range_fill"
                        | "quiet_enter"
                        | "next_test"
                        | "direct_next"
                        | "direct_string_slice"
                        | "direct_scalar"
                        | "local_load"
                        | "direct_field"
                        | "direct_float_view"
                        | "direct_unpack"
                        | "direct_getitem"
                        | "direct_setitem"
                        | "is_done"
                )
            {
                assert!(
                    body.lines()
                        .filter(|line| line.contains(" call "))
                        .all(|line| line.contains("@llvm.")
                            || (line.contains("tail") && line.contains(" i64 %"))),
                    "fast stencil {name} contains a non-tail runtime call"
                );
            }
            let continuations = body
                .lines()
                .filter(|line| {
                    line.contains(" call ") && line.contains(" i64 %") && !line.contains('@')
                })
                .collect::<Vec<_>>();
            assert!(
                !continuations.is_empty(),
                "stencil {name} lost its continuation"
            );
            assert!(
                continuations.iter().all(|line| line
                    .split_whitespace()
                    .any(|word| word == "tail" || word == "musttail")),
                "stencil {name} has a non-tail continuation"
            );
        }
        let code = &section.data().unwrap()[start as usize..(start + size) as usize];
        generated.push_str(&format!("({name:?}, &{code:?}),\n"));
        count += 1;
    }
    assert_eq!(count, 60, "missing or unexpected stencil symbols");
    generated.push_str("];\n");
    fs::write(out.join("stencils.rs"), generated).unwrap();
}
