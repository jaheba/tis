//! Line-oriented benchmark worker: restore the same scene, time advance, validate energy outside timing.
use std::{
    env,
    io::{self, BufRead, Write},
    time::Instant,
};
use tis::{
    BytecodeMode, Error, NumericMode, Object, Result, TierStats, Tiering, Value, Vm, compile,
};
fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let mut tier = Tiering::Bytecode;
    let mut bytecode = BytecodeMode::default();
    let mut numeric = NumericMode::default();
    let mut warmups = 2usize;
    let mut dump_lir = None;
    let mut dump_native = None;
    let mut no_nests = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--no-nests" => no_nests = true,
            "--dump-native" => {
                dump_native = Some(args.next().ok_or("missing native output prefix")?)
            }
            "--dump-lir" => dump_lir = Some(args.next().ok_or("missing LIR output path")?),
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
                    _ => return Err("unknown tier".into()),
                }
            }
            "--warmups" => warmups = args.next().ok_or("missing warmups")?.parse()?,
            _ => return Err("unknown option".into()),
        }
    }
    if warmups == 0 {
        return Err("at least one warmup is required".into());
    }
    let source = include_str!("../tests/fixtures/game_nbody.py");
    let (setup, _) = source
        .rsplit_once("\noffset_momentum(BODIES[DEFAULT_REFERENCE])\n")
        .ok_or("benchmark fixture changed")?;
    let start = Instant::now();
    let mut program = compile(setup)?;
    let compile_ns = start.elapsed().as_nanos();
    let mut vm = Vm::new();
    vm.tiering = tier;
    vm.disable_native_nests = no_nests;
    vm.capture_native_code = dump_native.is_some();
    vm.bytecode = bytecode;
    vm.numeric = numeric;
    let start = Instant::now();
    vm.run_program(&mut program)?;
    assert_eq!(vm.get_global("DEFAULT_ITERATIONS")?, Value::Int(20_000));
    let reference = vm.heap.get_item(
        vm.get_global("BODIES")?,
        vm.get_global("DEFAULT_REFERENCE")?,
    )?;
    vm.call(vm.get_global("offset_momentum")?, &[reference])?;
    let mut snapshot = vec![];
    let Object::List(bodies) = vm.heap.get(vm.get_global("SYSTEM")?)? else {
        return Err("SYSTEM is not a list".into());
    };
    for &body in bodies {
        let Object::Tuple(parts) = vm.heap.get(body)? else {
            return Err("body is not a tuple".into());
        };
        for &list in &parts[..2] {
            let Object::List(items) = vm.heap.get(list)? else {
                return Err("body coordinates are not lists".into());
            };
            snapshot.push((list, items.to_vec()));
        }
    }
    let setup_ns = start.elapsed().as_nanos();
    let advance = vm.get_global("advance")?;
    let energy = vm.get_global("report_energy")?;
    let mut warmup_ns = vec![];
    for _ in 0..warmups {
        warmup_ns.push(sample(&mut vm, advance, energy, &snapshot)?.0);
    }
    if let Some(prefix) = dump_native {
        let d = vm.native_diagnostics(advance)?;
        std::fs::write(format!("{prefix}.clif"), &d.clif)?;
        std::fs::write(format!("{prefix}.asm"), &d.disassembly)?;
        std::fs::write(format!("{prefix}.bin"), &d.machine_code)?;
    }
    if let Some(path) = dump_lir {
        std::fs::write(
            path,
            vm.function_hir(advance)?
                .lower_lir(tis::lir::LoweringMode::Profiled)?
                .dump(),
        )?;
    }
    let backend_compile_ns = vm.tier_stats.native_compile_ns + vm.tier_stats.copy_patch_compile_ns;
    let profile = vm.function_hir(advance)?.profile();
    let site_samples: u64 = profile.sites.iter().map(|site| site.executions).sum();
    let phase = profile.phase;
    let windows = profile.windows;
    let numeric_specializations = vm.tier_stats.numeric_specializations;
    let numeric_fast_paths = vm.tier_stats.numeric_fast_paths;
    println!(
        "{{\"ready\":true,\"numeric_specializations\":{numeric_specializations},\"numeric_fast_paths\":{numeric_fast_paths},\"compile_ns\":{compile_ns},\"setup_ns\":{setup_ns},\"warmup_ns\":{warmup_ns:?},\"backend_compile_ns\":{backend_compile_ns},\"profile_phase\":\"{phase:?}\",\"profile_windows\":{windows},\"site_samples\":{site_samples}}}"
    );
    io::stdout().flush()?;
    for line in io::stdin().lock().lines() {
        let line = line?;
        if line == "quit" {
            break;
        }
        if line != "run" {
            return Err("unknown worker command".into());
        }
        let (elapsed, before, stats) = sample(&mut vm, advance, energy, &snapshot)?;
        println!(
            "{{\"run_ns\":{elapsed},\"region_entries\":{},\"region_iterations\":{},\"region_failures\":{},\"runtime_calls\":{},\"feedback_probes\":{},\"root_syncs\":{},\"unpack_batches\":{},\"truth_fallbacks\":{},\"float_views\":{},\"float_view_failures\":{},\"numeric_fast_paths\":{},\"numeric_type_misses\":{},\"numeric_arithmetic_fallbacks\":{},\"energy\":\"-0.169089263\",\"backend_compile_ns\":{},\"deoptimizations\":{},\"native_entries\":{},\"copy_patch_entries\":{},\"lir_entries\":{}}}",
            stats.native_region_entries - before.native_region_entries,
            stats.native_region_iterations - before.native_region_iterations,
            stats.native_region_failures - before.native_region_failures,
            stats.native_runtime_calls - before.native_runtime_calls,
            stats.native_feedback_probes - before.native_feedback_probes,
            stats.native_root_syncs - before.native_root_syncs,
            stats.native_unpack_batches - before.native_unpack_batches,
            stats.native_truth_fallbacks - before.native_truth_fallbacks,
            stats.native_float_views - before.native_float_views,
            stats.native_float_view_failures - before.native_float_view_failures,
            stats.numeric_fast_paths - before.numeric_fast_paths,
            stats.numeric_type_misses - before.numeric_type_misses,
            stats.numeric_arithmetic_fallbacks - before.numeric_arithmetic_fallbacks,
            stats.native_compile_ns + stats.copy_patch_compile_ns
                - before.native_compile_ns
                - before.copy_patch_compile_ns,
            stats.deoptimizations - before.deoptimizations,
            stats.native_entries - before.native_entries,
            stats.copy_patch_entries - before.copy_patch_entries,
            stats.lir_entries - before.lir_entries
        );
        io::stdout().flush()?;
    }
    Ok(())
}
fn sample(
    vm: &mut Vm,
    advance: Value,
    energy: Value,
    snapshot: &[(Value, Vec<Value>)],
) -> Result<(u128, TierStats, TierStats)> {
    for (value, items) in snapshot {
        let mut object = vm.heap.get_mut(*value)?;
        let Object::List(values) = &mut *object else {
            return Err(Error::InvalidObject);
        };
        values.copy_from_slice(items);
    }
    let before = vm.tier_stats.clone();
    let start = Instant::now();
    assert_eq!(
        vm.call(advance, &[Value::Float(0.01), Value::Int(20_000)])?,
        Value::None
    );
    let elapsed = start.elapsed().as_nanos();
    // Energy validation can itself become hot during long profiles; exclude its tiering from advance's counters.
    let after = vm.tier_stats.clone();
    let Value::Float(value) = vm.call(energy, &[])? else {
        return Err(Error::Type("energy is not a float"));
    };
    if format!("{value:.9}") != "-0.169089263" {
        return Err(Error::Runtime("benchmark energy mismatch".into()));
    }
    Ok((elapsed, before, after))
}
