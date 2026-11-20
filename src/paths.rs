//! Where agora keeps its files, following the XDG base directories.
//!
//! - Settings you edit (`channels`, `theme`): `$XDG_CONFIG_HOME/agora`, else
//!   `~/.config/agora`.
//! - State agora writes (`state`, `pins`): `$XDG_STATE_HOME/agora`, else
//!   `~/.local/state/agora`. Files left in `~/.config/agora` by older
//!   versions move here the first time they are needed.
//!
//! The XDG variables count only when absolute, as the spec says. Tests ignore
//! them, so a test can never touch a real config or state folder.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

fn base(home: &Path, var: Option<OsString>, fallback: &str) -> PathBuf {
    match var.map(PathBuf::from) {
        Some(p) if p.is_absolute() => p.join("agora"),
        _ => home.join(fallback).join("agora"),
    }
}

#[cfg(not(test))]
fn var(name: &str) -> Option<OsString> {
    std::env::var_os(name)
}

#[cfg(test)]
fn var(_: &str) -> Option<OsString> {
    None
}

/// A settings file, such as `channels`.
pub fn config(home: &Path, name: &str) -> PathBuf {
    base(home, var("XDG_CONFIG_HOME"), ".config").join(name)
}

/// A state file, such as `pins`, moved from `~/.config/agora` if it was left
/// there by an older agora.
pub fn state(home: &Path, name: &str) -> PathBuf {
    let new = base(home, var("XDG_STATE_HOME"), ".local/state").join(name);
    let old = home.join(".config/agora").join(name);
    if !new.exists() && old.is_file() {
        if let Some(dir) = new.parent() {
            let _ = fs::create_dir_all(dir);
        }
        if fs::rename(&old, &new).is_err() && fs::copy(&old, &new).is_ok() {
            let _ = fs::remove_file(&old);
        }
    }
    new
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xdg_variables_count_only_when_absolute() {
        let home = Path::new("/h");
        assert_eq!(base(home, None, ".config"), Path::new("/h/.config/agora"));
        assert_eq!(
            base(home, Some("/x/cfg".into()), ".config"),
            Path::new("/x/cfg/agora")
        );
        assert_eq!(
            base(home, Some("rel/cfg".into()), ".local/state"),
            Path::new("/h/.local/state/agora")
        );
    }

    #[test]
    fn old_state_files_move_to_the_state_folder() {
        let home = std::env::temp_dir().join(format!("agora-paths-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        fs::create_dir_all(home.join(".config/agora")).unwrap();
        fs::write(home.join(".config/agora/pins"), "claude a\n").unwrap();
        let p = state(&home, "pins");
        assert_eq!(p, home.join(".local/state/agora/pins"));
        assert_eq!(fs::read_to_string(&p).unwrap(), "claude a\n");
        assert!(
            !home.join(".config/agora/pins").exists(),
            "moved, not copied"
        );
        // A newer file in the state folder wins over a stale old one.
        fs::write(home.join(".config/agora/pins"), "stale\n").unwrap();
        assert_eq!(
            fs::read_to_string(state(&home, "pins")).unwrap(),
            "claude a\n"
        );
        fs::remove_dir_all(&home).unwrap();
    }
}
