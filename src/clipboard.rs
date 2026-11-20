//! Put text on the system clipboard.
//!
//! Tries `pbcopy`, `wl-copy` and `xclip` in turn; if none works, falls back to
//! the OSC 52 escape, which most terminals honour, over SSH too.

#[cfg(not(test))]
pub fn copy(text: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    const TOOLS: &[&[&str]] = &[
        &["pbcopy"],
        &["wl-copy"],
        &["xclip", "-selection", "clipboard"],
    ];
    for argv in TOOLS {
        let Ok(mut child) = Command::new(argv[0])
            .args(&argv[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        else {
            continue;
        };
        let wrote = child
            .stdin
            .take()
            .is_some_and(|mut w| w.write_all(text.as_bytes()).is_ok());
        if child.wait().is_ok_and(|s| s.success()) && wrote {
            return Ok(());
        }
    }
    let mut out = std::io::stdout();
    write!(out, "\x1b]52;c;{}\x07", base64(text.as_bytes()))?;
    out.flush()
}

#[cfg(test)]
thread_local! {
    static LAST: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// In tests the clipboard is a per-thread slot, read back with `last`.
#[cfg(test)]
pub fn copy(text: &str) -> std::io::Result<()> {
    LAST.with(|l| *l.borrow_mut() = Some(text.to_string()));
    Ok(())
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
