//! agora: one TUI for live and past Claude Code, Codex and pi sessions.

mod app;
mod channels;
mod ghostty;
mod live;
mod preview;
mod search;
mod sessions;
mod theme;
mod ui;

use anyhow::{anyhow, Context, Result};
use ratatui::crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use ratatui::crossterm::execute;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

fn main() -> Result<()> {
    if let Some(arg) = std::env::args().nth(1) {
        if arg == "-V" || arg == "--version" {
            println!("agora {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        if arg == "-h" || arg == "--help" {
            print!(
                "agora: browse live and past Claude Code, Codex and pi sessions.\n\n\
                 usage: agora                  open the browser\n\
                 \x20      agora --list           live and recent sessions, tab-separated\n\
                 \x20      agora --search <words> sessions whose conversation text matches\n\
                 \x20      agora --channels       channels and their session counts\n\
                 \x20      agora --version        print the version\n\n\
                 keys:\n"
            );
            for (key, what) in ui::KEYS {
                println!("  {key:<8} {what}");
            }
            println!(
                "\nchannels are set in ~/.config/agora/channels; the theme is kept in\n\
                 ~/.config/agora/theme."
            );
            return Ok(());
        }
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is not set")?;
    let sessions = sessions::load_all(&home);

    // Plain listing for scripts: live sessions first, then the most recent.
    if std::env::args().nth(1).as_deref() == Some("--list") {
        let live = live::resolve(&sessions, &live::snapshot());
        let mut order: Vec<usize> = (0..sessions.len()).collect();
        order.sort_by_key(|i| live.get(i).map_or(3, |l| l.state as u8));
        for i in order.into_iter().take(50) {
            let s = &sessions[i];
            let state = live
                .get(&i)
                .map_or("-".into(), |l| format!("{:?}", l.state));
            println!("{state}\t{}\t{}\t{}", s.agent.name(), s.project(), s.title);
        }
        return Ok(());
    }

    // Every channel with its session count, in the order `c` steps through them.
    if std::env::args().nth(1).as_deref() == Some("--channels") {
        let rules = channels::Channels::load(&home);
        let tagged: Vec<(String, std::time::SystemTime)> = sessions
            .iter()
            .map(|s| (rules.of(&s.cwd), s.mtime))
            .collect();
        for (name, count) in rules.ordered(tagged.iter().map(|(c, t)| (c.as_str(), *t))) {
            println!("{name}\t{count}");
        }
        return Ok(());
    }

    // Search conversation text from the shell: `agora --search "tfc plan"`.
    if std::env::args().nth(1).as_deref() == Some("--search") {
        let query: Vec<String> = std::env::args().skip(2).collect();
        let terms = search::terms(&query.join(" "));
        let searcher = search::Searcher::spawn();
        let files = std::sync::Arc::new(
            sessions
                .iter()
                .enumerate()
                .map(|(i, s)| (i, s.agent, s.path.clone()))
                .collect(),
        );
        searcher.start(1, terms, files);
        while let Ok(p) = searcher.results.recv() {
            for h in p.hits {
                let s = &sessions[h.idx];
                println!(
                    "{}\t{}\t{}\t{}",
                    s.agent.name(),
                    s.project(),
                    s.title,
                    h.snippet
                );
            }
            if p.finished {
                break;
            }
        }
        return Ok(());
    }

    let mut terminal = ratatui::init();
    // Mouse capture lets the wheel scroll the panes; shift-drag still selects text.
    let _ = execute!(std::io::stdout(), EnableMouseCapture);
    let result = app::App::new(home, sessions).run(&mut terminal);
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();

    match result? {
        app::Exit::Quit => Ok(()),
        app::Exit::Exec { cwd, argv } => {
            let mut cmd = Command::new(&argv[0]);
            cmd.args(&argv[1..]);
            if let Some(dir) = cwd {
                cmd.current_dir(&dir).env("PWD", &dir);
            }
            Err(anyhow!("could not run {}: {}", argv[0], cmd.exec()))
        }
    }
}
