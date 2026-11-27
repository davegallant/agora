//! Render a conversation for the preview pane: word-wrapped rows with a role
//! gutter on every row. Only the current search match is highlighted, and its
//! row's gutter points at it, so there is one place to look.
//!
//! Wrapping is done here rather than by `Paragraph` so the row count, and the
//! row each match lands on, are exact; scrolling to a match depends on it.

use crate::sessions::Msg;
use crate::theme::Theme;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

pub struct Rendered {
    pub rows: Vec<Line<'static>>,
    /// The row of each match, in conversation order.
    pub matches: Vec<usize>,
}

/// Gutter of the row holding the current match.
pub const MARKER: &str = "▶ ";

const GUTTER: usize = 2;

pub fn current_style(theme: &Theme) -> Style {
    Style::new()
        .fg(theme.on_accent)
        .bg(theme.accent)
        .add_modifier(Modifier::BOLD)
}

/// Byte ranges of every case-insensitive occurrence of any term that starts
/// a word, as the search matches them, merged and sorted. Hits of different
/// terms with only spaces between them merge too, so "redis cache" is one
/// match, not two; a repeated word ("x x") stays two. ASCII lowercasing keeps
/// byte offsets aligned with `text`.
pub fn find_ranges(text: &str, terms: &[String]) -> Vec<(usize, usize)> {
    let lower = text.to_ascii_lowercase();
    // (start, end, which term)
    let mut ranges: Vec<(usize, usize, usize)> = Vec::new();
    for (n, t) in terms.iter().enumerate().filter(|(_, t)| !t.is_empty()) {
        ranges.extend(
            lower
                .match_indices(t.as_str())
                .filter(|(at, _)| crate::search::starts_word(&lower, *at))
                .map(|(at, m)| (at, at + m.len(), n)),
        );
    }
    ranges.sort_unstable();
    let mut merged: Vec<(usize, usize, usize)> = Vec::new();
    for (s, e, n) in ranges {
        match merged.last_mut() {
            Some(last)
                if s <= last.1 || (n != last.2 && text[last.1..s].bytes().all(|b| b == b' ')) =>
            {
                last.1 = last.1.max(e);
                last.2 = n;
            }
            _ => merged.push((s, e, n)),
        }
    }
    merged.into_iter().map(|(s, e, _)| (s, e)).collect()
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

/// Lay out `msgs` in `width` columns. `current` is the index of the one match
/// to highlight; the others are found but drawn as plain text.
pub fn render(
    theme: &Theme,
    msgs: &[Msg],
    terms: &[String],
    width: usize,
    current: Option<usize>,
) -> Rendered {
    let avail = width.saturating_sub(GUTTER).max(1);
    let mut rows = Vec::new();
    let mut matches = Vec::new();

    for m in msgs {
        let (gutter, base) = if m.user {
            (
                Span::styled("▌ ", Style::new().fg(theme.accent)),
                Style::new().fg(theme.accent),
            )
        } else {
            (
                Span::styled("│ ", Style::new().fg(theme.muted)),
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
                            current_style(theme)
                        } else {
                            base
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
    if let Some(row) = current
        .and_then(|c| matches.get(c))
        .and_then(|&r| rows.get_mut(r))
    {
        row.spans[0] = Span::styled(
            MARKER,
            Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
        );
    }
    Rendered { rows, matches }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    const T: &Theme = &crate::theme::THEMES[0];

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
        let r = render(T, &[msg(true, "alpha beta gamma")], &[], 12, None);
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
        let r = render(T, &msgs, &terms, 80, Some(1));
        // row 0: user msg, row 1: blank, row 2: "one", row 3: two matches.
        assert_eq!(r.matches, [0, 3, 3]);
        let hit = r.rows[3]
            .spans
            .iter()
            .find(|s| s.content == "NEEDLE")
            .unwrap();
        assert_eq!(hit.style, current_style(T));
    }

    #[test]
    fn merges_overlapping_terms() {
        let terms = vec!["foo bar".to_string(), "bar baz".to_string()];
        assert_eq!(find_ranges("x foo bar baz", &terms), [(2, 13)]);
    }

    fn rows(r: &Rendered) -> Vec<String> {
        r.rows.iter().map(text).collect()
    }

    fn terms(t: &[&str]) -> Vec<String> {
        t.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn find_ranges_cases() {
        assert!(find_ranges("anything", &[]).is_empty());
        assert!(find_ranges("anything", &terms(&[""])).is_empty());
        assert_eq!(find_ranges("Foo-BAR", &terms(&["bar"])), [(4, 7)]);
        // Only where a word starts, as the search matches.
        assert!(find_ranges("FooBAR consider", &terms(&["bar", "sid"])).is_empty());
        assert_eq!(find_ranges("x", &terms(&["", "x"])), [(0, 1)]);
        // Touching ranges merge into one highlight.
        assert_eq!(find_ranges("ab cd", &terms(&["ab", "ab c"])), [(0, 4)]);
        assert_eq!(find_ranges("ab ab", &terms(&["ab"])), [(0, 2), (3, 5)]);
        assert_eq!(
            find_ranges("the Redis  cache, then cache", &terms(&["redis", "cache"])),
            [(4, 16), (23, 28)],
            "words with only spaces between are one match"
        );
        // Offsets are bytes, past multi-byte text.
        assert_eq!(find_ranges("日本 needle", &terms(&["needle"])), [(7, 13)]);
        assert!(find_ranges("short", &terms(&["longer than text"])).is_empty());
    }

    #[test]
    fn nothing_to_render() {
        let r = render(T, &[], &terms(&["x"]), 40, None);
        assert!(r.rows.is_empty() && r.matches.is_empty());
    }

    #[test]
    fn messages_are_separated_by_one_blank_row() {
        let r = render(T, &[msg(true, "q"), msg(false, "a")], &[], 20, None);
        assert_eq!(rows(&r), ["▌ q", "", "│ a"]);
    }

    #[test]
    fn blank_lines_keep_the_gutter() {
        let r = render(T, &[msg(false, "a\n\nb")], &[], 20, None);
        assert_eq!(rows(&r), ["│ a", "│ ", "│ b"]);
    }

    #[test]
    fn breaks_at_a_space_that_does_not_fit() {
        let r = render(T, &[msg(true, "aaaa bbbb")], &[], 6, None);
        assert_eq!(rows(&r), ["▌ aaaa", "▌ bbbb"]);
    }

    #[test]
    fn long_words_are_split() {
        let r = render(T, &[msg(true, "abcdefgh")], &[], 5, None);
        assert_eq!(rows(&r), ["▌ abc", "▌ def", "▌ gh"]);
    }

    #[test]
    fn wide_chars_take_two_columns() {
        let r = render(T, &[msg(true, "日本語")], &[], 6, None);
        assert_eq!(rows(&r), ["▌ 日本", "▌ 語"]);
        // Narrower than one wide char: still one per row, never stuck.
        let r = render(T, &[msg(true, "日本")], &[], 1, None);
        assert_eq!(rows(&r), ["▌ 日", "▌ 本"]);
    }

    #[test]
    fn tabs_expand_and_control_chars_drop() {
        let r = render(T, &[msg(true, "a\tb\u{7}c")], &[], 40, None);
        assert_eq!(rows(&r), ["▌ a    bc"]);
        let r = render(T, &[msg(true, "line\r\nnext")], &[], 40, None);
        assert_eq!(rows(&r), ["▌ line", "▌ next"]);
        // A line of only control chars is a blank row.
        let r = render(T, &[msg(true, "\u{1b}")], &[], 40, None);
        assert_eq!(rows(&r), ["▌ "]);
    }

    #[test]
    fn every_row_fits_the_width() {
        let text = "The quick brown fox jumps over the lazy dog. ".repeat(20)
            + "averyveryverylongwordthatmustbesplitacrossrows 日本語のテキスト";
        for width in [3, 7, 10, 23, 80] {
            let r = render(T, &[msg(false, &text)], &[], width, None);
            for row in &r.rows {
                assert!(row.width() <= width.max(GUTTER + 2), "{width}: {row:?}");
            }
        }
    }

    #[test]
    fn a_wrapped_match_is_on_the_row_it_starts() {
        let t = terms(&["needle"]);
        let r = render(T, &[msg(true, "xx needle")], &t, 7, None);
        assert_eq!(rows(&r), ["▌ xx ", "▌ needl", "▌ e"]);
        assert_eq!(r.matches, [1]);
    }

    #[test]
    fn styles() {
        let t = terms(&["b"]);
        let r = render(T, &[msg(true, "a b"), msg(false, "b c")], &t, 40, None);
        assert_eq!(r.matches, [0, 2]);
        let user = &r.rows[0].spans;
        assert_eq!(user[0].content, "▌ ");
        assert_eq!(user[1].style, Style::new().fg(Color::Yellow));
        assert_eq!(
            user[1].content, "a b",
            "no current match: nothing highlighted"
        );
        let asst = &r.rows[2].spans;
        assert_eq!(asst[0].content, "│ ");
        assert_eq!(asst[1].content, "b c");
        assert_eq!(asst[1].style, Style::new());

        let r = render(T, &[msg(true, "a b"), msg(false, "b c")], &t, 40, Some(0));
        assert_eq!(r.rows[0].spans[0].content, MARKER);
        assert_eq!(r.rows[0].spans[2].style, current_style(T));
        assert_eq!(
            r.rows[2].spans[0].content, "│ ",
            "only the current row is marked"
        );
        assert_eq!(r.rows[2].spans[1].content, "b c", "other matches are plain");
    }

    #[test]
    fn current_match_counts_across_lines_and_messages() {
        let t = terms(&["x"]);
        let msgs = [msg(true, "x x\nx"), msg(false, "x")];
        let r = render(T, &msgs, &t, 40, Some(2));
        assert_eq!(r.matches, [0, 0, 1, 3]);
        let current: Vec<usize> = r
            .rows
            .iter()
            .enumerate()
            .flat_map(|(n, row)| {
                row.spans
                    .iter()
                    .filter(|s| s.style == current_style(T))
                    .map(move |_| n)
            })
            .collect();
        assert_eq!(current, [1], "only the third match is current");
    }

    #[test]
    fn a_multi_char_match_is_one_span() {
        let t = terms(&["needle"]);
        let r = render(T, &[msg(false, "a NEEDLE b")], &t, 40, Some(0));
        let spans: Vec<&str> = r.rows[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(spans, [MARKER, "a ", "NEEDLE", " b"]);
    }
}
