//! Channels: named groups of sessions, decided by where each session ran.
//!
//! Rules live in `~/.config/agora/channels`, one channel per line:
//!
//! ```text
//! # name = folder, folder, ...
//! work   = ~/src/github.com/Unity-Technologies/*, ~/src/github.com/davegallant/pipe-*
//! agora  = ~/src/github.com/davegallant/agora
//! ```
//!
//! A folder covers itself and everything under it, and `*` matches any run of
//! characters within one path segment. The first matching line wins; a
//! session no line matches is in a channel named after its folder.

use std::cmp::Reverse;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::SystemTime;

#[derive(Default, Debug)]
pub struct Channels {
    rules: Vec<(String, Vec<Vec<String>>)>,
}

/// Path segments, ignoring empty ones (leading, trailing and doubled slashes).
fn segments(p: &str) -> Vec<&str> {
    p.split('/').filter(|s| !s.is_empty()).collect()
}

/// Whether `text` matches `pat`, where `*` stands for any run of characters.
fn glob(pat: &str, text: &str) -> bool {
    let mut parts = pat.split('*');
    let first = parts.next().unwrap_or("");
    let Some(mut rest) = text.strip_prefix(first) else {
        return false;
    };
    let parts: Vec<&str> = parts.collect();
    let Some((last, middle)) = parts.split_last() else {
        return rest.is_empty();
    };
    for p in middle {
        match rest.find(p) {
            Some(at) => rest = &rest[at + p.len()..],
            None => return false,
        }
    }
    rest.len() >= last.len() && rest.ends_with(last)
}

impl Channels {
    /// Read the rules file under `home`; a missing or unreadable file means no rules.
    pub fn load(home: &Path) -> Self {
        let path = home.join(".config/agora/channels");
        fs::read_to_string(path)
            .map(|text| Self::parse(&text, home))
            .unwrap_or_default()
    }

    /// Parse rules, expanding a leading `~` to `home`. Lines that aren't
    /// `name = folders` are skipped.
    pub fn parse(text: &str, home: &Path) -> Self {
        let home = home.to_string_lossy();
        let rules = text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .filter_map(|l| {
                let (name, folders) = l.split_once('=')?;
                let name = name.trim();
                let pats: Vec<Vec<String>> = folders
                    .split(',')
                    .map(str::trim)
                    .filter(|f| !f.is_empty())
                    .map(|f| match f.strip_prefix('~') {
                        Some(rest) => format!("{home}{rest}"),
                        None => f.to_string(),
                    })
                    .map(|f| segments(&f).into_iter().map(String::from).collect())
                    .filter(|segs: &Vec<String>| !segs.is_empty())
                    .collect();
                (!name.is_empty() && !pats.is_empty()).then(|| (name.to_string(), pats))
            })
            .collect();
        Channels { rules }
    }

    /// Configured channel names, in the order they were written.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.rules.iter().map(|(n, _)| n.as_str())
    }

    /// Channels that have sessions, with how many each has: configured ones
    /// in the order written, then the rest by their most recent session.
    /// Takes each session's channel and modification time.
    pub fn ordered<'a>(
        &self,
        sessions: impl IntoIterator<Item = (&'a str, SystemTime)>,
    ) -> Vec<(String, usize)> {
        let mut seen: HashMap<&str, (usize, SystemTime)> = HashMap::new();
        for (c, t) in sessions {
            let e = seen.entry(c).or_insert((0, t));
            e.0 += 1;
            e.1 = e.1.max(t);
        }
        let mut out: Vec<(String, usize)> = self
            .names()
            .filter_map(|n| seen.remove(n).map(|(count, _)| (n.to_string(), count)))
            .collect();
        let mut rest: Vec<(&str, (usize, SystemTime))> = seen.into_iter().collect();
        rest.sort_by_key(|&(name, (_, t))| (Reverse(t), name));
        out.extend(
            rest.into_iter()
                .map(|(n, (count, _))| (n.to_string(), count)),
        );
        out
    }

    /// The channel of a session that ran in `cwd`.
    pub fn of(&self, cwd: &str) -> String {
        let segs = segments(cwd);
        for (name, pats) in &self.rules {
            let hit = pats.iter().any(|pat| {
                pat.len() <= segs.len() && pat.iter().zip(&segs).all(|(p, s)| glob(p, s))
            });
            if hit {
                return name.clone();
            }
        }
        segs.last()
            .map_or_else(|| cwd.to_string(), |s| s.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_stars_match_within_a_segment() {
        assert!(glob("pipe-*", "pipe-960"));
        assert!(glob("pipe-*", "pipe-"));
        assert!(!glob("pipe-*", "pip"));
        assert!(glob("*", "anything"));
        assert!(glob("a*b*c", "aXbYc"));
        assert!(!glob("a*b*c", "aXbY"));
        assert!(glob("*-api", "build-api"));
        assert!(!glob("ab*ba", "aba"), "the ends may not overlap");
        assert!(glob("exact", "exact"));
        assert!(!glob("exact", "exactly"));
    }

    #[test]
    fn rules_cover_folders_and_their_descendants_first_match_wins() {
        let c = Channels::parse(
            "# comment\n\
             \n\
             work = ~/src/corp/*, ~/src/me/pipe-*\n\
             agora=~/src/me/agora/\n\
             all-src = ~/src\n\
             not a rule\n\
             empty =\n\
             = /orphan\n",
            Path::new("/home/d"),
        );
        assert_eq!(c.names().collect::<Vec<_>>(), ["work", "agora", "all-src"]);
        assert_eq!(c.of("/home/d/src/corp/api"), "work");
        assert_eq!(c.of("/home/d/src/corp/api/sub/dir"), "work");
        assert_eq!(c.of("/home/d/src/me/pipe-960"), "work");
        assert_eq!(c.of("/home/d/src/me/agora"), "agora");
        assert_eq!(c.of("/home/d/src/me/agora/src"), "agora");
        assert_eq!(c.of("/home/d/src/me/other"), "all-src");
        assert_eq!(c.of("/home/d/src/corp"), "all-src", "`*` needs a segment");
        assert_eq!(c.of("/tmp/scratch"), "scratch");
        assert_eq!(c.of("/"), "/");
    }

    #[test]
    fn ordered_puts_configured_first_then_recent_with_counts() {
        let c = Channels::parse("b = /b\nunused = /u\na = /a\n", Path::new("/h"));
        let t = |s| SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(s);
        let got = c.ordered([
            ("old", t(1)),
            ("a", t(5)),
            ("new", t(9)),
            ("b", t(2)),
            ("old", t(3)),
            ("a", t(4)),
        ]);
        assert_eq!(
            got,
            [
                ("b".to_string(), 1),
                ("a".into(), 2),
                ("new".into(), 1),
                ("old".into(), 2),
            ]
        );
    }

    #[test]
    fn without_rules_the_channel_is_the_folder() {
        let c = Channels::load(Path::new("/nonexistent"));
        assert_eq!(c.names().count(), 0);
        assert_eq!(c.of("/w/proj"), "proj");
    }
}
