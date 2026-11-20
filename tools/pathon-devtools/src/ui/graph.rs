use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::canvas::{Canvas, Line as CanvasLine};
use ratatui::widgets::{Block, Clear, Paragraph};

use crate::protocol::ViewSnapshot;

pub(crate) const NODE_WIDTH: u16 = 22;
pub(crate) const NODE_HEIGHT: u16 = 4;
const COLUMN_GAP: u16 = 6;
const ROW_GAP: u16 = 1;

pub(crate) const STAGE_COLORS: [Color; 10] = [
    Color::Gray,
    Color::Blue,
    Color::Cyan,
    Color::Green,
    Color::LightBlue,
    Color::LightCyan,
    Color::Magenta,
    Color::LightMagenta,
    Color::LightYellow,
    Color::LightRed,
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Node {
    pub pass: usize,
    pub column: usize,
    pub row: usize,
}

// Columns are dependency depth (a pass sits right of everything it depends on); culled passes get a column of
// their own at the end when shown.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct GraphLayout {
    pub nodes: Vec<Node>,
    pub columns: usize,
    pub culled_column: Option<usize>,
}

pub fn layout(view: &ViewSnapshot, show_culled: bool) -> GraphLayout {
    let mut live: Vec<usize> = (0..view.passes.len())
        .filter(|pass| view.passes[*pass].order.is_some())
        .collect();
    live.sort_by_key(|pass| view.passes[*pass].order);
    let mut depth = vec![0_usize; view.passes.len()];
    for pass in &live {
        for (from, to) in &view.edges {
            if to == pass {
                depth[*pass] = depth[*pass].max(depth[*from] + 1);
            }
        }
    }
    let columns = live.iter().map(|pass| depth[*pass] + 1).max().unwrap_or(0);
    let mut nodes = Vec::with_capacity(view.passes.len());
    for column in 0..columns {
        let in_column = live.iter().filter(|pass| depth[**pass] == column);
        for (row, pass) in in_column.enumerate() {
            nodes.push(Node {
                pass: *pass,
                column,
                row,
            });
        }
    }
    let mut culled: Vec<usize> = (0..view.passes.len())
        .filter(|pass| view.passes[*pass].order.is_none())
        .collect();
    let culled_column = (show_culled && !culled.is_empty()).then_some(columns);
    if let Some(column) = culled_column {
        culled.sort_by(|a, b| {
            let (a, b) = (&view.passes[*a], &view.passes[*b]);
            (a.stage_rank, &a.name).cmp(&(b.stage_rank, &b.name))
        });
        for (row, pass) in culled.into_iter().enumerate() {
            nodes.push(Node { pass, column, row });
        }
    }
    GraphLayout {
        nodes,
        columns: columns + usize::from(culled_column.is_some()),
        culled_column,
    }
}

impl GraphLayout {
    pub fn node_of(&self, pass: usize) -> Option<Node> {
        self.nodes.iter().copied().find(|node| node.pass == pass)
    }

    pub fn first(&self) -> Option<Node> {
        self.nodes.first().copied()
    }

    // The node in `column` whose row is closest to `row`.
    pub fn nearest(&self, column: usize, row: usize) -> Option<Node> {
        self.nodes
            .iter()
            .copied()
            .filter(|node| node.column == column)
            .min_by_key(|node| node.row.abs_diff(row))
    }

    fn origin(node: Node) -> (u16, u16) {
        let column = u16::try_from(node.column).unwrap_or(u16::MAX);
        let row = u16::try_from(node.row).unwrap_or(u16::MAX);
        (
            column.saturating_mul(NODE_WIDTH + COLUMN_GAP),
            row.saturating_mul(NODE_HEIGHT + ROW_GAP),
        )
    }

    fn extent(&self) -> (u16, u16) {
        let rows = self.nodes.iter().map(|node| node.row + 1).max().unwrap_or(0);
        let width = u16::try_from(self.columns)
            .unwrap_or(u16::MAX)
            .saturating_mul(NODE_WIDTH + COLUMN_GAP);
        let height = u16::try_from(rows)
            .unwrap_or(u16::MAX)
            .saturating_mul(NODE_HEIGHT + ROW_GAP);
        (width, height)
    }
}

fn scroll_for(selected: Option<Node>, extent: (u16, u16), area: Rect) -> (u16, u16) {
    let Some(node) = selected else {
        return (0, 0);
    };
    let (x, y) = GraphLayout::origin(node);
    let centre_x = (x + NODE_WIDTH / 2).saturating_sub(area.width / 2);
    let centre_y = (y + NODE_HEIGHT / 2).saturating_sub(area.height / 2);
    (
        centre_x.min(extent.0.saturating_sub(area.width)),
        centre_y.min(extent.1.saturating_sub(area.height)),
    )
}

pub(crate) fn draw_graph(
    frame: &mut Frame<'_>,
    area: Rect,
    view: &ViewSnapshot,
    selected: Option<usize>,
    show_culled: bool,
) {
    let graph = layout(view, show_culled);
    let inner = Block::bordered().inner(area);
    let selected_node = selected.and_then(|pass| graph.node_of(pass));
    let extent = graph.extent();
    let (scroll_x, scroll_y) = scroll_for(selected_node, extent, inner);
    let more_left = scroll_x > 0;
    let more_right = extent.0.saturating_sub(COLUMN_GAP) > scroll_x + inner.width;

    let culled_count = view.passes.iter().filter(|pass| pass.order.is_none()).count();
    let title = format!(
        " {}graf klatki · widok {} · {} live / {} wyciętych{}{} ",
        if more_left { "◀ " } else { "" },
        view.index,
        view.stats.passes_live,
        culled_count,
        if show_culled { "" } else { " (c: pokaż)" },
        if more_right { " ▶" } else { "" }
    );
    let block = Block::bordered()
        .title(title)
        .border_style(Style::default().fg(Color::DarkGray));
    frame.render_widget(block, area);
    let to_screen = |node: Node| -> (i32, i32) {
        let (x, y) = GraphLayout::origin(node);
        (i32::from(x) - i32::from(scroll_x), i32::from(y) - i32::from(scroll_y))
    };

    let height = f64::from(inner.height);
    let mut edges: Vec<(bool, [(f64, f64); 4])> = Vec::new();
    for (from, to) in &view.edges {
        let (Some(source), Some(target)) = (graph.node_of(*from), graph.node_of(*to)) else {
            continue;
        };
        let (sx, sy) = to_screen(source);
        let (tx, ty) = to_screen(target);
        let start = (
            f64::from(sx) + f64::from(NODE_WIDTH),
            height - f64::from(sy) - f64::from(NODE_HEIGHT) / 2.0,
        );
        let end = (f64::from(tx), height - f64::from(ty) - f64::from(NODE_HEIGHT) / 2.0);
        let bend = start.0 + f64::from(COLUMN_GAP) / 2.0;
        let touches_selection = selected == Some(*from) || selected == Some(*to);
        edges.push((touches_selection, [start, (bend, start.1), (bend, end.1), end]));
    }
    edges.sort_by_key(|(highlighted, _)| *highlighted);
    let canvas = Canvas::default()
        .marker(Marker::Braille)
        .x_bounds([0.0, f64::from(inner.width)])
        .y_bounds([0.0, height])
        .paint(|ctx| {
            for (highlighted, points) in &edges {
                let color = if *highlighted { Color::Yellow } else { Color::DarkGray };
                for segment in points.windows(2) {
                    ctx.draw(&CanvasLine {
                        x1: segment[0].0,
                        y1: segment[0].1,
                        x2: segment[1].0,
                        y2: segment[1].1,
                        color,
                    });
                }
            }
        });
    frame.render_widget(canvas, inner);

    for node in &graph.nodes {
        let (x, y) = to_screen(*node);
        let fits = x >= 0
            && y >= 0
            && x + i32::from(NODE_WIDTH) <= i32::from(inner.width)
            && y + i32::from(NODE_HEIGHT) <= i32::from(inner.height);
        if !fits {
            continue;
        }
        let rect = Rect::new(
            inner.x + u16::try_from(x).unwrap_or(0),
            inner.y + u16::try_from(y).unwrap_or(0),
            NODE_WIDTH,
            NODE_HEIGHT,
        );
        let pass = &view.passes[node.pass];
        let is_selected = selected == Some(node.pass);
        let stage_color = STAGE_COLORS[usize::from(pass.stage_rank) % STAGE_COLORS.len()];
        let border = match (is_selected, pass.order) {
            (true, _) => Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            (false, None) => Style::default().fg(Color::DarkGray),
            (false, Some(_)) => Style::default().fg(stage_color),
        };
        let name_style = if pass.order.is_none() {
            Style::default().fg(Color::DarkGray).add_modifier(Modifier::CROSSED_OUT)
        } else {
            Style::default().add_modifier(Modifier::BOLD)
        };
        let detail = match pass.order {
            Some(order) => format!("{} · #{order}", pass.stage),
            None => format!("{} · wycięty", pass.stage),
        };
        let marker = if pass.side_effect { " ◆" } else { "" };
        let lines = vec![
            Line::from(vec![
                Span::styled(truncate(&pass.name, usize::from(NODE_WIDTH) - 4), name_style),
                Span::raw(marker),
            ]),
            Line::from(Span::styled(
                truncate(&detail, usize::from(NODE_WIDTH) - 2),
                Style::default().fg(Color::Gray),
            )),
        ];
        frame.render_widget(Clear, rect);
        frame.render_widget(
            Paragraph::new(lines).block(Block::bordered().border_style(border)),
            rect,
        );
    }
}

pub(crate) fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_owned();
    }
    let mut short: String = text.chars().take(width.saturating_sub(1)).collect();
    short.push('…');
    short
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::PassSnapshot;

    fn pass(name: &str, order: Option<usize>) -> PassSnapshot {
        PassSnapshot {
            name: name.into(),
            order,
            ..PassSnapshot::default()
        }
    }

    #[test]
    fn columns_follow_dependency_depth_and_culled_passes_go_last() {
        let view = ViewSnapshot {
            passes: vec![
                pass("a", Some(0)),
                pass("b", Some(1)),
                pass("c", Some(2)),
                pass("gone", None),
            ],
            edges: vec![(0, 1), (1, 2), (0, 2)],
            ..ViewSnapshot::default()
        };
        let graph = layout(&view, true);

        assert_eq!(graph.node_of(0).map(|node| node.column), Some(0));
        assert_eq!(graph.node_of(1).map(|node| node.column), Some(1));
        assert_eq!(graph.node_of(2).map(|node| node.column), Some(2));
        assert_eq!(graph.node_of(3).map(|node| node.column), Some(3));
        assert_eq!(graph.culled_column, Some(3));
        assert_eq!(layout(&view, false).node_of(3), None);
    }

    #[test]
    fn independent_passes_share_a_column() {
        let view = ViewSnapshot {
            passes: vec![pass("a", Some(0)), pass("b", Some(1)), pass("c", Some(2))],
            edges: vec![(0, 2), (1, 2)],
            ..ViewSnapshot::default()
        };
        let graph = layout(&view, false);
        assert_eq!(
            graph.node_of(1),
            Some(Node {
                pass: 1,
                column: 0,
                row: 1
            })
        );
        assert_eq!(graph.nearest(0, 5).map(|node| node.pass), Some(1));
    }
}
