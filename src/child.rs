//! Run an agent in the foreground and wait for it, so agora can come back.

use std::io;
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitStatus};

/// Run `argv` in `cwd` on this terminal and wait for it to exit.
///
/// `ctrl-c` and `ctrl-\` reach the whole foreground process group, so agora
/// ignores them while it waits; the agent gets the default handling back.
pub fn run(cwd: Option<&str>, argv: &[String]) -> io::Result<ExitStatus> {
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..]);
    if let Some(dir) = cwd {
        cmd.current_dir(dir).env("PWD", dir);
    }
    // SAFETY: `signal` is async-signal-safe, so it may run between fork and exec.
    unsafe {
        cmd.pre_exec(|| {
            libc::signal(libc::SIGINT, libc::SIG_DFL);
            libc::signal(libc::SIGQUIT, libc::SIG_DFL);
            Ok(())
        });
    }
    // SAFETY: swapping a signal disposition and restoring it afterwards.
    let (int, quit) = unsafe {
        (
            libc::signal(libc::SIGINT, libc::SIG_IGN),
            libc::signal(libc::SIGQUIT, libc::SIG_IGN),
        )
    };
    let status = cmd.status();
    unsafe {
        libc::signal(libc::SIGINT, int);
        libc::signal(libc::SIGQUIT, quit);
    }
    status
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;

    fn sh(script: &str) -> Vec<String> {
        vec!["sh".into(), "-c".into(), script.into()]
    }

    #[test]
    fn returns_the_exit_status() {
        assert_eq!(run(None, &sh("exit 3")).unwrap().code(), Some(3));
        assert!(run(None, &sh("true")).unwrap().success());
        assert!(run(None, &["no-such-agent-binary".into()]).is_err());
    }

    #[test]
    fn runs_in_the_session_folder() {
        let dir = std::env::temp_dir().join(format!("agora-child-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dir = dir.canonicalize().unwrap();
        let st = run(
            Some(dir.to_str().unwrap()),
            &sh("[ \"$(pwd -P)\" = \"$PWD\" ] && pwd -P > here"),
        )
        .unwrap();
        assert!(st.success());
        let here = std::fs::read_to_string(dir.join("here")).unwrap();
        assert_eq!(here.trim(), dir.to_str().unwrap());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_agent_still_gets_ctrl_c() {
        let st = run(None, &sh("kill -INT $$; sleep 1")).unwrap();
        assert_eq!(st.signal(), Some(2), "{st:?}");
    }
}
