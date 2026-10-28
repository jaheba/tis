use std::{env, fs, process};
use tis::{Tiering, Vm, compile};

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        process::exit(1);
    }
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let mut tier = Tiering::Bytecode;
    let mut path = None;
    let mut stats = false;
    while let Some(argument) = args.next() {
        match argument.as_str() {
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
            _ if path.is_none() => path = Some(argument),
            _ => return Err("unexpected argument".into()),
        }
    }
    let path = path.ok_or(
        "usage: cargo run --example run -- [--tier bytecode|lir|native|copy-patch] [--stats] FILE.py",
    )?;
    let source = fs::read_to_string(path)?;
    let mut program = compile(&source)?;
    let mut vm = Vm::new();
    vm.tiering = tier;
    let result = vm.run_program(&mut program);
    print!("{}", vm.output);
    if stats {
        eprintln!("{:?}", vm.tier_stats);
    }
    result?;
    Ok(())
}
