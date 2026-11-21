use std::fmt::Write;

use crate::capture::GraphCapture;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DotPass<'a> {
    pub name: &'a str,
    pub stage: &'a str,
    // Position in the executed order; `None` draws the pass as culled.
    pub order: Option<usize>,
}

pub fn to_dot(capture: &GraphCapture) -> String {
    let stages: Vec<String> = capture.passes.iter().map(|pass| format!("{:?}", pass.stage)).collect();
    let passes: Vec<DotPass<'_>> = capture
        .passes
        .iter()
        .zip(&stages)
        .map(|(pass, stage)| DotPass {
            name: pass.name,
            stage,
            order: pass.order,
        })
        .collect();
    render_dot(&passes, &capture.edges)
}

pub fn render_dot(passes: &[DotPass<'_>], edges: &[(usize, usize)]) -> String {
    let mut dot = String::from("digraph frame_graph {\n    rankdir=LR;\n    node [shape=box];\n");
    for (index, pass) in passes.iter().enumerate() {
        let name = escape(pass.name);
        let stage = escape(pass.stage);
        let _ = match pass.order {
            Some(order) => writeln!(dot, "    p{index} [label=\"{name}\\n{stage} #{order}\"];"),
            None => writeln!(
                dot,
                "    p{index} [label=\"{name}\\n{stage}\" style=\"dashed,filled\" fillcolor=lightgrey fontcolor=grey40];"
            ),
        };
    }
    for (from, to) in edges {
        let _ = writeln!(dot, "    p{from} -> p{to};");
    }
    dot.push_str("}\n");
    dot
}

fn escape(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n")
}
