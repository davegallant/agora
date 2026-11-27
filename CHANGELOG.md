# Changelog

All notable changes to agora. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- `ctrl-w` deletes the last word of a search; `ctrl-u` clears it, as before.
- The list shows each session's latest prompt in grey after its title.
- Ages in the list are colored by how old they are: green under an hour,
  then yellow under a day, orange under a week, and red after that.

### Changed

- The list no longer shows each transcript's size, and `s` no longer sorts
  by it: it switches between recent and project.
- The `?` keys list and the channel picker dim the screen behind them.
- The preview pane under the list is gone, so the list fills the screen; `v`
  reads a conversation full-screen as before.

### Fixed

- Closing a session with `x` keeps the cursor on its row instead of
  following the session down the list.
- A session titled from its first prompt (every pi session, and others
  without a title of their own) shows the first line, cut to about 60
  characters, instead of the whole prompt; a pi skill prompt reads as
  `/skill args`.

## [0.6.0] - 2026-11-25

### Added

- `x` closes a live session: it closes the session's Ghostty split or tab, or
  runs `claude stop` for a background Claude session. Press it twice to
  confirm. It leaves the live group at once, and the transcript stays, so the
  session can still be resumed.
- `d` reviews the selected session's folder in `hunk diff --watch`, or
  `git diff` when hunk isn't installed, and returns to agora afterwards.
- `#` opens a channel picker that lists every channel with its session count.
- The preview names the selected session's channel.
- A search inside a channel shows how many more sessions match in other
  channels.

### Changed

- Search matches every word in any order, in titles and conversation text
  alike, each at the start of a word (`id` finds "ID" and "identity", not
  "consider"); put words in quotes to match them as a phrase. Sessions found
  by name list first, then conversations with the words side by side, in one
  message, and spread apart. A session found only in its conversation shows
  the closest matching passage beside its title.
- `esc` no longer quits. It clears the search, then the channel, agent and
  live-only filters, one per press; `q` quits.
- Opening a long conversation (`v`, or a conversation-search hit) no longer
  freezes the list: the whole transcript loads in the background while the
  tail shows, and only the last three are kept in memory.
- agora polls less: live state every 15s instead of 2s while an agent it
  resumed is running in its terminal, and `git status` only for live and
  selected sessions' folders each round, with the rest every five minutes. A
  `git status` that hangs is stopped after 5s.
- Conversation search reads the sessions in the current channel and agent
  first, so their matches show sooner.
- The screen redraws only when something changes, instead of five times a
  second.
- The saved view and pins move to `~/.local/state/agora/` (or
  `$XDG_STATE_HOME/agora/`), out of the settings folder; existing files move
  there on first use. Settings follow `$XDG_CONFIG_HOME` when it is set.

### Fixed

- Background task notifications no longer count as conversation text.
- The selected row's size and age stay readable on the selection.
- A new session no longer loses its Ghostty tab to an older session with the
  same title in the same folder.
- Long project names end in `…` when cut, and "1 match" / "2 matches" read
  right.
- A session that moves folder partway through, like one Claude's agent view
  moves into `.claude/worktrees/…`, shows git status, `d` and `Y` for the
  folder it works in now. It still resumes from where it started.
- The full view (`v`) stays on the session it opened, even when the list
  changes underneath it.
- A selection lost to a reload or an unfinished conversation search comes back
  when its session does.
- When a conversation search finishes, the preview stays on the most recent
  match instead of jumping to the first.
- New sessions, and sessions that move folder, get git status without `r`.
- Conversation search finds text with quotes and backslashes, and phrases
  split across a line break or extra spaces.
- A session whose first prompt is over 128 KB is listed again, and a huge last
  line no longer hides the latest title.
- Titles can no longer carry terminal escape codes from pasted logs.
- No crash on very small terminals while a search is on.
- After a crash, the shell no longer fills with mouse escapes.
- Tab titles starting with punctuation (`[WIP] …`) match their sessions, and a
  running Claude session keeps its tab over a newer one with the same title.
- A hung Ghostty or a first-use Automation prompt no longer freezes agora.
- Wide-character project names keep their `±` mark.
- `ctrl` chords are no longer typed into the search, and `g` on an empty list
  does nothing.
- An automatic reload no longer reports itself in the status line.
- Watching thousands of transcripts costs far less, and a write that keeps the
  same timestamp is still noticed.

## [0.5.0] - 2026-11-20

### Added

- The list shows each transcript's size beside its age.
- `y` copies the selected session's resume command and `Y` its project path.
  Without `pbcopy`, `wl-copy` or `xclip` it falls back to the terminal's
  clipboard (OSC 52) and says so.
- `s` sorts the list by recent, project or size; live sessions stay on top.
- agora reopens with the channel, agent, live-only and sort you left it with.
- `p` pins a session so it sits just under the live ones, with `★` before
  its title and "pinned" in the preview; pins are kept in
  `~/.config/agora/pins`.
- `v` opens the whole conversation full-screen. `/` searches just that
  transcript (the list filter is left alone), `n`/`N` jump between matches,
  and `v`, `esc` or `q` go back to the list.
- Git context: a `±` after the project marks a folder with uncommitted
  changes, and the preview shows its branch, how many files are uncommitted,
  and how far it is ahead of or behind its remote. Checked in the background
  every 15 seconds for the 100 most recently used folders.
- `◐` marks a Claude session that has finished its turn but still has shells
  running in the background, and the preview says how many.
- Themes: `T` opens a picker that previews each theme as you move through it;
  `enter` keeps one and `esc` goes back. Ships `terminal` (your terminal's own
  colors, the default), `tokyo-night`, `monokai`, `catppuccin-mocha`,
  `dracula`, `gruvbox-dark` and `nord`. The choice is saved in
  `~/.config/agora/theme`.
- `?` shows every key, and `agora --help` prints the same list.

### Changed

- Quitting a session you resumed or attached with `enter` brings agora back,
  on the same row with the same search, instead of leaving you at the shell.
  A non-zero exit shows in the status line.
- With live-only on and nothing live, the list says `l` shows everything.
- The footer shows only the main keys; the rest are behind `?`.
- Conversation search matches the whole query as one phrase: `redis cache`
  finds those words together, not each word on its own. The title filter
  still matches each word anywhere.
- The preview highlights only the current match, marks its row with `▶`, and
  ticks every match on the scrollbar.

### Fixed

- The preview says "just now" for a fresh session, not "now ago".
- A Ghostty bell marks its session `!` again. Ghostty puts the 🔔 on the tab's
  title, not the terminal's, so agora missed it.

## [0.4.0] - 2026-11-17

### Added

- Channels narrow the list to one area of your work. Name them by folder in
  `~/.config/agora/channels` (`name = folder, folder`, with `*` matching part
  of a folder name); a session no rule matches is in a channel named after its
  folder. `c`/`C` step through channels, the header shows the one in use, and
  search matches channel names.
- `agora --channels` prints every channel and its session count.

## [0.3.0] - 2026-11-16

### Added

- The list refreshes on its own: new sessions appear, busy ones move up, and
  the preview follows a conversation as it is written. Only transcripts that
  changed are re-read; `r` still reloads everything.
- The preview scrolls (`J`/`K`, `ctrl-f`/`ctrl-b`, the mouse wheel) and
  highlights search matches; `n`/`N` jump between them.
- A spinner shows while conversation text is being searched.
- `curl | sh` installer for macOS and Linux (arm64 and x86_64). It verifies the
  checksum, needs no sudo, takes `--version` and `--dir`, and prints only the
  installed path on stdout, so agents and scripts can drive it.
- `agora --version`, and `--help` lists the `--list` and `--search` modes.

### Changed

- The preview sits below the list instead of beside it.

### Fixed

- Without Ghostty installed, agora no longer asks macOS to locate it on every
  poll.

## [0.2.0] - 2026-11-16

### Added

- Search inside conversation text, with the matching passage shown in the
  preview; `agora --search <words>` does the same from the shell.

## [0.1.0] - 2026-11-16

### Added

- One terminal UI for live and past Claude Code, Codex and pi sessions: live
  sessions first, history below, a preview of the conversation, and `enter` /
  `t` to jump to, attach to, or resume a session.

[Unreleased]: https://github.com/davegallant/agora/compare/v0.6.0...HEAD
[0.6.0]: https://github.com/davegallant/agora/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/davegallant/agora/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/davegallant/agora/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/davegallant/agora/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/davegallant/agora/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/davegallant/agora/releases/tag/v0.1.0
