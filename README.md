# agora

One terminal UI for your AI coding agent sessions: everything that is running
now, and everything you can resume, across **Claude Code**, **Codex** and **pi**.

![agora demo](demo/agora.gif)

- **Live at the top.** Sessions that need you come first, then ones that are
  working or open.
- **History underneath**, newest first, each with its latest prompt beside
  the title.
- **Refreshes as you work.** New sessions appear and busy ones move up on their
  own, and the full view (`v`) follows a conversation as it is written.
- **Search** by title, project, path or agent as you type, and through the
  conversation text itself a moment later. Every word must match the start of
  a word, in any order (`id` finds "ID" and "identity", not "consider");
  `"quoted words"` must appear together, across line breaks. Sessions found
  by name come first, then those whose conversation has the words together,
  and one found only in its conversation shows the matching passage beside
  its title. Tool output and file contents don't
  count.
- **Full view** (`v`) of a whole conversation, with its own search.
- **Git context**: `±` marks a session whose folder has uncommitted changes,
  and the full view shows its branch and how far it is ahead of or behind.
- **Enter** attaches to a running background Claude agent, or resumes the
  session here in its own directory. When you quit the agent, agora comes back
  where you left it.

It works in any terminal, with no tmux. Live state comes from
`claude agents --json`, which covers running Claude sessions, interactive and
background.

### The name

In ancient Greek cities the *agora* (ἀγορά) was the open central square where
people gathered to trade, talk and settle public business. agora is that
square for your agents: one place where every session, live or past, shows up
and you can step into any of them.

### Ghostty integration

On macOS with Ghostty 1.3+, agora also reads Ghostty's tab titles through its
AppleScript dictionary. That adds:

- live state for Codex sessions, which title their tab after the thread, and a
  🔔 when a session's tab rings its bell;
- **enter** jumps to a session's open tab instead of resuming it again;
- **t** resumes a session in a new tab, so agora stays open as a dashboard.

Other terminals don't have these yet.

## Keys

| Key | Action |
| --- | --- |
| `enter` | attach / resume here (jump to tab in Ghostty) |
| `t` | resume (or attach) in a new tab (Ghostty only) |
| `v` | read the whole conversation full-screen: `/` searches it, `n`/`N` jump, `esc` closes |
| `x` | close a live session: its Ghostty split or tab, or `claude stop` for a background one; press twice to confirm |
| `y` `Y` | copy the resume command / the project path |
| `d` | review the session folder's changes in [hunk](https://github.com/modem-dev/hunk) (`git diff` without it); agora returns after |
| `/` | search titles and conversation text; `ctrl-w` deletes a word, `ctrl-u` or `esc` clears |
| `c` `C` | next / previous channel; back to all after the last |
| `#` | pick a channel from a list with session counts: `j`/`k` try one, `enter` keeps it, `esc` cancels |
| `tab` | cycle agent: all, claude, codex, pi |
| `l` | live sessions only |
| `s` | sort by recent, project or size; live sessions stay on top |
| `p` | pin / unpin: pinned sessions sit just under the live ones, `★` before the title |
| `j` `k` `↑` `↓` `g` `G` | move |
| `T` | choose a theme: `j`/`k` try one, `enter` keeps it, `esc` cancels |
| `r` | reload every session from disk |
| `?` | show every key |
| `esc` | clear the search, then the channel, agent and live filters, one per press |
| `q` | quit |

agora reopens with the channel, agent, live-only and sort you left it with.
That view and your pins (one `<agent> <id>` per line) are kept in
`~/.local/state/agora/` (or `$XDG_STATE_HOME/agora/`); settings you edit, like
`channels` and `theme`, live in `~/.config/agora/` (or `$XDG_CONFIG_HOME/agora/`).
Older agora versions kept `state` and `pins` in `~/.config/agora/`; they move on
first use.

Themes: `terminal` (your terminal's own colors), `tokyo-night`, `monokai`,
`catppuccin-mocha`, `dracula`, `gruvbox-dark` and `nord`. The choice is saved in
`~/.config/agora/theme`.

`agora --list` prints live sessions and recent history as tab-separated text, and
`agora --search <words>` prints sessions whose conversation text matches.

## Channels

A channel narrows the list to one area of your work, so sessions from other
areas stay out of the way. Name your channels by folder in
`~/.config/agora/channels`:

```
# name = folder, folder, ...
work     = ~/src/github.com/my-company/*, ~/src/github.com/me/ticket-*
personal = ~/src/github.com/me/blog, ~/dotfiles
```

A folder covers itself and everything under it, and `*` matches part of one
folder name. The first matching line wins. A session that no line matches gets
a channel named after its own folder. `c` steps through your channels in the
order written, then through those folder channels, most recent first; `#`
lists them all with their session counts. The full view names the
session's channel, and a search inside a channel says how many more sessions
match in other channels. Search matches channel names too. `agora --channels` prints every channel and its
session count, tab-separated, in that same order.

## Where sessions come from

| Agent | Transcripts | Title |
| --- | --- | --- |
| Claude Code | `~/.claude/projects/*/*.jsonl` | Claude's AI title, else first prompt |
| Codex | `~/.codex/sessions/**/rollout-*.jsonl` | `~/.codex/session_index.jsonl`, else first prompt |
| pi | `~/.pi/agent/sessions/*/*.jsonl` | first prompt |

## Install

On macOS or Linux (arm64 or x86_64), from the latest release:

```sh
curl -fsSL https://raw.githubusercontent.com/davegallant/agora/main/install.sh | sh
```

It installs to `~/.local/bin` without sudo. Pass options after `sh -s --`:
`--dir DIR` to install elsewhere, `--version v0.2.0` to pick a release
(`AGORA_INSTALL_DIR` and `AGORA_VERSION` work too).

For agents and scripts: it never prompts, progress goes to stderr, and the
only stdout line is the installed binary's absolute path, so
`bin=$(curl -fsSL …/install.sh | sh)` then `"$bin" --version` installs and
checks it. Exit status is 0 on success, 1 on failure (reason on stderr), 2 on
bad options. Once installed, `agora --list` and `agora --search <words>` print
sessions as tab-separated text.

With Nix:

```sh
nix run github:davegallant/agora
```

or add `github:davegallant/agora` as a flake input and install
`agora.packages.${system}.default`. Development: `nix develop`, then `cargo test`. To try your changes against made-up sessions
(your real ones are untouched): `nix develop -c demo/try.sh`.

The demo above is recorded from made-up sessions: `demo/render.sh` seeds a fake
`$HOME`, drives agora through a scripted run and renders the asciinema cast to a
GIF with [agg](https://github.com/asciinema/agg). The `demo` workflow re-records
it when `src/` changes and commits the result to `main`.
