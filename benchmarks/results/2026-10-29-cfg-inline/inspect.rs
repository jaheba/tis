use std::{collections::BTreeSet, fs};
use tis::{Vm, Tiering, compile, lir::{Function,LoweringMode}};
fn main() -> Result<(),Box<dyn std::error::Error>> {
    let source=fs::read_to_string("tests/fixtures/barnes_hut.py")?;
    let mut vm=Vm::new();vm.tiering=Tiering::Native;vm.cfg_inlining=true;
    vm.run_program(&mut compile(&source)?)?;
    assert_eq!(vm.output,fs::read_to_string("tests/fixtures/barnes_hut.out")?);
    let mut found=false;
    for request in vm.take_compilation_requests() {
        if let Ok(lir)=Function::lower_with_cfg(request.hir(),LoweringMode::Profiled) {
            let names:BTreeSet<_>=lir.inline_scopes.iter().map(|scope|scope.source.name().to_owned()).collect();
            if !names.is_empty() {println!("{}: {} LIR blocks, copied {:?}",request.hir().name(),lir.blocks.len(),names);}
            found |= names.contains("calculate_force");
            if request.hir().name()=="calculate_force" {
                fs::write("benchmarks/results/2026-10-29-cfg-inline/calculate-force.lir.txt",lir.dump())?;
            }
        }
    }
    assert!(found,"recursive force body was not incorporated");
    Ok(())
}
