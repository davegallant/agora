//! The view agora reopens with, kept in `~/.local/state/agora/state` as
//! `key = value` lines: `channel`, `agent`, `live` and `sort`. Unknown keys and
//! bad values are ignored, so each field falls back to its default on its own.

use crate::sessions::Agent;
use std::fs;
use std::path::{Path, PathBuf};

/// The filters and sort agora reopens with.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ViewState {
    pub channel: Option<String>,
    pub agent: Option<Agent>,
    pub live_only: bool,
    pub sort: Sort,
}

fn path(home: &Path) -> PathBuf {
    crate::paths::state(home, "state")
}

pub fn load(home: &Path) -> ViewState {
    let mut v = ViewState::default();
    let text = fs::read_to_string(path(home)).unwrap_or_default();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "channel" if !value.is_empty() => v.channel = Some(value.to_string()),
            "agent" => v.agent = Agent::from_name(value),
            "live" => v.live_only = value == "true",
            "sort" => v.sort = Sort::from_name(value).unwrap_or_default(),
            _ => {}
        }
    }
    v
}

pub fn save(home: &Path, v: &ViewState) -> std::io::Result<()> {
    let p = path(home);
    if let Some(dir) = p.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut out = String::new();
    if let Some(c) = &v.channel {
        out += &format!("channel = {c}\n");
    }
    if let Some(a) = v.agent {
        out += &format!("agent = {}\n", a.name());
    }
    out += &format!("live = {}\nsort = {}\n", v.live_only, v.sort.name());
    // Write then rename, so two agora windows never leave half a file.
    let tmp = p.with_extension(format!("tmp{}", std::process::id()));
    fs::write(&tmp, out)?;
    fs::rename(tmp, p)
}

/// How the list is ordered under the live sessions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sort {
    /// Newest first.
    #[default]
    Recent,
    /// By project name, newest first within each.
    Project,
}

impl Sort {
    const ALL: [Sort; 2] = [Sort::Recent, Sort::Project];

    pub fn next(self) -> Sort {
        let i = Self::ALL.iter().position(|&s| s == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }

    pub fn name(self) -> &'static str {
        match self {
            Sort::Recent => "recent",
            Sort::Project => "project",
        }
    }

    pub fn from_name(name: &str) -> Option<Sort> {
        Self::ALL.into_iter().find(|s| s.name() == name.trim())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::Agent;

    fn home(tag: &str) -> std::path::PathBuf {
        let h = std::env::temp_dir().join(format!("agora-state-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&h);
        h
    }

    #[test]
    fn saves_and_loads_every_field() {
        let h = home("roundtrip");
        let v = ViewState {
            channel: Some("work".into()),
            agent: Some(Agent::Codex),
            live_only: true,
            sort: Sort::Project,
        };
        save(&h, &v).unwrap();
        assert_eq!(load(&h), v);
        save(&h, &ViewState::default()).unwrap();
        assert_eq!(load(&h), ViewState::default());
        fs::remove_dir_all(&h).unwrap();
    }

    #[test]
    fn bad_or_missing_lines_fall_back_field_by_field() {
        let h = home("bad");
        assert_eq!(load(&h), ViewState::default(), "no file yet");
        fs::create_dir_all(h.join(".local/state/agora")).unwrap();
        fs::write(
            h.join(".local/state/agora/state"),
            "agent = nope\nlive = true\ngarbage\nsort = size\nchannel =   \n",
        )
        .unwrap();
        assert_eq!(
            load(&h),
            ViewState {
                channel: None,
                agent: None,
                live_only: true,
                sort: Sort::Recent,
            }
        );
        fs::remove_dir_all(&h).unwrap();
    }

    #[test]
    fn sorts_cycle_and_round_trip_by_name() {
        assert_eq!(Sort::Recent.next(), Sort::Project);
        assert_eq!(Sort::Project.next(), Sort::Recent);
        assert_eq!(Sort::from_name("size"), None, "no longer a sort");
        for s in Sort::ALL {
            assert_eq!(Sort::from_name(s.name()), Some(s));
        }
        assert_eq!(Sort::from_name("nope"), None);
    }
}
