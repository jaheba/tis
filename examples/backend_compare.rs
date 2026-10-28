//! Small-kernel comparison. Compile timing excludes common HIR-to-LIR lowering and first-call execution.
use std::time::Instant;
use tis::{Tiering, Value, Vm, compile, profile::HotThresholds};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("backend,median_compile_us,code_bytes,patch_data_bytes,execution_ns_per_call");
    for (name, tier) in [
        ("cranelift", Tiering::Native),
        ("copy-patch", Tiering::CopyPatch),
    ] {
        let mut program = compile("def kernel(x,y):\n    a=x+y\n    b=a*y\n    return b-x\n")?;
        let mut vm = Vm::new();
        vm.profiling = tis::profile::ProfileConfig::eager();
        vm.thresholds = HotThresholds {
            function_entries: 0,
            loop_backedges: 0,
        };
        vm.run_program(&mut program)?;
        let function = vm.get_global("kernel")?;
        let args = [Value::Float(3.0), Value::Float(2.0)];
        for _ in 0..16 {
            assert_eq!(vm.call(function, &args)?, Value::Float(7.0));
        }
        vm.tiering = tier;
        let mut samples = vec![];
        let mut code_bytes = 0;
        let mut data_bytes = 0;
        for round in 0..23 {
            let hir = vm.function_hir(function)?;
            vm.compile_lir(&hir)?;
            let before = vm.tier_stats.clone();
            assert_eq!(vm.call(function, &args)?, Value::Float(7.0));
            let ns = match tier {
                Tiering::Native => vm.tier_stats.native_compile_ns - before.native_compile_ns,
                _ => vm.tier_stats.copy_patch_compile_ns - before.copy_patch_compile_ns,
            };
            if round >= 3 {
                samples.push(ns);
            }
            code_bytes = match tier {
                Tiering::Native => vm.tier_stats.native_code_bytes - before.native_code_bytes,
                _ => vm.tier_stats.copy_patch_code_bytes - before.copy_patch_code_bytes,
            };
            data_bytes = vm.tier_stats.copy_patch_data_bytes - before.copy_patch_data_bytes;
        }
        samples.sort_unstable();
        let start = Instant::now();
        let calls = 10_000;
        for _ in 0..calls {
            assert_eq!(vm.call(function, &args)?, Value::Float(7.0));
        }
        println!(
            "{name},{:.3},{code_bytes},{data_bytes},{:.1}",
            samples[samples.len() / 2] as f64 / 1000.0,
            start.elapsed().as_nanos() as f64 / calls as f64
        );
    }
    Ok(())
}
