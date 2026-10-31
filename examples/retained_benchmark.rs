//! Retained-VM measurements, separate from Flying Circus fresh-session comparisons.
use std::{env, fs, time::Instant};
use tis::{Tiering, Vm, compile};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = env::args().skip(1).collect::<Vec<_>>();
    if args.len() != 3 {
        return Err(
            "usage: retained_benchmark SOURCE bytecode|native|copy-patch EXPECTED_STDOUT".into(),
        );
    }
    let source = fs::read_to_string(&args[0])?;
    let expected = fs::read_to_string(&args[2])?;
    let tier = match args[1].as_str() {
        "bytecode" => Tiering::Bytecode,
        "native" => Tiering::Native,
        "copy-patch" => Tiering::CopyPatch,
        _ => return Err("unknown tier".into()),
    };
    let start = Instant::now();
    let mut program = compile(&source)?;
    let frontend_ns = start.elapsed().as_nanos();
    let mut vm = Vm::new();
    vm.tiering = tier;
    vm.readonly_regions = matches!(tier, Tiering::Native | Tiering::CopyPatch);
    vm.mutation_regions = matches!(tier, Tiering::Native | Tiering::CopyPatch);
    let mut samples_ns = vec![];
    let mut native_compile_deltas = vec![];
    let mut native_entry_deltas = vec![];
    let mut first_ns = 0;
    let mut first_native_compile_ns = 0;
    for iteration in 0..7 {
        vm.output.clear();
        let compile_before = vm.tier_stats.native_compile_ns + vm.tier_stats.copy_patch_compile_ns;
        let entries_before = vm.tier_stats.native_entries;
        let start = Instant::now();
        vm.run_program(&mut program)?;
        let elapsed = start.elapsed().as_nanos();
        if vm.output != expected {
            return Err(format!("output mismatch at iteration {iteration}").into());
        }
        if iteration == 0 {
            first_ns = elapsed;
            first_native_compile_ns = vm.tier_stats.native_compile_ns
                + vm.tier_stats.copy_patch_compile_ns
                - compile_before;
        } else if iteration >= 4 {
            samples_ns.push(elapsed);
            native_compile_deltas.push(
                vm.tier_stats.native_compile_ns + vm.tier_stats.copy_patch_compile_ns
                    - compile_before,
            );
            native_entry_deltas.push(vm.tier_stats.native_entries - entries_before);
        }
    }
    let mut sorted = samples_ns.clone();
    sorted.sort_unstable();
    println!(
        "{}",
        serde_json::json!({
            "source":args[0], "tier":args[1], "frontend_ns":frontend_ns,
            "first_execution_ns":first_ns, "first_backend_compile_ns":first_native_compile_ns,
            "warmups":3, "retained_samples_ns":samples_ns,
            "retained_median_ns":sorted[1], "backend_compile_deltas_ns":native_compile_deltas,
            "native_entry_deltas":native_entry_deltas,
            "stats":format!("{:?}",vm.tier_stats),
            "boundary":"one compiled Program and VM; globals retained; execution only; output capture included, delivery excluded"
        })
    );
    Ok(())
}
