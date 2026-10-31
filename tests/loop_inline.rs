use tis::{
    Object, Tiering, Value, Vm, compile,
    lir::{Helper, LoweringMode, Operation},
};

#[test]
fn closed_helpers_keep_float_accumulators_and_recheck_each_loop_entry() {
    for tier in [Tiering::CopyPatch, Tiering::Native] {
        let mut vm = Vm::new();
        vm.tiering = tier;
        vm.thresholds.function_entries = 12;
        vm.thresholds.loop_backedges = 16;
        vm.profiling.window_ticks = 128;
        vm.run_program(
            &mut compile(
                r#"
def norm(x,y,z):
    return x*x+y*y+z*z
switch=False
events=[]
def replacement(x,y,z):
    events.append(1)
    return x+y+z+100.0
def finish_round():
    global norm
    if switch:
        norm=replacement
def batch(points):
    total=0.0
    for round in range(2):
        for point in points:
            x,y,z=point
            total+=norm(x,y,z)
        finish_round()
    return total
points=[[i*0.125,1.0,2.0] for i in range(64)]
for repeat in range(80):
    batch(points)
"#,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(vm.tier_stats.compilation_errors, 0);
        let batch = vm.get_global("batch").unwrap();
        let points = vm.get_global("points").unwrap();
        let expected = (0..64)
            .map(|i| {
                let x = i as f64 * 0.125;
                x * x + 5.0
            })
            .sum::<f64>();
        let before = vm.tier_stats.native_runtime_calls;
        assert_eq!(
            vm.call(batch, &[points]).unwrap(),
            Value::Float(2.0 * expected)
        );
        assert!(
            vm.tier_stats.native_runtime_calls - before < 40,
            "{tier:?}: {:?}",
            vm.tier_stats
        );
        let hir = vm.function_hir(batch).unwrap();
        let mut lir = hir.lower_lir(LoweringMode::Profiled).unwrap();
        let mut bad_sites = lir.clone();
        bad_sites
            .regions
            .iter_mut()
            .find(|region| !region.inline_sites.is_empty())
            .unwrap()
            .inline_sites
            .clear();
        assert!(
            bad_sites.verify().is_err(),
            "guard site lists must match the loop body"
        );
        let mut bad_arity = lir.clone();
        let call = bad_arity
            .blocks
            .iter_mut()
            .flat_map(|b| &mut b.instructions)
            .find(|i| {
                matches!(
                    i.operation,
                    Operation::Call {
                        helper: Helper::InlineCall { .. },
                        ..
                    }
                )
            })
            .unwrap();
        if let Operation::Call {
            helper: Helper::InlineCall { names, .. },
            arguments,
            ..
        } = &mut call.operation
        {
            names.pop();
            arguments.pop();
        }
        assert!(
            bad_arity.verify().is_err(),
            "copied argument indices require the actual signature"
        );
        let region = lir
            .regions
            .iter_mut()
            .find(|r| {
                lir.blocks[r.body.0].instructions.iter().any(|i| {
                    matches!(
                        i.operation,
                        Operation::Call {
                            helper: Helper::InlineCall { .. },
                            ..
                        }
                    )
                })
            })
            .expect("closed helper region");
        assert!(region.schema.take().is_some());
        assert!(
            lir.verify().is_err(),
            "closed calls cannot omit entry type proofs"
        );
        vm.run_program(&mut compile("switch=True\n").unwrap())
            .unwrap();
        assert_eq!(
            vm.call(batch, &[points]).unwrap(),
            Value::Float(expected + 6844.0)
        );
        assert_eq!(vm.heap.len(vm.get_global("events").unwrap()).unwrap(), 64);
        vm.run_program(&mut compile("del norm\n").unwrap()).unwrap();
        let empty = vm.heap.allocate(Object::List(vec![].into()));
        assert_eq!(vm.call(batch, &[empty]).unwrap(), Value::Float(0.0));
        assert_eq!(vm.stack_len(), 0);
    }
}

#[test]
fn non_float_arguments_keep_python_effects_and_rebinding_order() {
    for tier in [Tiering::CopyPatch, Tiering::Native] {
        let mut vm = Vm::new();
        vm.tiering = tier;
        vm.thresholds.function_entries = 12;
        vm.thresholds.loop_backedges = 16;
        vm.run_program(
            &mut compile(
                r#"
def norm(x,y,z):
    return x*x+y*y+z*z
def batch(points):
    total=0.0
    for point in points:
        x,y,z=point
        total+=norm(x,y,z)
    return total
good=[[1.0,2.0,3.0] for i in range(64)]
for repeat in range(80):
    batch(good)
events=[]
def replacement(x,y,z):
    events.append('replacement')
    return 100.0
class Custom:
    def __mul__(self,other):
        global norm
        events.append('mul')
        for i in range(2000):
            trash=[i]
        norm=replacement
        return 1.0
bad=[[Custom(),1.0,2.0],[2.0,1.0,2.0]]
assert batch(bad)==106.0
assert events==['mul','replacement']
"#,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(vm.tier_stats.compilation_errors, 0);
        assert_eq!(vm.stack_len(), 0);
    }
}
