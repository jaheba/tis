use std::fmt::Write;

use super::{AddStrategy, Block, Edge, Function, Op, Terminator, ValueId};

impl Function {
    /// Textual IR with the runtime counters attached to each addition site.
    pub fn dump(&self) -> String {
        self.blocks
            .iter()
            .enumerate()
            .map(|(id, block)| block_lines(id, block).join("\n") + "\n")
            .collect()
    }

    /// Graphviz control-flow graph. Edge labels show block-parameter transfers.
    pub fn to_dot(&self) -> String {
        let mut output = String::from(
            "digraph ir {\n  graph [rankdir=TB];\n  node [shape=box, fontname=monospace];\n  edge [fontname=monospace];\n",
        );
        for (id, block) in self.blocks.iter().enumerate() {
            let label = block_lines(id, block)
                .iter()
                .map(|line| dot_escape(line) + "\\l")
                .collect::<String>();
            let color = if block.instructions.iter().any(|instruction| {
                instruction
                    .feedback
                    .as_ref()
                    .is_some_and(|feedback| feedback.strategy == AddStrategy::Integer)
            }) {
                "#16836c"
            } else {
                "#52617a"
            };
            writeln!(output, "  b{id} [label=\"{label}\", color=\"{color}\"];").unwrap();
            for (kind, edge) in edges(block) {
                writeln!(
                    output,
                    "  b{id} -> b{} [label=\"{}\"];",
                    edge.block.0,
                    dot_escape(&edge_label(kind, edge, &self.blocks))
                )
                .unwrap();
            }
        }
        output.push_str("}\n");
        output
    }

    /// A standalone HTML viewer; no server, package installation or network access is needed.
    pub fn to_html(&self) -> String {
        let mut nodes = Vec::new();
        let mut links = Vec::new();
        for (id, block) in self.blocks.iter().enumerate() {
            let instructions = block.instructions.iter().map(|instruction| {
                // Strings preserve u64 counters beyond JavaScript's exact integer range.
                let feedback = match &instruction.feedback {
                    Some(feedback) => format!("{{\"strategy\":{},\"executions\":{},\"fast_paths\":{},\"guard_failures\":{}}}", json(&format!("{:?}", feedback.strategy)), json(&feedback.executions.to_string()), json(&feedback.fast_paths.to_string()), json(&feedback.guard_failures.to_string())),
                    None => "null".into(),
                };
                let inputs = instruction.op.inputs().iter().map(|value| value.0.to_string()).collect::<Vec<_>>().join(",");
                format!("{{\"id\":{},\"text\":{},\"inputs\":[{}],\"feedback\":{}}}", instruction.result.0, json(&instruction.to_string()), inputs, feedback)
            }).collect::<Vec<_>>().join(",");
            let parameters = block
                .parameters
                .iter()
                .map(|value| value.0.to_string())
                .collect::<Vec<_>>()
                .join(",");
            nodes.push(format!("{{\"id\":{id},\"header\":{},\"parameters\":[{}],\"instructions\":[{}],\"terminator\":{},\"terminated\":{}}}", json(&header(id, block)), parameters, instructions, json(&terminator(block)), block.terminator.is_some()));
            for (kind, edge) in edges(block) {
                links.push(format!(
                    "{{\"from\":{id},\"to\":{},\"kind\":{},\"label\":{}}}",
                    edge.block.0,
                    json(kind),
                    json(&edge_label(kind, edge, &self.blocks))
                ));
            }
        }
        let data = format!(
            "{{\"nodes\":[{}],\"edges\":[{}],\"dot\":{}}}",
            nodes.join(","),
            links.join(","),
            json(&self.to_dot())
        );
        include_str!("viewer.html")
            .replace("__TIS_IR_STYLE__", include_str!("viewer.css"))
            .replace("__TIS_IR_SCRIPT__", include_str!("viewer.js"))
            .replace("__TIS_IR_DATA__", &data)
    }
}

impl Op {
    /// Operand IDs in evaluation order, including the inputs of effectful operations.
    pub fn inputs(&self) -> Vec<ValueId> {
        match self {
            Self::Constant(_) => Vec::new(),
            Self::Add(lhs, rhs)
            | Self::Subtract(lhs, rhs)
            | Self::Less(lhs, rhs)
            | Self::GetItem(lhs, rhs) => vec![*lhs, *rhs],
            Self::List(values) | Self::Tuple(values) => values.clone(),
            Self::Dict(entries) => entries
                .iter()
                .flat_map(|&(key, value)| [key, value])
                .collect(),
            Self::SetItem { object, key, value } => vec![*object, *key, *value],
            Self::Len(object) => vec![*object],
            Self::Runtime(_, values) => values.clone(),
        }
    }
}

fn block_lines(id: usize, block: &Block) -> Vec<String> {
    let mut lines = vec![header(id, block) + ":"];
    for instruction in &block.instructions {
        lines.push(format!("    {instruction}"));
        if let Some(feedback) = &instruction.feedback {
            lines.push(format!(
                "      ; executions={} fast_paths={} guard_failures={}",
                feedback.executions, feedback.fast_paths, feedback.guard_failures
            ));
        }
    }
    lines.push(format!("    {}", terminator(block)));
    lines
}

fn header(id: usize, block: &Block) -> String {
    format!(
        "b{id}({})",
        block
            .parameters
            .iter()
            .map(|value| format!("%{}", value.0))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn terminator(block: &Block) -> String {
    block
        .terminator
        .as_ref()
        .map_or_else(|| "<unterminated>".into(), ToString::to_string)
}

fn edges(block: &Block) -> Vec<(&'static str, &Edge)> {
    match &block.terminator {
        Some(Terminator::Jump(edge)) => vec![("jump", edge)],
        Some(Terminator::Branch { yes, no, .. }) => vec![("true", yes), ("false", no)],
        _ => Vec::new(),
    }
}

fn edge_label(kind: &str, edge: &Edge, blocks: &[Block]) -> String {
    let parameters = blocks
        .get(edge.block.0)
        .map(|block| block.parameters.as_slice())
        .unwrap_or_default();
    let arguments = edge
        .arguments
        .iter()
        .enumerate()
        .map(|(index, argument)| match parameters.get(index) {
            Some(parameter) => format!("%{} → %{}", argument.0, parameter.0),
            None => format!("%{} → ?", argument.0),
        })
        .collect::<Vec<_>>()
        .join(", ");
    if arguments.is_empty() {
        kind.into()
    } else {
        format!("{kind}: {arguments}")
    }
}

fn dot_escape(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

fn json(text: &str) -> String {
    let mut output = String::from("\"");
    for character in text.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            '\n' => output.push_str("\\n"),
            '\r' => output.push_str("\\r"),
            '\t' => output.push_str("\\t"),
            '<' | '>' | '&' | '\u{2028}' | '\u{2029}' => {
                write!(output, "\\u{:04x}", u32::from(character)).unwrap()
            }
            character if character.is_control() => {
                write!(output, "\\u{:04x}", u32::from(character)).unwrap()
            }
            character => output.push(character),
        }
    }
    output.push('"');
    output
}
