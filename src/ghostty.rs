//! Ghostty tabs via its AppleScript dictionary (macOS, Ghostty 1.3+).

use anyhow::{bail, Context, Result};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

#[derive(Clone, Debug, PartialEq)]
pub struct Term {
    pub id: String,
    pub title: String,
    pub cwd: String,
}

/// Is Ghostty installed? Checked once: scripting a missing app fails on every
/// poll, and `osascript` may ask the user where it is.
pub fn available() -> bool {
    static INSTALLED: OnceLock<bool> = OnceLock::new();
    *INSTALLED.get_or_init(|| cfg!(target_os = "macos") && installed())
}

fn installed() -> bool {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let dirs = [
        Some(PathBuf::from("/Applications")),
        home.map(|h| h.join("Applications")),
    ];
    if dirs
        .iter()
        .flatten()
        .any(|d| d.join("Ghostty.app").is_dir())
    {
        return true;
    }
    // Elsewhere, e.g. nix-darwin's "Nix Apps": ask Spotlight by bundle id.
    Command::new("mdfind")
        .arg("kMDItemCFBundleIdentifier == 'com.mitchellh.ghostty'")
        .stderr(Stdio::null())
        .output()
        .is_ok_and(|o| o.status.success() && !o.stdout.trim_ascii().is_empty())
}

fn osascript(script: &str, args: &[&str]) -> Result<String> {
    let mut child = Command::new("osascript")
        .arg("-")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("running osascript")?;
    child
        .stdin
        .take()
        .context("osascript stdin")?
        .write_all(script.as_bytes())?;
    let out = child.wait_with_output()?;
    if !out.status.success() {
        bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Every Ghostty terminal surface: id, title and working directory.
pub fn terminals() -> Vec<Term> {
    if !available() {
        return Vec::new();
    }
    // `tab` is a Ghostty class inside the tell block, so spell out the character.
    const SCRIPT: &str = r#"
set sep to character id 9
if application "Ghostty" is not running then return ""
tell application "Ghostty"
  set out to ""
  repeat with t in terminals
    set out to out & (id of t) & sep & (name of t) & sep & (working directory of t) & linefeed
  end repeat
  return out
end tell"#;
    parse_terminals(&osascript(SCRIPT, &[]).unwrap_or_default())
}

/// Parse the script's output: one `id<TAB>title<TAB>cwd` line per terminal.
fn parse_terminals(out: &str) -> Vec<Term> {
    out.lines()
        .filter_map(|l| {
            let mut f = l.splitn(3, '\t');
            Some(Term {
                id: f.next()?.to_string(),
                title: f.next()?.to_string(),
                cwd: f.next().unwrap_or("").to_string(),
            })
        })
        .filter(|t| !t.id.is_empty())
        .collect()
}

pub fn focus(id: &str) -> Result<()> {
    const SCRIPT: &str = r#"
on run argv
  tell application "Ghostty"
    focus (first terminal whose id is (item 1 of argv))
    activate
  end tell
end run"#;
    osascript(SCRIPT, &[id]).map(|_| ())
}

/// Open a new tab in the front window (or a new window) running `cmd` in `cwd`.
pub fn new_tab(cwd: &str, cmd: &str) -> Result<()> {
    const SCRIPT: &str = r#"
on run argv
  set {d, c} to argv
  tell application "Ghostty"
    set cfg to {initial working directory:d, initial input:c & linefeed}
    if (count of windows) is 0 then
      new window with configuration cfg
    else
      new tab in front window with configuration cfg
    end if
    activate
  end tell
end run"#;
    osascript(SCRIPT, &[cwd, cmd]).map(|_| ())
}

/// Status glyphs agents put in front of their title.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    Bell,
    Busy,
    None,
}

/// Split a tab title into its leading status glyphs and the bare title.
/// Ghostty prefixes 🔔 on a bell; Claude shows a spinner while working and
/// ✳ when idle.
pub fn parse_title(raw: &str) -> (String, Glyph) {
    const SPINNER: &[char] = &[
        '◐', '◓', '◑', '◒', '⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏', '·', '✢', '✶', '✻',
        '✽',
    ];
    let mut glyph = Glyph::None;
    let mut rest = raw;
    while let Some(c) = rest.chars().next() {
        if c.is_alphanumeric() || c == '~' || c == '/' {
            break;
        }
        if c == '🔔' {
            glyph = Glyph::Bell;
        } else if SPINNER.contains(&c) && glyph == Glyph::None {
            glyph = Glyph::Busy;
        }
        rest = &rest[c.len_utf8()..];
    }
    (rest.trim().to_string(), glyph)
}

/// Quote an argument for the shell the new tab runs (fish or POSIX).
pub fn shell_quote(s: &str) -> String {
    let safe = |c: char| c.is_ascii_alphanumeric() || "_-./:@%+=,".contains(c);
    if !s.is_empty() && s.chars().all(safe) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_titles() {
        assert_eq!(
            parse_title("🔔 ✳ Fix the bug"),
            ("Fix the bug".into(), Glyph::Bell)
        );
        assert_eq!(
            parse_title("◐ Ghostty setup"),
            ("Ghostty setup".into(), Glyph::Busy)
        );
        assert_eq!(parse_title("✳ Idle one"), ("Idle one".into(), Glyph::None));
        assert_eq!(
            parse_title("~/s/g/d/nix-config"),
            ("~/s/g/d/nix-config".into(), Glyph::None)
        );
    }

    #[test]
    fn quoting() {
        assert_eq!(shell_quote("abc-1.2"), "abc-1.2");
        assert_eq!(shell_quote("/a b/c"), "'/a b/c'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
    }

    #[test]
    fn more_titles() {
        assert_eq!(parse_title(""), (String::new(), Glyph::None));
        assert_eq!(parse_title("🔔"), (String::new(), Glyph::Bell));
        assert_eq!(parse_title("⠋ Working"), ("Working".into(), Glyph::Busy));
        assert_eq!(parse_title("· dots"), ("dots".into(), Glyph::Busy));
        // A bell wins over a spinner, whichever comes first.
        assert_eq!(parse_title("⠋ 🔔 x"), ("x".into(), Glyph::Bell));
        assert_eq!(parse_title("🔔 ⠋ x"), ("x".into(), Glyph::Bell));
        assert_eq!(parse_title("  padded  "), ("padded".into(), Glyph::None));
        assert_eq!(parse_title("/tmp"), ("/tmp".into(), Glyph::None));
        assert_eq!(parse_title("1 thing"), ("1 thing".into(), Glyph::None));
        // Glyphs inside the title are part of it.
        assert_eq!(parse_title("a ◐ b"), ("a ◐ b".into(), Glyph::None));
        assert_eq!(parse_title("Été"), ("Été".into(), Glyph::None));
    }

    #[test]
    fn more_quoting() {
        assert_eq!(shell_quote(""), "''");
        assert_eq!(shell_quote("_-./:@%+=,"), "_-./:@%+=,");
        for s in ["$HOME", "a;b", "a*", "日本", "a\nb", "`x`", "~"] {
            assert_eq!(shell_quote(s), format!("'{s}'"), "{s:?}");
        }
        assert_eq!(shell_quote("''"), r"''\'''\'''");
    }

    #[test]
    fn terminal_listing() {
        let out = "id1\tFix bug\t/w/a\nid2\tno cwd\n\tno id\t/w\nbare\nid3\ttab\tin\tcwd\n";
        assert_eq!(
            parse_terminals(out),
            [
                Term {
                    id: "id1".into(),
                    title: "Fix bug".into(),
                    cwd: "/w/a".into()
                },
                Term {
                    id: "id2".into(),
                    title: "no cwd".into(),
                    cwd: String::new()
                },
                Term {
                    id: "id3".into(),
                    title: "tab".into(),
                    cwd: "in\tcwd".into()
                },
            ]
        );
        assert!(parse_terminals("").is_empty());
    }
}
