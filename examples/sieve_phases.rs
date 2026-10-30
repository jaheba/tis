//! Diagnostic phase timings; the unchanged end-to-end sieve remains the performance baseline.
use std::{env, fs, time::Instant};
use tis::{Object, Tiering, Value, Vm, compile};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = env::args().collect::<Vec<_>>();
    let source = fs::read_to_string(&args[1])?;
    let mut vm = Vm::new();
    vm.tiering = if args.get(2).is_some_and(|v| v == "bytecode") {
        Tiering::Bytecode
    } else {
        Tiering::Native
    };
    vm.run_program(&mut compile(&source)?)?;
    let allocate = vm.get_global("allocate")?;
    let mark = vm.get_global("mark")?;
    let collect = vm.get_global("collect")?;
    let n = Value::Int(30_000_000);
    let start = Instant::now();
    let flags = vm.call(allocate, &[n])?;
    let allocate_ns = start.elapsed().as_nanos();
    let pin = vm.pin(flags)?;
    let start = Instant::now();
    vm.call(mark, &[flags, n])?;
    let mark_ns = start.elapsed().as_nanos();
    let calls_before = vm.tier_stats.native_runtime_calls;
    let start = Instant::now();
    let primes = vm.call(collect, &[flags, n])?;
    let collect_ns = start.elapsed().as_nanos();
    let Object::List(primes) = vm.heap.get(primes)? else {
        return Err("expected primes".into());
    };
    assert_eq!(primes.len(), 1_857_859);
    println!(
        "{}",
        serde_json::json!({"allocate_ns":allocate_ns,"mark_ns":mark_ns,"collect_ns":collect_ns,"collection_callbacks":vm.tier_stats.native_runtime_calls-calls_before,"callbacks":vm.tier_stats.native_runtime_calls,"native_compile_ns":vm.tier_stats.native_compile_ns,"count":primes.len()})
    );
    vm.unpin(pin);
    Ok(())
}
