//! agora: one TUI for live and past Claude Code, Codex and pi sessions.

mod app;
mod ghostty;
mod live;
mod search;
mod sessions;
mod ui;

use anyhow::{anyhow, Context, Result};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

fn main() -> Result<()> {
    if let Some(arg) = std::env::args().nth(1) {
        if arg == "-h" || arg == "--help" {
            println!(
                "agora: browse live and past Claude Code, Codex and pi sessions.\n\n\
                 enter  jump to the session's Ghostty tab, or resume it here\n\
                 t      resume in a new Ghostty tab\n\
                 /      search    tab  cycle agent    l  live only\n\
                 r      reload    q    quit"
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
    let result = app::App::new(home, sessions).run(&mut terminal);
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
