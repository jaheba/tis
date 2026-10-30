use tis::{Error, Tiering, Vm, compile};

#[test]
fn exception_arguments_survive_gc_and_preserve_identity() {
    let source = r#"
items = [1]
e = ValueError(items)
assert isinstance(e, Exception)
assert isinstance(e, BaseException)
assert not isinstance(BaseException(), Exception)
assert not isinstance(e, TypeError)
assert e.args[0] is items
items.append(2)
assert str(e) == '[1, 2]'
assert repr(e) == 'ValueError([1, 2])'
assert str(ValueError()) == ''
assert str(ValueError('a', 2)) == "('a', 2)"
for i in range(5000):
    temporary = [i]
assert str(e) == '[1, 2]'
"#;
    for tier in [
        Tiering::Bytecode,
        Tiering::Lir,
        Tiering::Native,
        Tiering::CopyPatch,
    ] {
        let mut vm = Vm::new();
        vm.tiering = tier;
        vm.thresholds.function_entries = 1;
        vm.thresholds.loop_backedges = 4;
        vm.run_program(&mut compile(source).unwrap()).unwrap();
        vm.collect_garbage(&[]).unwrap();
        assert_eq!(
            vm.run_program(&mut compile("raise e").unwrap())
                .unwrap_err(),
            Error::Runtime("ValueError: [1, 2]".into())
        );
    }
}

#[test]
fn raises_evaluate_the_actual_exception_expression() {
    for (source, expected) in [
        ("raise ValueError('bad')", "ValueError: bad"),
        ("raise ValueError", "ValueError"),
        (
            "raise None",
            "type error: exceptions must derive from BaseException",
        ),
        (
            "raise 1",
            "type error: exceptions must derive from BaseException",
        ),
        ("raise", "RuntimeError: No active exception to reraise"),
        (
            "def make():\n    return TypeError('rebound')\nValueError = make\nraise ValueError()",
            "TypeError: rebound",
        ),
        (
            "ValueError = 7\nraise ValueError('bad')",
            "type error: expected an object",
        ),
    ] {
        for tier in [
            Tiering::Bytecode,
            Tiering::Lir,
            Tiering::Native,
            Tiering::CopyPatch,
        ] {
            let mut vm = Vm::new();
            vm.tiering = tier;
            vm.thresholds.function_entries = 1;
            let error = vm.run_program(&mut compile(source).unwrap()).unwrap_err();
            assert_eq!(error.to_string(), expected, "{tier:?}: {source}");
        }
    }
}

#[test]
fn digit_method_checks_unicode_digit_property_not_numeric_property() {
    for tier in [
        Tiering::Bytecode,
        Tiering::Lir,
        Tiering::Native,
        Tiering::CopyPatch,
    ] {
        let mut vm = Vm::new();
        vm.tiering = tier;
        vm.thresholds.function_entries = 1;
        vm.thresholds.loop_backedges = 4;
        vm.run_program(
            &mut compile(
                r#"
for i in range(50):
    assert '0123456789'.isdigit()
    assert '²٣४𝟠'.isdigit()
    assert not ''.isdigit()
    assert not 'Ⅳ'.isdigit()
    assert not '½'.isdigit()
    assert not '-1'.isdigit()
    assert not '１２x'.isdigit()
    check = '٠'.isdigit
    assert check()
"#,
            )
            .unwrap(),
        )
        .unwrap();
    }
}

#[test]
fn hot_raise_propagates_from_compiled_loop() {
    for tier in [Tiering::Lir, Tiering::Native, Tiering::CopyPatch] {
        let mut vm = Vm::new();
        vm.tiering = tier;
        vm.thresholds.function_entries = 1;
        vm.thresholds.loop_backedges = 4;
        let error = vm
            .run_program(
                &mut compile("for i in range(300):\n    if i == 250:\n        raise ValueError(i)")
                    .unwrap(),
            )
            .unwrap_err();
        assert_eq!(error, Error::Runtime("ValueError: 250".into()));
        assert_eq!(vm.tier_stats.compilation_errors, 0);
        if tier == Tiering::Native {
            assert!(vm.tier_stats.native_compilations > 0);
        }
    }
}

#[test]
fn uncommon_exception_details_do_not_expand_every_runtime_result() {
    assert!(std::mem::size_of::<Error>() <= 32);
    assert!(std::mem::size_of::<tis::Result<tis::Value>>() <= 40);
}
