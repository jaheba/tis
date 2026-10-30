use tis::{BytecodeMode, Tiering, Vm, compile};

fn run_fixture(source: &str, output: &str) {
    for tier in [
        Tiering::Bytecode,
        Tiering::Lir,
        Tiering::Native,
        Tiering::CopyPatch,
    ] {
        for bytecode in [BytecodeMode::Register, BytecodeMode::Stack] {
            let mut vm = Vm::new();
            vm.tiering = tier;
            vm.bytecode = bytecode;
            vm.thresholds.function_entries = 2;
            vm.thresholds.loop_backedges = 4;
            vm.profiling.window_ticks = 8;
            vm.run_program(&mut compile(source).unwrap()).unwrap();
            vm.collect_garbage(&[]).unwrap();
            assert_eq!(vm.output, output);
            assert_eq!(vm.stack_len(), 0);
            assert_eq!(vm.tier_stats.compilation_errors, 0);
        }
    }
}

#[test]
fn sorting_protocols_stability_and_callback_roots_work_in_every_tier() {
    run_fixture(
        include_str!("fixtures/sorted.py"),
        "sorted compatibility ok\n",
    );
}

#[test]
fn ascii_and_unicode_slices_keep_python_character_bounds_in_every_tier() {
    run_fixture(
        include_str!("fixtures/string_slices.py"),
        "string slices ok\n",
    );
}
