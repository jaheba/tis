use tis::{Tiering, Vm, compile};
#[test]
fn decimal_context_precision_scale_and_factorial() {
    for tier in [
        Tiering::Bytecode,
        Tiering::Lir,
        Tiering::Native,
        Tiering::CopyPatch,
    ] {
        let mut vm = Vm::new();
        vm.tiering = tier;
        vm.run_program(
            &mut compile(
                r#"
import decimal
assert decimal.getcontext().prec == 28
with decimal.localcontext() as ctx:
    ctx.prec=3
    assert decimal.Decimal('1.245') + 0 == decimal.Decimal('1.24')
    assert decimal.Decimal('1.255') + 0 == decimal.Decimal('1.26')
    assert decimal.Decimal('-7') // 3 == decimal.Decimal('-2')
    with decimal.localcontext() as inner:
        inner.prec=2
        assert decimal.Decimal('123') + 0 == decimal.Decimal('120')
    assert decimal.getcontext() is ctx
assert decimal.getcontext().prec == 28
assert decimal.Decimal('1.20').as_tuple().digits == (1,2,0)
"#,
            )
            .unwrap(),
        )
        .unwrap();
        vm.run_program(&mut compile("with decimal.localcontext() as ctx:\n    ctx.prec=3000\n    result=decimal.Decimal(1)\n    for i in range(1,1001):\n        result=result*i\n    assert len(result.as_tuple().digits)==2568\n").unwrap()).unwrap();
        vm.collect_garbage(&[]).unwrap();
    }
}
