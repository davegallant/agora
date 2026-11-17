use crate::app::App;
use crate::live::State;
use crate::sessions::Agent;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table, Wrap};
use ratatui::Frame;
use std::time::SystemTime;

fn agent_color(a: Agent) -> Color {
    match a {
        Agent::Claude => Color::LightRed,
        Agent::Codex => Color::LightBlue,
        Agent::Pi => Color::LightMagenta,
    }
}

fn age(t: SystemTime) -> String {
    let secs = SystemTime::now()
        .duration_since(t)
        .map_or(0, |d| d.as_secs());
    match secs {
        s if s < 60 => "now".into(),
        s if s < 3600 => format!("{}m", s / 60),
        s if s < 86_400 => format!("{}h", s / 3600),
        s if s < 86_400 * 14 => format!("{}d", s / 86_400),
        s => format!("{}w", s / (86_400 * 7)),
    }
}

fn badge(state: Option<State>) -> Span<'static> {
    match state {
        Some(State::NeedsYou) => Span::styled(
            " ! ",
            Style::new().fg(Color::Black).bg(Color::Yellow).bold(),
        ),
        Some(State::Busy) => Span::styled(" ● ", Style::new().fg(Color::Green)),
        Some(State::Open) => Span::styled(" ○ ", Style::new().fg(Color::Cyan)),
        None => Span::raw("   "),
    }
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let [main, footer] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).areas(f.area());
    let [list, preview] =
        Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)]).areas(main);
    draw_list(f, app, list);
    draw_preview(f, app, preview);
    draw_footer(f, app, footer);
}

fn draw_list(f: &mut Frame, app: &mut App, area: Rect) {
    let scope = match (app.agent_filter, app.live_only) {
        (None, false) => String::new(),
        (a, live) => format!(
            " [{}{}]",
            a.map_or("all", |a| a.name()),
            if live { ", live" } else { "" }
        ),
    };
    let title = format!(
        " agora · {} live · {}/{} sessions{} ",
        app.live_count(),
        app.view.len(),
        app.sessions.len(),
        scope
    );
    let block = Block::default().borders(Borders::ALL).title(title.bold());
    let inner = block.inner(area);
    f.render_widget(block, area);

    let [search, table_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(inner);
    let search_line = if app.filtering || !app.filter.is_empty() {
        Line::from(vec![
            Span::styled(" / ", Style::new().fg(Color::Yellow)),
            Span::raw(app.filter.clone()),
            if app.filtering {
                Span::raw("█")
            } else {
                Span::raw("")
            },
        ])
    } else {
        Line::from(Span::styled(
            " / to search",
            Style::new().fg(Color::DarkGray),
        ))
    };
    f.render_widget(Paragraph::new(search_line), search);

    let rows: Vec<Row> = app
        .view
        .iter()
        .map(|&i| {
            let s = &app.sessions[i];
            let state = app.live.get(&i).map(|l| l.state);
            let title_style = if state.is_some() {
                Style::new().bold()
            } else {
                Style::new()
            };
            Row::new(vec![
                Cell::from(badge(state)),
                Cell::from(Span::styled(
                    s.agent.name(),
                    Style::new().fg(agent_color(s.agent)),
                )),
                Cell::from(Span::styled(
                    s.project().to_string(),
                    Style::new().fg(Color::Gray),
                )),
                Cell::from(Span::styled(s.title.clone(), title_style)),
                Cell::from(Line::from(age(s.mtime)).right_aligned().fg(Color::DarkGray)),
            ])
        })
        .collect();
    let table = Table::new(
        rows,
        [
            Constraint::Length(3),
            Constraint::Length(6),
            Constraint::Length(18),
            Constraint::Fill(1),
            Constraint::Length(4),
        ],
    )
    .column_spacing(1)
    .row_highlight_style(
        Style::new()
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    );
    f.render_stateful_widget(table, table_area, &mut app.table);
}

fn draw_preview(f: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::default().borders(Borders::ALL).title(" preview ");
    let inner = block.inner(area);
    f.render_widget(block, area);
    let Some(i) = app.selected() else {
        f.render_widget(
            Paragraph::new("no sessions match").fg(Color::DarkGray),
            inner,
        );
        return;
    };

    let s = app.sessions[i].clone();
    let live = app.live.get(&i).cloned();
    let mut header = vec![
        Line::from(s.title.clone()).bold(),
        Line::from(vec![
            Span::styled(s.agent.name(), Style::new().fg(agent_color(s.agent))),
            Span::raw(format!(" · {} ago · {}", age(s.mtime), s.cwd)).fg(Color::Gray),
        ]),
    ];
    let action = match &live {
        Some(l) if l.term.is_some() => "enter: jump to its tab",
        Some(l) if l.background.is_some() => "enter: attach (background) · t: attach in new tab",
        _ => "enter: resume here · t: resume in new tab",
    };
    let state = match live.as_ref().map(|l| l.state) {
        Some(State::NeedsYou) => Span::styled("needs you  ", Style::new().fg(Color::Yellow).bold()),
        Some(State::Busy) => Span::styled("working  ", Style::new().fg(Color::Green)),
        Some(State::Open) => Span::styled("open  ", Style::new().fg(Color::Cyan)),
        None => Span::raw(""),
    };
    header.push(Line::from(vec![
        state,
        Span::raw(action).fg(Color::DarkGray),
    ]));
    header.push(Line::raw(""));

    let [head_area, body_area] =
        Layout::vertical([Constraint::Length(header.len() as u16), Constraint::Min(1)])
            .areas(inner);
    f.render_widget(Paragraph::new(header).wrap(Wrap { trim: true }), head_area);

    // Show the end of the conversation: build the text, then scroll so its
    // last line sits at the bottom of the pane.
    let width = body_area.width.max(1) as usize;
    let mut lines: Vec<Line> = Vec::new();
    for m in app.preview(i) {
        let (prefix, style) = if m.user {
            ("› ", Style::new().fg(Color::Yellow))
        } else {
            ("", Style::new())
        };
        for (n, l) in m.text.lines().enumerate() {
            let p = if n == 0 { prefix } else { "  " };
            let p = if m.user { p } else { "" };
            lines.push(Line::styled(format!("{p}{l}"), style));
        }
        lines.push(Line::raw(""));
    }
    let rows: usize = lines.iter().map(|l| l.width().max(1).div_ceil(width)).sum();
    let scroll = rows.saturating_sub(body_area.height as usize);
    f.render_widget(
        Paragraph::new(Text::from(lines))
            .wrap(Wrap { trim: false })
            .scroll((scroll.min(u16::MAX as usize) as u16, 0)),
        body_area,
    );
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let text = if let Some(s) = &app.status {
        Line::from(s.clone()).fg(Color::Yellow)
    } else if app.filtering {
        Line::from(" type to filter · enter done · esc clear").fg(Color::DarkGray)
    } else {
        Line::from(
            " enter open · t new tab · / search · tab agent · l live only · r reload · q quit",
        )
        .fg(Color::DarkGray)
    };
    f.render_widget(Paragraph::new(text), area);
}

#[cfg(test)]
mod tests {
    use crate::app::App;
    use crate::sessions::{Agent, Session};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use std::path::PathBuf;
    use std::time::SystemTime;

    #[test]
    fn renders_list_preview_and_footer() {
        let dir = std::env::temp_dir().join(format!("agora-ui-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("s1.jsonl");
        std::fs::write(
            &path,
            "{\"type\":\"user\",\"cwd\":\"/w/proj\",\"message\":{\"content\":\"hello there\"}}\n\
             {\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"hi back\"}]}}\n",
        )
        .unwrap();
        let sessions = vec![Session {
            agent: Agent::Claude,
            id: "s1".into(),
            path,
            cwd: "/w/proj".into(),
            title: "Fix the login bug".into(),
            mtime: SystemTime::now(),
        }];
        let mut app = App::new(PathBuf::from("/nonexistent"), sessions);
        let mut term = Terminal::new(TestBackend::new(120, 20)).unwrap();
        term.draw(|f| super::draw(f, &mut app)).unwrap();
        let screen: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        for want in [
            "agora",
            "Fix the login bug",
            "› hello there",
            "hi back",
            "enter open",
            "proj",
        ] {
            assert!(screen.contains(want), "missing {want:?} in:\n{screen}");
        }
    }
}
