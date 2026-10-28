//! Fresh application sessions in a reused process, matching Flying Circus request timing.
use std::{
    env, fs,
    io::{self, BufRead, Write},
};
use tis::{Tiering, Vm, compile};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let tier = match args.next().as_deref() {
        Some("native") => Tiering::Native,
        Some("bytecode") => Tiering::Bytecode,
        _ => return Err("expected native or bytecode".into()),
    };
    let source = fs::read_to_string(args.next().ok_or("missing source")?)?;
    let mut vm = Vm::new();
    vm.tiering = tier;
    println!("{{\"ready\":true}}");
    io::stdout().flush()?;
    for line in io::stdin().lock().lines() {
        match line?.as_str() {
            "reset" => {
                vm = Vm::new();
                vm.tiering = tier;
                println!("{{\"reset\":true}}");
            }
            "run" => {
                let mut p = compile(&source)?;
                vm.run_program(&mut p)?;
                println!("{{\"stdout\":{:?},\"stderr\":\"\"}}", vm.output);
            }
            "quit" => break,
            _ => return Err("unknown command".into()),
        }
        io::stdout().flush()?;
    }
    Ok(())
}
