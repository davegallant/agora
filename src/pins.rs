//! Pinned sessions, kept one per line as `<agent> <id>` in
//! `~/.config/agora/pins`. Blank lines, `#` comments and unknown agents are
//! skipped.

use crate::sessions::Agent;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Default)]
pub struct Pins(BTreeSet<(&'static str, String)>);

fn path(home: &Path) -> PathBuf {
    home.join(".config/agora/pins")
}

impl Pins {
    pub fn load(home: &Path) -> Pins {
        let text = fs::read_to_string(path(home)).unwrap_or_default();
        Pins(
            text.lines()
                .filter(|l| !l.trim_start().starts_with('#'))
                .filter_map(|l| {
                    let mut words = l.split_whitespace();
                    let agent = Agent::from_name(words.next()?)?;
                    Some((agent.name(), words.next()?.to_string()))
                })
                .collect(),
        )
    }

    pub fn contains(&self, agent: Agent, id: &str) -> bool {
        self.0.contains(&(agent.name(), id.to_string()))
    }

    /// Pin or unpin; returns whether it is now pinned.
    pub fn toggle(&mut self, agent: Agent, id: &str) -> bool {
        let key = (agent.name(), id.to_string());
        if self.0.remove(&key) {
            false
        } else {
            self.0.insert(key);
            true
        }
    }

    pub fn save(&self, home: &Path) -> std::io::Result<()> {
        let p = path(home);
        if let Some(dir) = p.parent() {
            fs::create_dir_all(dir)?;
        }
        let text: String = self.0.iter().map(|(a, id)| format!("{a} {id}\n")).collect();
        // Write then rename, so a reader never sees half a file.
        let tmp = p.with_extension(format!("tmp{}", std::process::id()));
        fs::write(&tmp, text)?;
        fs::rename(tmp, p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home(tag: &str) -> PathBuf {
        let h = std::env::temp_dir().join(format!("agora-pins-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&h);
        h
    }

    #[test]
    fn toggles_saves_and_loads() {
        let h = home("roundtrip");
        let mut p = Pins::load(&h);
        assert!(!p.contains(Agent::Claude, "a"), "nothing pinned yet");
        assert!(p.toggle(Agent::Claude, "a"));
        assert!(p.toggle(Agent::Codex, "b"));
        p.save(&h).unwrap();
        let back = Pins::load(&h);
        assert!(back.contains(Agent::Claude, "a"));
        assert!(back.contains(Agent::Codex, "b"));
        assert!(
            !back.contains(Agent::Codex, "a"),
            "agent is part of the key"
        );

        assert!(!p.toggle(Agent::Claude, "a"), "toggling again unpins");
        p.save(&h).unwrap();
        let text = fs::read_to_string(path(&h)).unwrap();
        assert_eq!(text, "codex b\n");
        fs::remove_dir_all(&h).unwrap();
    }

    #[test]
    fn skips_comments_blanks_and_unknown_agents() {
        let h = home("junk");
        fs::create_dir_all(path(&h).parent().unwrap()).unwrap();
        fs::write(path(&h), "# mine\n\nnope x\npi  p1 \nclaude\n").unwrap();
        let p = Pins::load(&h);
        assert!(p.contains(Agent::Pi, "p1"));
        assert_eq!(p.0.len(), 1);
        fs::remove_dir_all(&h).unwrap();
    }
}
