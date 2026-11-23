//! Git context for session folders: branch, uncommitted changes, ahead/behind.
//!
//! A background thread runs `git status` and sends back what it found. Hot
//! folders (live sessions, the selection) are checked every round; the rest
//! once at first sight and then only every few minutes. Folders that are gone
//! or are not repositories are left out.

use crate::live::{pace, Focus};
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Git {
    pub branch: String,
    /// Changed, staged and untracked files.
    pub dirty: usize,
    pub ahead: usize,
    pub behind: usize,
}

/// Parse `git status --porcelain=v1 --branch`.
pub fn parse(out: &str) -> Git {
    let mut g = Git::default();
    for line in out.lines() {
        let Some(head) = line.strip_prefix("## ") else {
            if !line.is_empty() {
                g.dirty += 1;
            }
            continue;
        };
        let (name, counts) = match head.split_once(" [") {
            Some((n, c)) => (n, c.trim_end_matches(']')),
            None => (head, ""),
        };
        let name = name
            .strip_prefix("No commits yet on ")
            .or_else(|| name.strip_prefix("Initial commit on "))
            .unwrap_or(name);
        g.branch = match name.split_once("...") {
            Some((local, _)) => local.to_string(),
            None if name.starts_with("HEAD") => "detached".into(),
            None => name.to_string(),
        };
        for part in counts.split(", ") {
            if let Some(n) = part.strip_prefix("ahead ") {
                g.ahead = n.parse().unwrap_or(0);
            } else if let Some(n) = part.strip_prefix("behind ") {
                g.behind = n.parse().unwrap_or(0);
            }
        }
    }
    g
}

/// A `git status` that takes longer than this is killed: a huge or wedged
/// repository must not hold up every other folder.
const TIMEOUT: Duration = Duration::from_secs(5);

/// Git context for one folder, or `None` if it isn't in a repository (or git
/// took too long).
pub fn status(dir: &str) -> Option<Git> {
    if !Path::new(dir).is_dir() {
        return None;
    }
    let mut child = Command::new("git")
        .args(["-C", dir, "status", "--porcelain=v1", "--branch"])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    // Read on another thread, so a full pipe cannot stall the wait below.
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    let deadline = Instant::now() + TIMEOUT;
    let ok = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st.success(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
    };
    let out = reader.join().ok()?;
    ok.then(|| parse(&String::from_utf8_lossy(&out)))
}

/// Folders to check; the app keeps this up to date.
#[derive(Debug, Default, PartialEq)]
pub struct Folders {
    /// Checked every round: live sessions' folders and the selection's.
    pub hot: Vec<String>,
    /// Every folder worth knowing about, most recent first.
    pub all: Vec<String>,
}

pub type Wanted = Arc<Mutex<Folders>>;

/// How often a folder that isn't hot is checked again.
const COLD_EVERY: Duration = Duration::from_secs(300);

/// Each round, check the hot folders and any others not checked for
/// `COLD_EVERY`, sending each result as it comes; rounds start `fast` apart
/// while agora has focus and `slow` apart while it doesn't.
pub fn spawn_poller(
    wanted: Wanted,
    focus: Focus,
    fast: Duration,
    slow: Duration,
) -> Receiver<(String, Option<Git>)> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut checked: HashMap<String, Instant> = HashMap::new();
        loop {
            let started = Instant::now();
            let dirs = due(&wanted, &checked);
            // Nothing yet at startup: look again soon.
            if dirs.is_empty() && checked.is_empty() {
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            for d in dirs {
                let g = status(&d);
                checked.insert(d.clone(), Instant::now());
                if tx.send((d, g)).is_err() {
                    return;
                }
            }
            pace(&focus, started, fast, slow);
        }
    });
    rx
}

/// The folders to check this round: hot ones, then cold ones never checked
/// or not checked for `COLD_EVERY`.
fn due(wanted: &Wanted, checked: &HashMap<String, Instant>) -> Vec<String> {
    let Ok(w) = wanted.lock() else {
        return Vec::new();
    };
    let mut dirs = w.hot.clone();
    for d in &w.all {
        let stale = checked.get(d).is_none_or(|t| t.elapsed() >= COLD_EVERY);
        if stale && !dirs.contains(d) {
            dirs.push(d.clone());
        }
    }
    dirs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_branch_counts_and_changes() {
        let g = parse("## main...origin/main [ahead 2, behind 1]\n M src/a.rs\n?? new.txt\n");
        assert_eq!(
            g,
            Git {
                branch: "main".into(),
                dirty: 2,
                ahead: 2,
                behind: 1,
            }
        );
        assert_eq!(parse("## feat/x\n").branch, "feat/x");
        assert_eq!(parse("## HEAD (no branch)\n").branch, "detached");
        assert_eq!(parse("## No commits yet on main\n").branch, "main");
        assert_eq!(parse("## main...origin/main [gone]\n").ahead, 0);
    }

    #[test]
    fn hot_folders_every_round_cold_ones_when_stale() {
        let wanted: Wanted = Arc::default();
        *wanted.lock().unwrap() = Folders {
            hot: vec!["/a".into()],
            all: vec!["/a".into(), "/b".into(), "/c".into()],
        };
        let mut checked = HashMap::new();
        assert_eq!(due(&wanted, &checked), ["/a", "/b", "/c"]);
        for d in ["/a", "/b", "/c"] {
            checked.insert(d.to_string(), Instant::now());
        }
        assert_eq!(due(&wanted, &checked), ["/a"]);
        checked.insert("/c".into(), Instant::now() - COLD_EVERY);
        assert_eq!(due(&wanted, &checked), ["/a", "/c"]);
    }

    #[test]
    fn a_folder_outside_git_has_no_status() {
        assert_eq!(status("/nonexistent/agora"), None);
        assert_eq!(status(&std::env::temp_dir().to_string_lossy()), None);
    }
}
