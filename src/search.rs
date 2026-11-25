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
    pub score: Score,
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

/// Search terms, lowercase: each word on its own, so `redis timeout` matches
/// both words anywhere, in any order. `"redis cache"` in quotes is one
/// phrase whose words must appear together; an unclosed quote runs to the
/// end. Runs of whitespace inside a phrase collapse to one space.
pub fn terms(query: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (n, part) in query.split('"').enumerate() {
        if n % 2 == 1 {
            let phrase = part.split_whitespace().collect::<Vec<_>>().join(" ");
            if !phrase.is_empty() {
                out.push(phrase.to_ascii_lowercase());
            }
        } else {
            out.extend(part.split_whitespace().map(str::to_ascii_lowercase));
        }
    }
    out.dedup();
    out
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

/// One line of context around byte offset `at` in `text`. Little comes
/// before the match, so it stays visible in a narrow list column.
fn snippet(text: &str, at: usize, len: usize) -> String {
    let start = floor_boundary(text, at.saturating_sub(24));
    let end = ceil_boundary(text, (at + len + 80).min(text.len()));
    let body = text[start..end]
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let lead = if start > 0 { "…" } else { "" };
    let tail = if end < text.len() { "…" } else { "" };
    format!("{lead}{body}{tail}")
}

/// How a word looks inside a JSON string: quotes, backslashes and control
/// characters are escaped there, so the raw prefilter looks for that form.
fn json_form(word: &str) -> Vec<u8> {
    let quoted = serde_json::to_string(word).unwrap_or_default();
    quoted.as_bytes()[1..quoted.len().saturating_sub(1).max(1)].to_vec()
}

/// Where `term` first starts a word in `hay`: the character before it, if
/// any, is not a letter or digit. Only the start is pinned, so a word still
/// matches while it is being typed (`ide` finds "identity"), but `id` no
/// longer finds "consider". Both are lowercase already.
pub fn find_word(hay: &str, term: &str) -> Option<usize> {
    hay.match_indices(term)
        .map(|(at, _)| at)
        .find(|&at| starts_word(hay, at))
}

/// Does a word start at byte `at` of `hay`?
pub fn starts_word(hay: &str, at: usize) -> bool {
    !hay[..at]
        .chars()
        .next_back()
        .is_some_and(char::is_alphanumeric)
}

/// How well a transcript matches. Higher is better.
pub type Score = u8;
/// The terms, in order, as one run of words in one message.
const TOGETHER: Score = 3;
/// Every term in one message.
const ONE_MESSAGE: Score = 2;
/// Every term, but spread over several messages.
const SPREAD: Score = 1;

/// Does this transcript's conversation text contain every term, each at
/// the start of a word? Returns how closely they match and a snippet from
/// the best message. Whitespace in the text is collapsed before matching,
/// so a phrase matches across line breaks.
pub fn search_file(path: &Path, agent: Agent, terms: &[String]) -> Option<(Score, String)> {
    if terms.is_empty() {
        return None;
    }
    let raw = std::fs::read(path).ok()?;
    let lower = raw.to_ascii_lowercase();
    // Each word of each term, as it appears in the raw JSON. A phrase may be
    // split by a line break in the file, so words are checked separately.
    let words: Vec<Vec<Vec<u8>>> = terms
        .iter()
        .map(|t| t.split(' ').map(json_form).collect())
        .collect();
    let has = |hay: &[u8], w: &Vec<u8>| memmem::find(hay, w).is_some();
    if !words.iter().flatten().all(|w| has(&lower, w)) {
        return None;
    }
    let together = terms.join(" ");

    let mut found: HashSet<usize> = HashSet::new();
    let mut best: Option<(Score, String)> = None;
    let mut offset = 0;
    for line in lower.split(|&b| b == b'\n') {
        let range = offset..offset + line.len();
        offset += line.len() + 1;
        if !words.iter().any(|ws| ws.iter().all(|w| has(line, w))) {
            continue;
        }
        let Ok(v) = serde_json::from_slice(&raw[range]) else {
            continue;
        };
        let Some(msg) = message_of(agent, &v) else {
            continue;
        };
        let flat = msg.text.split_whitespace().collect::<Vec<_>>().join(" ");
        let flat_lower = flat.to_ascii_lowercase();
        let at: Vec<Option<usize>> = terms.iter().map(|t| find_word(&flat_lower, t)).collect();
        found.extend(
            at.iter()
                .enumerate()
                .filter(|(_, a)| a.is_some())
                .map(|(n, _)| n),
        );
        let here = if let Some(p) = find_word(&flat_lower, &together) {
            Some((TOGETHER, p, together.len()))
        } else if at.iter().all(Option::is_some) {
            at[0].map(|p| (ONE_MESSAGE, p, terms[0].len()))
        } else {
            let first = at
                .iter()
                .zip(terms)
                .find_map(|(a, t)| a.map(|p| (p, t.len())));
            first.map(|(p, len)| (SPREAD, p, len))
        };
        if let Some((score, p, len)) = here {
            if best.as_ref().is_none_or(|(b, _)| score > *b) {
                best = Some((score, snippet(&flat, p, len)));
            }
        }
        if best.as_ref().is_some_and(|(b, _)| *b == TOGETHER) {
            break;
        }
    }
    (found.len() == terms.len()).then_some(best).flatten()
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
                        if let Some((score, snippet)) = search_file(path, *agent, &req.terms) {
                            hits.push(Hit {
                                idx: *idx,
                                score,
                                snippet,
                            });
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
        let snip = search_file(&p, Agent::Claude, &terms("tfc PLAN"))
            .unwrap()
            .1;
        assert!(snip.contains("TFC plan"), "{snip}");
        // Words match anywhere, even in different messages and any order...
        assert!(search_file(&p, Agent::Claude, &terms("drifted tfc")).is_some());
        // ...but a quoted phrase must appear as written.
        assert!(search_file(&p, Agent::Claude, &terms("\"drifted tfc\"")).is_none());
        assert!(search_file(&p, Agent::Claude, &terms("\"the plan\" drifted")).is_some());
        // Tool input is not conversation text.
        assert!(search_file(&p, Agent::Claude, &terms("secret-term")).is_none());
        assert!(search_file(&p, Agent::Claude, &terms("nowhere")).is_none());
    }

    #[test]
    fn terms_match_only_at_the_start_of_a_word() {
        assert_eq!(find_word("consider the id", "id"), Some(13));
        assert_eq!(
            find_word("identity", "id"),
            Some(0),
            "a prefix still matches"
        );
        assert_eq!(find_word("said bidding", "id"), None);
        assert_eq!(
            find_word("<task-id>", "id"),
            Some(6),
            "punctuation splits words"
        );
        assert_eq!(
            find_word("éid", "id"),
            None,
            "letters beyond ASCII join words"
        );
        let line =
            serde_json::json!({"type":"user","message":{"content":"swedencentral is central"}});
        let p = file("words.jsonl", &format!("{line}\n"));
        assert!(search_file(&p, Agent::Claude, &terms("entra")).is_none());
        assert!(search_file(&p, Agent::Claude, &terms("cent")).is_some());
    }

    #[test]
    fn words_together_score_above_words_apart() {
        let msg = |t: &str| serde_json::json!({"type":"user","message":{"content":t}}).to_string();
        let score = |name: &str, lines: &[&str]| {
            let body: String = lines.iter().map(|l| msg(l) + "\n").collect();
            search_file(&file(name, &body), Agent::Claude, &terms("entra id"))
        };
        let (s, snip) = score(
            "sc-together",
            &["entra here, id there", "moved to Entra ID today"],
        )
        .unwrap();
        assert_eq!((s, snip.as_str()), (TOGETHER, "moved to Entra ID today"));
        let (s, snip) = score("sc-one", &["about entra", "entra, then an id"]).unwrap();
        assert_eq!((s, snip.as_str()), (ONE_MESSAGE, "entra, then an id"));
        let (s, _) = score("sc-spread", &["about entra", "an id"]).unwrap();
        assert_eq!(s, SPREAD);
        assert!(score("sc-none", &["about entra", "consider it"]).is_none());
    }

    #[test]
    fn matches_text_that_json_escapes() {
        let line = serde_json::json!({"type":"user","cwd":"/w","message":{"content":"please run \"make test\" in C:\\Users\\me"}});
        let p = file("esc.jsonl", &format!("{line}\n"));
        assert!(search_file(&p, Agent::Claude, &terms("\"make test\"")).is_some());
        assert!(search_file(&p, Agent::Claude, &terms("c:\\users")).is_some());
    }

    #[test]
    fn a_phrase_matches_across_line_breaks_and_extra_spaces() {
        let line = serde_json::json!({"type":"user","cwd":"/w","message":{"content":"we should fix  the\nbug today"}});
        let p = file("ws.jsonl", &format!("{line}\n"));
        let snip = search_file(&p, Agent::Claude, &terms("fix the bug"))
            .unwrap()
            .1;
        assert!(snip.contains("fix the bug"), "{snip}");
        assert!(search_file(&p, Agent::Claude, &terms("the bug")).is_some());
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
    fn terms_are_lowercase_words_and_quoted_phrases() {
        assert_eq!(terms("  Foo   BAR\tbaz "), ["foo", "bar", "baz"]);
        assert_eq!(terms(r#"a "Redis   Cache" b"#), ["a", "redis cache", "b"]);
        assert_eq!(terms(r#"x "open phrase"#), ["x", "open phrase"]);
        assert_eq!(terms(r#""" "  " y"#), ["y"]);
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
        let snip = search_file(&p, Agent::Claude, &terms("alpha beta"))
            .unwrap()
            .1;
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
        let snip = search_file(&codex, Agent::Codex, &terms("kubernetes"))
            .unwrap()
            .1;
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
