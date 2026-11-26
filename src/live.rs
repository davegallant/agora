//! Which sessions are running right now, and which need you.
//!
//! Two sources, polled on a background thread:
//! - `claude agents --json`: running Claude sessions, interactive and background.
//! - `ps`: shells Claude started for a session, which keep running in the
//!   background after its turn ends.
//! - Ghostty tab titles: agents title their tab after the session (Claude's
//!   AI title, Codex's thread name), and Ghostty prefixes 🔔 on a bell.

use crate::ghostty::{self, Glyph, Term};
use crate::sessions::{Agent, Session};
use serde_json::Value;
use std::collections::HashMap;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

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
    /// Shells Claude started for this session that are still running.
    pub shells: usize,
}

#[derive(Clone, Debug, Default)]
pub struct ClaudeLive {
    pub background: Option<String>,
    pub state: Option<String>,
    pub pid: Option<u32>,
}

#[derive(Default)]
pub struct Snapshot {
    pub claude: HashMap<String, ClaudeLive>,
    pub terms: Vec<Term>,
    /// Process id -> how many Claude shells it is the parent of.
    pub shells: HashMap<u32, usize>,
}

fn claude_agents() -> HashMap<String, ClaudeLive> {
    let Ok(out) = Command::new("claude").args(["agents", "--json"]).output() else {
        return HashMap::new();
    };
    parse_agents(&out.stdout)
}

/// Parse `claude agents --json`: session id -> what is running it.
fn parse_agents(json: &[u8]) -> HashMap<String, ClaudeLive> {
    let Ok(Value::Array(items)) = serde_json::from_slice::<Value>(json) else {
        return HashMap::new();
    };
    items
        .iter()
        .filter_map(|v| {
            let sid = v["sessionId"].as_str()?.to_string();
            let background = (v["kind"] == "background")
                .then(|| v["id"].as_str().map(String::from))
                .flatten();
            // Not `status`: Claude reports "busy" while a background shell
            // runs, even after its turn has ended.
            let state = v["state"].as_str().map(String::from);
            let pid = v["pid"].as_u64().and_then(|p| u32::try_from(p).ok());
            Some((
                sid,
                ClaudeLive {
                    background,
                    state,
                    pid,
                },
            ))
        })
        .collect()
}

fn claude_shells() -> HashMap<u32, usize> {
    let Ok(out) = Command::new("ps")
        .args(["-A", "-o", "ppid=,command="])
        .output()
    else {
        return HashMap::new();
    };
    parse_shells(&String::from_utf8_lossy(&out.stdout))
}

/// Count Claude's shells by parent pid in `ps -o ppid=,command=` output.
/// Claude runs every shell command as `<shell> -c source
/// ~/.claude/shell-snapshots/...`, which nothing else it starts (MCP servers,
/// hooks) does.
fn parse_shells(ps: &str) -> HashMap<u32, usize> {
    let mut shells = HashMap::new();
    for line in ps.lines() {
        let Some((ppid, command)) = line.trim_start().split_once(char::is_whitespace) else {
            continue;
        };
        let Ok(ppid) = ppid.parse() else {
            continue;
        };
        if command.contains(" -c source ") && command.contains("/.claude/shell-snapshots/") {
            *shells.entry(ppid).or_insert(0) += 1;
        }
    }
    shells
}

/// Stop a background Claude session with `claude stop`, giving up after a few
/// seconds so a hung `claude` cannot freeze the UI.
pub fn stop_background(id: &str) -> anyhow::Result<()> {
    let child = Command::new("claude")
        .args(["stop", id])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    let out = ghostty::wait_output(child, Duration::from_secs(5))?;
    if !out.status.success() {
        anyhow::bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

pub fn snapshot() -> Snapshot {
    let claude = claude_agents();
    let shells = if claude.is_empty() {
        HashMap::new()
    } else {
        claude_shells()
    };
    Snapshot {
        claude,
        terms: ghostty::terminals(),
        shells,
    }
}

/// Whether agora is on screen: cleared while an agent it resumed has the
/// terminal. Background pollers slow down then, since nobody is looking.
/// (Not terminal focus: agora is often watched from an unfocused split.)
pub type Focus = Arc<AtomicBool>;

/// Sleep `fast` while focused and `slow` while not, measured from `since`.
/// Checks focus a few times a second, so regaining it cuts a slow wait short.
pub fn pace(focus: &AtomicBool, since: Instant, fast: Duration, slow: Duration) {
    loop {
        let every = if focus.load(Ordering::Relaxed) {
            fast
        } else {
            slow
        };
        let left = every.saturating_sub(since.elapsed());
        if left.is_zero() {
            return;
        }
        std::thread::sleep(left.min(Duration::from_millis(250)));
    }
}

pub fn spawn_poller(focus: Focus, fast: Duration, slow: Duration) -> Receiver<Snapshot> {
    let (tx, rx): (Sender<Snapshot>, _) = std::sync::mpsc::channel();
    std::thread::spawn(move || loop {
        let started = Instant::now();
        if tx.send(snapshot()).is_err() {
            return;
        }
        pace(&focus, started, fast, slow);
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

/// Map session index -> live state. When several sessions share a title, the
/// most recent one claims the tab; `sessions` may be in any order.
pub fn resolve(sessions: &[Session], snap: &Snapshot) -> HashMap<usize, Live> {
    let mut live: HashMap<usize, Live> = HashMap::new();

    for term in &snap.terms {
        let (title, glyph) = ghostty::parse_title(&term.title);
        let fits = |(i, s): &(usize, &Session)| {
            !live.contains_key(i)
                && (term.cwd.is_empty() || term.cwd == s.cwd || term.cwd == s.dir)
                && title_matches(&title, s)
        };
        // A Claude session that `claude agents` reports running beats a newer
        // past one with the same title.
        // `min_by_key` keeps the first of equals, so ties go to list order.
        let newest = |(_, s): &(usize, &Session)| std::cmp::Reverse(s.mtime);
        let hit = sessions
            .iter()
            .enumerate()
            .filter(fits)
            .filter(|(_, s)| s.agent == Agent::Claude && snap.claude.contains_key(&s.id))
            .min_by_key(newest)
            .or_else(|| sessions.iter().enumerate().filter(fits).min_by_key(newest));
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
                    shells: 0,
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
            shells: 0,
        });
        entry.state = entry.state.min(state);
        entry.background = c.background.clone();
        entry.shells = c
            .pid
            .and_then(|p| snap.shells.get(&p))
            .copied()
            .unwrap_or(0);
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
            dir: cwd.into(),
            title: title.into(),
            mtime: SystemTime::UNIX_EPOCH,
            size: 0,
            last_prompt: String::new(),
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
                pid: None,
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

    #[test]
    fn a_running_claude_session_keeps_its_tab() {
        // The newer session shares the title but is not the one running.
        let sessions = vec![
            s(Agent::Claude, "c-new", "Fix bug", "/w/a"),
            s(Agent::Claude, "c-run", "Fix bug", "/w/a"),
        ];
        let mut snap = Snapshot {
            terms: vec![term("T1", "✳ Fix bug", "/w/a")],
            ..Default::default()
        };
        snap.claude.insert("c-run".into(), ClaudeLive::default());
        let live = resolve(&sessions, &snap);
        assert_eq!(live[&1].term.as_ref().map(|t| t.id.as_str()), Some("T1"));
        assert!(!live.contains_key(&0), "{live:?}");
    }

    #[test]
    fn the_newest_session_claims_a_shared_title_wherever_it_sits() {
        // A session that appeared since startup is appended to the list.
        let mut old = s(Agent::Codex, "x-old", "Reply hi", "/w/b");
        let mut new = s(Agent::Codex, "x-new", "Reply hi", "/w/b");
        old.mtime = SystemTime::UNIX_EPOCH + Duration::from_secs(10);
        new.mtime = SystemTime::UNIX_EPOCH + Duration::from_secs(20);
        let snap = Snapshot {
            terms: vec![term("T1", "Reply hi | b", "/w/b")],
            ..Default::default()
        };
        let live = resolve(&[old, new], &snap);
        assert!(live.contains_key(&1), "{live:?}");
        assert!(!live.contains_key(&0));
    }

    #[test]
    fn pace_waits_longer_without_focus() {
        let focus = AtomicBool::new(true);
        let t = Instant::now();
        pace(
            &focus,
            t,
            Duration::from_millis(20),
            Duration::from_secs(60),
        );
        assert!(t.elapsed() < Duration::from_secs(1));
        focus.store(false, Ordering::Relaxed);
        let t = Instant::now();
        std::thread::scope(|sc| {
            sc.spawn(|| {
                std::thread::sleep(Duration::from_millis(100));
                focus.store(true, Ordering::Relaxed);
            });
            pace(
                &focus,
                t,
                Duration::from_millis(20),
                Duration::from_secs(60),
            );
        });
        let took = t.elapsed();
        assert!(took >= Duration::from_millis(100) && took < Duration::from_secs(2));
    }

    #[test]
    fn claude_states() {
        for s in ["blocked", "needs_input", "waiting", "input"] {
            assert_eq!(claude_state(s), State::NeedsYou, "{s}");
        }
        for s in ["running", "busy", "working"] {
            assert_eq!(claude_state(s), State::Busy, "{s}");
        }
        for s in ["idle", "", "Running"] {
            assert_eq!(claude_state(s), State::Open, "{s}");
        }
        assert!(State::NeedsYou < State::Busy && State::Busy < State::Open);
    }

    #[test]
    fn agents_json() {
        assert!(parse_agents(b"").is_empty());
        assert!(parse_agents(b"not json").is_empty());
        assert!(parse_agents(br#"{"sessionId":"x"}"#).is_empty());
        let a = parse_agents(
            br#"[
                {"sessionId":"s1","kind":"interactive","id":"i1","state":"running"},
                {"sessionId":"s2","kind":"background","id":"b2","state":"blocked"},
                {"sessionId":"s3","kind":"background"},
                {"kind":"background","id":"orphan"},
                {"sessionId":"s4"}
            ]"#,
        );
        assert_eq!(a.len(), 4);
        assert_eq!(
            a["s1"].background, None,
            "interactive ids are not attachable"
        );
        assert_eq!(a["s1"].state.as_deref(), Some("running"));
        assert_eq!(a["s2"].background.as_deref(), Some("b2"));
        assert_eq!(a["s3"].background, None);
        assert_eq!(a["s4"].state, None);
    }

    #[test]
    fn agents_json_pid_and_no_status() {
        let a = parse_agents(
            br#"[{"pid":29965,"sessionId":"s1","kind":"interactive","status":"busy"}]"#,
        );
        assert_eq!(a["s1"].state, None);
        assert_eq!(a["s1"].pid, Some(29965));
    }

    #[test]
    fn counts_claude_shells_by_parent() {
        let ps = "\
    1 /sbin/launchd
29965 uv tool uvx pagerduty-mcp --enable-write-tools
29965 /bin/zsh -c source /Users/me/.claude/shell-snapshots/snapshot-zsh-1-a.sh 2>/dev/null || true && eval 'sleep 45'
29965 /bin/bash -c source /Users/me/.claude/shell-snapshots/snapshot-bash-1-b.sh && make
87206 sleep 45
  412 /bin/zsh -c source /Users/me/.claude/shell-snapshots/snapshot-zsh-2-c.sh
  500 vim /Users/me/.claude/shell-snapshots/snapshot-zsh-1-a.sh
garbage
";
        let shells = parse_shells(ps);
        assert_eq!(shells.get(&29965), Some(&2));
        assert_eq!(shells.get(&412), Some(&1));
        assert_eq!(shells.len(), 2, "{shells:?}");
    }

    #[test]
    fn claude_sessions_carry_their_shell_count() {
        let sessions = vec![
            s(Agent::Claude, "c1", "One", "/w"),
            s(Agent::Claude, "c2", "Two", "/w"),
        ];
        let mut snap = Snapshot::default();
        for (id, pid) in [("c1", 10), ("c2", 20)] {
            snap.claude.insert(
                id.into(),
                ClaudeLive {
                    state: Some("idle".into()),
                    pid: Some(pid),
                    ..Default::default()
                },
            );
        }
        snap.shells.insert(10, 2);
        let live = resolve(&sessions, &snap);
        assert_eq!(live[&0].shells, 2);
        assert_eq!(live[&1].shells, 0);
    }

    #[test]
    fn tab_title_matching() {
        let fix = s(Agent::Claude, "c", "Fix bug", "/w");
        assert!(title_matches("Fix bug", &fix));
        assert!(title_matches("Fix bug | proj", &fix));
        assert!(!title_matches("Fix bugs", &fix));
        assert!(!title_matches("Fix bug|proj", &fix));
        assert!(!title_matches("fix bug", &fix));
        assert!(!title_matches("Fix", &fix));
        let untitled = s(Agent::Claude, "c", "", "/w");
        assert!(!title_matches("", &untitled));
        assert!(!title_matches(" | proj", &untitled));
    }

    #[test]
    fn nothing_running() {
        let sessions = vec![s(Agent::Claude, "c", "T", "/w")];
        assert!(resolve(&sessions, &Snapshot::default()).is_empty());
        assert!(resolve(&[], &Snapshot::default()).is_empty());
    }

    #[test]
    fn tabs_without_a_cwd_match_any_session() {
        let sessions = vec![s(Agent::Pi, "p", "Spinning", "/w/p")];
        let snap = Snapshot {
            terms: vec![term("T", "⠋ Spinning", "")],
            ..Default::default()
        };
        let live = resolve(&sessions, &snap);
        assert_eq!(live[&0].state, State::Busy);
        assert_eq!(live[&0].background, None);
    }

    #[test]
    fn each_tab_claims_the_newest_unclaimed_session() {
        let sessions = vec![
            s(Agent::Claude, "new", "Same", "/w"),
            s(Agent::Claude, "old", "Same", "/w"),
            s(Agent::Claude, "older", "Same", "/w"),
        ];
        let snap = Snapshot {
            terms: vec![term("T1", "Same", "/w"), term("T2", "✳ Same", "/w")],
            ..Default::default()
        };
        let live = resolve(&sessions, &snap);
        assert_eq!(live[&0].term.as_ref().unwrap().id, "T1");
        assert_eq!(live[&1].term.as_ref().unwrap().id, "T2");
        assert!(!live.contains_key(&2));
        assert!(live.values().all(|l| l.state == State::Open));
    }

    #[test]
    fn claude_agents_refine_tab_state() {
        let sessions = vec![
            s(Agent::Claude, "c1", "One", "/w"),
            s(Agent::Claude, "c2", "Two", "/w"),
            s(Agent::Codex, "c3", "Three", "/w"),
            s(Agent::Claude, "c4", "Four", "/w"),
        ];
        let mut snap = Snapshot {
            terms: vec![term("T1", "One", "/w"), term("T2", "🔔 Two", "/w")],
            ..Default::default()
        };
        let agent = |state: Option<&str>| ClaudeLive {
            state: state.map(String::from),
            ..Default::default()
        };
        // An open tab whose agent reports work in progress is busy.
        snap.claude.insert("c1".into(), agent(Some("running")));
        // The more urgent of the two states wins.
        snap.claude.insert("c2".into(), agent(Some("running")));
        // Session ids from `claude agents` only apply to Claude sessions.
        snap.claude.insert("c3".into(), agent(Some("blocked")));
        // A running agent with no state is open, even without a tab.
        snap.claude.insert("c4".into(), agent(None));

        let live = resolve(&sessions, &snap);
        assert_eq!(live[&0].state, State::Busy);
        assert_eq!(live[&0].term.as_ref().unwrap().id, "T1");
        assert_eq!(live[&1].state, State::NeedsYou);
        assert!(!live.contains_key(&2));
        assert_eq!(live[&3].state, State::Open);
        assert!(live[&3].term.is_none());
    }
}
