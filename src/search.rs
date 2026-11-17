//! Full-text search over conversation text, run on background threads.
//!
//! Each file is first checked with a raw byte search (cheap, catches most
//! misses); only lines that contain a term are parsed, so the user/assistant
//! text is what has to match, not tool output or JSON keys.

use crate::sessions::{message_of, Agent};
use memchr::memmem;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

pub type Files = Arc<Vec<(usize, Agent, PathBuf)>>;

pub struct Hit {
    pub idx: usize,
    pub snippet: String,
}

pub struct Progress {
    pub generation: u64,
    pub hits: Vec<Hit>,
    pub done: usize,
    pub total: usize,
    pub finished: bool,
}

struct Request {
    generation: u64,
    terms: Vec<String>,
    files: Files,
}

pub struct Searcher {
    requests: Sender<Request>,
    pub results: Receiver<Progress>,
    current: Arc<AtomicU64>,
}

/// Search terms: lowercase, whitespace-separated, all must match.
pub fn terms(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .map(|t| t.to_ascii_lowercase())
        .collect()
}

fn floor_boundary(s: &str, mut i: usize) -> usize {
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn ceil_boundary(s: &str, mut i: usize) -> usize {
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// One line of context around byte offset `at` in `text`.
fn snippet(text: &str, at: usize, len: usize) -> String {
    let start = floor_boundary(text, at.saturating_sub(60));
    let end = ceil_boundary(text, (at + len + 80).min(text.len()));
    let body = text[start..end]
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let lead = if start > 0 { "…" } else { "" };
    let tail = if end < text.len() { "…" } else { "" };
    format!("{lead}{body}{tail}")
}

/// Does this transcript's conversation text contain every term? Returns a
/// snippet around the first term's first match.
pub fn search_file(path: &Path, agent: Agent, terms: &[String]) -> Option<String> {
    if terms.is_empty() {
        return None;
    }
    let raw = std::fs::read(path).ok()?;
    let lower = raw.to_ascii_lowercase();
    if !terms
        .iter()
        .all(|t| memmem::find(&lower, t.as_bytes()).is_some())
    {
        return None;
    }

    let mut found: HashSet<usize> = HashSet::new();
    let mut first: Option<String> = None;
    let mut offset = 0;
    for line in lower.split(|&b| b == b'\n') {
        let range = offset..offset + line.len();
        offset += line.len() + 1;
        if !terms
            .iter()
            .any(|t| memmem::find(line, t.as_bytes()).is_some())
        {
            continue;
        }
        let Ok(v) = serde_json::from_slice(&raw[range]) else {
            continue;
        };
        let Some(msg) = message_of(agent, &v) else {
            continue;
        };
        let text_lower = msg.text.to_ascii_lowercase();
        for (n, t) in terms.iter().enumerate() {
            if let Some(at) = text_lower.find(t.as_str()) {
                if found.insert(n) && first.is_none() {
                    first = Some(snippet(&msg.text, at, t.len()));
                }
            }
        }
        if found.len() == terms.len() {
            return first;
        }
    }
    None
}

impl Searcher {
    pub fn spawn() -> Self {
        let (requests, rx) = channel::<Request>();
        let (tx, results) = channel();
        let current = Arc::new(AtomicU64::new(0));
        let latest = current.clone();
        std::thread::spawn(move || {
            while let Ok(mut req) = rx.recv() {
                // Only the newest query matters.
                while let Ok(newer) = rx.try_recv() {
                    req = newer;
                }
                if run(&req, &latest, &tx).is_err() {
                    return;
                }
            }
        });
        Searcher {
            requests,
            results,
            current,
        }
    }

    /// Start a search; any earlier one stops at its next file.
    pub fn start(&self, generation: u64, terms: Vec<String>, files: Files) {
        self.current.store(generation, Ordering::SeqCst);
        let _ = self.requests.send(Request {
            generation,
            terms,
            files,
        });
    }

    pub fn cancel(&self, generation: u64) {
        self.current.store(generation, Ordering::SeqCst);
    }
}

fn run(req: &Request, latest: &AtomicU64, tx: &Sender<Progress>) -> Result<(), ()> {
    let total = req.files.len();
    let done = AtomicUsize::new(0);
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    let next = AtomicUsize::new(0);
    let stale = || latest.load(Ordering::SeqCst) != req.generation;

    let failed = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|_| {
                scope.spawn(|| -> Result<(), ()> {
                    let mut hits = Vec::new();
                    loop {
                        if stale() {
                            return Ok(());
                        }
                        let i = next.fetch_add(1, Ordering::SeqCst);
                        let Some((idx, agent, path)) = req.files.get(i) else {
                            break;
                        };
                        if let Some(snippet) = search_file(path, *agent, &req.terms) {
                            hits.push(Hit { idx: *idx, snippet });
                        }
                        let d = done.fetch_add(1, Ordering::SeqCst) + 1;
                        // Stream results in batches so matches show up early.
                        if !hits.is_empty() || d.is_multiple_of(64) {
                            tx.send(Progress {
                                generation: req.generation,
                                hits: std::mem::take(&mut hits),
                                done: d,
                                total,
                                finished: false,
                            })
                            .map_err(|_| ())?;
                        }
                    }
                    Ok(())
                })
            })
            .collect();
        workers
            .into_iter()
            .any(|w| w.join().unwrap_or(Err(())).is_err())
    });
    if failed {
        return Err(());
    }
    if !stale() {
        tx.send(Progress {
            generation: req.generation,
            hits: Vec::new(),
            done: total,
            total,
            finished: true,
        })
        .map_err(|_| ())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn file(name: &str, body: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("agora-search-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, body).unwrap();
        p
    }

    const CLAUDE: &str = concat!(
        r#"{"type":"user","cwd":"/w","message":{"content":"why is the TFC plan failing?"}}"#,
        "\n",
        r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"grep secret-term"}}]}}"#,
        "\n",
        r#"{"type":"assistant","message":{"content":[{"type":"text","text":"The workspace drifted; re-running the Plan fixed it."}]}}"#,
        "\n",
    );

    #[test]
    fn matches_conversation_text_only() {
        let p = file("c.jsonl", CLAUDE);
        let snip = search_file(&p, Agent::Claude, &terms("tfc PLAN")).unwrap();
        assert!(snip.contains("TFC plan"), "{snip}");
        // Terms may come from different messages.
        assert!(search_file(&p, Agent::Claude, &terms("drifted tfc")).is_some());
        // Tool input is not conversation text.
        assert!(search_file(&p, Agent::Claude, &terms("secret-term")).is_none());
        assert!(search_file(&p, Agent::Claude, &terms("nowhere")).is_none());
    }

    #[test]
    fn snippet_is_char_safe() {
        let text = "é".repeat(100) + "needle" + &"ü".repeat(100);
        let at = text.find("needle").unwrap();
        let s = snippet(&text, at, 6);
        assert!(s.contains("needle") && s.starts_with('…') && s.ends_with('…'));
    }

    #[test]
    fn background_search_streams_hits() {
        let p = file("bg.jsonl", CLAUDE);
        let q = file(
            "other.jsonl",
            r#"{"type":"user","message":{"content":"unrelated"}}"#,
        );
        let s = Searcher::spawn();
        let files: Files = Arc::new(vec![(7, Agent::Claude, p), (8, Agent::Claude, q)]);
        s.start(1, terms("drifted"), files);
        let mut hits = Vec::new();
        loop {
            let pr = s.results.recv_timeout(Duration::from_secs(5)).unwrap();
            hits.extend(pr.hits.into_iter().map(|h| h.idx));
            if pr.finished {
                break;
            }
        }
        assert_eq!(hits, vec![7]);
    }
}
