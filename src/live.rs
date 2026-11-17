//! Which sessions are running right now, and which need you.
//!
//! Two sources, polled on a background thread:
//! - `claude agents --json`: running Claude sessions, interactive and background.
//! - Ghostty tab titles: agents title their tab after the session (Claude's
//!   AI title, Codex's thread name), and Ghostty prefixes 🔔 on a bell.

use crate::ghostty::{self, Glyph, Term};
use crate::sessions::{Agent, Session};
use serde_json::Value;
use std::collections::HashMap;
use std::process::Command;
use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum State {
    NeedsYou,
    Busy,
    Open,
}

#[derive(Clone, Debug)]
pub struct Live {
    pub state: State,
    pub term: Option<Term>,
    /// Short id for `claude attach` when this is a background session.
    pub background: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ClaudeLive {
    pub background: Option<String>,
    pub state: Option<String>,
}

#[derive(Default)]
pub struct Snapshot {
    pub claude: HashMap<String, ClaudeLive>,
    pub terms: Vec<Term>,
}

fn claude_agents() -> HashMap<String, ClaudeLive> {
    let Ok(out) = Command::new("claude").args(["agents", "--json"]).output() else {
        return HashMap::new();
    };
    let Ok(Value::Array(items)) = serde_json::from_slice::<Value>(&out.stdout) else {
        return HashMap::new();
    };
    items
        .iter()
        .filter_map(|v| {
            let sid = v["sessionId"].as_str()?.to_string();
            let background = (v["kind"] == "background")
                .then(|| v["id"].as_str().map(String::from))
                .flatten();
            let state = v["state"].as_str().map(String::from);
            Some((sid, ClaudeLive { background, state }))
        })
        .collect()
}

pub fn snapshot() -> Snapshot {
    Snapshot {
        claude: claude_agents(),
        terms: ghostty::terminals(),
    }
}

pub fn spawn_poller(every: Duration) -> Receiver<Snapshot> {
    let (tx, rx): (Sender<Snapshot>, _) = std::sync::mpsc::channel();
    std::thread::spawn(move || loop {
        if tx.send(snapshot()).is_err() {
            return;
        }
        std::thread::sleep(every);
    });
    rx
}

fn claude_state(s: &str) -> State {
    match s {
        "blocked" | "needs_input" | "waiting" | "input" => State::NeedsYou,
        "running" | "busy" | "working" => State::Busy,
        _ => State::Open,
    }
}

/// Does a bare tab title belong to this session?
fn title_matches(tab: &str, s: &Session) -> bool {
    !s.title.is_empty()
        && (tab == s.title
            // Codex titles tabs "<thread name> | <project>".
            || tab.strip_prefix(s.title.as_str()).is_some_and(|r| r.starts_with(" |")))
}

/// Map session index -> live state. `sessions` is sorted newest first, so when
/// several sessions share a title the most recent one claims the tab.
pub fn resolve(sessions: &[Session], snap: &Snapshot) -> HashMap<usize, Live> {
    let mut live: HashMap<usize, Live> = HashMap::new();

    for term in &snap.terms {
        let (title, glyph) = ghostty::parse_title(&term.title);
        let hit = sessions.iter().enumerate().find(|(i, s)| {
            !live.contains_key(i)
                && (term.cwd.is_empty() || term.cwd == s.cwd)
                && title_matches(&title, s)
        });
        if let Some((i, _)) = hit {
            let state = match glyph {
                Glyph::Bell => State::NeedsYou,
                Glyph::Busy => State::Busy,
                Glyph::None => State::Open,
            };
            live.insert(
                i,
                Live {
                    state,
                    term: Some(term.clone()),
                    background: None,
                },
            );
        }
    }

    for (i, s) in sessions.iter().enumerate() {
        if s.agent != Agent::Claude {
            continue;
        }
        let Some(c) = snap.claude.get(&s.id) else {
            continue;
        };
        let state = c.state.as_deref().map_or(State::Open, claude_state);
        let entry = live.entry(i).or_insert(Live {
            state,
            term: None,
            background: None,
        });
        entry.state = entry.state.min(state);
        entry.background = c.background.clone();
    }
    live
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::SystemTime;

    fn s(agent: Agent, id: &str, title: &str, cwd: &str) -> Session {
        Session {
            agent,
            id: id.into(),
            path: PathBuf::new(),
            cwd: cwd.into(),
            title: title.into(),
            mtime: SystemTime::UNIX_EPOCH,
        }
    }

    fn term(id: &str, title: &str, cwd: &str) -> Term {
        Term {
            id: id.into(),
            title: title.into(),
            cwd: cwd.into(),
        }
    }

    #[test]
    fn matches_tabs_and_claude_agents() {
        let sessions = vec![
            s(Agent::Claude, "c-new", "Fix bug", "/w/a"),
            s(Agent::Claude, "c-old", "Fix bug", "/w/a"),
            s(Agent::Codex, "x1", "Reply hi", "/w/b"),
            s(Agent::Claude, "c-bg", "Background job", "/w/c"),
            s(Agent::Pi, "p1", "untouched", "/w/d"),
        ];
        let mut snap = Snapshot {
            terms: vec![
                term("T1", "🔔 ✳ Fix bug", "/w/a"),
                term("T2", "Reply hi | b", "/w/b"),
                term("T3", "Fix bug", "/elsewhere"),
            ],
            ..Default::default()
        };
        snap.claude.insert(
            "c-bg".into(),
            ClaudeLive {
                background: Some("abcd".into()),
                state: Some("blocked".into()),
            },
        );

        let live = resolve(&sessions, &snap);
        assert_eq!(live[&0].state, State::NeedsYou);
        assert_eq!(live[&0].term.as_ref().unwrap().id, "T1");
        assert!(!live.contains_key(&1), "older duplicate or other cwd");
        assert_eq!(live[&2].term.as_ref().unwrap().id, "T2");
        assert_eq!(live[&3].state, State::NeedsYou);
        assert_eq!(live[&3].background.as_deref(), Some("abcd"));
        assert!(!live.contains_key(&4));
    }
}
