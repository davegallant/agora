//! Run the `agora` binary against a throwaway `$HOME`, with `claude` and
//! `osascript` stubbed out so nothing touches real sessions or Ghostty.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, SystemTime};

struct Home {
    dir: PathBuf,
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn write(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn stub(dir: &Path, name: &str, script: &str) {
    let path = dir.join(name);
    write(&path, script);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn set_mtime(path: &Path, secs_ago: u64) {
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(secs_ago))
        .unwrap();
}

/// A home with a Claude, a Codex and a pi session; `claude agents` reports
/// the Claude one as blocked.
fn home(tag: &str) -> Home {
    let dir = std::env::temp_dir().join(format!("agora-cli-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);

    let claude = dir.join(".claude/projects/-w-web/c1.jsonl");
    write(
        &claude,
        concat!(
            r#"{"type":"user","cwd":"/w/web","message":{"content":"fix the login form"}}"#,
            "\n",
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"The CSRF token was stale."}]}}"#,
            "\n",
            r#"{"type":"ai-title","aiTitle":"Fix login form"}"#,
            "\n",
        ),
    );
    let codex = dir.join(".codex/sessions/2026/01/02/rollout-1.jsonl");
    write(
        &codex,
        concat!(
            r#"{"type":"session_meta","payload":{"id":"x1","cwd":"/w/api"}}"#,
            "\n",
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"speed up the csrf check"}]}}"#,
            "\n",
        ),
    );
    let pi = dir.join(".pi/agent/sessions/-w-docs/1_p1.jsonl");
    write(
        &pi,
        concat!(
            r#"{"type":"session","id":"p1","cwd":"/w/docs"}"#,
            "\n",
            r#"{"type":"message","message":{"role":"user","content":"write the readme"}}"#,
            "\n",
        ),
    );
    // Claude is the oldest; being live puts it first anyway.
    set_mtime(&claude, 3000);
    set_mtime(&codex, 2000);
    set_mtime(&pi, 1000);

    write(
        &dir.join("agents.json"),
        r#"[{"sessionId":"c1","kind":"interactive","id":"i1","state":"blocked"}]"#,
    );
    let bin = dir.join("bin");
    stub(
        &bin,
        "claude",
        "#!/bin/sh\n[ \"$1\" = agents ] && exec cat \"$HOME/agents.json\"\nexit 1\n",
    );
    stub(&bin, "osascript", "#!/bin/sh\ncat >/dev/null\n");
    stub(&bin, "mdfind", "#!/bin/sh\nexit 0\n");
    Home { dir }
}

fn agora(home: &Home, args: &[&str]) -> Output {
    let path = format!(
        "{}:{}",
        home.dir.join("bin").display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let out = Command::new(env!("CARGO_BIN_EXE_agora"))
        .args(args)
        .env("HOME", &home.dir)
        .env("PATH", path)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "agora {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}

fn stdout(out: &Output) -> String {
    String::from_utf8(out.stdout.clone()).unwrap()
}

#[test]
fn version() {
    let h = home("version");
    for flag in ["-V", "--version"] {
        let out = stdout(&agora(&h, &[flag]));
        assert_eq!(out, format!("agora {}\n", env!("CARGO_PKG_VERSION")));
    }
}

#[test]
fn help() {
    let h = home("help");
    for flag in ["-h", "--help"] {
        let out = stdout(&agora(&h, &[flag]));
        for want in [
            "usage: agora",
            "--list",
            "--search <words>",
            "  ?        show these keys",
        ] {
            assert!(out.contains(want), "missing {want:?} in:\n{out}");
        }
    }
}

#[test]
fn list_puts_live_sessions_first() {
    let h = home("list");
    let out = stdout(&agora(&h, &["--list"]));
    let rows: Vec<Vec<&str>> = out.lines().map(|l| l.split('\t').collect()).collect();
    assert_eq!(
        rows,
        [
            vec!["NeedsYou", "claude", "web", "Fix login form"],
            vec!["-", "pi", "docs", "write the readme"],
            vec!["-", "codex", "api", "speed up the csrf check"],
        ]
    );
}

#[test]
fn channels_lists_configured_then_folder_channels_with_counts() {
    let h = home("channels");
    let out = stdout(&agora(&h, &["--channels"]));
    assert_eq!(out, "docs\t1\napi\t1\nweb\t1\n", "no rules: one per folder");

    write(
        &h.dir.join(".config/agora/channels"),
        "# rules\nnone = /nowhere\nback = /w/web, /w/api\n",
    );
    let out = stdout(&agora(&h, &["--channels"]));
    assert_eq!(out, "back\t2\ndocs\t1\n");
}

#[test]
fn list_of_an_empty_home_prints_nothing() {
    let h = home("empty");
    for d in [".claude", ".codex", ".pi"] {
        fs::remove_dir_all(h.dir.join(d)).unwrap();
    }
    assert_eq!(stdout(&agora(&h, &["--list"])), "");
}

#[test]
fn search_matches_conversation_text_across_agents() {
    let h = home("search");
    let out = stdout(&agora(&h, &["--search", "CSRF"]));
    let mut rows: Vec<&str> = out.lines().collect();
    rows.sort();
    assert_eq!(
        rows,
        [
            "claude\tweb\tFix login form\tThe CSRF token was stale.",
            "codex\tapi\tspeed up the csrf check\tspeed up the csrf check",
        ]
    );

    // Every word must match; words are joined across arguments.
    let out = stdout(&agora(&h, &["--search", "csrf", "token"]));
    assert_eq!(out.lines().count(), 1);
    assert!(out.starts_with("claude\t"));
    assert_eq!(stdout(&agora(&h, &["--search", "csrf nowhere"])), "");
    // JSON keys and ids are not conversation text.
    assert_eq!(stdout(&agora(&h, &["--search", "session_meta"])), "");
}

#[test]
fn search_with_no_words_finds_nothing() {
    let h = home("noterms");
    assert_eq!(stdout(&agora(&h, &["--search"])), "");
}
