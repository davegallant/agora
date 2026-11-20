//! Put text on the system clipboard.
//!
//! Tries `pbcopy`, `wl-copy` and `xclip` in turn; if none works, falls back to
//! the OSC 52 escape, which most terminals honour, over SSH too.

use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// How the text reached the clipboard.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Via {
    /// A clipboard tool took it.
    Tool,
    /// Sent as OSC 52; the terminal may or may not honour it.
    Terminal,
}

/// Pipe `text` into `argv`, giving up (and killing it) after `timeout`.
/// True if it took the text and exited cleanly.
pub fn run_tool(argv: &[&str], text: &str, timeout: Duration) -> bool {
    let Ok(mut child) = Command::new(argv[0])
        .args(&argv[1..])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    // Dropping stdin closes it, so the tool sees the end of the text.
    let wrote = child
        .stdin
        .take()
        .is_some_and(|mut w| w.write_all(text.as_bytes()).is_ok());
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return wrote && status.success(),
            Ok(None) if start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

#[cfg(not(test))]
pub fn copy(text: &str) -> std::io::Result<Via> {
    const TOOLS: &[&[&str]] = &[
        &["pbcopy"],
        &["wl-copy"],
        &["xclip", "-selection", "clipboard"],
    ];
    for argv in TOOLS {
        if run_tool(argv, text, Duration::from_secs(1)) {
            return Ok(Via::Tool);
        }
    }
    // Safe beside ratatui: it has flushed its frame, and OSC moves no cursor.
    let mut out = std::io::stdout();
    write!(out, "\x1b]52;c;{}\x07", base64(text.as_bytes()))?;
    out.flush()?;
    Ok(Via::Terminal)
}

#[cfg(test)]
thread_local! {
    static LAST: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// In tests the clipboard is a per-thread slot, read back with `last`.
#[cfg(test)]
pub fn copy(text: &str) -> std::io::Result<Via> {
    LAST.with(|l| *l.borrow_mut() = Some(text.to_string()));
    Ok(Via::Tool)
}

#[cfg(test)]
pub fn last() -> Option<String> {
    LAST.with(|l| l.borrow().clone())
}

/// Standard base64 with padding, for OSC 52.
fn base64(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::{Duration, Instant};

    fn sh(script: &str) -> Vec<&str> {
        vec!["sh", "-c", script]
    }

    #[test]
    fn a_tool_that_takes_the_text_and_exits_works() {
        assert!(run_tool(
            &sh("cat >/dev/null"),
            "hi",
            Duration::from_secs(2)
        ));
        assert!(!run_tool(
            &sh("cat >/dev/null; exit 1"),
            "hi",
            Duration::from_secs(2)
        ));
        assert!(!run_tool(
            &["no-such-clipboard-tool"],
            "hi",
            Duration::from_secs(2)
        ));
    }

    #[test]
    fn a_stuck_tool_is_given_up_on() {
        let start = Instant::now();
        assert!(!run_tool(&sh("sleep 5"), "hi", Duration::from_millis(200)));
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "{:?}",
            start.elapsed()
        );
    }

    #[test]
    fn base64_matches_the_standard() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64("cd '/a b' ✓".as_bytes()), "Y2QgJy9hIGInIOKckw==");
    }
}
