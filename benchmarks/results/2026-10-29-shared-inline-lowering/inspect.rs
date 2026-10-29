use tis::{Vm, Tiering, Value, compile};
fn main()->Result<(),Box<dyn std::error::Error>> {
    let source=std::fs::read_to_string("tests/fixtures/barnes_hut.py")?;
    let source=source.rsplit_once("\nprint(f'").unwrap().0;
    let mut vm=Vm::new();vm.tiering=Tiering::Native;vm.cfg_inlining=true;
    vm.run_program(&mut compile(source)?)?;
    let f=vm.get_global("bench_quadtree_nbody")?;
    for _ in 0..2 {vm.call(f,&[Value::Int(1),Value::Int(200),Value::Int(100),Value::Float(0.5)])?;}
    vm.run_program(&mut compile("force=QuadTree.calculate_force\ninsert=QuadTree.insert\nupdate=Particle.update\nadvance=advance_system\n")?)?;
    for name in ["force","insert","update","advance"] {
        let f=vm.function_hir(vm.get_global(name)?)?;
        let p=f.profile();println!("{name}: {:?} deopts={}",p.phase,p.deoptimizations);
        for (i,site) in p.sites.iter().enumerate().filter(|(_,s)|s.guard_failures>0) {println!("  %{i} guards={} {:?}",site.guard_failures,site.exit_reasons);}
    }
    Ok(())
}
