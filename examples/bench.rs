//! Warmed benchmark worker for allocation- and object-heavy workloads.
use std::{
    env,
    io::{self, BufRead, Write},
    time::Instant,
};
use tis::{Tiering, Value, Vm, compile};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let benchmark = args.next().ok_or("expected barnes-hut or ticket-search")?;
    let mut tier = Tiering::Native;
    let mut warmups = 2usize;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--tier" => {
                tier = match args.next().as_deref() {
                    Some("bytecode") => Tiering::Bytecode,
                    Some("lir") => Tiering::Lir,
                    Some("native") => Tiering::Native,
                    Some("copy-patch") => Tiering::CopyPatch,
                    _ => return Err("invalid tier".into()),
                }
            }
            "--warmups" => warmups = args.next().ok_or("missing warmups")?.parse()?,
            _ => return Err("unknown option".into()),
        }
    }
    let (source, tail, name, expected, arguments) = match benchmark.as_str() {
        "barnes-hut" => (
            include_str!("../tests/fixtures/barnes_hut.py"),
            "\nprint(f'",
            "bench_quadtree_nbody",
            include_str!("../tests/fixtures/barnes_hut.out"),
            vec![
                Value::Int(1),
                Value::Int(200),
                Value::Int(100),
                Value::Float(0.5),
            ],
        ),
        "ticket-search" => (
            include_str!("../tests/fixtures/ticket_search.py"),
            "\nsearch()",
            "search",
            include_str!("../tests/fixtures/ticket_search.out"),
            vec![],
        ),
        _ => return Err("unknown benchmark".into()),
    };
    let source = source.rsplit_once(tail).ok_or("fixture footer changed")?.0;
    let start = Instant::now();
    let mut p = compile(source)?;
    let compile_ns = start.elapsed().as_nanos();
    let mut vm = Vm::new();
    vm.tiering = tier;
    vm.run_program(&mut p)?;
    let f = vm.get_global(name)?;
    let sample = |vm: &mut Vm| -> Result<u128, Box<dyn std::error::Error>> {
        vm.output.clear();
        let start = Instant::now();
        let value = vm.call(f, &arguments)?;
        let elapsed = start.elapsed().as_nanos();
        if benchmark == "barnes-hut" {
            let Value::Float(value) = value else {
                return Err("energy is not float".into());
            };
            vm.output = format!("{value:.6}\n");
        }
        if vm.output != expected {
            return Err(format!("output mismatch: {:?}", vm.output).into());
        }
        Ok(elapsed)
    };
    let mut warmup_ns = vec![];
    for _ in 0..warmups {
        warmup_ns.push(sample(&mut vm)?);
    }
    println!("{{\"ready\":true,\"compile_ns\":{compile_ns},\"warmup_ns\":{warmup_ns:?}}}");
    io::stdout().flush()?;
    for line in io::stdin().lock().lines() {
        match line?.as_str() {
            "quit" => break,
            "run" => {}
            _ => return Err("unknown command".into()),
        }
        let before = vm.tier_stats.clone();
        let run_ns = sample(&mut vm)?;
        let stats = &vm.tier_stats;
        println!(
            "{{\"run_ns\":{run_ns},\"validated\":true,\"backend_compile_ns\":{},\"deoptimizations\":{},\"runtime_calls\":{},\"root_syncs\":{},\"field_hits\":{}}}",
            stats.native_compile_ns + stats.copy_patch_compile_ns
                - before.native_compile_ns
                - before.copy_patch_compile_ns,
            stats.deoptimizations - before.deoptimizations,
            stats.native_runtime_calls - before.native_runtime_calls,
            stats.native_root_syncs - before.native_root_syncs,
            stats.native_field_hits - before.native_field_hits
        );
        io::stdout().flush()?;
    }
    Ok(())
}
