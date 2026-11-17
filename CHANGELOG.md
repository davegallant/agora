# Changelog

All notable changes to agora. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

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

[0.4.0]: https://github.com/davegallant/agora/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/davegallant/agora/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/davegallant/agora/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/davegallant/agora/releases/tag/v0.1.0
