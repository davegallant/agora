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
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::time::{Duration, SystemTime};

/// Longest title made from a prompt, when the agent wrote none.
const TITLE_CHARS: usize = 60;
const HEAD_BYTES: u64 = 128 * 1024;
const TAIL_BYTES: u64 = 64 * 1024;
/// How far back to look when the last line alone is longer than `TAIL_BYTES`.
const TAIL_MAX: u64 = 4 * 1024 * 1024;
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

    pub fn from_name(name: &str) -> Option<Agent> {
        [Agent::Claude, Agent::Codex, Agent::Pi]
            .into_iter()
            .find(|a| a.name() == name.trim())
    }
}

#[derive(Clone, Debug)]
pub struct Session {
    pub agent: Agent,
    pub id: String,
    pub path: PathBuf,
    /// Where the session started, and where it resumes from.
    pub cwd: String,
    /// The folder it last worked in: `cwd`, unless it moved (for example into
    /// a worktree) partway through.
    pub dir: String,
    pub title: String,
    pub mtime: SystemTime,
    /// The transcript's size in bytes.
    pub size: u64,
    /// The latest prompt found in the transcript's tail; empty when none.
    pub last_prompt: String,
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

/// The folder the last turn in `text` worked in.
fn last_dir(agent: Agent, text: &str) -> Option<String> {
    json_lines(text)
        .filter_map(|v| {
            let cwd = match agent {
                Agent::Claude => &v["cwd"],
                Agent::Codex if v["type"] == "turn_context" => &v["payload"]["cwd"],
                _ => return None,
            };
            cwd.as_str().filter(|c| !c.is_empty()).map(String::from)
        })
        .last()
}

/// The end of a file with at least one whole line, reading further back when
/// the last line is longer than `TAIL_BYTES` (a big tool result, say).
fn read_last_lines(path: &Path) -> std::io::Result<String> {
    let mut n = TAIL_BYTES;
    loop {
        let tail = read_tail(path, n)?;
        if !tail.trim().is_empty() || n >= TAIL_MAX {
            return Ok(tail);
        }
        n *= 4;
    }
}

/// The latest prompt in `tail`: Claude's own `last-prompt` record, else the
/// last user message.
fn last_prompt(agent: Agent, tail: &str) -> String {
    let mut out = String::new();
    for v in json_lines(tail) {
        let t = if agent == Agent::Claude && v["type"] == "last-prompt" {
            v["lastPrompt"].as_str().map(clean)
        } else {
            message_of(agent, &v)
                .filter(|m| m.user)
                .map(|m| prompt_line(&m.text))
        };
        if let Some(t) = t.filter(|t| !is_noise(t)) {
            out = t;
        }
    }
    out
}

fn json_lines(s: &str) -> impl Iterator<Item = Value> + '_ {
    s.lines().filter_map(|l| serde_json::from_str(l).ok())
}

/// One line of plain text: whitespace collapsed and control characters
/// (terminal escapes from pasted logs, say) dropped, so a title can never
/// steer the terminal it is drawn on.
fn clean(s: &str) -> String {
    s.split_whitespace()
        .map(|w| w.chars().filter(|c| !c.is_control()).collect::<String>())
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// A prompt's first line. A skill pi expanded into the prompt
/// (`<skill name="x" …>…</skill>` then the user's words) reads as `/x words`.
fn prompt_line(t: &str) -> String {
    let first = |t: &str| {
        t.lines()
            .map(clean)
            .find(|l| !l.is_empty())
            .unwrap_or_default()
    };
    if let Some(rest) = t.trim_start().strip_prefix("<skill name=\"") {
        if let Some((name, _)) = rest.split_once('"') {
            let args = rest.split_once("</skill>").map_or("", |(_, a)| a);
            return clean(&format!("/{name} {}", first(args)));
        }
    }
    first(t)
}

/// A title made from a prompt: cut at a word to about `TITLE_CHARS`.
fn prompt_title(t: String) -> String {
    if t.chars().count() <= TITLE_CHARS {
        return t;
    }
    let cut: String = t.chars().take(TITLE_CHARS).collect();
    let cut = match cut.rfind(' ') {
        Some(i) if i > TITLE_CHARS / 2 => &cut[..i],
        _ => &cut,
    };
    format!("{}…", cut.trim_end())
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
        "<task-notification>",
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
                if let Some(t) = v["aiTitle"].as_str().map(clean).filter(|t| !t.is_empty()) {
                    title = Some(t);
                }
            }
            Some("user") if prompt.is_none() && v["isMeta"].as_bool() != Some(true) => {
                let t = prompt_line(&content_text(&v["message"]["content"]));
                if !is_noise(&t) {
                    prompt = Some(t);
                }
            }
            _ => {}
        }
    }
    // Claude rewrites its title as the conversation goes; the latest wins.
    let mut dir = last_dir(Agent::Claude, &head);
    let mut last = String::new();
    if let Ok(tail) = read_last_lines(path) {
        dir = last_dir(Agent::Claude, &tail).or(dir);
        last = last_prompt(Agent::Claude, &tail);
        for v in json_lines(&tail) {
            if v["type"] == "ai-title" {
                if let Some(t) = v["aiTitle"].as_str().map(clean).filter(|t| !t.is_empty()) {
                    title = Some(t);
                }
            }
        }
    }
    // A first prompt longer than the head is cut and unreadable; the folder
    // it would have given is still on every later line.
    let cwd = cwd.or_else(|| dir.clone())?;
    Some(Session {
        agent: Agent::Claude,
        id: path.file_stem()?.to_str()?.to_string(),
        path: path.to_path_buf(),
        dir: dir.unwrap_or_else(|| cwd.clone()),
        cwd,
        title: title
            .or(prompt.map(prompt_title))
            .unwrap_or_else(|| "(no prompt)".into()),
        mtime,
        size: 0,
        last_prompt: last,
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
                    let t = prompt_line(b["text"].as_str().unwrap_or(""));
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
    let cwd = cwd?;
    let tail = read_last_lines(path).unwrap_or_default();
    let dir = last_dir(Agent::Codex, &tail).unwrap_or_else(|| cwd.clone());
    let title = titles
        .get(&id)
        .cloned()
        .or(prompt.map(prompt_title))
        .unwrap_or_else(|| "(no prompt)".into());
    Some(Session {
        agent: Agent::Codex,
        id,
        path: path.to_path_buf(),
        cwd,
        dir,
        title,
        mtime,
        size: 0,
        last_prompt: last_prompt(Agent::Codex, &tail),
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
                let t = prompt_line(&content_text(&v["message"]["content"]));
                if !is_noise(&t) {
                    prompt = Some(t);
                }
            }
            _ => {}
        }
    }
    let cwd: String = cwd?;
    Some(Session {
        agent: Agent::Pi,
        id: id?,
        path: path.to_path_buf(),
        dir: cwd.clone(),
        cwd,
        title: prompt
            .map(prompt_title)
            .unwrap_or_else(|| "(no prompt)".into()),
        mtime,
        size: 0,
        last_prompt: read_last_lines(path)
            .map(|t| last_prompt(Agent::Pi, &t))
            .unwrap_or_default(),
    })
}

fn codex_titles(home: &Path) -> HashMap<String, String> {
    let mut titles = HashMap::new();
    if let Ok(s) = fs::read_to_string(home.join(".codex/session_index.jsonl")) {
        for v in json_lines(&s) {
            if let (Some(id), Some(name)) = (v["id"].as_str(), v["thread_name"].as_str()) {
                let name = clean(name);
                if !name.is_empty() {
                    titles.insert(id.to_string(), name);
                }
            }
        }
    }
    titles
}

/// `*.jsonl` files exactly `depth` directories below `dir`.
fn jsonl_files(dir: &Path, depth: usize, out: &mut Vec<(PathBuf, Stamp)>) {
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
            let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            out.push((path, (mtime, meta.len())));
        }
    }
}

/// When a file last changed, and its size: a write within one tick of a
/// coarse filesystem clock still changes the size.
type Stamp = (SystemTime, u64);

/// A transcript file: whose it is, where, and when it last changed.
type Transcript = (Agent, PathBuf, Stamp);

/// Every transcript file on disk. Only directories are read, not the files.
fn scan(home: &Path) -> Vec<Transcript> {
    let mut out = Vec::new();
    for (agent, dir, depth) in [
        (Agent::Claude, home.join(".claude/projects"), 1),
        (Agent::Codex, home.join(".codex/sessions"), 3),
        (Agent::Pi, home.join(".pi/agent/sessions"), 1),
    ] {
        let mut files = Vec::new();
        jsonl_files(&dir, depth, &mut files);
        out.extend(files.into_iter().map(|(p, m)| (agent, p, m)));
    }
    out
}

fn parse(
    (agent, path, (mtime, _)): &Transcript,
    titles: &HashMap<String, String>,
) -> Option<Session> {
    let mut s = match agent {
        Agent::Claude => parse_claude(path, *mtime),
        Agent::Codex => parse_codex(path, *mtime, titles),
        Agent::Pi => parse_pi(path, *mtime),
    }?;
    s.size = fs::metadata(path).map_or(0, |m| m.len());
    Some(s)
}

pub fn load_all(home: &Path) -> Vec<Session> {
    let jobs = scan(home);
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
                        .filter_map(|t| parse(t, titles))
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

/// Transcripts written or removed since the last look.
#[derive(Debug, Default)]
pub struct Changes {
    /// New sessions, and existing ones re-read because their file changed.
    pub updated: Vec<Session>,
    /// Transcripts that are gone, or no longer read as a session.
    pub removed: Vec<PathBuf>,
}

/// Watch the transcript directories, sending what changed. Only files whose
/// modification time moved are re-read, so this is cheap to run often.
pub fn spawn_watcher(home: PathBuf, every: Duration) -> Receiver<Changes> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut known: HashMap<PathBuf, Stamp> =
            scan(&home).into_iter().map(|(_, p, m)| (p, m)).collect();
        loop {
            std::thread::sleep(every);
            let now = scan(&home);
            let changed: Vec<&Transcript> = now
                .iter()
                .filter(|(_, p, m)| known.get(p) != Some(m))
                .collect();
            let present: HashSet<&PathBuf> = now.iter().map(|(_, p, _)| p).collect();
            let mut removed: Vec<PathBuf> = known
                .keys()
                .filter(|p| !present.contains(p))
                .cloned()
                .collect();
            if changed.is_empty() && removed.is_empty() {
                continue;
            }
            let titles = if changed.iter().any(|(a, _, _)| *a == Agent::Codex) {
                codex_titles(&home)
            } else {
                HashMap::new()
            };
            let mut updated = Vec::new();
            for t in changed {
                match parse(t, &titles) {
                    Some(s) => updated.push(s),
                    None => removed.push(t.1.clone()),
                }
            }
            known = now.into_iter().map(|(_, p, m)| (p, m)).collect();
            if tx.send(Changes { updated, removed }).is_err() {
                return;
            }
        }
    });
    rx
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

    #[test]
    fn watcher_reports_new_and_removed_transcripts() {
        let home = std::env::temp_dir().join(format!("agora-watch-{}", std::process::id()));
        let dir = home.join(".claude/projects/-w-proj");
        fs::create_dir_all(&dir).unwrap();
        let line = r#"{"type":"user","cwd":"/w/proj","message":{"content":"first prompt"}}"#;
        fs::write(dir.join("old.jsonl"), format!("{line}\n")).unwrap();
        let rx = spawn_watcher(home.clone(), Duration::from_millis(20));
        let wait = || rx.recv_timeout(Duration::from_secs(5)).unwrap();
        // Let the watcher take its first look before anything changes.
        std::thread::sleep(Duration::from_millis(100));

        put(&dir.join("new.jsonl"), &[line], None);
        let c = wait();
        assert_eq!(c.updated.len(), 1);
        assert_eq!(c.updated[0].id, "new");
        assert_eq!(c.updated[0].title, "first prompt");
        assert!(c.removed.is_empty());

        fs::remove_file(dir.join("old.jsonl")).unwrap();
        let c = wait();
        assert!(c.updated.is_empty());
        assert_eq!(c.removed, vec![dir.join("old.jsonl")]);
        fs::remove_dir_all(&home).unwrap();
    }

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
            "hello"
        );
    }

    #[test]
    fn last_prompt_comes_from_the_tail() {
        let p = tmp(
            "last.jsonl",
            &[
                r#"{"type":"user","cwd":"/w","message":{"content":"first"}}"#,
                r#"{"type":"user","cwd":"/w","message":{"content":"second\n ask"}}"#,
                r#"{"type":"user","cwd":"/w","message":{"content":"<system-reminder>x"}}"#,
            ],
        );
        let s = parse_claude(&p, SystemTime::UNIX_EPOCH).unwrap();
        assert_eq!(s.last_prompt, "second");
        // Claude's own record wins over the messages around it.
        let p = tmp(
            "last-record.jsonl",
            &[
                r#"{"type":"user","cwd":"/w","message":{"content":"first"}}"#,
                r#"{"type":"last-prompt","lastPrompt":"latest ask"}"#,
            ],
        );
        let s = parse_claude(&p, SystemTime::UNIX_EPOCH).unwrap();
        assert_eq!(s.last_prompt, "latest ask");
    }

    #[test]
    fn a_pi_skill_prompt_reads_as_a_command() {
        assert_eq!(
            prompt_line("<skill name=\"commit\" location=\"/s/SKILL.md\">\n# Commit\nlong text\n</skill>\n\namend it"),
            "/commit amend it"
        );
        assert_eq!(
            prompt_line("<skill name=\"commit\">\nbody\n</skill>"),
            "/commit"
        );
        assert_eq!(prompt_line("\n first  line\nsecond"), "first line");
        let long = "word ".repeat(30);
        let t = prompt_title(long.trim().into());
        assert!(
            t.ends_with("word…") && t.chars().count() <= TITLE_CHARS + 1,
            "{t}"
        );
        assert_eq!(prompt_title("short".into()), "short");
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
        assert_eq!(s.last_prompt, "reply hi");

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

    /// A fresh directory for one test, so tests running in parallel never
    /// share files.
    fn tmpdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("agora-s-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, lines: &[&str]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let body: String = lines.iter().map(|l| format!("{l}\n")).collect();
        fs::write(path, body).unwrap();
    }

    /// Write `lines` to `path` in one step, as a watcher sees it: written
    /// and dated under another name, then renamed into place. A watcher
    /// looking mid-write would date the file before the write lands and
    /// report it again on its next look.
    fn put(path: &Path, lines: &[&str], mtime: Option<u64>) {
        let tmp = path.with_extension("tmp");
        write(&tmp, lines);
        if let Some(secs) = mtime {
            set_mtime(&tmp, secs);
        }
        fs::rename(&tmp, path).unwrap();
    }

    fn set_mtime(path: &Path, secs: u64) {
        File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
            .unwrap();
    }

    fn session(agent: Agent, id: &str, path: &str, cwd: &str) -> Session {
        Session {
            agent,
            id: id.into(),
            path: path.into(),
            cwd: cwd.into(),
            dir: cwd.into(),
            title: String::new(),
            mtime: SystemTime::UNIX_EPOCH,
            size: 0,
            last_prompt: String::new(),
        }
    }

    #[test]
    fn agent_names() {
        assert_eq!(Agent::Claude.name(), "claude");
        assert_eq!(Agent::Codex.name(), "codex");
        assert_eq!(Agent::Pi.name(), "pi");
    }

    #[test]
    fn project_is_the_last_cwd_component() {
        let p = |cwd| session(Agent::Claude, "x", "", cwd).project().to_string();
        assert_eq!(p("/w/proj"), "proj");
        assert_eq!(p("/w/proj/"), "proj");
        assert_eq!(p("relative"), "relative");
        assert_eq!(p("/"), "/");
        assert_eq!(p(""), "");
    }

    #[test]
    fn resume_argv_per_agent() {
        let pi = session(Agent::Pi, "pi1", "/h/.pi/s/x.jsonl", "/w");
        assert_eq!(pi.resume_argv(), ["pi", "--session", "/h/.pi/s/x.jsonl"]);
        let codex = session(Agent::Codex, "cx", "/p", "/w");
        assert_eq!(codex.resume_argv(), ["codex", "resume", "cx"]);
    }

    #[test]
    fn clean_collapses_whitespace() {
        assert_eq!(clean("  a\n\tb   c  "), "a b c");
        assert_eq!(clean(" \n "), "");
    }

    #[test]
    fn noise_detection() {
        for t in [
            "",
            "<local-command-stdout>x",
            "<command-name>/clear",
            "Caveat: the messages below",
            "<bash-input>ls",
            "<user-prompt-submit-hook>",
            "<system-reminder>be nice",
            "<environment_context>",
            "<permissions instructions>",
            "<user_instructions>",
            "# AGENTS.md instructions for /w",
            "[Request interrupted by user]",
        ] {
            assert!(is_noise(t), "{t:?} should be noise");
        }
        for t in ["fix the bug", " <command-name>", "Caveat", "## AGENTS.md"] {
            assert!(!is_noise(t), "{t:?} should not be noise");
        }
    }

    #[test]
    fn content_text_keeps_only_text_blocks() {
        let v: Value = serde_json::from_str(
            r#"[{"type":"text","text":"a"},{"type":"tool_use","text":"no"},
                {"type":"input_text","text":"b"},{"type":"output_text","text":"c"},
                {"type":"thinking","thinking":"no"},{"type":"text"}]"#,
        )
        .unwrap();
        assert_eq!(content_text(&v), "a\nb\nc");
        assert_eq!(content_text(&Value::String("plain".into())), "plain");
        assert_eq!(content_text(&Value::Null), "");
        assert_eq!(content_text(&serde_json::json!(42)), "");
    }

    #[test]
    fn read_head_and_tail_of_short_files_are_whole() {
        let p = tmpdir("short").join("f.jsonl");
        write(&p, &["one", "two"]);
        assert_eq!(read_head(&p, 1024).unwrap(), "one\ntwo\n");
        assert_eq!(read_tail(&p, 1024).unwrap(), "one\ntwo\n");
        assert!(read_head(&p.with_extension("missing"), 10).is_err());
        assert!(read_tail(&p.with_extension("missing"), 10).is_err());
    }

    #[test]
    fn claude_without_cwd_is_skipped() {
        let p = tmpdir("nocwd").join("x.jsonl");
        write(&p, &[r#"{"type":"user","message":{"content":"hi"}}"#]);
        assert!(parse_claude(&p, SystemTime::UNIX_EPOCH).is_none());
    }

    #[test]
    fn claude_takes_cwd_from_first_line_that_has_one() {
        let p = tmpdir("latecwd").join("x.jsonl");
        write(
            &p,
            &[
                "not json",
                r#"{"type":"summary"}"#,
                r#"{"type":"user","cwd":"/first","message":{"content":"hi"}}"#,
                r#"{"type":"user","cwd":"/second","message":{"content":"again"}}"#,
            ],
        );
        let s = parse_claude(&p, SystemTime::UNIX_EPOCH).unwrap();
        assert_eq!(s.cwd, "/first");
        assert_eq!(s.title, "hi");
    }

    #[test]
    fn claude_with_only_noise_has_placeholder_title() {
        let p = tmpdir("noise").join("x.jsonl");
        write(
            &p,
            &[
                r#"{"type":"user","cwd":"/w","message":{"content":"<command-name>/clear</command-name>"}}"#,
                r#"{"type":"ai-title","aiTitle":""}"#,
            ],
        );
        let s = parse_claude(&p, SystemTime::UNIX_EPOCH).unwrap();
        assert_eq!(s.title, "(no prompt)");
    }

    #[test]
    fn claude_title_written_past_the_head_is_found_in_the_tail() {
        let p = tmpdir("bigtitle").join("x.jsonl");
        let filler = format!(
            r#"{{"type":"assistant","message":{{"content":"{}"}}}}"#,
            "x".repeat(HEAD_BYTES as usize)
        );
        write(
            &p,
            &[
                r#"{"type":"user","cwd":"/w","message":{"content":"first prompt"}}"#,
                r#"{"type":"ai-title","aiTitle":"Early"}"#,
                &filler,
                r#"{"type":"ai-title","aiTitle":"Late"}"#,
            ],
        );
        assert_eq!(
            parse_claude(&p, SystemTime::UNIX_EPOCH).unwrap().title,
            "Late"
        );
    }

    #[test]
    fn codex_requires_session_meta_and_cwd() {
        let dir = tmpdir("codexmeta");
        let no_meta = dir.join("a.jsonl");
        write(
            &no_meta,
            &[
                r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"hi"}]}}"#,
            ],
        );
        assert!(parse_codex(&no_meta, SystemTime::UNIX_EPOCH, &HashMap::new()).is_none());

        let no_cwd = dir.join("b.jsonl");
        write(
            &no_cwd,
            &[r#"{"type":"session_meta","payload":{"id":"x"}}"#],
        );
        assert!(parse_codex(&no_cwd, SystemTime::UNIX_EPOCH, &HashMap::new()).is_none());

        // No thread_source at all: an ordinary user session.
        let plain = dir.join("c.jsonl");
        write(
            &plain,
            &[r#"{"type":"session_meta","payload":{"id":"x","cwd":"/w"}}"#],
        );
        let s = parse_codex(&plain, SystemTime::UNIX_EPOCH, &HashMap::new()).unwrap();
        assert_eq!((s.id.as_str(), s.title.as_str()), ("x", "(no prompt)"));
        assert_eq!(s.agent, Agent::Codex);
    }

    #[test]
    fn codex_title_index() {
        let home = tmpdir("codexidx");
        assert!(codex_titles(&home).is_empty(), "missing index");
        write(
            &home.join(".codex/session_index.jsonl"),
            &[
                r#"{"id":"a","thread_name":"Alpha"}"#,
                r#"{"id":"b","thread_name":""}"#,
                r#"{"id":"c"}"#,
                "garbage",
                r#"{"id":"a","thread_name":"Alpha renamed"}"#,
            ],
        );
        let t = codex_titles(&home);
        assert_eq!(t.len(), 1);
        assert_eq!(t["a"], "Alpha renamed", "later entries win");
    }

    #[test]
    fn pi_requires_session_header() {
        let dir = tmpdir("pihdr");
        let p = dir.join("x.jsonl");
        write(
            &p,
            &[r#"{"type":"message","message":{"role":"user","content":"hi"}}"#],
        );
        assert!(parse_pi(&p, SystemTime::UNIX_EPOCH).is_none());

        let q = dir.join("y.jsonl");
        write(&q, &[r#"{"type":"session","id":"p","cwd":"/w"}"#]);
        let s = parse_pi(&q, SystemTime::UNIX_EPOCH).unwrap();
        assert_eq!(s.title, "(no prompt)");
        assert_eq!(s.resume_argv(), ["pi", "--session", q.to_str().unwrap()]);
    }

    #[test]
    fn jsonl_files_only_at_the_given_depth() {
        let dir = tmpdir("depth");
        for f in ["top.jsonl", "a/one.jsonl", "a/b/deep.jsonl", "a/notes.txt"] {
            write(&dir.join(f), &["{}"]);
        }
        let names = |depth| {
            let mut out = Vec::new();
            jsonl_files(&dir, depth, &mut out);
            let mut n: Vec<String> = out
                .iter()
                .map(|(p, _)| p.file_name().unwrap().to_string_lossy().into_owned())
                .collect();
            n.sort();
            n
        };
        assert_eq!(names(0), ["top.jsonl"]);
        assert_eq!(names(1), ["one.jsonl"]);
        assert_eq!(names(2), ["deep.jsonl"]);
        let mut out = Vec::new();
        jsonl_files(&dir.join("missing"), 1, &mut out);
        assert!(out.is_empty());
    }

    /// A home with one transcript per agent, plus files to be ignored.
    fn seeded_home(tag: &str) -> PathBuf {
        let home = tmpdir(tag);
        let claude = home.join(".claude/projects/-w-a/c1.jsonl");
        write(
            &claude,
            &[r#"{"type":"user","cwd":"/w/a","message":{"content":"claude prompt"}}"#],
        );
        let codex = home.join(".codex/sessions/2026/01/02/rollout-1.jsonl");
        write(
            &codex,
            &[
                r#"{"type":"session_meta","payload":{"id":"x1","cwd":"/w/b"}}"#,
                r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"codex prompt"}]}}"#,
            ],
        );
        write(
            &home.join(".codex/session_index.jsonl"),
            &[r#"{"id":"x1","thread_name":"Codex thread"}"#],
        );
        let pi = home.join(".pi/agent/sessions/-w-c/1_p1.jsonl");
        write(
            &pi,
            &[
                r#"{"type":"session","id":"p1","cwd":"/w/c"}"#,
                r#"{"type":"message","message":{"role":"user","content":"pi prompt"}}"#,
            ],
        );
        // Unreadable as a session, wrong depth, wrong extension.
        write(&home.join(".claude/projects/-w-a/junk.jsonl"), &["nope"]);
        write(&home.join(".claude/projects/stray.jsonl"), &["{}"]);
        write(&home.join(".codex/sessions/2026/01/02/notes.md"), &["x"]);
        set_mtime(&claude, 300);
        set_mtime(&codex, 100);
        set_mtime(&pi, 200);
        home
    }

    #[test]
    fn load_all_reads_every_agent_newest_first() {
        let home = seeded_home("loadall");
        let sessions = load_all(&home);
        let got: Vec<(Agent, &str, &str, &str)> = sessions
            .iter()
            .map(|s| (s.agent, s.id.as_str(), s.title.as_str(), s.cwd.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                (Agent::Claude, "c1", "claude prompt", "/w/a"),
                (Agent::Pi, "p1", "pi prompt", "/w/c"),
                (Agent::Codex, "x1", "Codex thread", "/w/b"),
            ]
        );
        assert_eq!(
            sessions[0].mtime,
            SystemTime::UNIX_EPOCH + Duration::from_secs(300)
        );
        for s in &sessions {
            assert_eq!(s.size, fs::metadata(&s.path).unwrap().len(), "{}", s.id);
            assert!(s.size > 0);
        }
        fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn a_session_that_moves_folder_keeps_where_it_started() {
        let home = tmpdir("moved");
        let claude = home.join(".claude/projects/-w-a/c1.jsonl");
        write(
            &claude,
            &[
                r#"{"type":"user","cwd":"/w/a","message":{"content":"start here"}}"#,
                r#"{"type":"assistant","cwd":"/w/a/.claude/worktrees/x","message":{"content":"moved"}}"#,
            ],
        );
        let s = parse_claude(&claude, SystemTime::UNIX_EPOCH).unwrap();
        assert_eq!(s.cwd, "/w/a", "resume where it was filed");
        assert_eq!(s.dir, "/w/a/.claude/worktrees/x", "working there now");

        let codex = home.join(".codex/sessions/2026/01/02/rollout-1.jsonl");
        write(
            &codex,
            &[
                r#"{"type":"session_meta","payload":{"id":"x1","cwd":"/w/b"}}"#,
                r#"{"type":"turn_context","payload":{"cwd":"/w/b"}}"#,
                r#"{"type":"turn_context","payload":{"cwd":"/w/b2"}}"#,
            ],
        );
        let s = parse_codex(&codex, SystemTime::UNIX_EPOCH, &HashMap::new()).unwrap();
        assert_eq!((s.cwd.as_str(), s.dir.as_str()), ("/w/b", "/w/b2"));

        let pi = home.join(".pi/agent/sessions/-w-c/1_p1.jsonl");
        write(&pi, &[r#"{"type":"session","id":"p1","cwd":"/w/c"}"#]);
        let s = parse_pi(&pi, SystemTime::UNIX_EPOCH).unwrap();
        assert_eq!(s.dir, s.cwd, "without a move, the same folder");
        fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn titles_lose_control_characters() {
        let home = tmpdir("ctrl-title");
        let path = home.join(".claude/projects/-w/esc.jsonl");
        write(
            &path,
            &[
                r#"{"type":"user","cwd":"/w","message":{"content":"x"}}"#,
                r#"{"type":"ai-title","aiTitle":"\u001b]0;pwned\u0007Red\u001b[31m title"}"#,
            ],
        );
        let s = parse_claude(&path, SystemTime::UNIX_EPOCH).unwrap();
        assert!(!s.title.chars().any(char::is_control), "{:?}", s.title);
        let prompt = home.join(".claude/projects/-w/esc2.jsonl");
        write(
            &prompt,
            &[r#"{"type":"user","cwd":"/w","message":{"content":"log: \u001b[2Jcleared"}}"#],
        );
        let s = parse_claude(&prompt, SystemTime::UNIX_EPOCH).unwrap();
        assert!(!s.title.chars().any(char::is_control), "{:?}", s.title);
        write(
            &home.join(".codex/session_index.jsonl"),
            &[r#"{"id":"x","thread_name":"a\u001b[1mb"}"#],
        );
        assert_eq!(codex_titles(&home)["x"], "a[1mb");
        fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn load_all_of_an_empty_home() {
        assert!(load_all(&tmpdir("empty")).is_empty());
        assert!(load_all(Path::new("/nonexistent/agora")).is_empty());
    }

    #[test]
    fn message_of_each_agent() {
        let j = |s: &str| serde_json::from_str::<Value>(s).unwrap();
        let m = message_of(
            Agent::Claude,
            &j(r#"{"type":"assistant","message":{"content":"  hi  "}}"#),
        )
        .unwrap();
        assert!(!m.user);
        assert_eq!(m.text, "hi", "trimmed");

        for (agent, line) in [
            // Meta lines, other roles, empty and noisy user text are skipped.
            (
                Agent::Claude,
                r#"{"type":"user","isMeta":true,"message":{"content":"x"}}"#,
            ),
            (Agent::Claude, r#"{"type":"ai-title","aiTitle":"t"}"#),
            (
                Agent::Claude,
                r#"{"type":"user","message":{"content":"  "}}"#,
            ),
            (
                Agent::Claude,
                r#"{"type":"user","message":{"content":"<system-reminder>x"}}"#,
            ),
            (Agent::Claude, r#"{"message":{"content":"no type"}}"#),
            (
                Agent::Codex,
                r#"{"type":"response_item","payload":{"type":"function_call","role":"user"}}"#,
            ),
            (
                Agent::Codex,
                r#"{"type":"event_msg","payload":{"type":"message","role":"user","content":"x"}}"#,
            ),
            (
                Agent::Codex,
                r#"{"type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"x"}]}}"#,
            ),
            (
                Agent::Pi,
                r#"{"type":"message","message":{"role":"toolResult","content":"x"}}"#,
            ),
            (
                Agent::Pi,
                r#"{"type":"session","message":{"role":"user","content":"x"}}"#,
            ),
        ] {
            assert!(message_of(agent, &j(line)).is_none(), "{line}");
        }

        // Noise filtering applies to the user only.
        let m = message_of(
            Agent::Claude,
            &j(r#"{"type":"assistant","message":{"content":"<command-name> is a tag"}}"#),
        );
        assert!(m.is_some());

        let m = message_of(
            Agent::Codex,
            &j(r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"q"}]}}"#),
        )
        .unwrap();
        assert!(m.user && m.text == "q");
        let m = message_of(
            Agent::Pi,
            &j(r#"{"type":"message","message":{"role":"user","content":"p"}}"#),
        )
        .unwrap();
        assert!(m.user && m.text == "p");
    }

    #[test]
    fn preview_keeps_the_last_max_messages() {
        let p = tmpdir("max").join("x.jsonl");
        let lines: Vec<String> = (0..10)
            .map(|n| format!(r#"{{"type":"user","cwd":"/w","message":{{"content":"m{n}"}}}}"#))
            .collect();
        write(&p, &lines.iter().map(String::as_str).collect::<Vec<_>>());
        let s = parse_claude(&p, SystemTime::UNIX_EPOCH).unwrap();
        let texts: Vec<String> = preview(&s, 3, false).into_iter().map(|m| m.text).collect();
        assert_eq!(texts, ["m7", "m8", "m9"]);
        assert!(preview(&s, 0, false).is_empty());
        assert_eq!(preview(&s, 100, false).len(), 10);
    }

    #[test]
    fn preview_reads_the_whole_file_only_when_full() {
        let p = tmpdir("full").join("x.jsonl");
        let filler = format!(
            r#"{{"type":"assistant","message":{{"content":"{}"}}}}"#,
            "y".repeat(PREVIEW_BYTES as usize)
        );
        write(
            &p,
            &[
                r#"{"type":"user","cwd":"/w","message":{"content":"opening"}}"#,
                &filler,
                r#"{"type":"user","message":{"content":"closing"}}"#,
            ],
        );
        let s = parse_claude(&p, SystemTime::UNIX_EPOCH).unwrap();
        let tail: Vec<String> = preview(&s, 100, false)
            .into_iter()
            .map(|m| m.text)
            .collect();
        assert_eq!(tail, ["closing"], "the filler line is cut, so skipped");
        let full = preview(&s, 100, true);
        assert_eq!(full.len(), 3);
        assert_eq!(full[0].text, "opening");
    }

    #[test]
    fn preview_of_a_missing_file_is_empty() {
        let s = session(Agent::Claude, "x", "/nonexistent/x.jsonl", "/w");
        assert!(preview(&s, 10, true).is_empty());
    }

    #[test]
    fn watcher_reports_rewrites_and_files_that_stop_parsing() {
        let home = tmpdir("watch2");
        let path = home.join(".claude/projects/-w/s.jsonl");
        write(
            &path,
            &[r#"{"type":"user","cwd":"/w","message":{"content":"before"}}"#],
        );
        set_mtime(&path, 1000);
        let rx = spawn_watcher(home.clone(), Duration::from_millis(20));
        let wait = || rx.recv_timeout(Duration::from_secs(5)).unwrap();
        std::thread::sleep(Duration::from_millis(100));

        put(
            &path,
            &[
                r#"{"type":"user","cwd":"/w","message":{"content":"before"}}"#,
                r#"{"type":"ai-title","aiTitle":"After"}"#,
            ],
            Some(2000),
        );
        let c = wait();
        assert_eq!(c.updated.len(), 1);
        assert_eq!(c.updated[0].title, "After");
        assert!(c.removed.is_empty());

        // Still on disk, but no longer a session: reported as removed.
        put(&path, &["not a session"], Some(3000));
        let c = wait();
        assert!(c.updated.is_empty());
        assert_eq!(c.removed, vec![path.clone()]);
        fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn watcher_sees_a_write_that_keeps_the_same_mtime() {
        let home = tmpdir("watch-samemtime");
        let path = home.join(".claude/projects/-w/s.jsonl");
        let first = r#"{"type":"user","cwd":"/w","message":{"content":"before"}}"#;
        put(&path, &[first], Some(1000));
        let rx = spawn_watcher(home.clone(), Duration::from_millis(20));
        std::thread::sleep(Duration::from_millis(100));
        put(
            &path,
            &[first, r#"{"type":"ai-title","aiTitle":"Same tick"}"#],
            Some(1000),
        );
        let c = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(c.updated[0].title, "Same tick");
        fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn a_huge_first_prompt_still_lists_the_session() {
        let home = tmpdir("bigfirst");
        let path = home.join(".claude/projects/-w/big.jsonl");
        let big = "x".repeat(200 * 1024);
        let prompt = format!(r#"{{"type":"user","cwd":"/w","message":{{"content":"{big}"}}}}"#);
        write(
            &path,
            &[
                r#"{"type":"permission-mode","mode":"default"}"#,
                &prompt,
                r#"{"type":"assistant","cwd":"/w","message":{"content":"ok"}}"#,
            ],
        );
        let s = parse_claude(&path, SystemTime::UNIX_EPOCH).expect("listed");
        assert_eq!(s.cwd, "/w");
        fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn a_huge_last_line_keeps_the_latest_title_and_folder() {
        let home = tmpdir("biglast");
        let path = home.join(".claude/projects/-w/biglast.jsonl");
        let big = "y".repeat(100 * 1024);
        let pad = format!(
            r#"{{"type":"progress","data":"{}"}}"#,
            "p".repeat(200 * 1024)
        );
        let last = format!(r#"{{"type":"user","cwd":"/w/moved","message":{{"content":"{big}"}}}}"#);
        write(
            &path,
            &[
                r#"{"type":"user","cwd":"/w","message":{"content":"start"}}"#,
                r#"{"type":"ai-title","aiTitle":"Early"}"#,
                &pad,
                r#"{"type":"ai-title","aiTitle":"Late"}"#,
                &last,
            ],
        );
        let s = parse_claude(&path, SystemTime::UNIX_EPOCH).unwrap();
        assert_eq!(s.title, "Late");
        assert_eq!(s.dir, "/w/moved");

        let codex = home.join(".codex/sessions/2026/01/02/rollout-big.jsonl");
        let tc_big = format!(
            r#"{{"type":"response_item","payload":{{"type":"message","role":"assistant","content":[{{"type":"output_text","text":"{big}"}}]}}}}"#
        );
        write(
            &codex,
            &[
                r#"{"type":"session_meta","payload":{"id":"x1","cwd":"/w/b"}}"#,
                &pad,
                r#"{"type":"turn_context","payload":{"cwd":"/w/b2"}}"#,
                &tc_big,
            ],
        );
        let s = parse_codex(&codex, SystemTime::UNIX_EPOCH, &HashMap::new()).unwrap();
        assert_eq!(s.dir, "/w/b2");
        fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn watcher_rereads_codex_titles() {
        let home = tmpdir("watchcodex");
        let rx = spawn_watcher(home.clone(), Duration::from_millis(20));
        std::thread::sleep(Duration::from_millis(100));
        write(
            &home.join(".codex/session_index.jsonl"),
            &[r#"{"id":"x9","thread_name":"Named"}"#],
        );
        let rollout = home.join(".codex/sessions/2026/02/03/rollout-9.jsonl");
        fs::create_dir_all(rollout.parent().unwrap()).unwrap();
        put(
            &rollout,
            &[r#"{"type":"session_meta","payload":{"id":"x9","cwd":"/w"}}"#],
            None,
        );
        let c = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(c.updated.len(), 1);
        assert_eq!(c.updated[0].title, "Named");
        fs::remove_dir_all(&home).unwrap();
    }
}
