use std::{env, fs, process};
use tis::{Vm, compile};
fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        process::exit(1);
    }
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let path = env::args()
        .nth(1)
        .ok_or("usage: cargo run --example run -- FILE.py")?;
    let source = fs::read_to_string(path)?;
    let mut program = compile(&source)?;
    let mut vm = Vm::new();
    let result = vm.run_program(&mut program);
    print!("{}", vm.output);
    result?;
    Ok(())
}
