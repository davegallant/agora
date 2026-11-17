//! Color themes. `terminal` keeps the terminal's own palette and background;
//! the others paint their background and use exact colors.
//!
//! The chosen theme is kept by name in `~/.config/agora/theme`.

use ratatui::style::Color;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, PartialEq)]
pub struct Theme {
    pub name: &'static str,
    /// Background and default text; `None` leaves the terminal's.
    pub bg: Option<Color>,
    pub fg: Option<Color>,
    /// Hints, ages, rules: text that should recede.
    pub muted: Color,
    /// Secondary text: project names, the session's folder.
    pub subtle: Color,
    /// Search, the user's turns, the attention badge.
    pub accent: Color,
    /// Text drawn on an `accent` background.
    pub on_accent: Color,
    pub busy: Color,
    pub open: Color,
    pub selection: Color,
    /// The current match's tick on the scrollbar.
    pub marker: Color,
    pub claude: Color,
    pub codex: Color,
    pub pi: Color,
}

const fn rgb(hex: u32) -> Color {
    Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

pub const THEMES: &[Theme] = &[
    Theme {
        name: "terminal",
        bg: None,
        fg: None,
        muted: Color::DarkGray,
        subtle: Color::Gray,
        accent: Color::Yellow,
        on_accent: Color::Black,
        busy: Color::Green,
        open: Color::Cyan,
        selection: Color::DarkGray,
        marker: Color::LightRed,
        claude: Color::LightRed,
        codex: Color::LightBlue,
        pi: Color::LightMagenta,
    },
    Theme {
        name: "tokyo-night",
        bg: Some(rgb(0x1a1b26)),
        fg: Some(rgb(0xc0caf5)),
        muted: rgb(0x565f89),
        subtle: rgb(0xa9b1d6),
        accent: rgb(0xe0af68),
        on_accent: rgb(0x1a1b26),
        busy: rgb(0x9ece6a),
        open: rgb(0x7dcfff),
        selection: rgb(0x283457),
        marker: rgb(0xf7768e),
        claude: rgb(0xff9e64),
        codex: rgb(0x7aa2f7),
        pi: rgb(0xbb9af7),
    },
    Theme {
        name: "monokai",
        bg: Some(rgb(0x272822)),
        fg: Some(rgb(0xf8f8f2)),
        muted: rgb(0x75715e),
        subtle: rgb(0xcfcfc2),
        accent: rgb(0xe6db74),
        on_accent: rgb(0x272822),
        busy: rgb(0xa6e22e),
        open: rgb(0x66d9ef),
        selection: rgb(0x49483e),
        marker: rgb(0xf92672),
        claude: rgb(0xfd971f),
        codex: rgb(0x66d9ef),
        pi: rgb(0xae81ff),
    },
    Theme {
        name: "catppuccin-mocha",
        bg: Some(rgb(0x1e1e2e)),
        fg: Some(rgb(0xcdd6f4)),
        muted: rgb(0x6c7086),
        subtle: rgb(0xbac2de),
        accent: rgb(0xf9e2af),
        on_accent: rgb(0x1e1e2e),
        busy: rgb(0xa6e3a1),
        open: rgb(0x89dceb),
        selection: rgb(0x45475a),
        marker: rgb(0xf38ba8),
        claude: rgb(0xfab387),
        codex: rgb(0x89b4fa),
        pi: rgb(0xcba6f7),
    },
    Theme {
        name: "dracula",
        bg: Some(rgb(0x282a36)),
        fg: Some(rgb(0xf8f8f2)),
        muted: rgb(0x6272a4),
        subtle: rgb(0xd6d6d0),
        accent: rgb(0xf1fa8c),
        on_accent: rgb(0x282a36),
        busy: rgb(0x50fa7b),
        open: rgb(0x8be9fd),
        selection: rgb(0x44475a),
        marker: rgb(0xff5555),
        claude: rgb(0xffb86c),
        codex: rgb(0x8be9fd),
        pi: rgb(0xbd93f9),
    },
    Theme {
        name: "gruvbox-dark",
        bg: Some(rgb(0x282828)),
        fg: Some(rgb(0xebdbb2)),
        muted: rgb(0x928374),
        subtle: rgb(0xd5c4a1),
        accent: rgb(0xfabd2f),
        on_accent: rgb(0x282828),
        busy: rgb(0xb8bb26),
        open: rgb(0x8ec07c),
        selection: rgb(0x504945),
        marker: rgb(0xfb4934),
        claude: rgb(0xfe8019),
        codex: rgb(0x83a598),
        pi: rgb(0xd3869b),
    },
    Theme {
        name: "nord",
        bg: Some(rgb(0x2e3440)),
        fg: Some(rgb(0xd8dee9)),
        muted: rgb(0x616e88),
        subtle: rgb(0xe5e9f0),
        accent: rgb(0xebcb8b),
        on_accent: rgb(0x2e3440),
        busy: rgb(0xa3be8c),
        open: rgb(0x88c0d0),
        selection: rgb(0x434c5e),
        marker: rgb(0xbf616a),
        claude: rgb(0xd08770),
        codex: rgb(0x81a1c1),
        pi: rgb(0xb48ead),
    },
];

/// The index of the theme called `name`.
pub fn find(name: &str) -> Option<usize> {
    THEMES.iter().position(|t| t.name == name.trim())
}

fn path(home: &Path) -> PathBuf {
    home.join(".config/agora/theme")
}

/// The saved theme; a missing, unreadable or unknown one means `terminal`.
pub fn load(home: &Path) -> usize {
    fs::read_to_string(path(home))
        .ok()
        .and_then(|s| find(&s))
        .unwrap_or(0)
}

pub fn save(home: &Path, i: usize) -> std::io::Result<()> {
    let p = path(home);
    if let Some(dir) = p.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(p, format!("{}\n", THEMES[i].name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_unique_and_terminal_is_first() {
        assert_eq!(THEMES[0].name, "terminal");
        for (i, t) in THEMES.iter().enumerate() {
            assert_eq!(find(t.name), Some(i));
        }
        assert_eq!(find(" monokai\n"), find("monokai"));
        assert_eq!(find("nope"), None);
    }

    #[test]
    fn saves_and_loads_by_name() {
        let home = std::env::temp_dir().join(format!("agora-theme-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        assert_eq!(load(&home), 0, "nothing saved yet");
        let tokyo = find("tokyo-night").unwrap();
        save(&home, tokyo).unwrap();
        assert_eq!(load(&home), tokyo);
        fs::write(path(&home), "no-such-theme\n").unwrap();
        assert_eq!(load(&home), 0);
        fs::remove_dir_all(&home).unwrap();
    }
}
