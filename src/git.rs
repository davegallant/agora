//! Git context for session folders: branch, uncommitted changes, ahead/behind.
//!
//! A background thread runs `git status` in each folder it is given, a few
//! seconds apart, and sends back what it found. Folders that are gone or are
//! not repositories are left out.

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::Duration;

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

/// Git context for one folder, or `None` if it isn't in a repository.
pub fn status(dir: &str) -> Option<Git> {
    if !Path::new(dir).is_dir() {
        return None;
    }
    let out = Command::new("git")
        .args(["-C", dir, "status", "--porcelain=v1", "--branch"])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| parse(&String::from_utf8_lossy(&out.stdout)))
}

/// Folders to check, most important first; the app keeps this up to date.
pub type Wanted = Arc<Mutex<Vec<String>>>;

/// Check every wanted folder, sending each result as it comes, then start
/// over `every` later. Waits until there are folders to check.
pub fn spawn_poller(wanted: Wanted, every: Duration) -> Receiver<(String, Option<Git>)> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || loop {
        let dirs = wanted.lock().map(|w| w.clone()).unwrap_or_default();
        if dirs.is_empty() {
            std::thread::sleep(Duration::from_millis(100));
            continue;
        }
        for d in dirs {
            let g = status(&d);
            if tx.send((d, g)).is_err() {
                return;
            }
        }
        std::thread::sleep(every);
    });
    rx
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
    fn a_folder_outside_git_has_no_status() {
        assert_eq!(status("/nonexistent/agora"), None);
        assert_eq!(status(&std::env::temp_dir().to_string_lossy()), None);
    }
}
