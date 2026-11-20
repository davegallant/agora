use crate::channels::Channels;
use crate::ghostty;
use crate::live::{self, Live, Snapshot, State};
use crate::preview::{self, Rendered};
use crate::search::{self, Files, Searcher};
use crate::sessions::{self, Agent, Changes, Msg, Session};
use crate::theme::{self, Theme, THEMES};
use anyhow::Result;
use ratatui::crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Position, Rect};
use ratatui::widgets::TableState;
use ratatui::DefaultTerminal;
use std::cmp::Reverse;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub enum Exit {
    Quit,
    Exec {
        cwd: Option<String>,
        argv: Vec<String>,
    },
}

/// Where the preview is scrolled to; resolved to a top row at draw time.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Follow {
    /// Pinned to the end of the conversation.
    Bottom,
    /// Keep the current match in view.
    Match,
    /// The user scrolled: this row is at the top.
    Top(usize),
}

pub struct PreviewPane {
    /// (session, width, filter, current match, full transcript, theme) of `r`.
    key: Option<(usize, usize, String, Option<usize>, bool, usize)>,
    pub r: Rendered,
    session: Option<usize>,
    filter: String,
    pub follow: Follow,
    pub current: Option<usize>,
    /// The top row and height last drawn, for relative scrolling.
    pub top: usize,
    pub height: usize,
}

pub struct App {
    home: PathBuf,
    pub sessions: Vec<Session>,
    haystacks: Vec<String>,
    pub channels: Channels,
    /// Each session's channel, parallel to `sessions`.
    pub channel_of: Vec<String>,
    /// Show only this channel's sessions.
    pub channel_filter: Option<String>,
    pub live: HashMap<usize, Live>,
    last_snapshot: Snapshot,
    /// Indices into `sessions`, in display order.
    pub view: Vec<usize>,
    pub table: TableState,
    pub filter: String,
    pub filtering: bool,
    pub agent_filter: Option<Agent>,
    pub live_only: bool,
    pub status: Option<String>,
    previews: HashMap<(usize, bool), Vec<Msg>>,
    pub pane: PreviewPane,
    /// Where the list and preview were last drawn, for routing the mouse wheel.
    pub list_area: Rect,
    pub preview_area: Rect,
    rx: Receiver<Snapshot>,
    /// Transcripts written or removed on disk.
    watch: Receiver<Changes>,
    searcher: Searcher,
    files: Files,
    search_gen: u64,
    search_due: Option<Instant>,
    /// Sessions whose conversation text matches the filter, with a snippet.
    pub text_hits: HashMap<usize, String>,
    /// (files searched, total) while a text search runs.
    pub search_progress: Option<(usize, usize)>,
    /// Drives the search spinner's animation.
    started: Instant,
    /// Index into `THEMES`.
    pub theme: usize,
    /// While choosing a theme: the one to go back to on esc.
    pub picking_theme: Option<usize>,
    /// The `?` overlay is up; the next key closes it.
    pub showing_keys: bool,
}

/// Text search starts after this pause in typing, and only for longer queries.
const SEARCH_DELAY: Duration = Duration::from_millis(250);
const SEARCH_MIN_CHARS: usize = 3;

fn files_of(sessions: &[Session]) -> Files {
    Arc::new(
        sessions
            .iter()
            .enumerate()
            .map(|(i, s)| (i, s.agent, s.path.clone()))
            .collect(),
    )
}

fn haystack(s: &Session, channel: &str) -> String {
    format!(
        "{} {} {} {} {}",
        s.agent.name(),
        s.project(),
        s.title,
        s.cwd,
        channel
    )
    .to_lowercase()
}

impl App {
    pub fn new(home: PathBuf, sessions: Vec<Session>) -> Self {
        let channels = Channels::load(&home);
        let theme = theme::load(&home);
        let files = files_of(&sessions);
        let watch = sessions::spawn_watcher(home.clone(), Duration::from_secs(2));
        let mut app = App {
            home,
            sessions,
            haystacks: Vec::new(),
            channels,
            channel_of: Vec::new(),
            channel_filter: None,
            live: HashMap::new(),
            last_snapshot: Snapshot::default(),
            view: Vec::new(),
            table: TableState::default(),
            filter: String::new(),
            filtering: false,
            agent_filter: None,
            live_only: false,
            status: None,
            previews: HashMap::new(),
            pane: PreviewPane {
                key: None,
                r: Rendered {
                    rows: Vec::new(),
                    matches: Vec::new(),
                },
                session: None,
                filter: String::new(),
                follow: Follow::Bottom,
                current: None,
                top: 0,
                height: 0,
            },
            list_area: Rect::default(),
            preview_area: Rect::default(),
            rx: live::spawn_poller(Duration::from_secs(2)),
            watch,
            searcher: Searcher::spawn(),
            files,
            search_gen: 0,
            search_due: None,
            text_hits: HashMap::new(),
            search_progress: None,
            started: Instant::now(),
            theme,
            picking_theme: None,
            showing_keys: false,
        };
        app.index_sessions();
        app.rebuild_view();
        app
    }

    /// Recompute every session's channel and search text.
    fn index_sessions(&mut self) {
        self.channel_of = self
            .sessions
            .iter()
            .map(|s| self.channels.of(&s.cwd))
            .collect();
        self.haystacks = self
            .sessions
            .iter()
            .zip(&self.channel_of)
            .map(|(s, c)| haystack(s, c))
            .collect();
    }

    /// Use `channels` from now on.
    #[cfg(test)]
    pub fn set_channels(&mut self, channels: Channels) {
        self.channels = channels;
        self.index_sessions();
        self.rebuild_view();
    }

    /// Channels to cycle through: configured ones that have sessions, in the
    /// order written, then the rest by their most recent session.
    pub fn channel_list(&self) -> Vec<String> {
        let tagged = self
            .channel_of
            .iter()
            .map(String::as_str)
            .zip(self.sessions.iter().map(|s| s.mtime));
        self.channels
            .ordered(tagged)
            .into_iter()
            .map(|(name, _)| name)
            .collect()
    }

    /// Step the channel filter: all, then each channel, then all again.
    fn cycle_channel(&mut self, delta: isize) {
        let list = self.channel_list();
        let n = list.len() as isize + 1; // slot 0 is "all"
        let at = self
            .channel_filter
            .as_ref()
            .and_then(|c| list.iter().position(|x| x == c))
            .map_or(0, |p| p as isize + 1);
        let next = (at + delta).rem_euclid(n);
        self.channel_filter = (next > 0).then(|| list[next as usize - 1].clone());
        self.rebuild_view();
    }

    pub fn selected(&self) -> Option<usize> {
        self.table
            .selected()
            .and_then(|i| self.view.get(i).copied())
    }

    fn rank(&self, i: usize) -> u8 {
        match self.live.get(&i).map(|l| l.state) {
            Some(State::NeedsYou) => 0,
            Some(_) => 1,
            None => 2,
        }
    }

    pub fn rebuild_view(&mut self) {
        let keep = self.selected();
        let terms: Vec<String> = self
            .filter
            .to_lowercase()
            .split_whitespace()
            .map(String::from)
            .collect();
        let mut view: Vec<usize> = (0..self.sessions.len())
            .filter(|&i| {
                let s = &self.sessions[i];
                self.agent_filter.is_none_or(|a| a == s.agent)
                    && self
                        .channel_filter
                        .as_ref()
                        .is_none_or(|c| *c == self.channel_of[i])
                    && (!self.live_only || self.live.contains_key(&i))
                    && (terms.iter().all(|t| self.haystacks[i].contains(t.as_str()))
                        || self.text_hits.contains_key(&i))
            })
            .collect();
        view.sort_by_key(|&i| (self.rank(i), Reverse(self.sessions[i].mtime)));
        self.view = view;
        let pos = keep
            .and_then(|k| self.view.iter().position(|&i| i == k))
            .unwrap_or(0);
        self.table
            .select((!self.view.is_empty()).then(|| pos.min(self.view.len() - 1)));
    }

    fn apply(&mut self, snap: Snapshot) {
        self.live = live::resolve(&self.sessions, &snap);
        self.last_snapshot = snap;
        self.rebuild_view();
    }

    fn reload(&mut self) {
        let keep = self.selected().map(|i| self.sessions[i].path.clone());
        self.sessions = sessions::load_all(&self.home);
        self.index_sessions();
        self.files = files_of(&self.sessions);
        self.previews.clear();
        self.pane.key = None;
        self.pane.session = None;
        self.filter_changed();
        self.live = live::resolve(&self.sessions, &self.last_snapshot);
        self.table.select(None);
        self.rebuild_view();
        if let Some(p) = keep {
            if let Some(pos) = self.view.iter().position(|&i| self.sessions[i].path == p) {
                self.table.select(Some(pos));
            }
        }
        self.status = Some(format!("reloaded {} sessions", self.sessions.len()));
    }

    /// Fold in transcripts that changed on disk. Sessions keep their index, so
    /// the selection, preview scroll and search hits stay with them; a
    /// session that is still being written moves up the list and its preview
    /// shows the new messages.
    fn merge(&mut self, c: Changes) {
        let index = |sessions: &[Session], p: &Path| sessions.iter().position(|s| s.path == p);
        // Removing one would shift every index after it: start over instead.
        if c.removed.iter().any(|p| index(&self.sessions, p).is_some()) {
            self.reload();
            return;
        }
        if c.updated.is_empty() {
            return;
        }
        for s in c.updated {
            let ch = self.channels.of(&s.cwd);
            let h = haystack(&s, &ch);
            let i = match index(&self.sessions, &s.path) {
                Some(i) => {
                    self.sessions[i] = s;
                    self.haystacks[i] = h;
                    self.channel_of[i] = ch;
                    i
                }
                None => {
                    self.sessions.push(s);
                    self.haystacks.push(h);
                    self.channel_of.push(ch);
                    self.sessions.len() - 1
                }
            };
            self.previews.remove(&(i, false));
            self.previews.remove(&(i, true));
            if self.pane.session == Some(i) {
                self.pane.key = None;
            }
        }
        self.files = files_of(&self.sessions);
        self.live = live::resolve(&self.sessions, &self.last_snapshot);
        self.rebuild_view();
    }

    /// Lay out session `i`'s conversation for a pane `width` columns wide.
    /// A session that matched on its text loads in full, so the match is in it.
    pub fn preview_pane(&mut self, i: usize, width: usize) -> &mut PreviewPane {
        if self.pane.session != Some(i) || self.pane.filter != self.filter {
            self.pane.session = Some(i);
            self.pane.filter = self.filter.clone();
            self.pane.current = None;
            self.pane.follow = Follow::Bottom;
        }
        let full = self.text_hits.contains_key(&i);
        let t = self.theme;
        let key = (i, width, self.filter.clone(), self.pane.current, full, t);
        if self.pane.key.as_ref() != Some(&key) {
            let terms = search::terms(&self.filter);
            let msgs = self.previews.entry((i, full)).or_insert_with(|| {
                sessions::preview(&self.sessions[i], if full { usize::MAX } else { 200 }, full)
            });
            let mut r = preview::render(&THEMES[t], msgs, &terms, width, self.pane.current);
            // First sight of matches: start on the most recent one.
            if self.pane.current.is_none() && !r.matches.is_empty() {
                let last = r.matches.len() - 1;
                r = preview::render(&THEMES[t], msgs, &terms, width, Some(last));
                self.pane.current = Some(last);
                self.pane.follow = Follow::Match;
            }
            self.pane.key = Some((i, width, self.filter.clone(), self.pane.current, full, t));
            self.pane.r = r;
        }
        &mut self.pane
    }

    fn scroll_preview(&mut self, delta: isize) {
        let top = self.pane.top as isize + delta;
        self.pane.follow = Follow::Top(top.max(0) as usize);
    }

    fn jump_match(&mut self, delta: isize) {
        let n = self.pane.r.matches.len() as isize;
        if n == 0 {
            self.status = Some("no matches in this preview".into());
            return;
        }
        let cur = self.pane.current.map_or(n - 1, |c| c as isize);
        self.pane.current = Some((cur + delta).rem_euclid(n) as usize);
        self.pane.follow = Follow::Match;
    }

    /// A text search is pending or running.
    pub fn searching(&self) -> bool {
        self.search_due.is_some() || self.search_progress.is_some()
    }

    /// The spinner frame for this moment.
    pub fn spinner(&self) -> &'static str {
        const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        FRAMES[(self.started.elapsed().as_millis() / 80) as usize % FRAMES.len()]
    }

    pub fn theme(&self) -> &'static Theme {
        &THEMES[self.theme]
    }

    /// Keys while the theme picker is open: moving previews, enter keeps.
    fn on_theme_key(&mut self, k: KeyEvent, was: usize) {
        let n = THEMES.len() as isize;
        let step = |d: isize| (self.theme as isize + d).rem_euclid(n) as usize;
        match k.code {
            KeyCode::Char('j') | KeyCode::Down | KeyCode::Tab => self.theme = step(1),
            KeyCode::Char('k') | KeyCode::Up | KeyCode::BackTab => self.theme = step(-1),
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('T') => {
                self.theme = was;
                self.picking_theme = None;
            }
            KeyCode::Enter => {
                self.picking_theme = None;
                if let Err(e) = theme::save(&self.home, self.theme) {
                    self.status = Some(format!("could not save the theme: {e}"));
                }
            }
            _ => {}
        }
    }

    pub fn live_count(&self) -> usize {
        self.live.len()
    }

    pub fn run(mut self, terminal: &mut DefaultTerminal) -> Result<Exit> {
        loop {
            terminal.draw(|f| crate::ui::draw(f, &mut self))?;
            let mut latest = None;
            while let Ok(s) = self.rx.try_recv() {
                latest = Some(s);
            }
            if let Some(s) = latest {
                self.apply(s);
            }
            while let Ok(c) = self.watch.try_recv() {
                self.merge(c);
            }
            self.poll_search();
            // Redraw faster while the spinner is animating.
            let tick = if self.searching() { 80 } else { 200 };
            if event::poll(Duration::from_millis(tick))? {
                match event::read()? {
                    Event::Key(k) if k.kind == KeyEventKind::Press => {
                        if let Some(exit) = self.on_key(k) {
                            return Ok(exit);
                        }
                    }
                    Event::Mouse(m) => self.on_mouse(m),
                    _ => {}
                }
            }
        }
    }

    /// The wheel scrolls whichever pane it is over.
    fn on_mouse(&mut self, m: MouseEvent) {
        let delta = match m.kind {
            MouseEventKind::ScrollDown => 1,
            MouseEventKind::ScrollUp => -1,
            _ => return,
        };
        let at = Position::new(m.column, m.row);
        if self.preview_area.contains(at) {
            self.scroll_preview(3 * delta);
        } else if self.list_area.contains(at) {
            self.move_by(delta);
        }
    }

    fn move_by(&mut self, delta: isize) {
        if self.view.is_empty() {
            return;
        }
        let cur = self.table.selected().unwrap_or(0) as isize;
        let next = (cur + delta).clamp(0, self.view.len() as isize - 1);
        self.table.select(Some(next as usize));
    }

    fn on_key(&mut self, k: KeyEvent) -> Option<Exit> {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && k.code == KeyCode::Char('c') {
            return Some(Exit::Quit);
        }
        if let Some(was) = self.picking_theme {
            self.on_theme_key(k, was);
            return None;
        }
        if self.showing_keys {
            self.showing_keys = false;
            return None;
        }
        match k.code {
            KeyCode::Down => return self.nav(1),
            KeyCode::Up => return self.nav(-1),
            KeyCode::PageDown => return self.nav(15),
            KeyCode::PageUp => return self.nav(-15),
            _ => {}
        }
        if self.filtering {
            match k.code {
                KeyCode::Esc => {
                    self.filtering = false;
                    self.filter.clear();
                }
                KeyCode::Enter => {
                    self.filtering = false;
                    return None;
                }
                KeyCode::Backspace => {
                    self.filter.pop();
                }
                KeyCode::Char('u') if ctrl => self.filter.clear(),
                KeyCode::Char(c) => self.filter.push(c),
                _ => return None,
            }
            self.filter_changed();
            return None;
        }
        self.status = None;
        match k.code {
            KeyCode::Char('q') => return Some(Exit::Quit),
            KeyCode::Esc if !self.filter.is_empty() => {
                self.filter.clear();
                self.filter_changed();
            }
            KeyCode::Esc => return Some(Exit::Quit),
            KeyCode::Char('j') => return self.nav(1),
            KeyCode::Char('k') => return self.nav(-1),
            KeyCode::Char('d') if ctrl => return self.nav(15),
            KeyCode::Char('u') if ctrl => return self.nav(-15),
            KeyCode::Char('g') | KeyCode::Home => self.table.select(Some(0)),
            KeyCode::Char('G') | KeyCode::End => {
                self.table.select(self.view.len().checked_sub(1));
            }
            KeyCode::Char('/') => self.filtering = true,
            KeyCode::Tab => {
                self.agent_filter = match self.agent_filter {
                    None => Some(Agent::Claude),
                    Some(Agent::Claude) => Some(Agent::Codex),
                    Some(Agent::Codex) => Some(Agent::Pi),
                    Some(Agent::Pi) => None,
                };
                self.rebuild_view();
            }
            KeyCode::Char('c') => self.cycle_channel(1),
            KeyCode::Char('C') => self.cycle_channel(-1),
            KeyCode::Char('l') => {
                self.live_only = !self.live_only;
                self.rebuild_view();
            }
            KeyCode::Char('r') => self.reload(),
            KeyCode::Char('J') => self.scroll_preview(3),
            KeyCode::Char('K') => self.scroll_preview(-3),
            KeyCode::Char('f') if ctrl => self.scroll_preview(self.pane.height as isize - 2),
            KeyCode::Char('b') if ctrl => self.scroll_preview(-(self.pane.height as isize - 2)),
            KeyCode::Char('n') => self.jump_match(1),
            KeyCode::Char('N') => self.jump_match(-1),
            KeyCode::Enter => return self.open(),
            KeyCode::Char('t') => self.open_in_new_tab(),
            KeyCode::Char('y') => self.copy(false),
            KeyCode::Char('Y') => self.copy(true),
            KeyCode::Char('T') => self.picking_theme = Some(self.theme),
            KeyCode::Char('?') => self.showing_keys = true,
            _ => {}
        }
        None
    }

    /// Title matches update immediately; the text search is debounced.
    fn filter_changed(&mut self) {
        self.search_gen += 1;
        self.searcher.cancel(self.search_gen);
        self.text_hits.clear();
        self.search_progress = None;
        let long_enough = self.filter.trim().chars().count() >= SEARCH_MIN_CHARS;
        self.search_due = long_enough.then(|| Instant::now() + SEARCH_DELAY);
        self.rebuild_view();
    }

    fn poll_search(&mut self) {
        if self.search_due.is_some_and(|t| Instant::now() >= t) {
            self.search_due = None;
            self.search_progress = Some((0, self.files.len()));
            self.searcher.start(
                self.search_gen,
                search::terms(&self.filter),
                self.files.clone(),
            );
        }
        let mut changed = false;
        while let Ok(p) = self.searcher.results.try_recv() {
            if p.generation != self.search_gen {
                continue;
            }
            self.search_progress = (!p.finished).then_some((p.done, p.total));
            for h in p.hits {
                self.text_hits.insert(h.idx, h.snippet);
                changed = true;
            }
        }
        if changed {
            self.rebuild_view();
        }
    }

    fn nav(&mut self, delta: isize) -> Option<Exit> {
        self.move_by(delta);
        None
    }

    /// Jump to the session's tab if it has one; otherwise take over this
    /// terminal to attach (background Claude) or resume it.
    fn open(&mut self) -> Option<Exit> {
        let i = self.selected()?;
        let live = self.live.get(&i).cloned();
        if let Some(term) = live.as_ref().and_then(|l| l.term.as_ref()) {
            self.status = Some(match ghostty::focus(&term.id) {
                Ok(()) => format!("jumped to “{}”", self.sessions[i].title),
                Err(e) => format!("could not focus tab: {e}"),
            });
            return None;
        }
        if let Some(id) = live.and_then(|l| l.background) {
            return Some(Exit::Exec {
                cwd: None,
                argv: vec!["claude".into(), "attach".into(), id],
            });
        }
        let s = &self.sessions[i];
        if !Path::new(&s.cwd).is_dir() {
            self.status = Some(format!("{} no longer exists", s.cwd));
            return None;
        }
        Some(Exit::Exec {
            cwd: Some(s.cwd.clone()),
            argv: s.resume_argv(),
        })
    }

    /// Copy the selected session's resume command, or with `path` its folder.
    fn copy(&mut self, path: bool) {
        let Some(i) = self.selected() else { return };
        let s = &self.sessions[i];
        let text = if path {
            s.cwd.clone()
        } else if let Some(id) = self.live.get(&i).and_then(|l| l.background.clone()) {
            format!("claude attach {}", ghostty::shell_quote(&id))
        } else {
            let argv: Vec<String> = s
                .resume_argv()
                .iter()
                .map(|a| ghostty::shell_quote(a))
                .collect();
            format!("cd {} && {}", ghostty::shell_quote(&s.cwd), argv.join(" "))
        };
        self.status = Some(match crate::clipboard::copy(&text) {
            Ok(()) => format!("copied “{text}”"),
            Err(e) => format!("could not copy: {e}"),
        });
    }

    fn open_in_new_tab(&mut self) {
        let Some(i) = self.selected() else { return };
        if !ghostty::available() {
            self.status = Some("new tabs need Ghostty on macOS".into());
            return;
        }
        let s = &self.sessions[i];
        let argv = match self.live.get(&i).and_then(|l| l.background.clone()) {
            Some(id) => vec!["claude".into(), "attach".into(), id],
            None => s.resume_argv(),
        };
        let cmd: Vec<String> = argv.iter().map(|a| ghostty::shell_quote(a)).collect();
        self.status = Some(match ghostty::new_tab(&s.cwd, &cmd.join(" ")) {
            Ok(()) => format!("opened “{}” in a new tab", s.title),
            Err(e) => format!("could not open tab: {e}"),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    fn session(id: &str, title: &str, age_secs: u64) -> Session {
        Session {
            agent: Agent::Claude,
            id: id.into(),
            path: PathBuf::from(format!("/nonexistent/{id}.jsonl")),
            cwd: "/w/proj".into(),
            title: title.into(),
            mtime: SystemTime::now() - Duration::from_secs(age_secs),
            size: 0,
        }
    }

    fn titles(app: &App) -> Vec<&str> {
        app.view
            .iter()
            .map(|&i| app.sessions[i].title.as_str())
            .collect()
    }

    #[test]
    fn merge_moves_written_sessions_up_and_keeps_the_selection() {
        let mut app = App::new(
            PathBuf::from("/nonexistent"),
            vec![session("b", "Bravo", 10), session("a", "Alpha", 20)],
        );
        app.table.select(Some(1));
        assert_eq!(titles(&app), ["Bravo", "Alpha"]);

        app.merge(Changes {
            updated: vec![session("a", "Alpha renamed", 0)],
            removed: Vec::new(),
        });
        assert_eq!(titles(&app), ["Alpha renamed", "Bravo"]);
        assert_eq!(
            app.table.selected(),
            Some(0),
            "selection follows the session"
        );

        app.merge(Changes {
            updated: vec![session("c", "Charlie", 5)],
            removed: Vec::new(),
        });
        assert_eq!(titles(&app), ["Alpha renamed", "Charlie", "Bravo"]);
        assert_eq!(app.table.selected(), Some(0));

        app.filter = "charlie".into();
        app.rebuild_view();
        assert_eq!(titles(&app), ["Charlie"], "new sessions are searchable");
    }

    use crate::ghostty::Term;
    use std::fs;

    fn tmpdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("agora-app-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn with(agent: Agent, id: &str, title: &str, cwd: &str, age_secs: u64) -> Session {
        Session {
            agent,
            cwd: cwd.into(),
            ..session(id, title, age_secs)
        }
    }

    /// A session backed by a real transcript holding `lines` of conversation.
    fn on_disk(dir: &Path, id: &str, title: &str, lines: &[(bool, &str)]) -> Session {
        let path = dir.join(format!("{id}.jsonl"));
        let body: String = lines
            .iter()
            .map(|(user, text)| {
                let kind = if *user { "user" } else { "assistant" };
                format!(
                    "{}\n",
                    serde_json::json!({"type": kind, "cwd": "/w", "message": {"content": text}})
                )
            })
            .collect();
        fs::write(&path, body).unwrap();
        Session {
            path,
            ..session(id, title, 0)
        }
    }

    fn app(sessions: Vec<Session>) -> App {
        App::new(PathBuf::from("/nonexistent"), sessions)
    }

    fn press(app: &mut App, code: KeyCode) -> Option<Exit> {
        app.on_key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn ctrl(app: &mut App, c: char) -> Option<Exit> {
        app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
    }

    fn typed(app: &mut App, s: &str) {
        for c in s.chars() {
            assert!(press(app, KeyCode::Char(c)).is_none());
        }
    }

    fn live(state: State) -> Live {
        Live {
            state,
            term: None,
            background: None,
            shells: 0,
        }
    }

    fn quits(e: Option<Exit>) -> bool {
        matches!(e, Some(Exit::Quit))
    }

    fn five() -> App {
        app((0..5)
            .map(|n| session(&format!("s{n}"), &format!("T{n}"), n * 10))
            .collect())
    }

    #[test]
    fn empty_app_has_no_selection() {
        let mut a = app(Vec::new());
        assert!(a.view.is_empty());
        assert_eq!(a.table.selected(), None);
        assert_eq!(a.selected(), None);
        assert!(press(&mut a, KeyCode::Char('j')).is_none());
        assert!(press(&mut a, KeyCode::Enter).is_none());
        press(&mut a, KeyCode::Char('t'));
        assert_eq!(a.table.selected(), None);
    }

    #[test]
    fn newest_first_with_live_sessions_on_top() {
        let mut a = app(vec![
            session("a", "Newest", 0),
            session("b", "Busy", 10),
            session("c", "Needs", 20),
            session("d", "Oldest", 30),
            session("e", "Open", 40),
        ]);
        assert_eq!(titles(&a), ["Newest", "Busy", "Needs", "Oldest", "Open"]);
        a.live.insert(1, live(State::Busy));
        a.live.insert(2, live(State::NeedsYou));
        a.live.insert(4, live(State::Open));
        a.rebuild_view();
        assert_eq!(titles(&a), ["Needs", "Busy", "Open", "Newest", "Oldest"]);
        assert_eq!(a.live_count(), 3);
    }

    #[test]
    fn title_filter_matches_every_word_anywhere() {
        let mut a = app(vec![
            with(Agent::Claude, "a", "Fix login bug", "/w/web", 0),
            with(Agent::Codex, "b", "Fix flaky test", "/w/api", 10),
            with(Agent::Pi, "c", "Write docs", "/w/web", 20),
        ]);
        let check = |a: &mut App, f: &str, want: &[&str]| {
            a.filter = f.into();
            a.rebuild_view();
            assert_eq!(titles(a), want, "filter {f:?}");
        };
        check(&mut a, "fix", &["Fix login bug", "Fix flaky test"]);
        check(&mut a, "FIX  bug", &["Fix login bug"]);
        check(&mut a, "web", &["Fix login bug", "Write docs"]);
        check(&mut a, "codex", &["Fix flaky test"]);
        check(&mut a, "/w/api", &["Fix flaky test"]);
        check(&mut a, "nothing", &[]);
        assert_eq!(a.selected(), None);
        check(
            &mut a,
            "",
            &["Fix login bug", "Fix flaky test", "Write docs"],
        );
    }

    #[test]
    fn text_hits_join_the_view() {
        let mut a = app(vec![session("a", "Alpha", 0), session("b", "Bravo", 10)]);
        a.filter = "zebra".into();
        a.rebuild_view();
        assert!(a.view.is_empty());
        a.text_hits.insert(1, "…a zebra…".into());
        a.rebuild_view();
        assert_eq!(titles(&a), ["Bravo"]);
    }

    #[test]
    fn c_cycles_channels_configured_first_then_by_recency() {
        let mut a = app(vec![
            with(Agent::Claude, "a", "Fold", "/home/d/scratch", 0),
            with(Agent::Claude, "b", "Pipe", "/home/d/src/pipe-960", 10),
            with(Agent::Codex, "c", "Corp", "/home/d/corp/api/sub", 20),
            with(Agent::Claude, "d", "Old", "/home/d/old", 30),
        ]);
        a.set_channels(Channels::parse(
            "unused = /nowhere\nwork = ~/corp/*, ~/src/pipe-*\n",
            Path::new("/home/d"),
        ));
        assert_eq!(a.channel_list(), ["work", "scratch", "old"]);

        let mut seen = Vec::new();
        for _ in 0..4 {
            press(&mut a, KeyCode::Char('c'));
            seen.push((a.channel_filter.clone(), titles(&a).join(",")));
        }
        assert_eq!(
            seen,
            [
                (Some("work".to_string()), "Pipe,Corp".to_string()),
                (Some("scratch".into()), "Fold".into()),
                (Some("old".into()), "Old".into()),
                (None, "Fold,Pipe,Corp,Old".into()),
            ]
        );

        // C goes backwards and wraps; it combines with the agent filter.
        press(&mut a, KeyCode::Char('C'));
        assert_eq!(a.channel_filter.as_deref(), Some("old"));
        press(&mut a, KeyCode::Char('C'));
        press(&mut a, KeyCode::Char('C'));
        assert_eq!(a.channel_filter.as_deref(), Some("work"));
        press(&mut a, KeyCode::Tab);
        press(&mut a, KeyCode::Tab);
        assert_eq!(titles(&a), ["Corp"]);
    }

    #[test]
    fn channel_names_are_searchable_and_follow_merged_sessions() {
        let mut a = app(vec![with(Agent::Claude, "a", "A", "/home/d/corp/x", 10)]);
        a.set_channels(Channels::parse("work = ~/corp/*", Path::new("/home/d")));
        press(&mut a, KeyCode::Char('/'));
        typed(&mut a, "work");
        assert_eq!(titles(&a), ["A"]);
        press(&mut a, KeyCode::Esc);
        assert!(a.filter.is_empty());

        a.channel_filter = Some("work".into());
        a.merge(Changes {
            updated: vec![
                with(Agent::Claude, "b", "B", "/home/d/corp/y", 0),
                with(Agent::Claude, "c", "C", "/elsewhere", 0),
            ],
            removed: vec![],
        });
        assert_eq!(a.channel_of, ["work", "work", "elsewhere"]);
        assert_eq!(titles(&a), ["B", "A"]);
    }

    #[test]
    fn tab_cycles_agents_and_l_toggles_live() {
        let mut a = app(vec![
            with(Agent::Claude, "a", "C", "/w", 0),
            with(Agent::Codex, "b", "X", "/w", 10),
            with(Agent::Pi, "c", "P", "/w", 20),
        ]);
        let mut seen = Vec::new();
        for _ in 0..4 {
            press(&mut a, KeyCode::Tab);
            seen.push((a.agent_filter, titles(&a).join(",")));
        }
        assert_eq!(
            seen,
            [
                (Some(Agent::Claude), "C".to_string()),
                (Some(Agent::Codex), "X".into()),
                (Some(Agent::Pi), "P".into()),
                (None, "C,X,P".into()),
            ]
        );

        a.live.insert(1, live(State::Open));
        press(&mut a, KeyCode::Char('l'));
        assert!(a.live_only);
        assert_eq!(titles(&a), ["X"]);
        press(&mut a, KeyCode::Tab);
        assert!(a.view.is_empty(), "live claude sessions: none");
        press(&mut a, KeyCode::Char('l'));
        assert_eq!(titles(&a), ["C"]);
    }

    #[test]
    fn navigation_keys() {
        let mut a = five();
        let at = |a: &App| a.table.selected().unwrap();
        assert_eq!(at(&a), 0);
        press(&mut a, KeyCode::Char('j'));
        press(&mut a, KeyCode::Down);
        assert_eq!(at(&a), 2);
        press(&mut a, KeyCode::Char('k'));
        assert_eq!(at(&a), 1);
        press(&mut a, KeyCode::Up);
        press(&mut a, KeyCode::Up);
        assert_eq!(at(&a), 0, "clamped at the top");
        press(&mut a, KeyCode::Char('G'));
        assert_eq!(at(&a), 4);
        press(&mut a, KeyCode::Char('j'));
        assert_eq!(at(&a), 4, "clamped at the bottom");
        press(&mut a, KeyCode::Char('g'));
        assert_eq!(at(&a), 0);
        press(&mut a, KeyCode::End);
        press(&mut a, KeyCode::Home);
        assert_eq!(at(&a), 0);
        press(&mut a, KeyCode::PageDown);
        assert_eq!(at(&a), 4);
        press(&mut a, KeyCode::PageUp);
        assert_eq!(at(&a), 0);
        ctrl(&mut a, 'd');
        assert_eq!(at(&a), 4);
        ctrl(&mut a, 'u');
        assert_eq!(at(&a), 0);
    }

    #[test]
    fn quitting() {
        let mut a = five();
        assert!(quits(press(&mut a, KeyCode::Char('q'))));
        assert!(quits(press(&mut a, KeyCode::Esc)));
        assert!(quits(ctrl(&mut a, 'c')));
        // ctrl-c quits even while typing a filter.
        press(&mut a, KeyCode::Char('/'));
        assert!(quits(ctrl(&mut a, 'c')));
    }

    #[test]
    fn typing_a_filter() {
        let mut a = five();
        press(&mut a, KeyCode::Char('/'));
        assert!(a.filtering);
        // Keys that are commands elsewhere are just text here.
        typed(&mut a, "qT3");
        assert_eq!(a.filter, "qT3");
        assert!(a.view.is_empty());
        press(&mut a, KeyCode::Backspace);
        press(&mut a, KeyCode::Backspace);
        press(&mut a, KeyCode::Backspace);
        typed(&mut a, "t3");
        assert_eq!(titles(&a), ["T3"]);
        // Arrows still move while filtering; other keys are ignored.
        press(&mut a, KeyCode::Down);
        press(&mut a, KeyCode::F(1));
        assert_eq!(a.filter, "t3");

        press(&mut a, KeyCode::Enter);
        assert!(!a.filtering);
        assert_eq!(a.filter, "t3", "enter keeps the filter");
        // Esc clears an applied filter before it quits.
        assert!(press(&mut a, KeyCode::Esc).is_none());
        assert_eq!(a.filter, "");
        assert_eq!(a.view.len(), 5);

        press(&mut a, KeyCode::Char('/'));
        typed(&mut a, "abc");
        ctrl(&mut a, 'u');
        assert_eq!(a.filter, "");
        assert!(a.filtering);
        typed(&mut a, "abc");
        press(&mut a, KeyCode::Esc);
        assert!(!a.filtering);
        assert_eq!(a.filter, "", "esc while typing clears and leaves");
    }

    #[test]
    fn text_search_waits_for_three_chars() {
        let mut a = five();
        press(&mut a, KeyCode::Char('/'));
        typed(&mut a, "ab");
        assert!(a.search_due.is_none());
        assert!(!a.searching());
        typed(&mut a, " ");
        assert!(a.search_due.is_none(), "whitespace doesn't count");
        typed(&mut a, "c");
        assert!(a.search_due.is_some());
        assert!(a.searching());
        let generation = a.search_gen;
        press(&mut a, KeyCode::Backspace);
        assert!(a.search_gen > generation, "each edit starts a new search");
        assert!(a.search_due.is_none());
    }

    #[test]
    fn selection_stays_on_its_session_as_the_view_changes() {
        let mut a = five();
        press(&mut a, KeyCode::Char('j'));
        press(&mut a, KeyCode::Char('j'));
        assert_eq!(a.selected(), Some(2));
        a.live.insert(2, live(State::NeedsYou));
        a.rebuild_view();
        assert_eq!(a.selected(), Some(2));
        assert_eq!(a.table.selected(), Some(0), "moved to the top with it");
        // When it leaves the view, the row index is kept within bounds.
        press(&mut a, KeyCode::Char('G'));
        a.filter = "t0".into();
        a.rebuild_view();
        assert_eq!(a.table.selected(), Some(0));
        assert_eq!(a.selected(), Some(0));
    }

    #[test]
    fn statuses_clear_on_the_next_key() {
        let mut a = five();
        a.status = Some("hello".into());
        press(&mut a, KeyCode::Char('j'));
        assert_eq!(a.status, None);
    }

    #[test]
    fn scrolling_the_preview() {
        let mut a = five();
        a.pane.top = 10;
        a.pane.height = 20;
        press(&mut a, KeyCode::Char('J'));
        assert_eq!(a.pane.follow, Follow::Top(13));
        press(&mut a, KeyCode::Char('K'));
        assert_eq!(a.pane.follow, Follow::Top(7));
        ctrl(&mut a, 'f');
        assert_eq!(a.pane.follow, Follow::Top(28));
        ctrl(&mut a, 'b');
        assert_eq!(a.pane.follow, Follow::Top(0), "clamped at zero");
        a.pane.top = 1;
        a.scroll_preview(-50);
        assert_eq!(a.pane.follow, Follow::Top(0));
    }

    #[test]
    fn mouse_wheel_scrolls_the_pane_under_it() {
        let mut a = five();
        a.list_area = Rect::new(0, 0, 80, 10);
        a.preview_area = Rect::new(0, 10, 80, 10);
        a.pane.top = 5;
        let wheel = |kind, row| MouseEvent {
            kind,
            column: 3,
            row,
            modifiers: KeyModifiers::NONE,
        };
        a.on_mouse(wheel(MouseEventKind::ScrollDown, 2));
        a.on_mouse(wheel(MouseEventKind::ScrollDown, 2));
        assert_eq!(a.table.selected(), Some(2));
        a.on_mouse(wheel(MouseEventKind::ScrollUp, 2));
        assert_eq!(a.table.selected(), Some(1));
        assert_eq!(a.pane.follow, Follow::Bottom, "preview untouched");

        a.on_mouse(wheel(MouseEventKind::ScrollDown, 12));
        assert_eq!(a.pane.follow, Follow::Top(8));
        a.pane.top = 8;
        a.on_mouse(wheel(MouseEventKind::ScrollUp, 12));
        assert_eq!(a.pane.follow, Follow::Top(5));
        assert_eq!(a.table.selected(), Some(1));

        // Outside both panes, and non-wheel events, do nothing.
        a.on_mouse(wheel(MouseEventKind::ScrollDown, 30));
        a.on_mouse(wheel(MouseEventKind::Moved, 2));
        assert_eq!(a.table.selected(), Some(1));
        assert_eq!(a.pane.follow, Follow::Top(5));
    }

    #[test]
    fn preview_pane_starts_at_the_bottom_or_the_last_match() {
        let dir = tmpdir("pane");
        let s = on_disk(
            &dir,
            "p",
            "Pane",
            &[
                (true, "find the needle"),
                (false, "no hit"),
                (true, "needle again"),
            ],
        );
        let mut a = app(vec![s]);
        let pane = a.preview_pane(0, 40);
        assert_eq!(pane.follow, Follow::Bottom);
        assert_eq!(pane.current, None);
        assert!(pane.r.matches.is_empty());
        assert_eq!(pane.r.rows.len(), 5);

        a.filter = "needle".into();
        let pane = a.preview_pane(0, 40);
        assert_eq!(pane.r.matches, [0, 4]);
        assert_eq!(pane.current, Some(1), "the most recent match");
        assert_eq!(pane.follow, Follow::Match);

        // n/N cycle through matches and wrap.
        press(&mut a, KeyCode::Char('n'));
        assert_eq!(a.pane.current, Some(0));
        press(&mut a, KeyCode::Char('N'));
        assert_eq!(a.pane.current, Some(1));
        press(&mut a, KeyCode::Char('n'));
        a.pane.follow = Follow::Top(0);
        // Laying out again keeps the chosen match and the user's scroll.
        let pane = a.preview_pane(0, 40);
        assert_eq!(pane.current, Some(0));
        assert_eq!(pane.follow, Follow::Top(0));

        // A new filter starts over.
        a.filter = "again".into();
        let pane = a.preview_pane(0, 40);
        assert_eq!(pane.current, Some(0));
        assert_eq!(pane.r.matches, [4]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn jumping_without_matches_says_so() {
        let mut a = five();
        press(&mut a, KeyCode::Char('n'));
        assert_eq!(a.status.as_deref(), Some("no matches in this preview"));
        assert_eq!(a.pane.current, None);
    }

    #[test]
    fn jumping_starts_from_the_last_match() {
        let mut a = five();
        a.pane.r.matches = vec![1, 2, 3];
        a.jump_match(1);
        assert_eq!(a.pane.current, Some(0));
        a.pane.current = None;
        a.jump_match(-1);
        assert_eq!(a.pane.current, Some(1));
        a.jump_match(5);
        assert_eq!(a.pane.current, Some(0));
    }

    #[test]
    fn merge_refreshes_the_preview_of_a_written_session() {
        let dir = tmpdir("merge");
        let s = on_disk(&dir, "m", "M", &[(true, "first")]);
        let mut a = app(vec![s]);
        assert_eq!(a.preview_pane(0, 40).r.rows.len(), 1);
        let s = on_disk(&dir, "m", "M", &[(true, "first"), (false, "second")]);
        a.merge(Changes {
            updated: vec![s],
            removed: Vec::new(),
        });
        assert_eq!(a.preview_pane(0, 40).r.rows.len(), 3);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn merge_ignores_unknown_removals_and_reloads_on_known_ones() {
        let mut a = five();
        a.merge(Changes {
            updated: Vec::new(),
            removed: vec![PathBuf::from("/elsewhere/x.jsonl")],
        });
        assert_eq!(a.sessions.len(), 5);
        assert_eq!(a.status, None);

        // Home is /nonexistent, so the reload finds nothing.
        let gone = a.sessions[2].path.clone();
        a.merge(Changes {
            updated: Vec::new(),
            removed: vec![gone],
        });
        assert!(a.sessions.is_empty());
        assert_eq!(a.status.as_deref(), Some("reloaded 0 sessions"));
        assert_eq!(a.table.selected(), None);
    }

    #[test]
    fn reload_reads_home_and_keeps_the_selection() {
        let home = tmpdir("reload");
        let proj = home.join(".claude/projects/-w");
        fs::create_dir_all(&proj).unwrap();
        for (id, prompt) in [("one", "first"), ("two", "second")] {
            fs::write(
                proj.join(format!("{id}.jsonl")),
                format!(
                    "{}\n",
                    serde_json::json!({"type":"user","cwd":"/w","message":{"content":prompt}})
                ),
            )
            .unwrap();
        }
        let sessions = sessions::load_all(&home);
        let mut a = App::new(home.clone(), sessions);
        press(&mut a, KeyCode::Char('G'));
        let picked = a.sessions[a.selected().unwrap()].path.clone();
        press(&mut a, KeyCode::Char('r'));
        assert_eq!(a.status.as_deref(), Some("reloaded 2 sessions"));
        assert_eq!(a.sessions[a.selected().unwrap()].path, picked);
        fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn open_resumes_attaches_or_jumps() {
        let dir = tmpdir("open");
        let mut a = app(vec![
            with(Agent::Codex, "x", "Here", dir.to_str().unwrap(), 0),
            with(Agent::Claude, "c", "Gone", "/nonexistent/dir", 10),
        ]);
        match press(&mut a, KeyCode::Enter) {
            Some(Exit::Exec { cwd, argv }) => {
                assert_eq!(cwd.as_deref(), dir.to_str());
                assert_eq!(argv, ["codex", "resume", "x"]);
            }
            _ => panic!("expected a resume"),
        }

        press(&mut a, KeyCode::Char('j'));
        assert!(press(&mut a, KeyCode::Enter).is_none());
        assert_eq!(
            a.status.as_deref(),
            Some("/nonexistent/dir no longer exists")
        );

        // A background session attaches, wherever its cwd is.
        a.live.insert(
            1,
            Live {
                background: Some("bg1".into()),
                ..live(State::NeedsYou)
            },
        );
        match press(&mut a, KeyCode::Enter) {
            Some(Exit::Exec { cwd, argv }) => {
                assert_eq!(cwd, None);
                assert_eq!(argv, ["claude", "attach", "bg1"]);
            }
            _ => panic!("expected an attach"),
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn open_with_a_tab_focuses_it_instead() {
        let mut a = five();
        a.live.insert(
            0,
            Live {
                term: Some(Term {
                    id: "T".into(),
                    title: "T0".into(),
                    cwd: "/w/proj".into(),
                }),
                ..live(State::Open)
            },
        );
        a.rebuild_view();
        // Focusing may fail without Ghostty; either way it stays in agora.
        assert!(press(&mut a, KeyCode::Enter).is_none());
        let status = a.status.unwrap();
        assert!(
            status == "jumped to “T0”" || status.starts_with("could not focus tab"),
            "{status}"
        );
    }

    #[test]
    fn background_text_search_finds_sessions() {
        let dir = tmpdir("search");
        let mut a = app(vec![
            on_disk(&dir, "a", "Alpha", &[(true, "talk about kubernetes")]),
            on_disk(&dir, "b", "Bravo", &[(true, "nothing here")]),
        ]);
        press(&mut a, KeyCode::Char('/'));
        typed(&mut a, "kubernetes");
        assert!(a.view.is_empty(), "titles don't match");
        a.search_due = Some(Instant::now());
        let deadline = Instant::now() + Duration::from_secs(5);
        while a.searching() {
            assert!(Instant::now() < deadline, "search never finished");
            a.poll_search();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(titles(&a), ["Alpha"]);
        assert_eq!(a.text_hits[&0], "talk about kubernetes");
        // A matching session previews in full, with the match current.
        let pane = a.preview_pane(0, 40);
        assert_eq!(pane.current, Some(0));

        // Editing the filter drops the old hits.
        press(&mut a, KeyCode::Backspace);
        assert!(a.text_hits.is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn spinner_frames_are_braille() {
        let a = five();
        let f = a.spinner();
        assert_eq!(f.chars().count(), 1);
        assert!(('⠀'..='⣿').contains(&f.chars().next().unwrap()));
    }

    #[test]
    fn t_picks_a_theme_live_and_saves_it_on_enter() {
        let home = tmpdir("theme");
        let mut a = App::new(home.clone(), vec![session("a", "Alpha", 0)]);
        assert_eq!(a.theme().name, "terminal");

        press(&mut a, KeyCode::Char('T'));
        assert!(a.picking_theme.is_some());
        press(&mut a, KeyCode::Char('j'));
        assert_eq!(a.theme().name, "tokyo-night", "moving previews the theme");
        assert_eq!(a.selected(), Some(0), "the list doesn't move underneath");
        press(&mut a, KeyCode::Esc);
        assert_eq!(a.theme().name, "terminal", "esc goes back");
        assert!(a.picking_theme.is_none());
        assert!(!home.join(".config/agora/theme").exists());

        press(&mut a, KeyCode::Char('T'));
        press(&mut a, KeyCode::Down);
        press(&mut a, KeyCode::Down);
        press(&mut a, KeyCode::Enter);
        assert_eq!(a.theme().name, "monokai");
        assert!(a.picking_theme.is_none());
        let saved = App::new(home.clone(), Vec::new());
        assert_eq!(saved.theme().name, "monokai", "kept across runs");

        // Up from the first wraps to the last.
        press(&mut a, KeyCode::Char('T'));
        for _ in 0..3 {
            press(&mut a, KeyCode::Up);
        }
        assert_eq!(a.theme().name, THEMES[THEMES.len() - 1].name);
        fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn y_copies_the_resume_command_and_shift_y_the_path() {
        let mut a = app(vec![with(Agent::Claude, "abc", "Alpha", "/w/my proj", 0)]);
        press(&mut a, KeyCode::Char('y'));
        assert_eq!(
            crate::clipboard::last().as_deref(),
            Some("cd '/w/my proj' && claude --resume abc")
        );
        assert!(a.status.as_deref().unwrap().starts_with("copied"));
        press(&mut a, KeyCode::Char('Y'));
        assert_eq!(crate::clipboard::last().as_deref(), Some("/w/my proj"));
    }

    #[test]
    fn y_copies_attach_for_a_background_agent() {
        let mut a = app(vec![session("abc", "Alpha", 0)]);
        a.live.insert(
            0,
            Live {
                background: Some("bg1".into()),
                ..live(State::Busy)
            },
        );
        press(&mut a, KeyCode::Char('y'));
        assert_eq!(
            crate::clipboard::last().as_deref(),
            Some("claude attach bg1")
        );
    }

    #[test]
    fn y_on_an_empty_list_does_nothing() {
        let mut a = app(Vec::new());
        crate::clipboard::copy("before").unwrap();
        assert!(press(&mut a, KeyCode::Char('y')).is_none());
        assert_eq!(crate::clipboard::last().as_deref(), Some("before"));
    }

    #[test]
    fn question_mark_shows_the_keys_and_any_key_closes_them() {
        let mut a = five();
        press(&mut a, KeyCode::Char('?'));
        assert!(a.showing_keys);
        assert!(
            press(&mut a, KeyCode::Char('q')).is_none(),
            "q only closes it"
        );
        assert!(!a.showing_keys);
        assert_eq!(a.selected(), Some(0));
    }
}
