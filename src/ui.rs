use crate::app::{App, Follow};
use crate::live::{Live, State};
use crate::search;
use crate::sessions::Agent;
use crate::theme::{Theme, THEMES};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, Borders, Cell, Clear, Paragraph, Row, Scrollbar, ScrollbarOrientation, ScrollbarState,
    Table, Wrap,
};
use ratatui::Frame;
use std::time::SystemTime;

/// Every key, for the `?` overlay and `--help`.
pub const KEYS: &[(&str, &str)] = &[
    ("enter", "jump to the session's tab, or resume it here"),
    ("t", "resume or attach in a new Ghostty tab"),
    ("/", "search titles and conversation text; esc clears"),
    ("n N", "next / previous match in the preview"),
    ("J K", "scroll the preview (ctrl-f / ctrl-b by page)"),
    (
        "j k ↑ ↓",
        "move (g / G first / last, ctrl-d / ctrl-u by page)",
    ),
    ("c C", "next / previous channel"),
    ("tab", "cycle agent: all, claude, codex, pi"),
    ("l", "live sessions only"),
    ("T", "choose a theme"),
    ("r", "reload every session from disk"),
    ("?", "show these keys"),
    ("q", "quit"),
];

fn agent_color(t: &Theme, a: Agent) -> Color {
    match a {
        Agent::Claude => t.claude,
        Agent::Codex => t.codex,
        Agent::Pi => t.pi,
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

/// A transcript size in at most five characters: `512B`, `240K`, `3.1M`.
fn size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "K", "M", "G"];
    let mut n = bytes as f64;
    let mut u = 0;
    while n >= 1024.0 && u < UNITS.len() - 1 {
        n /= 1024.0;
        u += 1;
    }
    if u >= 2 && n < 10.0 {
        format!("{n:.1}{}", UNITS[u])
    } else {
        format!("{}{}", n as u64, UNITS[u])
    }
}

/// An open session with shells still running looks idle but isn't done.
fn running_in_background(live: Option<&Live>) -> bool {
    live.is_some_and(|l| l.state == State::Open && l.shells > 0)
}

fn badge(t: &Theme, live: Option<&Live>) -> Span<'static> {
    if running_in_background(live) {
        return Span::styled(" ◐ ", Style::new().fg(t.open));
    }
    match live.map(|l| l.state) {
        Some(State::NeedsYou) => {
            Span::styled(" ! ", Style::new().fg(t.on_accent).bg(t.accent).bold())
        }
        Some(State::Busy) => Span::styled(" ● ", Style::new().fg(t.busy)),
        Some(State::Open) => Span::styled(" ○ ", Style::new().fg(t.open)),
        None => Span::raw("   "),
    }
}

pub fn draw(f: &mut Frame, app: &mut App) {
    let t = app.theme();
    if t.bg.is_some() || t.fg.is_some() {
        let mut base = Style::new();
        if let Some(bg) = t.bg {
            base = base.bg(bg);
        }
        if let Some(fg) = t.fg {
            base = base.fg(fg);
        }
        f.render_widget(Block::new().style(base), f.area());
    }
    let [main, footer] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).areas(f.area());
    let [list, preview] =
        Layout::vertical([Constraint::Percentage(45), Constraint::Percentage(55)]).areas(main);
    app.list_area = list;
    app.preview_area = preview;
    draw_list(f, app, list);
    draw_preview(f, app, preview);
    draw_footer(f, app, footer);
    if app.picking_theme.is_some() {
        draw_theme_picker(f, app);
    } else if app.showing_keys {
        draw_keys(f, app);
    }
}

/// A box over the middle of the screen, cleared and filled with the theme.
fn popup(f: &mut Frame, app: &App, title: &str, lines: Vec<Line>, width: u16) {
    let t = app.theme();
    let height = lines.len() as u16 + 2;
    let area = f.area().clamp(Rect::new(
        f.area().width.saturating_sub(width) / 2,
        f.area().height.saturating_sub(height) / 2,
        width,
        height,
    ));
    let mut style = Style::new();
    if let Some(bg) = t.bg {
        style = style.bg(bg);
    }
    if let Some(fg) = t.fg {
        style = style.fg(fg);
    }
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(lines).style(style).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::new().fg(t.accent))
                .title(format!(" {title} ").bold()),
        ),
        area,
    );
}

fn draw_keys(f: &mut Frame, app: &App) {
    let t = app.theme();
    let col = KEYS
        .iter()
        .map(|(k, _)| k.chars().count())
        .max()
        .unwrap_or(0);
    let lines: Vec<Line> = KEYS
        .iter()
        .map(|(k, what)| {
            Line::from(vec![
                Span::styled(format!(" {k:<col$}  "), Style::new().fg(t.accent).bold()),
                Span::raw(format!("{what} ")),
            ])
        })
        .collect();
    let width = lines.iter().map(|l| l.width()).max().unwrap_or(0) as u16 + 2;
    popup(f, app, "keys", lines, width);
}

/// A list of themes over the middle of the screen; the current one is applied.
fn draw_theme_picker(f: &mut Frame, app: &App) {
    let t = app.theme();
    let width = THEMES.iter().map(|t| t.name.len()).max().unwrap_or(0) as u16 + 8;
    let lines: Vec<Line> = THEMES
        .iter()
        .enumerate()
        .map(|(i, th)| {
            if i == app.theme {
                Line::from(format!(" ▶ {} ", th.name)).style(
                    Style::new()
                        .fg(t.on_accent)
                        .bg(t.accent)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                Line::from(format!("   {} ", th.name))
            }
        })
        .collect();
    popup(f, app, "theme", lines, width);
}

fn draw_list(f: &mut Frame, app: &mut App, area: Rect) {
    let t = app.theme();
    let mut parts: Vec<String> = Vec::new();
    if let Some(c) = &app.channel_filter {
        parts.push(format!("#{c}"));
    }
    match app.agent_filter {
        Some(a) => parts.push(a.name().into()),
        None if app.live_only && parts.is_empty() => parts.push("all".into()),
        None => {}
    }
    if app.live_only {
        parts.push("live".into());
    }
    let scope = if parts.is_empty() {
        String::new()
    } else {
        format!(" [{}]", parts.join(", "))
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
        let dim = Style::new().fg(t.muted);
        let mut spans = vec![
            Span::styled(" / ", Style::new().fg(t.accent)),
            Span::raw(app.filter.clone()),
            Span::raw(if app.filtering { "█" } else { "" }),
        ];
        if app.searching() {
            spans.push(Span::styled(
                format!("   {} ", app.spinner()),
                Style::new().fg(t.accent),
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
        Line::from(Span::styled(" / to search", Style::new().fg(t.muted)))
    };
    f.render_widget(Paragraph::new(search_line), search);

    let rows: Vec<Row> = app
        .view
        .iter()
        .map(|&i| {
            let s = &app.sessions[i];
            let live = app.live.get(&i);
            let title_style = if live.is_some() {
                Style::new().bold()
            } else {
                Style::new()
            };
            Row::new(vec![
                Cell::from(badge(t, live)),
                Cell::from(Span::styled(
                    s.agent.name(),
                    Style::new().fg(agent_color(t, s.agent)),
                )),
                Cell::from(Span::styled(
                    s.project().to_string(),
                    Style::new().fg(t.subtle),
                )),
                Cell::from(Span::styled(s.title.clone(), title_style)),
                Cell::from(Line::from(size(s.size)).right_aligned().fg(t.muted)),
                Cell::from(Line::from(age(s.mtime)).right_aligned().fg(t.muted)),
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
            Constraint::Length(5),
            Constraint::Length(4),
        ],
    )
    .column_spacing(1)
    .row_highlight_style(Style::new().bg(t.selection).add_modifier(Modifier::BOLD));
    f.render_stateful_widget(table, table_area, &mut app.table);
}

fn draw_preview(f: &mut Frame, app: &mut App, area: Rect) {
    let t = app.theme();
    let Some(i) = app.selected() else {
        let block = Block::default().borders(Borders::ALL).title(" preview ");
        let inner = block.inner(area);
        f.render_widget(block, area);
        f.render_widget(Paragraph::new("no sessions match").fg(t.muted), inner);
        return;
    };

    let s = app.sessions[i].clone();
    let live = app.live.get(&i).cloned();
    let terms = search::terms(&app.filter);
    let text_hit = app.text_hits.contains_key(&i);
    let mut header = vec![
        Line::from(s.title.clone()).bold(),
        Line::from(vec![
            Span::styled(s.agent.name(), Style::new().fg(agent_color(t, s.agent))),
            Span::raw(format!(" · {} ago · {}", age(s.mtime), s.cwd)).fg(t.subtle),
        ]),
    ];
    let action = match &live {
        Some(l) if l.term.is_some() => "enter: jump to its tab",
        Some(l) if l.background.is_some() => "enter: attach (background) · t: attach in new tab",
        _ => "enter: resume here · t: resume in new tab",
    };
    let state = match live.as_ref().map(|l| l.state) {
        Some(State::NeedsYou) => Span::styled("needs you  ", Style::new().fg(t.accent).bold()),
        Some(State::Busy) => Span::styled("working  ", Style::new().fg(t.busy)),
        Some(State::Open) if running_in_background(live.as_ref()) => {
            let n = live.as_ref().map_or(0, |l| l.shells);
            let plural = if n == 1 { "" } else { "s" };
            Span::styled(
                format!("open · {n} shell{plural} running  "),
                Style::new().fg(t.open),
            )
        }
        Some(State::Open) => Span::styled("open  ", Style::new().fg(t.open)),
        None => Span::raw(""),
    };
    header.push(Line::from(vec![state, Span::raw(action).fg(t.muted)]));

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
            Style::new().fg(t.on_accent).bg(t.accent).bold(),
        ));
        title.push(Span::styled(" n/N ", Style::new().fg(t.muted)));
    } else if !terms.is_empty() && text_hit {
        title.push(Span::styled(" no match shown ", Style::new().fg(t.muted)));
    }
    if total > height {
        let pct = (top * 100).checked_div(max_top).unwrap_or(100);
        title.push(Span::styled(format!(" {pct}% "), Style::new().fg(t.muted)));
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Line::from(title));
    f.render_widget(block, area);
    f.render_widget(Paragraph::new(header).wrap(Wrap { trim: true }), head_area);
    f.render_widget(
        Paragraph::new("─".repeat(rule_area.width as usize)).fg(t.muted),
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
                .thumb_style(Style::new().fg(t.subtle))
                .track_style(Style::new().fg(t.muted)),
            body_area,
            &mut sb,
        );
        // Tick each match's place on the scrollbar; the current one stands out.
        let x = body_area.right().saturating_sub(1);
        let at = |row: usize| body_area.y + (row * height.saturating_sub(1) / (total - 1)) as u16;
        let buf = f.buffer_mut();
        for &row in &pane.r.matches {
            if let Some(c) = buf.cell_mut((x, at(row))) {
                c.set_symbol("━").set_fg(t.accent);
            }
        }
        if let Some(&row) = pane.current.and_then(|c| pane.r.matches.get(c)) {
            if let Some(c) = buf.cell_mut((x, at(row))) {
                c.set_symbol("◆").set_fg(t.marker);
            }
        }
    }
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let t = app.theme();
    let text = if let Some(s) = &app.status {
        Line::from(s.clone()).fg(t.accent)
    } else if app.picking_theme.is_some() {
        Line::from(" j/k try a theme · enter keep · esc cancel").fg(t.muted)
    } else if app.showing_keys {
        Line::from(" any key closes").fg(t.muted)
    } else if app.filtering {
        Line::from(" type to filter · enter done · esc clear").fg(t.muted)
    } else {
        Line::from(" enter open · / search · ? keys · q quit").fg(t.muted)
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
            size: 240 * 1024,
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
            "240K",
        ] {
            assert!(screen.contains(want), "missing {want:?} in:\n{screen}");
        }
    }

    use super::{age, badge, size};
    const T: &crate::theme::Theme = &crate::theme::THEMES[0];
    use crate::app::Follow;
    use crate::live::{Live, State};
    use std::time::Duration;

    fn session(id: &str, title: &str, path: PathBuf) -> Session {
        Session {
            agent: Agent::Claude,
            id: id.into(),
            path,
            cwd: "/w/proj".into(),
            title: title.into(),
            mtime: SystemTime::now(),
            size: 0,
        }
    }

    fn screen(app: &mut App, w: u16, h: u16) -> String {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| super::draw(f, app)).unwrap();
        let buf = term.backend().buffer();
        (0..h)
            .map(|y| {
                (0..w)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn assert_has(screen: &str, wants: &[&str]) {
        for want in wants {
            assert!(screen.contains(want), "missing {want:?} in:\n{screen}");
        }
    }

    /// A transcript of `n` short exchanges.
    fn long_transcript(tag: &str, n: usize) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("agora-ui-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("long.jsonl");
        let body: String = (0..n)
            .map(|i| {
                format!(
                    "{}\n",
                    serde_json::json!({"type":"user","cwd":"/w","message":{"content":format!("question {i}")}})
                )
            })
            .collect();
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn sizes() {
        assert_eq!(size(0), "0B");
        assert_eq!(size(512), "512B");
        assert_eq!(size(240 * 1024), "240K");
        assert_eq!(size(3_250_000), "3.1M");
        assert_eq!(size(12 * 1024 * 1024), "12M");
        assert_eq!(size(1_300_000_000), "1.2G");
    }

    #[test]
    fn ages() {
        let ago = |s: u64| age(SystemTime::now() - Duration::from_secs(s));
        assert_eq!(ago(0), "now");
        assert_eq!(ago(59), "now");
        assert_eq!(ago(60), "1m");
        assert_eq!(ago(3599), "59m");
        assert_eq!(ago(3600), "1h");
        assert_eq!(ago(86_399), "23h");
        assert_eq!(ago(86_400), "1d");
        assert_eq!(ago(86_400 * 13), "13d");
        assert_eq!(ago(86_400 * 14), "2w");
        assert_eq!(ago(86_400 * 70), "10w");
        // A clock that moved backwards reads as now.
        assert_eq!(age(SystemTime::now() + Duration::from_secs(600)), "now");
    }

    #[test]
    fn badges() {
        let l = |state, shells| Live {
            state,
            term: None,
            background: None,
            shells,
        };
        assert_eq!(badge(T, Some(&l(State::NeedsYou, 0))).content, " ! ");
        assert_eq!(badge(T, Some(&l(State::Busy, 0))).content, " ● ");
        assert_eq!(badge(T, Some(&l(State::Open, 0))).content, " ○ ");
        assert_eq!(badge(T, Some(&l(State::Open, 1))).content, " ◐ ");
        // Busy and waiting sessions already show they aren't idle.
        assert_eq!(badge(T, Some(&l(State::Busy, 1))).content, " ● ");
        assert_eq!(badge(T, Some(&l(State::NeedsYou, 1))).content, " ! ");
        assert_eq!(badge(T, None).content, "   ");
    }

    #[test]
    fn no_sessions() {
        let mut app = App::new(PathBuf::from("/nonexistent"), Vec::new());
        let s = screen(&mut app, 80, 20);
        assert_has(
            &s,
            &["0 live · 0/0 sessions", "/ to search", "no sessions match"],
        );
    }

    #[test]
    fn scope_filter_and_footer_states() {
        let mut app = App::new(
            PathBuf::from("/nonexistent"),
            vec![session("a", "Alpha", PathBuf::from("/nonexistent/a"))],
        );
        app.agent_filter = Some(Agent::Codex);
        app.live_only = true;
        app.rebuild_view();
        assert_has(
            &screen(&mut app, 100, 20),
            &["[codex, live]", "0/1 sessions"],
        );
        app.agent_filter = None;
        app.rebuild_view();
        assert_has(&screen(&mut app, 100, 20), &["[all, live]"]);
        app.channel_filter = Some("a".into());
        app.rebuild_view();
        assert_has(&screen(&mut app, 100, 20), &["[#a, live]"]);
        app.live_only = false;
        app.rebuild_view();
        assert_has(&screen(&mut app, 100, 20), &["[#a]"]);
        app.channel_filter = None;

        app.live_only = false;
        app.filtering = true;
        app.filter = "al".into();
        let s = screen(&mut app, 100, 20);
        assert_has(&s, &[" / al█", "type to filter"]);
        assert!(!s.contains("["), "no scope shown:\n{s}");

        app.status = Some("something happened".into());
        assert_has(&screen(&mut app, 100, 20), &["something happened"]);
    }

    #[test]
    fn search_progress_and_hit_count() {
        let mut app = App::new(
            PathBuf::from("/nonexistent"),
            vec![session("a", "Alpha", PathBuf::from("/nonexistent/a"))],
        );
        app.filter = "zebra".into();
        app.search_progress = Some((3, 10));
        assert_has(
            &screen(&mut app, 100, 20),
            &["searching conversations 3/10"],
        );
        app.search_progress = None;
        app.text_hits.insert(0, "…zebra…".into());
        assert_has(
            &screen(&mut app, 100, 20),
            &["1 match in conversation text"],
        );
    }

    #[test]
    fn live_sessions_show_their_state_and_action() {
        let mut app = App::new(
            PathBuf::from("/nonexistent"),
            vec![session("a", "Alpha", PathBuf::from("/nonexistent/a"))],
        );
        let cases = [
            (
                State::NeedsYou,
                false,
                " ! ",
                "needs you",
                "enter: jump to its tab",
            ),
            (
                State::Busy,
                true,
                " ● ",
                "working",
                "enter: attach (background)",
            ),
            (State::Open, true, " ○ ", "open", "t: attach in new tab"),
        ];
        for (state, background, glyph, label, action) in cases {
            app.live.insert(
                0,
                Live {
                    state,
                    term: (!background).then(|| crate::ghostty::Term {
                        id: "T".into(),
                        title: "Alpha".into(),
                        cwd: "/w/proj".into(),
                    }),
                    background: background.then(|| "bg".into()),
                    shells: 0,
                },
            );
            let s = screen(&mut app, 120, 20);
            assert_has(&s, &[glyph, label, action, "1 live"]);
        }
        for (shells, label) in [
            (1, "open · 1 shell running"),
            (3, "open · 3 shells running"),
        ] {
            app.live.insert(
                0,
                Live {
                    state: State::Open,
                    term: None,
                    background: None,
                    shells,
                },
            );
            assert_has(&screen(&mut app, 120, 20), &[" ◐ ", label]);
        }
        app.live.clear();
        assert_has(
            &screen(&mut app, 120, 20),
            &["enter: resume here · t: resume in new tab", "0 live"],
        );
    }

    #[test]
    fn long_previews_follow_the_bottom_and_show_a_percentage() {
        let path = long_transcript("long", 40);
        let mut app = App::new(
            PathBuf::from("/nonexistent"),
            vec![session("a", "Long", path)],
        );
        let s = screen(&mut app, 80, 24);
        assert_has(&s, &["question 39", " 100% "]);
        assert!(!s.contains("▌ question 0 "), "{s}");

        app.pane.follow = Follow::Top(0);
        let s = screen(&mut app, 80, 24);
        assert_has(&s, &["▌ question 0", " 0% "]);
        assert_eq!(app.pane.top, 0);

        // Scrolling past the end stops at the last page.
        app.pane.follow = Follow::Top(10_000);
        screen(&mut app, 80, 24);
        assert_eq!(app.pane.top, app.pane.r.rows.len() - app.pane.height);
    }

    #[test]
    fn preview_title_counts_matches_and_scrolls_to_the_current_one() {
        let path = long_transcript("matches", 40);
        let mut app = App::new(
            PathBuf::from("/nonexistent"),
            vec![session("a", "Long", path)],
        );
        app.filter = "question".into();
        let s = screen(&mut app, 80, 24);
        assert_has(&s, &[" 40/40 ", " n/N ", "question 39"]);
        app.pane.current = Some(0);
        app.pane.follow = Follow::Match;
        let s = screen(&mut app, 80, 24);
        assert_has(&s, &[" 1/40 ", "▶ question 0 "]);
        // The scrollbar ticks every match and marks the current one.
        assert_has(&s, &["━", "◆"]);
        // The filter matches as one phrase: question 1 and 10-19.
        app.filter = "question 1".into();
        assert_has(&screen(&mut app, 80, 24), &[" 11/11 "]);
    }

    #[test]
    fn a_text_hit_without_a_visible_match_says_so() {
        let path = long_transcript("nomatch", 2);
        let mut app = App::new(
            PathBuf::from("/nonexistent"),
            vec![session("a", "Short", path)],
        );
        app.filter = "elsewhere".into();
        app.text_hits.insert(0, "…elsewhere…".into());
        assert_has(&screen(&mut app, 80, 20), &["no match shown"]);
    }

    #[test]
    fn tiny_terminals_do_not_panic() {
        let path = long_transcript("tiny", 5);
        let mut app = App::new(
            PathBuf::from("/nonexistent"),
            vec![session("a", "A title that is far too long to fit", path)],
        );
        app.filter = "question".into();
        for (w, h) in [(1, 1), (5, 3), (10, 5), (20, 8), (3, 40)] {
            screen(&mut app, w, h);
        }
    }

    #[test]
    fn theme_picker_lists_themes_and_paints_the_background() {
        let mut app = App::new(
            PathBuf::from("/nonexistent"),
            vec![session("a", "Alpha", PathBuf::from("/nonexistent/a"))],
        );
        app.picking_theme = Some(0);
        app.theme = crate::theme::find("tokyo-night").unwrap();
        let mut term = Terminal::new(TestBackend::new(100, 24)).unwrap();
        term.draw(|f| super::draw(f, &mut app)).unwrap();
        let buf = term.backend().buffer();
        let s: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert_has(&s, &[" theme ", "▶ tokyo-night", "monokai", "esc cancel"]);
        assert_eq!(buf[(0, 0)].bg, app.theme().bg.unwrap());
    }

    #[test]
    fn keys_overlay_lists_every_key() {
        let mut app = App::new(PathBuf::from("/nonexistent"), Vec::new());
        assert_has(&screen(&mut app, 100, 24), &["? keys"]);
        app.showing_keys = true;
        let s = screen(&mut app, 100, 24);
        assert_has(&s, &[" keys ", "any key closes"]);
        for (k, what) in super::KEYS {
            assert_has(&s, &[k, what]);
        }
    }
}
