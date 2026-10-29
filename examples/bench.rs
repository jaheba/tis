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
    let mut cfg_inlining = false;
    let mut readonly_regions = false;
    let mut mutation_regions = false;
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
            "--cfg-inlining" => cfg_inlining = true,
            "--readonly-regions" => readonly_regions = true,
            "--mutation-regions" => mutation_regions = true,
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
    vm.cfg_inlining = cfg_inlining;
    vm.readonly_regions = readonly_regions;
    vm.mutation_regions = mutation_regions;
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
    println!(
        "{{\"ready\":true,\"compile_ns\":{compile_ns},\"warmup_ns\":{warmup_ns:?},\"compilation_errors_total\":{}}}",
        vm.tier_stats.compilation_errors
    );
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
            "{{\"run_ns\":{run_ns},\"validated\":true,\"backend_compile_ns\":{},\"deoptimizations\":{},\"runtime_calls\":{},\"root_syncs\":{},\"field_hits\":{},\"inline_field_hits\":{},\"inline_blocks\":{},\"positional_calls\":{},\"leaf_calls\":{},\"method_calls\":{},\"literal_hits\":{},\"interned_literals\":{},\"name_hits\":{},\"positional_frames\":{},\"scratch_creations\":{},\"scratch_reuses\":{},\"shape_reads\":{},\"shape_writes\":{},\"body_inlines\":{},\"leaf_body_inlines\":{},\"native_leaf_entry_batches\":{},\"virtual_tuple_returns\":{},\"readonly_entries\":{},\"readonly_misses\":{},\"mutation_entries\":{},\"mutation_writes\":{},\"mutation_guard_exits\":{},\"mutation_cold_misses\":{},\"compilation_errors_total\":{}}}",
            stats.native_compile_ns + stats.copy_patch_compile_ns
                - before.native_compile_ns
                - before.copy_patch_compile_ns,
            stats.deoptimizations - before.deoptimizations,
            stats.native_runtime_calls - before.native_runtime_calls,
            stats.native_root_syncs - before.native_root_syncs,
            stats.native_field_hits - before.native_field_hits,
            stats.native_inline_field_hits - before.native_inline_field_hits,
            stats.native_inline_blocks - before.native_inline_blocks,
            stats.bytecode_positional_calls - before.bytecode_positional_calls,
            stats.bytecode_leaf_calls - before.bytecode_leaf_calls,
            stats.bytecode_method_calls - before.bytecode_method_calls,
            stats.literal_hits - before.literal_hits,
            stats.interned_literals - before.interned_literals,
            stats.name_cache_hits - before.name_cache_hits,
            stats.positional_frames - before.positional_frames,
            stats.register_scratch_creations - before.register_scratch_creations,
            stats.register_scratch_reuses - before.register_scratch_reuses,
            stats.shape_read_hits - before.shape_read_hits,
            stats.shape_write_hits - before.shape_write_hits,
            stats.native_body_inlines - before.native_body_inlines,
            stats.leaf_body_inlines - before.leaf_body_inlines,
            stats.native_leaf_entry_batches - before.native_leaf_entry_batches,
            stats.virtual_tuple_returns - before.virtual_tuple_returns,
            stats.readonly_entries - before.readonly_entries,
            stats.readonly_misses - before.readonly_misses,
            stats.mutation_entries - before.mutation_entries,
            stats.mutation_writes - before.mutation_writes,
            stats.mutation_guard_exits - before.mutation_guard_exits,
            stats.mutation_cold_misses - before.mutation_cold_misses,
            stats.compilation_errors
        );
        io::stdout().flush()?;
    }
    Ok(())
}
