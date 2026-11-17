//! Render a conversation for the preview pane: word-wrapped rows with a role
//! gutter on every row, and search terms highlighted.
//!
//! Wrapping is done here rather than by `Paragraph` so the row count, and the
//! row each match lands on, are exact; scrolling to a match depends on it.

use crate::sessions::Msg;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

pub struct Rendered {
    pub rows: Vec<Line<'static>>,
    /// The row of each match, in conversation order.
    pub matches: Vec<usize>,
}

const GUTTER: usize = 2;

fn match_style() -> Style {
    Style::new()
        .fg(Color::Black)
        .bg(Color::Yellow)
        .add_modifier(Modifier::BOLD)
}

fn current_style() -> Style {
    Style::new()
        .fg(Color::Black)
        .bg(Color::LightRed)
        .add_modifier(Modifier::BOLD)
}

/// Byte ranges of every case-insensitive occurrence of any term, merged and
/// sorted. ASCII lowercasing keeps byte offsets aligned with `text`.
pub fn find_ranges(text: &str, terms: &[String]) -> Vec<(usize, usize)> {
    let lower = text.to_ascii_lowercase();
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for t in terms.iter().filter(|t| !t.is_empty()) {
        ranges.extend(
            lower
                .match_indices(t.as_str())
                .map(|(at, m)| (at, at + m.len())),
        );
    }
    ranges.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (s, e) in ranges {
        match merged.last_mut() {
            Some(last) if s <= last.1 => last.1 = last.1.max(e),
            _ => merged.push((s, e)),
        }
    }
    merged
}

fn width_of(c: char) -> usize {
    c.width().unwrap_or(0)
}

/// Build a row from styled chars, merging runs of the same style into spans.
fn row(gutter: &Span<'static>, chars: &[(char, Style)]) -> Line<'static> {
    let mut spans = vec![gutter.clone()];
    let mut run = String::new();
    let mut style = None;
    for &(c, s) in chars {
        if style != Some(s) {
            if let Some(prev) = style {
                spans.push(Span::styled(std::mem::take(&mut run), prev));
            }
            style = Some(s);
        }
        run.push(c);
    }
    if let Some(s) = style {
        spans.push(Span::styled(run, s));
    }
    Line::from(spans)
}

/// Lay out `msgs` in `width` columns. `current` is the index of the match to
/// draw in the "current" colour.
pub fn render(msgs: &[Msg], terms: &[String], width: usize, current: Option<usize>) -> Rendered {
    let avail = width.saturating_sub(GUTTER).max(1);
    let mut rows = Vec::new();
    let mut matches = Vec::new();

    for m in msgs {
        let (gutter, base) = if m.user {
            (
                Span::styled("▌ ", Style::new().fg(Color::Yellow)),
                Style::new().fg(Color::Yellow),
            )
        } else {
            (
                Span::styled("│ ", Style::new().fg(Color::DarkGray)),
                Style::new(),
            )
        };
        for text in m.text.lines() {
            let text = text.replace('\t', "    ");
            let ranges = find_ranges(&text, terms);
            // Style each char; remember which chars start a match.
            let mut chars: Vec<(char, Style)> = Vec::with_capacity(text.len());
            let mut starts: Vec<usize> = Vec::new();
            let mut r = 0;
            for (at, c) in text.char_indices() {
                if c.is_control() {
                    continue;
                }
                while r < ranges.len() && at >= ranges[r].1 {
                    r += 1;
                }
                let style = match ranges.get(r) {
                    Some(&(s, _)) if at >= s => {
                        let idx = matches.len() + starts.len() - usize::from(at > s);
                        if at == s {
                            starts.push(chars.len());
                        }
                        if Some(idx) == current {
                            current_style()
                        } else {
                            match_style()
                        }
                    }
                    _ => base,
                };
                chars.push((c, style));
            }

            if chars.is_empty() {
                rows.push(Line::from(gutter.clone()));
                continue;
            }
            let mut next_start = 0;
            let mut start = 0;
            while start < chars.len() {
                let mut w = 0;
                let mut end = start;
                let mut last_space = None;
                while end < chars.len() {
                    let cw = width_of(chars[end].0);
                    if w + cw > avail {
                        break;
                    }
                    if chars[end].0 == ' ' {
                        last_space = Some(end);
                    }
                    w += cw;
                    end += 1;
                }
                // Break at the space that didn't fit, else after the last one.
                let mut next = end;
                if end < chars.len() {
                    if chars[end].0 == ' ' {
                        next = end + 1;
                    } else if let Some(sp) = last_space.filter(|&sp| sp > start) {
                        end = sp + 1;
                        next = end;
                    }
                }
                if end == start {
                    end = start + 1;
                    next = end;
                }
                while next_start < starts.len() && starts[next_start] < end {
                    matches.push(rows.len());
                    next_start += 1;
                }
                rows.push(row(&gutter, &chars[start..end]));
                start = next;
            }
        }
        rows.push(Line::raw(""));
    }
    rows.pop();
    Rendered { rows, matches }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(user: bool, text: &str) -> Msg {
        Msg {
            user,
            text: text.into(),
        }
    }

    fn text(l: &Line) -> String {
        l.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn wraps_on_words_and_keeps_gutter() {
        let r = render(&[msg(true, "alpha beta gamma")], &[], 12, None);
        let rows: Vec<String> = r.rows.iter().map(text).collect();
        assert_eq!(rows, ["▌ alpha beta", "▌ gamma"]);
    }

    #[test]
    fn records_match_rows_across_messages() {
        let msgs = [
            msg(true, "find the needle"),
            msg(false, "one\nNEEDLE two needle"),
        ];
        let terms = vec!["needle".to_string()];
        let r = render(&msgs, &terms, 80, Some(1));
        // row 0: user msg, row 1: blank, row 2: "one", row 3: two matches.
        assert_eq!(r.matches, [0, 3, 3]);
        let hit = r.rows[3]
            .spans
            .iter()
            .find(|s| s.content == "NEEDLE")
            .unwrap();
        assert_eq!(hit.style, current_style());
    }

    #[test]
    fn merges_overlapping_terms() {
        let terms = vec!["foo".to_string(), "oob".to_string()];
        assert_eq!(find_ranges("xfoobar", &terms), [(1, 5)]);
    }
}
