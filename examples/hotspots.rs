use std::{env, fs, process};
use tis::{Vm, compile, profile::HotThresholds};

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        process::exit(1);
    }
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let source = if let Some(path) = env::args().nth(1) {
        fs::read_to_string(path)?
    } else {
        "def sum(n):\n    result = 0\n    for i in range(n):\n        result = result + i\n    return result\nfor i in range(20):\n    sum(100)\n".into()
    };
    let mut program = compile(&source)?;
    let mut vm = Vm::new();
    vm.thresholds = HotThresholds {
        function_entries: 16,
        loop_backedges: 128,
    };
    vm.run_program(&mut program)?;
    print!("{}", vm.output);
    for request in vm.take_compilation_requests() {
        println!(
            "hot f{} {} revision {}: {:?} count={}",
            request.function.0,
            request.hir().name(),
            request.revision,
            request.hotspot,
            request.count
        );
        println!("{}", request.lower()?.dump());
    }
    Ok(())
}
