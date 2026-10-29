//! Compare opaque Rust JSON helpers with Python codec loops through each execution tier.
use std::time::Instant;
use tis::{Tiering, Value, Vm, compile};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let iterations: i64 = std::env::args()
        .nth(1)
        .map(|s| s.parse())
        .transpose()?
        .unwrap_or(200);
    if iterations < 1 {
        return Err("iterations must be positive".into());
    }
    for tier in [
        Tiering::Bytecode,
        Tiering::Lir,
        Tiering::Native,
        Tiering::CopyPatch,
    ] {
        for python in [false, true] {
            let (loads, dumps) = if python {
                ("json_loads", "json_dumps")
            } else {
                ("json.loads", "json.dumps")
            };
            let source = format!(
                "{}\nimport json\ntext = '{{\"name\":\"sample\",\"items\":[1,2,3,4,5],\"active\":true}}'\ndef work(n):\n    total = 0\n    for i in range(n):\n        value = {loads}(text)\n        encoded = {dumps}(value)\n        total += len(encoded)\n    return total\n",
                include_str!("../experiments/json/codec.py")
            );
            let mut vm = Vm::new();
            vm.tiering = tier;
            vm.thresholds.function_entries = 2;
            vm.thresholds.loop_backedges = 4;
            vm.run_program(&mut compile(&source)?)?;
            let work = vm.get_global("work")?;
            // Warm recursively reached Python functions before timing the same workload.
            for _ in 0..3 {
                vm.call(work, &[Value::Int(iterations)])?;
            }
            let start = Instant::now();
            let checksum = vm.call(work, &[Value::Int(iterations)])?;
            println!(
                "{tier:?} {}: {:.3} ms, checksum={checksum:?}, native_entries={}, native_osr_entries={}, copy_patch_entries={}, copy_patch_osr_entries={}, compilation_errors={}",
                if python { "python" } else { "rust" },
                start.elapsed().as_secs_f64() * 1000.0,
                vm.tier_stats.native_entries,
                vm.tier_stats.native_osr_entries,
                vm.tier_stats.copy_patch_entries,
                vm.tier_stats.copy_patch_osr_entries,
                vm.tier_stats.compilation_errors
            );
        }
    }
    Ok(())
}
