// The terminal UI: an interactive frame graph plus resources, packs and diagnostics panels.

mod graph;
mod panels;

use std::collections::VecDeque;
use std::time::Duration;
use std::{fs, io};

use crossbeam_channel::Receiver;
use ratatui::Frame;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::Tabs;

pub use self::graph::{GraphLayout, Node, layout};
use crate::protocol::{DiagnosticSnapshot, Snapshot, ViewSnapshot};
use crate::server::ClientEvent;

const FRAME_HISTORY: usize = 120;
const DIAGNOSTIC_HISTORY: usize = 500;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Graph,
    Resources,
    Packs,
    Diagnostics,
}

impl Tab {
    const ALL: [Self; 4] = [Self::Graph, Self::Resources, Self::Packs, Self::Diagnostics];

    fn title(self) -> &'static str {
        match self {
            Self::Graph => "1 graf",
            Self::Resources => "2 zasoby",
            Self::Packs => "3 paczki",
            Self::Diagnostics => "4 diagnostyka",
        }
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|tab| *tab == self).unwrap_or(0)
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DiagnosticEntry {
    pub diagnostic: DiagnosticSnapshot,
    pub count: u32,
}

#[derive(Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent UI toggles, not a state machine"
)]
pub struct App {
    pub snapshot: Option<Snapshot>,
    pub source: String,
    pub connected: bool,
    pub paused: bool,
    pub tab: Tab,
    pub view: usize,
    pub selected_pass: Option<usize>,
    pub show_culled: bool,
    pub selected_resource: usize,
    pub frame_times: VecDeque<u64>,
    pub diagnostics: Vec<DiagnosticEntry>,
    pub message: Option<String>,
    pub quit: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Direction {
    Left,
    Right,
    Up,
    Down,
}

impl App {
    pub fn new(source: impl Into<String>) -> Self {
        Self {
            snapshot: None,
            source: source.into(),
            connected: false,
            paused: false,
            tab: Tab::Graph,
            view: 0,
            selected_pass: None,
            show_culled: true,
            selected_resource: 0,
            frame_times: VecDeque::with_capacity(FRAME_HISTORY),
            diagnostics: Vec::new(),
            message: None,
            quit: false,
        }
    }

    pub fn current_view(&self) -> Option<&ViewSnapshot> {
        let views = &self.snapshot.as_ref()?.views;
        views.get(self.view.min(views.len().saturating_sub(1)))
    }

    pub fn on_event(&mut self, event: ClientEvent) {
        match event {
            ClientEvent::Connected(source) => {
                self.connected = true;
                self.source = source;
            }
            ClientEvent::Disconnected(source) => {
                self.connected = false;
                self.source = format!("{source} (łączę…)");
            }
            ClientEvent::Snapshot(snapshot) if !self.paused => self.on_snapshot(*snapshot),
            ClientEvent::Snapshot(_) => {}
        }
    }

    pub fn on_snapshot(&mut self, snapshot: Snapshot) {
        if self.frame_times.len() == FRAME_HISTORY {
            self.frame_times.pop_front();
        }
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "clamped, rounded microseconds"
        )]
        self.frame_times
            .push_back((f64::from(snapshot.frame_ms) * 1000.0).round().max(0.0) as u64);
        for diagnostic in &snapshot.diagnostics {
            let same = self.diagnostics.iter_mut().find(|entry| {
                entry.diagnostic.code == diagnostic.code && entry.diagnostic.message == diagnostic.message
            });
            if let Some(entry) = same {
                entry.count += 1;
                entry.diagnostic.frame = diagnostic.frame;
            } else {
                self.diagnostics.push(DiagnosticEntry {
                    diagnostic: diagnostic.clone(),
                    count: 1,
                });
            }
        }
        if self.diagnostics.len() > DIAGNOSTIC_HISTORY {
            let excess = self.diagnostics.len() - DIAGNOSTIC_HISTORY;
            self.diagnostics.drain(..excess);
        }
        self.snapshot = Some(snapshot);
        self.keep_selection_valid();
    }

    fn keep_selection_valid(&mut self) {
        let Some(view) = self.current_view() else {
            return;
        };
        let graph = layout(view, self.show_culled);
        let resources = view.resources.len();
        if self.selected_pass.and_then(|pass| graph.node_of(pass)).is_none() {
            self.selected_pass = graph.first().map(|node| node.pass);
        }
        self.selected_resource = self.selected_resource.min(resources.saturating_sub(1));
    }

    pub fn on_key(&mut self, code: KeyCode) {
        self.message = None;
        match code {
            KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
            KeyCode::Tab => self.tab = Tab::ALL[(self.tab.index() + 1) % Tab::ALL.len()],
            KeyCode::BackTab => self.tab = Tab::ALL[(self.tab.index() + Tab::ALL.len() - 1) % Tab::ALL.len()],
            KeyCode::Char('1') => self.tab = Tab::Graph,
            KeyCode::Char('2') => self.tab = Tab::Resources,
            KeyCode::Char('3') => self.tab = Tab::Packs,
            KeyCode::Char('4') => self.tab = Tab::Diagnostics,
            KeyCode::Char('c') => {
                self.show_culled = !self.show_culled;
                self.keep_selection_valid();
            }
            KeyCode::Char('v') => {
                let views = self.snapshot.as_ref().map_or(1, |snapshot| snapshot.views.len().max(1));
                self.view = (self.view + 1) % views;
                self.keep_selection_valid();
            }
            KeyCode::Char('p') => self.paused = !self.paused,
            KeyCode::Char('s') => self.save(),
            KeyCode::Left => self.navigate(Direction::Left),
            KeyCode::Right => self.navigate(Direction::Right),
            KeyCode::Up => self.navigate(Direction::Up),
            KeyCode::Down => self.navigate(Direction::Down),
            _ => {}
        }
    }

    fn navigate(&mut self, direction: Direction) {
        match self.tab {
            Tab::Graph => self.navigate_graph(direction),
            Tab::Resources => {
                let count = self.current_view().map_or(0, |view| view.resources.len());
                self.selected_resource = match direction {
                    Direction::Up => self.selected_resource.saturating_sub(1),
                    Direction::Down => (self.selected_resource + 1).min(count.saturating_sub(1)),
                    Direction::Left | Direction::Right => self.selected_resource,
                };
            }
            Tab::Packs | Tab::Diagnostics => {}
        }
    }

    fn navigate_graph(&mut self, direction: Direction) {
        let Some(view) = self.current_view() else {
            return;
        };
        let graph = layout(view, self.show_culled);
        let Some(current) = self.selected_pass.and_then(|pass| graph.node_of(pass)) else {
            self.selected_pass = graph.first().map(|node| node.pass);
            return;
        };
        let next = match direction {
            Direction::Left => current
                .column
                .checked_sub(1)
                .and_then(|column| graph.nearest(column, current.row)),
            Direction::Right => graph.nearest(current.column + 1, current.row),
            Direction::Up => current
                .row
                .checked_sub(1)
                .and_then(|row| graph.nearest(current.column, row)),
            Direction::Down => graph
                .nearest(current.column, current.row + 1)
                .filter(|node| node.row > current.row),
        };
        if let Some(node) = next {
            self.selected_pass = Some(node.pass);
        }
    }

    fn save(&mut self) {
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let path = format!("pathon-snapshot-{}.json", snapshot.frame);
        self.message = Some(match fs::write(&path, snapshot.to_line()) {
            Ok(()) => format!("zapisano {path}"),
            Err(error) => format!("zapis się nie udał: {error}"),
        });
    }

    pub fn draw(&self, frame: &mut Frame<'_>) {
        let [header, tabs, body, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(5),
            Constraint::Length(1),
        ])
        .areas(frame.area());
        panels::draw_header(frame, header, self);
        let titles: Vec<Line<'_>> = Tab::ALL.iter().map(|tab| Line::from(tab.title())).collect();
        frame.render_widget(
            Tabs::new(titles)
                .select(self.tab.index())
                .highlight_style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            tabs,
        );
        panels::draw_footer(frame, footer, self);

        let Some(snapshot) = &self.snapshot else {
            return;
        };
        match (self.tab, self.current_view()) {
            (Tab::Graph, Some(view)) => {
                let [graph_area, inspector_area] =
                    Layout::horizontal([Constraint::Percentage(68), Constraint::Percentage(32)]).areas(body);
                graph::draw_graph(frame, graph_area, view, self.selected_pass, self.show_culled);
                panels::draw_inspector(frame, inspector_area, view, self.selected_pass);
            }
            (Tab::Resources, Some(view)) => panels::draw_resources(frame, body, view, self.selected_resource),
            (Tab::Packs, _) => panels::draw_packs(frame, body, &snapshot.features),
            (Tab::Diagnostics, _) => panels::draw_diagnostics(frame, body, &self.diagnostics),
            (Tab::Graph | Tab::Resources, None) => {}
        }
    }
}

// Draws until `q`. Snapshots arrive on `events` from the client thread, the demo or nothing (a loaded file).
pub fn run(events: &Receiver<ClientEvent>, mut app: App) -> io::Result<()> {
    let mut terminal = ratatui::init();
    let result = (|| -> io::Result<()> {
        while !app.quit {
            terminal.draw(|frame| app.draw(frame))?;
            while let Ok(event) = events.try_recv() {
                app.on_event(event);
            }
            if event::poll(Duration::from_millis(50))?
                && let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                app.on_key(key.code);
            }
        }
        Ok(())
    })();
    ratatui::restore();
    result
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::demo::{demo_renderer, demo_snapshot};

    fn screen(app: &App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(160, 48)).expect("test terminal");
        terminal.draw(|frame| app.draw(frame)).expect("draws");
        let buffer = terminal.backend().buffer();
        let mut text = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                text.push_str(buffer[(x, y)].symbol());
            }
            text.push('\n');
        }
        text
    }

    fn demo_app() -> App {
        let (mut renderer, mut input) = demo_renderer();
        let mut app = App::new("test");
        app.on_snapshot(demo_snapshot(&mut renderer, &mut input));
        app
    }

    #[test]
    fn graph_tab_draws_live_and_culled_passes() {
        let mut app = demo_app();
        let text = screen(&app);
        assert!(text.contains("unlit mesh"), "{text}");
        assert!(text.contains("▶"), "more columns to the right are announced: {text}");
        for _ in 0..6 {
            app.on_key(KeyCode::Right);
        }
        let text = screen(&app);
        assert!(text.contains("test upscale"), "{text}");
        assert!(text.contains("wycięty"), "the culled debug mask is reachable: {text}");
    }

    #[test]
    fn arrows_walk_the_graph_along_dependencies() {
        let mut app = demo_app();
        let first = app.selected_pass;
        app.on_key(KeyCode::Right);
        assert_ne!(app.selected_pass, first);
        app.on_key(KeyCode::Left);
        assert_eq!(app.selected_pass, first);
    }

    #[test]
    fn panels_show_resources_packs_and_role_conflicts() {
        let mut app = demo_app();
        app.on_key(KeyCode::Char('2'));
        assert!(screen(&app).contains("hdr"));
        app.on_key(KeyCode::Char('3'));
        let packs = screen(&app);
        assert!(packs.contains("fallback geometry"));
        assert!(packs.contains("F0102"));
    }
}
