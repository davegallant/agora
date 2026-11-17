//! Discover past sessions from each agent's on-disk transcripts.
//!
//! - Claude Code: `~/.claude/projects/<encoded-cwd>/<session-id>.jsonl`
//! - Codex:       `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`, titles in
//!   `~/.codex/session_index.jsonl`
//! - pi:          `~/.pi/agent/sessions/<encoded-cwd>/<timestamp>_<id>.jsonl`
//!
//! Only the head (cwd, first prompt) and tail (latest title) of each file are
//! read, so listing hundreds of multi-megabyte transcripts stays fast.

use serde_json::Value;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const HEAD_BYTES: u64 = 128 * 1024;
const TAIL_BYTES: u64 = 64 * 1024;
const PREVIEW_BYTES: u64 = 512 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Agent {
    Claude,
    Codex,
    Pi,
}

impl Agent {
    pub fn name(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
            Agent::Pi => "pi",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Session {
    pub agent: Agent,
    pub id: String,
    pub path: PathBuf,
    pub cwd: String,
    pub title: String,
    pub mtime: SystemTime,
}

impl Session {
    pub fn project(&self) -> &str {
        Path::new(&self.cwd)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(&self.cwd)
    }

    /// Command that reopens this session in its working directory.
    pub fn resume_argv(&self) -> Vec<String> {
        let v: Vec<&str> = match self.agent {
            Agent::Claude => vec!["claude", "--resume", &self.id],
            Agent::Codex => vec!["codex", "resume", &self.id],
            Agent::Pi => vec!["pi", "--session", self.path.to_str().unwrap_or(&self.id)],
        };
        v.into_iter().map(String::from).collect()
    }
}

pub struct Msg {
    pub user: bool,
    pub text: String,
}

fn read_head(path: &Path, n: u64) -> std::io::Result<String> {
    let mut buf = Vec::new();
    File::open(path)?.take(n).read_to_end(&mut buf)?;
    if buf.len() as u64 == n {
        // Drop the partial last line.
        if let Some(i) = buf.iter().rposition(|&b| b == b'\n') {
            buf.truncate(i);
        }
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

fn read_tail(path: &Path, n: u64) -> std::io::Result<String> {
    let mut f = File::open(path)?;
    let len = f.metadata()?.len();
    let start = len.saturating_sub(n);
    f.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    if start > 0 {
        // Drop the partial first line.
        if let Some(i) = buf.iter().position(|&b| b == b'\n') {
            buf.drain(..=i);
        }
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

fn json_lines(s: &str) -> impl Iterator<Item = Value> + '_ {
    s.lines().filter_map(|l| serde_json::from_str(l).ok())
}

fn clean(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Injected context, slash commands and reminders make poor titles.
fn is_noise(t: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "<local-command",
        "<command-",
        "Caveat:",
        "<bash-",
        "<user-",
        "<system-reminder>",
        "<environment_context>",
        "<permissions",
        "<user_instructions>",
        "# AGENTS.md",
        "[Request interrupted",
    ];
    t.is_empty() || PREFIXES.iter().any(|p| t.starts_with(p))
}

/// Message content is either a plain string or an array of typed blocks.
fn content_text(c: &Value) -> String {
    match c {
        Value::String(s) => s.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter(|b| {
                matches!(
                    b["type"].as_str(),
                    Some("text" | "input_text" | "output_text")
                )
            })
            .filter_map(|b| b["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn parse_claude(path: &Path, mtime: SystemTime) -> Option<Session> {
    let head = read_head(path, HEAD_BYTES).ok()?;
    let (mut cwd, mut title, mut prompt) = (None, None, None);
    for v in json_lines(&head) {
        if cwd.is_none() {
            cwd = v["cwd"].as_str().map(String::from);
        }
        match v["type"].as_str() {
            Some("ai-title") => {
                if let Some(t) = v["aiTitle"].as_str().filter(|t| !t.is_empty()) {
                    title = Some(t.to_string());
                }
            }
            Some("user") if prompt.is_none() && v["isMeta"].as_bool() != Some(true) => {
                let t = clean(&content_text(&v["message"]["content"]));
                if !is_noise(&t) {
                    prompt = Some(t);
                }
            }
            _ => {}
        }
    }
    // Claude rewrites its title as the conversation goes; the latest wins.
    if let Ok(tail) = read_tail(path, TAIL_BYTES) {
        for v in json_lines(&tail) {
            if v["type"] == "ai-title" {
                if let Some(t) = v["aiTitle"].as_str().filter(|t| !t.is_empty()) {
                    title = Some(t.to_string());
                }
            }
        }
    }
    Some(Session {
        agent: Agent::Claude,
        id: path.file_stem()?.to_str()?.to_string(),
        path: path.to_path_buf(),
        cwd: cwd?,
        title: title.or(prompt).unwrap_or_else(|| "(no prompt)".into()),
        mtime,
    })
}

fn parse_codex(
    path: &Path,
    mtime: SystemTime,
    titles: &HashMap<String, String>,
) -> Option<Session> {
    let head = read_head(path, HEAD_BYTES).ok()?;
    let (mut id, mut cwd, mut prompt) = (None, None, None);
    for v in json_lines(&head) {
        let p = &v["payload"];
        match v["type"].as_str() {
            Some("session_meta") => {
                // Sub-agent threads belong to their parent session.
                if p["thread_source"].as_str().is_some_and(|s| s != "user") {
                    return None;
                }
                id = p["id"].as_str().map(String::from);
                cwd = p["cwd"].as_str().map(String::from);
            }
            Some("response_item")
                if prompt.is_none() && p["type"] == "message" && p["role"] == "user" =>
            {
                for b in p["content"].as_array().into_iter().flatten() {
                    let t = clean(b["text"].as_str().unwrap_or(""));
                    if !is_noise(&t) {
                        prompt = Some(t);
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    let id = id?;
    let title = titles
        .get(&id)
        .cloned()
        .or(prompt)
        .unwrap_or_else(|| "(no prompt)".into());
    Some(Session {
        agent: Agent::Codex,
        id,
        path: path.to_path_buf(),
        cwd: cwd?,
        title,
        mtime,
    })
}

fn parse_pi(path: &Path, mtime: SystemTime) -> Option<Session> {
    let head = read_head(path, HEAD_BYTES).ok()?;
    let (mut id, mut cwd, mut prompt) = (None, None, None);
    for v in json_lines(&head) {
        match v["type"].as_str() {
            Some("session") => {
                id = v["id"].as_str().map(String::from);
                cwd = v["cwd"].as_str().map(String::from);
            }
            Some("message") if prompt.is_none() && v["message"]["role"] == "user" => {
                let t = clean(&content_text(&v["message"]["content"]));
                if !is_noise(&t) {
                    prompt = Some(t);
                }
            }
            _ => {}
        }
    }
    Some(Session {
        agent: Agent::Pi,
        id: id?,
        path: path.to_path_buf(),
        cwd: cwd?,
        title: prompt.unwrap_or_else(|| "(no prompt)".into()),
        mtime,
    })
}

fn codex_titles(home: &Path) -> HashMap<String, String> {
    let mut titles = HashMap::new();
    if let Ok(s) = fs::read_to_string(home.join(".codex/session_index.jsonl")) {
        for v in json_lines(&s) {
            if let (Some(id), Some(name)) = (v["id"].as_str(), v["thread_name"].as_str()) {
                if !name.is_empty() {
                    titles.insert(id.to_string(), name.to_string());
                }
            }
        }
    }
    titles
}

/// `*.jsonl` files exactly `depth` directories below `dir`.
fn jsonl_files(dir: &Path, depth: usize, out: &mut Vec<(PathBuf, SystemTime)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let path = e.path();
        let Ok(meta) = e.metadata() else { continue };
        if depth > 0 {
            if meta.is_dir() {
                jsonl_files(&path, depth - 1, out);
            }
        } else if meta.is_file() && path.extension().is_some_and(|x| x == "jsonl") {
            out.push((path, meta.modified().unwrap_or(SystemTime::UNIX_EPOCH)));
        }
    }
}

pub fn load_all(home: &Path) -> Vec<Session> {
    let mut jobs: Vec<(Agent, PathBuf, SystemTime)> = Vec::new();
    for (agent, dir, depth) in [
        (Agent::Claude, home.join(".claude/projects"), 1),
        (Agent::Codex, home.join(".codex/sessions"), 3),
        (Agent::Pi, home.join(".pi/agent/sessions"), 1),
    ] {
        let mut files = Vec::new();
        jsonl_files(&dir, depth, &mut files);
        jobs.extend(files.into_iter().map(|(p, m)| (agent, p, m)));
    }
    let titles = codex_titles(home);
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let chunk = jobs.len().div_ceil(threads).max(1);

    let mut sessions: Vec<Session> = std::thread::scope(|scope| {
        let handles: Vec<_> = jobs
            .chunks(chunk)
            .map(|batch| {
                let titles = &titles;
                scope.spawn(move || {
                    batch
                        .iter()
                        .filter_map(|(agent, path, mtime)| match agent {
                            Agent::Claude => parse_claude(path, *mtime),
                            Agent::Codex => parse_codex(path, *mtime, titles),
                            Agent::Pi => parse_pi(path, *mtime),
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap_or_default())
            .collect()
    });
    sessions.sort_by_key(|s| std::cmp::Reverse(s.mtime));
    sessions
}

/// The user or assistant text carried by one transcript line, if any.
pub fn message_of(agent: Agent, v: &Value) -> Option<Msg> {
    let (role, content) = match agent {
        Agent::Claude => {
            if v["isMeta"].as_bool() == Some(true) {
                return None;
            }
            (v["type"].as_str()?, &v["message"]["content"])
        }
        Agent::Codex => {
            if v["type"] != "response_item" || v["payload"]["type"] != "message" {
                return None;
            }
            (v["payload"]["role"].as_str()?, &v["payload"]["content"])
        }
        Agent::Pi => {
            if v["type"] != "message" {
                return None;
            }
            (v["message"]["role"].as_str()?, &v["message"]["content"])
        }
    };
    let user = match role {
        "user" => true,
        "assistant" => false,
        _ => return None,
    };
    let text = content_text(content).trim().to_string();
    if text.is_empty() || (user && is_noise(&text)) {
        return None;
    }
    Some(Msg { user, text })
}

/// The last `max` user/assistant text messages of a session. With `full`,
/// the whole transcript is read instead of just its tail.
pub fn preview(s: &Session, max: usize, full: bool) -> Vec<Msg> {
    let bytes = if full { u64::MAX } else { PREVIEW_BYTES };
    let tail = read_tail(&s.path, bytes).unwrap_or_default();
    let mut out: Vec<Msg> = json_lines(&tail)
        .filter_map(|v| message_of(s.agent, &v))
        .collect();
    let keep = out.len().saturating_sub(max);
    out.split_off(keep)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmp(name: &str, lines: &[&str]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("agora-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        let mut f = File::create(&p).unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
        p
    }

    #[test]
    fn claude_prefers_latest_ai_title_and_skips_noise() {
        let p = tmp(
            "abc-123.jsonl",
            &[
                r#"{"type":"user","isMeta":true,"cwd":"/w/proj","message":{"content":"meta"}}"#,
                r#"{"type":"user","cwd":"/w/proj","message":{"content":"<command-name>/clear</command-name>"}}"#,
                r#"{"type":"user","message":{"content":[{"type":"text","text":"fix   the bug"}]}}"#,
                r#"{"type":"ai-title","aiTitle":"First"}"#,
                r#"{"type":"assistant","message":{"content":[{"type":"text","text":"done"}]}}"#,
                r#"{"type":"ai-title","aiTitle":"Fix the login bug"}"#,
            ],
        );
        let s = parse_claude(&p, SystemTime::UNIX_EPOCH).unwrap();
        assert_eq!(s.id, "abc-123");
        assert_eq!(s.cwd, "/w/proj");
        assert_eq!(s.title, "Fix the login bug");
        assert_eq!(s.resume_argv(), ["claude", "--resume", "abc-123"]);

        let msgs = preview(&s, 10, false);
        assert_eq!(msgs.len(), 2);
        assert!(msgs[0].user && msgs[0].text == "fix   the bug");
        assert!(!msgs[1].user && msgs[1].text == "done");
    }

    #[test]
    fn claude_falls_back_to_first_prompt() {
        let p = tmp(
            "no-title.jsonl",
            &[r#"{"type":"user","cwd":"/w","message":{"content":"hello\n there"}}"#],
        );
        assert_eq!(
            parse_claude(&p, SystemTime::UNIX_EPOCH).unwrap().title,
            "hello there"
        );
    }

    #[test]
    fn codex_uses_index_title_and_skips_subagents() {
        let p = tmp(
            "rollout-x.jsonl",
            &[
                r#"{"type":"session_meta","payload":{"id":"cx1","cwd":"/w/c","thread_source":"user"}}"#,
                r##"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"# AGENTS.md instructions"},{"type":"input_text","text":"<environment_context>x"}]}}"##,
                r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"reply hi"}]}}"#,
                r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"hi"}]}}"#,
            ],
        );
        let mut titles = HashMap::new();
        let s = parse_codex(&p, SystemTime::UNIX_EPOCH, &titles).unwrap();
        assert_eq!(s.title, "reply hi");
        titles.insert("cx1".into(), "Reply hi".into());
        let s = parse_codex(&p, SystemTime::UNIX_EPOCH, &titles).unwrap();
        assert_eq!(s.title, "Reply hi");
        assert_eq!(s.resume_argv(), ["codex", "resume", "cx1"]);
        assert_eq!(preview(&s, 10, false).len(), 2);

        let sub = tmp(
            "rollout-sub.jsonl",
            &[
                r#"{"type":"session_meta","payload":{"id":"cx2","cwd":"/w","thread_source":"subagent"}}"#,
            ],
        );
        assert!(parse_codex(&sub, SystemTime::UNIX_EPOCH, &titles).is_none());
    }

    #[test]
    fn pi_session() {
        let p = tmp(
            "pi.jsonl",
            &[
                r#"{"type":"session","id":"pi1","cwd":"/w/p"}"#,
                r#"{"type":"message","message":{"role":"system","content":""}}"#,
                r#"{"type":"message","message":{"role":"user","content":[{"type":"text","text":"say hi"}]}}"#,
                r#"{"type":"message","message":{"role":"assistant","content":[{"type":"thinking","thinking":"hm"},{"type":"text","text":"hi"}]}}"#,
            ],
        );
        let s = parse_pi(&p, SystemTime::UNIX_EPOCH).unwrap();
        assert_eq!((s.id.as_str(), s.title.as_str()), ("pi1", "say hi"));
        let msgs = preview(&s, 10, false);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[1].text, "hi");
    }

    #[test]
    fn head_and_tail_drop_partial_lines() {
        let p = tmp("partial.jsonl", &["aaaa", "bbbb", "cccc"]);
        assert_eq!(read_head(&p, 7).unwrap(), "aaaa");
        assert_eq!(read_tail(&p, 7).unwrap(), "cccc\n");
    }
}
