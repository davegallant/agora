use crate::app::{App, Follow};
use crate::live::State;
use crate::search;
use crate::sessions::Agent;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, Borders, Cell, Paragraph, Row, Scrollbar, ScrollbarOrientation, ScrollbarState, Table,
    Wrap,
};
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
        Layout::vertical([Constraint::Percentage(45), Constraint::Percentage(55)]).areas(main);
    app.list_area = list;
    app.preview_area = preview;
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
        let dim = Style::new().fg(Color::DarkGray);
        let mut spans = vec![
            Span::styled(" / ", Style::new().fg(Color::Yellow)),
            Span::raw(app.filter.clone()),
            Span::raw(if app.filtering { "█" } else { "" }),
        ];
        if app.searching() {
            spans.push(Span::styled(
                format!("   {} ", app.spinner()),
                Style::new().fg(Color::Yellow),
            ));
            spans.push(Span::styled(
                match app.search_progress {
                    Some((done, total)) => format!("searching conversations {done}/{total}"),
                    None => "searching conversations…".into(),
                },
                dim,
            ));
        } else if !app.text_hits.is_empty() {
            spans.push(Span::styled(
                format!("   {} match in conversation text", app.text_hits.len()),
                dim,
            ));
        }
        Line::from(spans)
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
    let Some(i) = app.selected() else {
        let block = Block::default().borders(Borders::ALL).title(" preview ");
        let inner = block.inner(area);
        f.render_widget(block, area);
        f.render_widget(
            Paragraph::new("no sessions match").fg(Color::DarkGray),
            inner,
        );
        return;
    };

    let s = app.sessions[i].clone();
    let live = app.live.get(&i).cloned();
    let terms = search::terms(&app.filter);
    let text_hit = app.text_hits.contains_key(&i);
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

    // The body is laid out first: the block title reports its matches.
    let outer = Block::default().borders(Borders::ALL).inner(area);
    let header_width = outer.width.max(1) as usize;
    let header_rows: usize = header
        .iter()
        .map(|l| l.width().max(1).div_ceil(header_width))
        .sum::<usize>()
        + 1;
    let [head_area, rule_area, body_area] = Layout::vertical([
        Constraint::Length(header_rows.saturating_sub(1).min(u16::MAX as usize) as u16),
        Constraint::Length(1),
        Constraint::Min(1),
    ])
    .areas(outer);
    // One column on the right for the scrollbar.
    let text_width = body_area.width.saturating_sub(1).max(1) as usize;
    let height = body_area.height as usize;

    let pane = app.preview_pane(i, text_width);
    let total = pane.r.rows.len();
    let max_top = total.saturating_sub(height);
    let top = match pane.follow {
        Follow::Bottom => max_top,
        Follow::Match => pane
            .current
            .and_then(|c| pane.r.matches.get(c))
            .map_or(max_top, |&row| row.saturating_sub(height / 3)),
        Follow::Top(t) => t,
    }
    .min(max_top);
    pane.top = top;
    pane.height = height;

    let mut title = vec![Span::raw(" preview ")];
    if !pane.r.matches.is_empty() {
        let cur = pane.current.map_or(0, |c| c + 1);
        title.push(Span::styled(
            format!(" {cur}/{} ", pane.r.matches.len()),
            Style::new().fg(Color::Black).bg(Color::Yellow).bold(),
        ));
        title.push(Span::styled(" n/N ", Style::new().fg(Color::DarkGray)));
    } else if !terms.is_empty() && text_hit {
        title.push(Span::styled(
            " no match shown ",
            Style::new().fg(Color::DarkGray),
        ));
    }
    if total > height {
        let pct = (top * 100).checked_div(max_top).unwrap_or(100);
        title.push(Span::styled(
            format!(" {pct}% "),
            Style::new().fg(Color::DarkGray),
        ));
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Line::from(title));
    f.render_widget(block, area);
    f.render_widget(Paragraph::new(header).wrap(Wrap { trim: true }), head_area);
    f.render_widget(
        Paragraph::new("─".repeat(rule_area.width as usize)).fg(Color::DarkGray),
        rule_area,
    );

    let visible: Vec<Line> = pane.r.rows[top..(top + height).min(total)].to_vec();
    f.render_widget(Paragraph::new(Text::from(visible)), body_area);
    if total > height {
        let mut sb = ScrollbarState::new(max_top).position(top);
        f.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .thumb_style(Style::new().fg(Color::Gray))
                .track_style(Style::new().fg(Color::DarkGray)),
            body_area,
            &mut sb,
        );
    }
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let text = if let Some(s) = &app.status {
        Line::from(s.clone()).fg(Color::Yellow)
    } else if app.filtering {
        Line::from(" type to filter · enter done · esc clear").fg(Color::DarkGray)
    } else {
        Line::from(
            " enter open · t new tab · / search · n/N match · J/K scroll · tab agent · l live · r reload · q quit",
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
            "▌ hello there",
            "hi back",
            "enter open",
            "proj",
        ] {
            assert!(screen.contains(want), "missing {want:?} in:\n{screen}");
        }
    }
}
