use tis::hir::{Constant, Edge, Function, Op, Terminator};

pub fn addition() -> Function {
    let mut function = Function::new(2);
    let entry = function.entry();
    let lhs = function.parameters(entry)[0];
    let rhs = function.parameters(entry)[1];
    let sum = function.append(entry, Op::Add(lhs, rhs));
    function.terminate(entry, Terminator::Return(sum));
    function
}

pub fn sum() -> Function {
    let mut function = Function::new(1);
    let entry = function.entry();
    let input = function.parameters(entry)[0];
    let zero = function.append(entry, Op::Constant(Constant::Int(0)));
    let one = function.append(entry, Op::Constant(Constant::Int(1)));
    let head = function.block(2);
    let n = function.parameters(head)[0];
    let sum = function.parameters(head)[1];
    let body = function.block(0);
    let exit = function.block(1);
    let result = function.parameters(exit)[0];
    function.terminate(entry, Terminator::Jump(Edge::new(head, vec![input, zero])));
    let condition = function.append(head, Op::Less(zero, n));
    function.terminate(
        head,
        Terminator::Branch {
            condition,
            yes: Edge::new(body, vec![]),
            no: Edge::new(exit, vec![sum]),
        },
    );
    let addition = function.append(body, Op::Add(sum, n));
    let next = function.append(body, Op::Subtract(n, one));
    function.terminate(
        body,
        Terminator::Jump(Edge::new(head, vec![next, addition])),
    );
    function.terminate(exit, Terminator::Return(result));
    function
}
