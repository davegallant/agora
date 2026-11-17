use crate::ghostty;
use crate::live::{self, Live, Snapshot, State};
use crate::preview::{self, Rendered};
use crate::search::{self, Files, Searcher};
use crate::sessions::{self, Agent, Msg, Session};
use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::widgets::TableState;
use ratatui::DefaultTerminal;
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
    /// (session, width, filter, current match, full transcript) of `r`.
    key: Option<(usize, usize, String, Option<usize>, bool)>,
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
    rx: Receiver<Snapshot>,
    searcher: Searcher,
    files: Files,
    search_gen: u64,
    search_due: Option<Instant>,
    /// Sessions whose conversation text matches the filter, with a snippet.
    pub text_hits: HashMap<usize, String>,
    /// (files searched, total) while a text search runs.
    pub search_progress: Option<(usize, usize)>,
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

fn haystack(s: &Session) -> String {
    format!("{} {} {} {}", s.agent.name(), s.project(), s.title, s.cwd).to_lowercase()
}

impl App {
    pub fn new(home: PathBuf, sessions: Vec<Session>) -> Self {
        let haystacks = sessions.iter().map(haystack).collect();
        let files = files_of(&sessions);
        let mut app = App {
            home,
            sessions,
            haystacks,
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
            rx: live::spawn_poller(Duration::from_secs(2)),
            searcher: Searcher::spawn(),
            files,
            search_gen: 0,
            search_due: None,
            text_hits: HashMap::new(),
            search_progress: None,
        };
        app.rebuild_view();
        app
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

    fn rebuild_view(&mut self) {
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
                    && (!self.live_only || self.live.contains_key(&i))
                    && (terms.iter().all(|t| self.haystacks[i].contains(t.as_str()))
                        || self.text_hits.contains_key(&i))
            })
            .collect();
        // Sessions are newest first already; a stable sort keeps that within a rank.
        view.sort_by_key(|&i| self.rank(i));
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
        self.haystacks = self.sessions.iter().map(haystack).collect();
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
        let key = (i, width, self.filter.clone(), self.pane.current, full);
        if self.pane.key.as_ref() != Some(&key) {
            let terms = search::terms(&self.filter);
            let msgs = self.previews.entry((i, full)).or_insert_with(|| {
                sessions::preview(&self.sessions[i], if full { usize::MAX } else { 200 }, full)
            });
            let mut r = preview::render(msgs, &terms, width, self.pane.current);
            // First sight of matches: start on the most recent one.
            if self.pane.current.is_none() && !r.matches.is_empty() {
                let last = r.matches.len() - 1;
                r = preview::render(msgs, &terms, width, Some(last));
                self.pane.current = Some(last);
                self.pane.follow = Follow::Match;
            }
            self.pane.key = Some((i, width, self.filter.clone(), self.pane.current, full));
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
            self.poll_search();
            if event::poll(Duration::from_millis(200))? {
                if let Event::Key(k) = event::read()? {
                    if k.kind == KeyEventKind::Press {
                        if let Some(exit) = self.on_key(k) {
                            return Ok(exit);
                        }
                    }
                }
            }
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
