use std::{env, fs, process};
use tis::{BytecodeMode, NumericMode, Tiering, Vm, compile};

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        process::exit(1);
    }
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let mut tier = Tiering::Bytecode;
    let mut bytecode = BytecodeMode::default();
    let mut numeric = NumericMode::default();
    let mut path = None;
    let mut stats = false;
    let mut cfg_inlining = false;
    let mut readonly_regions = false;
    let mut mutation_regions = false;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--numeric" => {
                numeric = match args.next().as_deref() {
                    Some("adaptive") => NumericMode::Adaptive,
                    Some("disabled") => NumericMode::Disabled,
                    _ => return Err("--numeric requires adaptive or disabled".into()),
                };
            }
            "--bytecode" => {
                bytecode = match args.next().as_deref() {
                    Some("stack") => BytecodeMode::Stack,
                    Some("register") => BytecodeMode::Register,
                    _ => return Err("--bytecode requires stack or register".into()),
                };
            }
            "--tier" => {
                tier = match args.next().as_deref() {
                    Some("bytecode") => Tiering::Bytecode,
                    Some("lir") => Tiering::Lir,
                    Some("native") => Tiering::Native,
                    Some("copy-patch") => Tiering::CopyPatch,
                    _ => return Err("--tier requires bytecode, lir, native or copy-patch".into()),
                }
            }
            "--stats" => stats = true,
            "--cfg-inlining" => cfg_inlining = true,
            "--readonly-regions" => readonly_regions = true,
            "--mutation-regions" => mutation_regions = true,
            _ if path.is_none() => path = Some(argument),
            _ => return Err("unexpected argument".into()),
        }
    }
    let path = path.ok_or(
        "usage: cargo run --example run -- [--tier bytecode|lir|native|copy-patch] [--bytecode stack|register] [--numeric adaptive|disabled] [--stats] [--cfg-inlining] [--readonly-regions] [--mutation-regions] FILE.py",
    )?;
    let source = fs::read_to_string(path)?;
    let mut program = compile(&source)?;
    let mut vm = Vm::new();
    vm.tiering = tier;
    vm.cfg_inlining = cfg_inlining;
    vm.readonly_regions = readonly_regions;
    vm.mutation_regions = mutation_regions;
    vm.bytecode = bytecode;
    vm.numeric = numeric;
    let result = vm.run_program(&mut program);
    print!("{}", vm.output);
    if stats {
        eprintln!("{:?}", vm.tier_stats);
    }
    result?;
    Ok(())
}
