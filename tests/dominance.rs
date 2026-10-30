use std::collections::BTreeSet;
use tis::hir::{BlockId, Constant, Edge, Function, Op, Terminator};

#[test]
fn dominators_cross_word_boundaries_and_keep_unreachable_blocks() {
    let mut function = Function::new(0);
    for _ in 1..132 {
        function.block(0);
    }
    let condition = function.append(BlockId(0), Op::Constant(Constant::Bool(true)));
    for block in 0..129 {
        function.terminate(
            BlockId(block),
            Terminator::Jump(Edge::new(BlockId(block + 1), vec![])),
        );
    }
    function.terminate(
        BlockId(129),
        Terminator::Branch {
            condition,
            yes: Edge::new(BlockId(65), vec![]),
            no: Edge::new(BlockId(130), vec![]),
        },
    );
    function.terminate(BlockId(130), Terminator::Return(condition));
    let dead = function.append(BlockId(131), Op::Constant(Constant::None));
    function.terminate(BlockId(131), Terminator::Return(dead));
    let analysis = function.analyze().unwrap();
    for block in 0..131 {
        assert_eq!(
            analysis.dominators[block],
            (0..=block).collect::<BTreeSet<_>>()
        );
    }
    assert_eq!(analysis.dominators[131], (0..131).collect());
    assert!(!analysis.reachable[131]);
    assert_eq!(analysis.backedges, BTreeSet::from([(129, 65)]));
}
