# agora

## Git

Work on `main`: commit approved changes straight to it and push. This personal
repo uses no feature branches or PRs. A push to `main` runs the `ci` workflow,
and changes under `src/` or `demo/` also run `demo`, which re-records the README
GIF and commits it to `main`. Pull before your next push.

Every user-visible change gets a line under `## [Unreleased]` in
`CHANGELOG.md`, in the commit that makes it. A new key also gets a row in
`ui::KEYS` (which drives both `?` and `--help`) and in the README keys table.

Design notes and plans stay local: `docs/superpowers/` and `.superpowers/` are
ignored, so stage explicit paths.

## Checking a change

`nix develop` provides the toolchain. Run `cargo fmt --check`, `cargo test`
and `cargo clippy --all-targets -- -D warnings` (what CI runs), then `demo/try.sh` to drive the TUI against
made-up sessions in a throwaway `$HOME`. To script it, run `demo/try.sh` in a
detached `tmux` session and read the screen with `tmux capture-pane`.

## Design rules

- **List order.** Needs-you, then other live sessions, then pinned, then the
  rest; the `s` sort applies only within each group (`App::rank`). Live
  sessions stay on top whatever the user picks.
- **Keep the UI thread free.** Live state, transcript changes and git status
  each come from a background thread over a channel, polled in `App::run`.
  Polling subprocesses (`claude agents`, Ghostty's tab list, `git status`) run
  there. The few a key starts (Ghostty focus or new tab, a clipboard tool) get
  a timeout, so a hung one cannot freeze the UI. `git status` runs with `GIT_OPTIONAL_LOCKS=0` so it
  never contends with an agent's own git commands.
- **Read transcripts cheaply.** Listing reads only each file's head and tail;
  sizes come from file metadata. Only the full view (`v`) and conversation
  search read whole files.
- **Files** go through `paths.rs`: settings you edit (`channels`, `theme`) in
  the XDG config folder, state agora writes (`state`, `pins`) in the XDG state
  folder; one small hand-editable text file each. Tests ignore the XDG
  variables, and anything that runs the binary in a throwaway `$HOME` must
  clear them. Loading falls back field by
  field on missing or bad values. A failed write shows in the status line and
  never stops the app. `pins` is re-read before each change and replaced by
  rename, because several agora windows can share it.
- **Two folders per session.** `Session::cwd` is where it started; Claude files
  the transcript there and `claude --resume` must run from it. `Session::dir`
  is the folder its last turn worked in (read from the transcript tail), used
  for git status, `d`, `Y` and the preview.
- **Saved view.** Channel, agent, live-only and sort are written once, at the
  end of `App::on_key`, when they differ from the last save. A key that changes
  one of them must reach the end of `on_key` rather than return early.
- **The full view (`v`) has its own search.** `/` there searches only that
  transcript and starts fresh; it never touches the list filter, which could
  filter away the session being read.
- **Resuming returns to agora.** `enter` tears the TUI down, runs the agent
  as a child (`child::run`) and redraws when it exits. agora ignores SIGINT and
  SIGQUIT only while it waits, and the child gets them back in `pre_exec`.
- **Tests** pass a `tmpdir` as `home`, so they never touch the real `$HOME`.
  The clipboard is a thread-local slot under `cfg(test)`, read with
  `clipboard::last()`.
