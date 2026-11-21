use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, List, ListItem, Paragraph, Row, Sparkline, Table, TableState, Wrap};

use super::graph::truncate;
use super::{App, DiagnosticEntry};
use crate::protocol::{AccessSnapshot, FeatureSnapshot, ViewSnapshot};

const ALIAS_COLORS: [Color; 8] = [
    Color::Cyan,
    Color::Green,
    Color::Magenta,
    Color::Yellow,
    Color::LightBlue,
    Color::LightRed,
    Color::LightGreen,
    Color::LightMagenta,
];

fn dim() -> Style {
    Style::default().fg(Color::DarkGray)
}

fn heading(text: &str) -> Line<'static> {
    Line::from(Span::styled(
        text.to_owned(),
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
    ))
}

pub(super) fn draw_header(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let [text_area, spark_area] = Layout::horizontal([Constraint::Min(40), Constraint::Length(32)]).areas(area);
    let (dot, dot_style) = if app.connected {
        ("●", Style::default().fg(Color::Green))
    } else {
        ("○", Style::default().fg(Color::Red))
    };
    let mut spans = vec![
        Span::styled(
            " pathon devtools ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(dot, dot_style),
        Span::raw(format!(" {} ", app.source)),
    ];
    if let Some(snapshot) = &app.snapshot {
        spans.push(Span::styled(
            format!("│ klatka {} │ {:.2} ms ", snapshot.frame, snapshot.frame_ms),
            dim(),
        ));
        if let Some(view) = app.current_view() {
            spans.push(Span::styled(
                format!(
                    "│ widok {}/{} │ passy {}/{} │ tekstury {}→{} ",
                    view.index + 1,
                    snapshot.views.len(),
                    view.stats.passes_live,
                    view.stats.passes_total,
                    view.stats.logical_textures,
                    view.stats.physical_textures
                ),
                dim(),
            ));
        }
    } else {
        spans.push(Span::styled("│ czekam na pierwszą klatkę…", dim()));
    }
    if app.paused {
        spans.push(Span::styled(
            " PAUZA ",
            Style::default().fg(Color::Black).bg(Color::LightRed),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), text_area);
    let data: Vec<u64> = app.frame_times.iter().copied().collect();
    frame.render_widget(
        Sparkline::default()
            .data(&data)
            .style(Style::default().fg(Color::Green)),
        spark_area,
    );
}

pub(super) fn draw_footer(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let keys = "←↑↓→ wybór · Tab/1-4 zakładki · c wycięte · v widok · p pauza · s zapisz · d zapis DOT · q wyjście";
    let mut spans = vec![Span::styled(keys, dim())];
    if let Some(message) = &app.message {
        spans.push(Span::styled(format!("   {message}"), Style::default().fg(Color::Green)));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn access_line(view: &ViewSnapshot, access: &AccessSnapshot) -> Line<'static> {
    let kind = match (access.read_version, access.write_version) {
        (Some(read), Some(write)) => format!("RW v{read}→v{write}"),
        (Some(read), None) => format!("R  v{read}"),
        (None, Some(write)) => format!("W  →v{write}"),
        (None, None) => "—".to_owned(),
    };
    let name = view
        .resources
        .get(access.resource)
        .map_or("?", |resource| resource.name.as_str());
    Line::from(vec![
        Span::styled(format!("  {kind:<10} "), Style::default().fg(Color::Cyan)),
        Span::styled(name.to_owned(), Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(format!("  {} · {}", access.access.join(" | "), access.range), dim()),
    ])
}

fn names(view: &ViewSnapshot, passes: impl Iterator<Item = usize>) -> String {
    let names: Vec<&str> = passes
        .filter_map(|pass| view.passes.get(pass))
        .map(|pass| pass.name.as_str())
        .collect();
    if names.is_empty() {
        "—".to_owned()
    } else {
        names.join(", ")
    }
}

pub(super) fn draw_inspector(frame: &mut Frame<'_>, area: Rect, view: &ViewSnapshot, selected: Option<usize>) {
    let block = Block::bordered().title(" pass ").border_style(dim());
    let Some(pass) = selected.and_then(|index| view.passes.get(index)) else {
        frame.render_widget(
            Paragraph::new("wybierz pass strzałkami").style(dim()).block(block),
            area,
        );
        return;
    };
    let index = selected.unwrap_or_default();
    let order = pass.order.map_or_else(
        || "wycięty (nikt nie czyta wyniku)".to_owned(),
        |order| format!("#{order}"),
    );
    let mut lines = vec![
        Line::from(Span::styled(
            pass.name.clone(),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::styled("etap ", dim()),
            Span::raw(pass.stage.clone()),
            Span::styled("  priorytet ", dim()),
            Span::raw(pass.priority.to_string()),
            Span::styled("  kolejność ", dim()),
            Span::raw(order),
        ]),
    ];
    if pass.side_effect {
        lines.push(Line::from(Span::styled(
            "◆ side effect: nie podlega cullingowi",
            Style::default().fg(Color::Magenta),
        )));
    }
    lines.push(Line::from(vec![
        Span::styled("zależy od ", dim()),
        Span::raw(names(
            view,
            view.edges.iter().filter(|(_, to)| *to == index).map(|(from, _)| *from),
        )),
    ]));
    lines.push(Line::from(vec![
        Span::styled("odbiorcy  ", dim()),
        Span::raw(names(
            view,
            view.edges.iter().filter(|(from, _)| *from == index).map(|(_, to)| *to),
        )),
    ]));
    lines.push(Line::default());
    lines.push(heading("dostępy"));
    lines.extend(pass.accesses.iter().map(|access| access_line(view, access)));
    lines.push(Line::default());
    lines.push(heading("bariery przed passem"));
    if pass.barriers.is_empty() {
        lines.push(Line::from(Span::styled("  brak — stany już się zgadzają", dim())));
    }
    for barrier in &pass.barriers {
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {}", names_of_resources(view, &barrier.resources)),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(" → ", dim()),
            Span::styled(barrier.access.join(" | "), Style::default().fg(Color::LightMagenta)),
            Span::styled(format!(" · {}", barrier.range), dim()),
        ]));
    }
    frame.render_widget(Paragraph::new(lines).block(block).wrap(Wrap { trim: false }), area);
}

fn names_of_resources(view: &ViewSnapshot, resources: &[usize]) -> String {
    let names: Vec<&str> = resources
        .iter()
        .filter_map(|resource| view.resources.get(*resource))
        .map(|resource| resource.name.as_str())
        .collect();
    if names.is_empty() {
        "(tekstura poza grafem)".to_owned()
    } else {
        names.join(" = ")
    }
}

// One character per executed pass: █ write, ▒ read, ─ alive between uses, · outside the lifetime.
fn timeline(view: &ViewSnapshot, resource: usize, width: usize) -> String {
    let lifetime = view.resources[resource].lifetime;
    let mut cells = vec!['·'; width];
    if let Some((first, last)) = lifetime {
        for cell in cells.iter_mut().take(last.min(width.saturating_sub(1)) + 1).skip(first) {
            *cell = '─';
        }
    }
    for pass in &view.passes {
        let Some(order) = pass.order.filter(|order| *order < width) else {
            continue;
        };
        for access in pass.accesses.iter().filter(|access| access.resource == resource) {
            cells[order] = if access.write_version.is_some() { '█' } else { '▒' };
        }
    }
    cells.into_iter().collect()
}

pub(super) fn draw_resources(frame: &mut Frame<'_>, area: Rect, view: &ViewSnapshot, selected: usize) {
    let [table_area, detail_area] = Layout::vertical([Constraint::Min(6), Constraint::Length(9)]).areas(area);
    let width = view.stats.passes_live.max(1);
    let rows: Vec<Row<'_>> = view
        .resources
        .iter()
        .enumerate()
        .map(|(index, resource)| {
            let color = match resource.alias_group {
                Some(group) => ALIAS_COLORS[group % ALIAS_COLORS.len()],
                None if resource.imported => Color::White,
                None => Color::DarkGray,
            };
            Row::new(vec![
                Cell::from(resource.name.clone()),
                Cell::from(if resource.imported {
                    format!("{} (import)", resource.kind)
                } else {
                    resource.kind.clone()
                }),
                Cell::from(resource.format.clone().unwrap_or_else(|| "—".into())),
                Cell::from(match (resource.bytes, resource.size) {
                    (Some(bytes), _) => format!("{bytes} B"),
                    (None, Some((w, h))) => format!("{w}×{h}"),
                    (None, None) => "—".into(),
                }),
                Cell::from(
                    resource
                        .alias_group
                        .map_or_else(|| "—".into(), |group| format!("#{group}")),
                ),
                Cell::from(Span::styled(timeline(view, index, width), Style::default().fg(color))),
            ])
        })
        .collect();
    let widths = [
        Constraint::Length(18),
        Constraint::Length(16),
        Constraint::Length(16),
        Constraint::Length(11),
        Constraint::Length(7),
        Constraint::Min(10),
    ];
    let header = Row::new(vec![
        "zasób",
        "rodzaj",
        "format",
        "rozmiar",
        "pamięć",
        "czas życia (kolejność passów)",
    ])
    .style(Style::default().fg(Color::Yellow));
    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::bordered()
                .title(" zasoby · kolor = wspólna pamięć fizyczna (aliasing) ")
                .border_style(dim()),
        )
        .row_highlight_style(Style::default().bg(Color::Rgb(40, 40, 60)));
    let mut state = TableState::default().with_selected(Some(selected));
    frame.render_stateful_widget(table, table_area, &mut state);

    let Some(resource) = view.resources.get(selected) else {
        return;
    };
    let mut lines = vec![
        Line::from(vec![
            Span::styled(resource.name.clone(), Style::default().add_modifier(Modifier::BOLD)),
            Span::styled(
                format!(
                    "  {} wersji · {} mipów · {} warstw",
                    resource.versions, resource.mips, resource.layers
                ),
                dim(),
            ),
        ]),
        Line::from(vec![
            Span::styled("użycie ", dim()),
            Span::raw(resource.usage.join(" | ")),
        ]),
    ];
    let mut users: Vec<(usize, &str, &AccessSnapshot)> = Vec::new();
    for pass in &view.passes {
        for access in pass.accesses.iter().filter(|access| access.resource == selected) {
            users.push((pass.order.unwrap_or(usize::MAX), pass.name.as_str(), access));
        }
    }
    users.sort_by_key(|(order, _, _)| *order);
    for (order, name, access) in users {
        let order = if order == usize::MAX {
            "wycięty".to_owned()
        } else {
            format!("#{order}")
        };
        let line = access_line(view, access);
        let mut spans = vec![Span::styled(format!("{order:>8} {name:<20}"), Style::default())];
        spans.extend(line.spans.into_iter().take(1));
        spans.push(Span::styled(format!(" {}", access.access.join(" | ")), dim()));
        lines.push(Line::from(spans));
    }
    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(" użycia ").border_style(dim())),
        detail_area,
    );
}

fn status_style(feature: &FeatureSnapshot) -> Style {
    match feature.status.as_str() {
        "aktywna" => Style::default().fg(Color::Green),
        "fallback" => Style::default().fg(Color::Yellow),
        _ => Style::default().fg(Color::Red),
    }
}

pub(super) fn draw_packs(frame: &mut Frame<'_>, area: Rect, features: &[FeatureSnapshot]) {
    let mut in_frame_order: Vec<&FeatureSnapshot> = features.iter().collect();
    in_frame_order.sort_by_key(|feature| (feature.order.is_none(), feature.order));
    let rows = in_frame_order.into_iter().map(|feature| {
        let status = match &feature.code {
            Some(code) => format!("{} [{code}]", feature.status),
            None => feature.status.clone(),
        };
        Row::new(vec![
            Cell::from(feature.order.map_or_else(|| "—".into(), |order| format!("#{order}"))),
            Cell::from(feature.name.clone()),
            Cell::from(Span::styled(status, status_style(feature))),
            Cell::from(feature.role.clone().unwrap_or_else(|| "—".into())),
            Cell::from(feature.stage.clone()),
            Cell::from(feature.priority.to_string()),
            Cell::from(Span::styled(feature.reason.clone().unwrap_or_default(), dim())),
        ])
    });
    let widths = [
        Constraint::Length(4),
        Constraint::Length(20),
        Constraint::Length(18),
        Constraint::Length(18),
        Constraint::Length(12),
        Constraint::Length(9),
        Constraint::Min(10),
    ];
    let header = Row::new(vec!["#", "paczka", "status", "rola", "etap", "priorytet", "powód"])
        .style(Style::default().fg(Color::Yellow));
    let table = Table::new(rows, widths).header(header).block(
        Block::bordered()
            .title(" paczki w kolejności klatki ")
            .border_style(dim()),
    );
    frame.render_widget(table, area);
}

pub(super) fn draw_diagnostics(frame: &mut Frame<'_>, area: Rect, entries: &[DiagnosticEntry]) {
    let items: Vec<ListItem<'_>> = entries
        .iter()
        .rev()
        .map(|entry| {
            let diagnostic = &entry.diagnostic;
            let color = match diagnostic.severity.as_str() {
                "error" => Color::Red,
                "warning" => Color::Yellow,
                _ => Color::Cyan,
            };
            let count = if entry.count > 1 {
                format!(" ×{}", entry.count)
            } else {
                String::new()
            };
            let mut lines = vec![Line::from(vec![
                Span::styled(format!("{:>6} ", diagnostic.frame), dim()),
                Span::styled(
                    format!("{}[{}]", diagnostic.severity, diagnostic.code),
                    Style::default().fg(color),
                ),
                Span::raw(format!(": {}", truncate(&diagnostic.message, 200))),
                Span::styled(count, dim()),
            ])];
            if let Some(hint) = &diagnostic.hint {
                lines.push(Line::from(Span::styled(format!("        = hint: {hint}"), dim())));
            }
            ListItem::new(lines)
        })
        .collect();
    let title = if entries.is_empty() {
        " diagnostyka · na razie cisza "
    } else {
        " diagnostyka · najnowsze na górze "
    };
    frame.render_widget(
        List::new(items).block(Block::bordered().title(title).border_style(dim())),
        area,
    );
}
