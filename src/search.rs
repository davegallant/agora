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

/// Search terms: the whole query as one lowercase phrase, so "redis cache"
/// matches those words together, not each word on its own. Runs of
/// whitespace collapse to one space.
pub fn terms(query: &str) -> Vec<String> {
    let phrase = query.split_whitespace().collect::<Vec<_>>().join(" ");
    if phrase.is_empty() {
        Vec::new()
    } else {
        vec![phrase.to_ascii_lowercase()]
    }
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
        // The query is one phrase: its words must appear together.
        assert!(search_file(&p, Agent::Claude, &terms("drifted tfc")).is_none());
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

    fn collect(s: &Searcher, generation: u64) -> (Vec<usize>, Vec<Progress>) {
        let mut hits = Vec::new();
        let mut seen = Vec::new();
        loop {
            let p = s.results.recv_timeout(Duration::from_secs(5)).unwrap();
            if p.generation != generation {
                continue;
            }
            hits.extend(p.hits.iter().map(|h| h.idx));
            let finished = p.finished;
            seen.push(p);
            if finished {
                hits.sort();
                return (hits, seen);
            }
        }
    }

    #[test]
    fn terms_are_one_lowercase_phrase() {
        assert_eq!(terms("  Foo   BAR\tbaz "), ["foo bar baz"]);
        assert!(terms("").is_empty());
        assert!(terms("   ").is_empty());
        // Only ASCII is folded, matching the byte search.
        assert_eq!(terms("ÉCOLE"), ["École"]);
    }

    #[test]
    fn char_boundaries() {
        let s = "aé"; // 'é' is bytes 1..3
        assert_eq!(floor_boundary(s, 2), 1);
        assert_eq!(ceil_boundary(s, 2), 3);
        assert_eq!(floor_boundary(s, 0), 0);
        assert_eq!(ceil_boundary(s, 3), 3);
        assert_eq!(ceil_boundary(s, 10), 10, "past the end is left alone");
    }

    #[test]
    fn snippets() {
        assert_eq!(snippet("short text", 0, 5), "short text");
        assert_eq!(snippet("a\n\n   b\tc", 0, 1), "a b c");
        let long = format!("{}needle{}", "a ".repeat(10), "b ".repeat(100));
        let s = snippet(&long, 20, 6);
        assert!(!s.starts_with('…') && s.ends_with('…'), "{s}");
        let long = format!("{}needle", "a ".repeat(100));
        let s = snippet(&long, 200, 6);
        assert!(s.starts_with('…') && !s.ends_with('…'), "{s}");
        assert!(s.ends_with("needle"));
    }

    #[test]
    fn search_file_edge_cases() {
        let p = file("edge.jsonl", CLAUDE);
        assert!(search_file(&p, Agent::Claude, &[]).is_none());
        assert!(search_file(&p.with_extension("gone"), Agent::Claude, &terms("tfc")).is_none());
        // Every term must match somewhere.
        assert!(search_file(&p, Agent::Claude, &terms("drifted nowhere")).is_none());
        // JSON keys are not conversation text.
        assert!(search_file(&p, Agent::Claude, &terms("content")).is_none());
        assert!(search_file(&p, Agent::Claude, &terms("assistant")).is_none());
    }

    #[test]
    fn search_skips_injected_user_context() {
        let p = file(
            "noise.jsonl",
            concat!(
                r#"{"type":"user","message":{"content":"<system-reminder>magicword</system-reminder>"}}"#,
                "\n",
                r#"{"type":"user","isMeta":true,"message":{"content":"othermagic"}}"#,
                "\n",
            ),
        );
        assert!(search_file(&p, Agent::Claude, &terms("magicword")).is_none());
        assert!(search_file(&p, Agent::Claude, &terms("othermagic")).is_none());
    }

    #[test]
    fn snippet_comes_from_the_earliest_matching_message() {
        let p = file(
            "order.jsonl",
            concat!(
                r#"{"type":"user","message":{"content":"first mentions alpha beta"}}"#,
                "\n",
                r#"{"type":"assistant","message":{"content":"then alpha beta again"}}"#,
                "\n",
            ),
        );
        let snip = search_file(&p, Agent::Claude, &terms("alpha beta")).unwrap();
        assert_eq!(snip, "first mentions alpha beta");
    }

    #[test]
    fn searches_codex_and_pi_transcripts() {
        let codex = file(
            "codex.jsonl",
            concat!(
                r#"{"type":"session_meta","payload":{"id":"x","cwd":"/w","instructions":"kubernetes"}}"#,
                "\n",
                r#"{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Deploy to Kubernetes now"}]}}"#,
                "\n",
            ),
        );
        let snip = search_file(&codex, Agent::Codex, &terms("kubernetes")).unwrap();
        assert_eq!(snip, "Deploy to Kubernetes now");
        // Read as the wrong agent, nothing is conversation text.
        assert!(search_file(&codex, Agent::Pi, &terms("kubernetes")).is_none());

        let pi = file(
            "pi.jsonl",
            concat!(
                r#"{"type":"session","id":"p","cwd":"/w"}"#,
                "\n",
                r#"{"type":"message","message":{"role":"user","content":[{"type":"text","text":"Rename the Widget"}]}}"#,
                "\n",
            ),
        );
        assert!(search_file(&pi, Agent::Pi, &terms("widget")).is_some());
    }

    #[test]
    fn empty_search_finishes_at_once() {
        let s = Searcher::spawn();
        s.start(3, terms("x"), Arc::new(Vec::new()));
        let (hits, seen) = collect(&s, 3);
        assert!(hits.is_empty());
        assert_eq!(seen.len(), 1);
        assert_eq!((seen[0].done, seen[0].total), (0, 0));
    }

    #[test]
    fn progress_is_reported_while_searching() {
        let p = file(
            "miss.jsonl",
            r#"{"type":"user","message":{"content":"nothing"}}"#,
        );
        let files: Files = Arc::new((0..130).map(|i| (i, Agent::Claude, p.clone())).collect());
        let s = Searcher::spawn();
        s.start(1, terms("absent"), files);
        let (hits, seen) = collect(&s, 1);
        assert!(hits.is_empty());
        let last = seen.last().unwrap();
        assert_eq!((last.done, last.total), (130, 130));
        assert!(
            seen.iter().any(|p| !p.finished && p.done % 64 == 0),
            "batches every 64 files"
        );
        assert!(seen.iter().all(|p| p.total == 130));
    }

    #[test]
    fn every_matching_file_is_reported_once() {
        let hit = file("many-hit.jsonl", CLAUDE);
        let miss = file(
            "many-miss.jsonl",
            r#"{"type":"user","message":{"content":"x"}}"#,
        );
        let files: Files = Arc::new(
            (0..40)
                .map(|i| {
                    (
                        i,
                        Agent::Claude,
                        if i % 3 == 0 {
                            hit.clone()
                        } else {
                            miss.clone()
                        },
                    )
                })
                .collect(),
        );
        let s = Searcher::spawn();
        s.start(1, terms("drifted"), files);
        let (hits, _) = collect(&s, 1);
        assert_eq!(hits, (0..40).filter(|i| i % 3 == 0).collect::<Vec<_>>());
    }

    #[test]
    fn a_newer_search_replaces_an_older_one() {
        let p = file("new-a.jsonl", CLAUDE);
        let q = file(
            "new-b.jsonl",
            r#"{"type":"user","message":{"content":"unrelated"}}"#,
        );
        let files: Files = Arc::new(vec![(7, Agent::Claude, p), (8, Agent::Claude, q)]);
        let s = Searcher::spawn();
        s.start(1, terms("drifted"), files.clone());
        s.start(2, terms("unrelated"), files.clone());
        let (hits, _) = collect(&s, 2);
        assert_eq!(hits, [8]);

        // Searching again after a cancel works.
        s.cancel(3);
        s.start(4, terms("tfc"), files);
        let (hits, _) = collect(&s, 4);
        assert_eq!(hits, [7]);
    }
}
