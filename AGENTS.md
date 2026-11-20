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

`nix develop` provides the toolchain. Run `cargo test` and
`cargo clippy --all-targets`, then `demo/try.sh` to drive the TUI against
made-up sessions in a throwaway `$HOME`. To script it, run `demo/try.sh` in a
detached `tmux` session and read the screen with `tmux capture-pane`.

## Design rules

- **List order.** Needs-you, then other live sessions, then pinned, then the
  rest; the `s` sort applies only within each group (`App::rank`). Live
  sessions stay on top whatever the user picks.
- **Keep the UI thread free.** Live state, transcript changes and git status
  each come from a background thread over a channel, polled in `App::run`.
  Subprocesses (`claude agents`, Ghostty AppleScript, `git status`) run there,
  never in a key handler. `git status` runs with `GIT_OPTIONAL_LOCKS=0` so it
  never contends with an agent's own git commands.
- **Read transcripts cheaply.** Listing reads only each file's head and tail;
  sizes come from file metadata. Only the full view (`v`) and conversation
  search read whole files.
- **Config files** live in `~/.config/agora/` (`channels`, `theme`, `state`,
  `pins`), one small hand-editable text file each. Loading falls back field by
  field on missing or bad values. A failed write shows in the status line and
  never stops the app. `pins` is re-read before each change and replaced by
  rename, because several agora windows can share it.
- **Saved view.** Channel, agent, live-only and sort are written once, at the
  end of `App::on_key`, when they differ from the last save. A key that changes
  one of them must reach the end of `on_key` rather than return early.
- **The full view (`v`) has its own search.** `/` there searches only that
  transcript and starts fresh; it never touches the list filter, which could
  filter away the session being read.
- **Tests** pass a `tmpdir` as `home`, so they never touch the real `$HOME`.
  The clipboard is a thread-local slot under `cfg(test)`, read with
  `clipboard::last()`.
