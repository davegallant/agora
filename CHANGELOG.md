# Changelog

All notable changes to agora. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added

- The list shows each transcript's size beside its age.
- `y` copies the selected session's resume command and `Y` its project path.
- `s` sorts the list by recent, project or size; live sessions stay on top.
- agora reopens with the channel, agent, live-only and sort you left it with.
- `p` pins a session so it sits just under the live ones, with `★` before
  its title and "pinned" in the preview; pins are kept in
  `~/.config/agora/pins`.
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

- The footer shows only the main keys; the rest are behind `?`.
- Conversation search matches the whole query as one phrase: `redis cache`
  finds those words together, not each word on its own. The title filter
  still matches each word anywhere.
- The preview highlights only the current match, marks its row with `▶`, and
  ticks every match on the scrollbar.

### Fixed

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

[Unreleased]: https://github.com/davegallant/agora/compare/v0.4.0...HEAD
[0.4.0]: https://github.com/davegallant/agora/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/davegallant/agora/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/davegallant/agora/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/davegallant/agora/releases/tag/v0.1.0
