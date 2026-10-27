use tis::{
    Object, Value, Vm,
    hir::{Function, Op, Terminator},
};

fn main() -> tis::Result<()> {
    let mut function = Function::new(2);
    let entry = function.entry();
    let lhs = function.parameters(entry)[0];
    let rhs = function.parameters(entry)[1];
    let sum = function.append(entry, Op::Add(lhs, rhs));
    function.terminate(entry, Terminator::Return(sum));

    println!("Before execution:\n{function}");
    let mut vm = Vm::new();
    for value in 0..16 {
        vm.run(&mut function, &[Value::Int(value), Value::Int(1)])?;
    }
    println!("After integer feedback:\n{function}");
    println!("Feedback: {:?}", function.feedback(sum).unwrap());

    let lhs = vm.heap.allocate(Object::String("hello ".into()));
    let rhs = vm.heap.allocate(Object::String("tis".into()));
    let result = vm.run(&mut function, &[lhs, rhs])?;
    println!("Generic fallback: {:?}", vm.heap.get(result)?);
    println!("After guard failure:\n{function}");
    Ok(())
}
