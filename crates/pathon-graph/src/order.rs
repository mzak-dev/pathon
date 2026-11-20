use std::cmp::Reverse;
use std::collections::BinaryHeap;

use pathon_diag::Diagnostic;

use crate::graph::Stage;

#[derive(Clone, Copy, Debug)]
pub struct OrderNode<'desc> {
    pub name: &'static str,
    pub stage: Stage,
    pub priority: i32,
    pub before: &'desc [&'static str],
    pub after: &'desc [&'static str],
    pub included: bool,
}

// Topological order over hard edges (data dependencies, before/after); among ready nodes the lowest
// (stage, priority) goes first and registration index breaks remaining ties with a G0006 warning.
// Shared by frame passes and by the renderer when it orders render features.
pub fn sort_nodes(
    nodes: &[OrderNode<'_>],
    data_edges: &[(usize, usize)],
) -> Result<(Vec<usize>, Vec<Diagnostic>), Vec<Diagnostic>> {
    let count = nodes.len();
    let mut warnings = Vec::new();
    let mut successors: Vec<Vec<usize>> = vec![Vec::new(); count];
    let mut add_edge = |from: usize, to: usize| {
        if from != to && nodes[from].included && nodes[to].included && !successors[from].contains(&to) {
            successors[from].push(to);
        }
    };
    for (from, to) in data_edges {
        add_edge(*from, *to);
    }
    for (index, node) in nodes.iter().enumerate() {
        for name in node.after {
            let others = named(nodes, name);
            if others.is_empty() {
                warnings.push(unknown_node(node.name, name));
            }
            for other in others {
                add_edge(other, index);
            }
        }
        for name in node.before {
            let others = named(nodes, name);
            if others.is_empty() {
                warnings.push(unknown_node(node.name, name));
            }
            for other in others {
                add_edge(index, other);
            }
        }
    }

    let mut in_degree = vec![0_usize; count];
    for targets in &successors {
        for target in targets {
            in_degree[*target] += 1;
        }
    }
    let mut ready: BinaryHeap<Reverse<(Stage, i32, usize)>> = BinaryHeap::new();
    for (index, node) in nodes.iter().enumerate() {
        if node.included && in_degree[index] == 0 {
            ready.push(Reverse((node.stage, node.priority, index)));
        }
    }
    let mut order = Vec::with_capacity(count);
    while let Some(Reverse((stage, priority, index))) = ready.pop() {
        if let Some(Reverse((next_stage, next_priority, next))) = ready.peek()
            && (*next_stage, *next_priority) == (stage, priority)
        {
            warnings.push(
                Diagnostic::warning(
                    "G0006",
                    format!(
                        "'{}' and '{}' have no ordering constraint; registration order decides",
                        nodes[index].name, nodes[*next].name
                    ),
                )
                .with_hint("add before/after or a priority if their order matters"),
            );
        }
        order.push(index);
        for target in &successors[index] {
            in_degree[*target] -= 1;
            if in_degree[*target] == 0 {
                let node = &nodes[*target];
                ready.push(Reverse((node.stage, node.priority, *target)));
            }
        }
    }

    let included = nodes.iter().filter(|node| node.included).count();
    if order.len() == included {
        return Ok((order, warnings));
    }
    let stuck: Vec<&str> = (0..count)
        .filter(|index| nodes[*index].included && !order.contains(index))
        .map(|index| nodes[index].name)
        .collect();
    Err(vec![
        Diagnostic::error("G0001", format!("{stuck:?} form a cycle"))
            .with_hint("remove a contradicting before/after constraint or split the shared resource"),
    ])
}

fn named(nodes: &[OrderNode<'_>], name: &str) -> Vec<usize> {
    nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| node.name == name)
        .map(|(index, _)| index)
        .collect()
}

fn unknown_node(node: &str, name: &str) -> Diagnostic {
    Diagnostic::warning("G0002", format!("'{node}' orders itself against unknown '{name}'"))
        .with_hint("check the name; constraints on passes that do not exist are ignored")
}
