//! Agent logos, drawn as small images in terminals that can show them.
//!
//! Only terminals known to draw images well get them (Kitty, Ghostty, iTerm2,
//! WezTerm, sixel ones), and never inside tmux; everywhere else the list keeps
//! its plain agent names. `AGORA_NO_IMAGES` turns them off. The logos come
//! from fast-resume (MIT, https://github.com/angristan/fast-resume).

use ratatui::layout::Rect;
use ratatui_image::{
    picker::{Picker, ProtocolType},
    protocol::Protocol,
    FilterType, Resize,
};

use crate::sessions::Agent;

/// Columns an icon takes in the list.
pub const WIDTH: u16 = 2;

pub struct Art {
    claude: Protocol,
    codex: Protocol,
    pi: Protocol,
}

impl Art {
    /// The logos, sized to the terminal's cells; `None` where images are off.
    /// Asks the terminal for its cell size, so it runs in raw mode before
    /// anything else reads input.
    pub fn load() -> Option<Art> {
        let kind = protocol(&|k| std::env::var(k).ok())?;
        let mut picker = Picker::from_query_stdio().ok()?;
        picker.set_protocol_type(kind);
        let make = |png: &[u8]| -> Option<Protocol> {
            let image = image::load_from_memory(png).ok()?;
            picker
                .new_protocol(
                    image,
                    Rect::new(0, 0, WIDTH, 1),
                    Resize::Fit(Some(FilterType::Lanczos3)),
                )
                .ok()
        };
        Some(Art {
            claude: make(include_bytes!("../assets/agents/claude.png"))?,
            codex: make(include_bytes!("../assets/agents/codex.png"))?,
            pi: make(include_bytes!("../assets/agents/pi.png"))?,
        })
    }

    pub fn get(&self, a: Agent) -> &Protocol {
        match a {
            Agent::Claude => &self.claude,
            Agent::Codex => &self.codex,
            Agent::Pi => &self.pi,
        }
    }
}

/// The image protocol this terminal speaks, judged from its environment.
fn protocol(env: &dyn Fn(&str) -> Option<String>) -> Option<ProtocolType> {
    let has = |k: &str| env(k).is_some_and(|v| !v.is_empty());
    let says = |k: &str, want: &str| env(k).is_some_and(|v| v.contains(want));
    if has("AGORA_NO_IMAGES") || has("TMUX") {
        return None;
    }
    if has("KITTY_WINDOW_ID") || says("TERM_PROGRAM", "ghostty") || says("TERM", "ghostty") {
        Some(ProtocolType::Kitty)
    } else if has("ITERM_SESSION_ID")
        || says("TERM_PROGRAM", "iTerm")
        || says("TERM_PROGRAM", "WezTerm")
    {
        Some(ProtocolType::Iterm2)
    } else if says("TERM", "sixel") {
        Some(ProtocolType::Sixel)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(vars: &[(&str, &str)]) -> Option<ProtocolType> {
        let vars: Vec<(String, String)> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        protocol(&|k| vars.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()))
    }

    #[test]
    fn picks_the_protocol_from_the_environment() {
        assert_eq!(
            with(&[("TERM_PROGRAM", "ghostty")]),
            Some(ProtocolType::Kitty)
        );
        assert_eq!(with(&[("KITTY_WINDOW_ID", "1")]), Some(ProtocolType::Kitty));
        assert_eq!(
            with(&[("TERM_PROGRAM", "iTerm.app")]),
            Some(ProtocolType::Iterm2)
        );
        assert_eq!(
            with(&[("TERM_PROGRAM", "WezTerm")]),
            Some(ProtocolType::Iterm2)
        );
        assert_eq!(with(&[("TERM", "foot-sixel")]), Some(ProtocolType::Sixel));
        assert_eq!(with(&[("TERM", "xterm-256color")]), None);
        assert_eq!(with(&[]), None);
    }

    #[test]
    fn tmux_and_the_switch_turn_images_off() {
        assert_eq!(
            with(&[
                ("TERM_PROGRAM", "ghostty"),
                ("TMUX", "/tmp/tmux-1/default,1,0")
            ]),
            None
        );
        assert_eq!(
            with(&[("TERM_PROGRAM", "ghostty"), ("AGORA_NO_IMAGES", "1")]),
            None
        );
    }

    #[test]
    fn the_logos_decode() {
        for png in [
            &include_bytes!("../assets/agents/claude.png")[..],
            include_bytes!("../assets/agents/codex.png"),
            include_bytes!("../assets/agents/pi.png"),
        ] {
            assert!(image::load_from_memory(png).is_ok());
        }
    }
}
