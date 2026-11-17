#!/bin/sh
# Rebuild demo/agora.cast and demo/agora.gif from made-up sessions.
# Set AGORA_BIN to skip the build. Needs cargo, python3 and agg (https://github.com/asciinema/agg).
set -eu
cd "$(dirname "$0")/.."

bin=${AGORA_BIN:-target/release/agora}
[ -n "${AGORA_BIN:-}" ] || cargo build --release --locked
home=$(mktemp -d)
trap 'rm -rf "$home"' EXIT

python3 demo/seed.py "$home"
python3 demo/record.py "$bin" "$home" demo/agora.cast
# Tokyo Night: background, foreground, then the 16 ANSI colours.
theme=1a1b26,c0caf5,15161e,f7768e,9ece6a,e0af68,7aa2f7,bb9af7,7dcfff,a9b1d6,565f89,f7768e,9ece6a,e0af68,7aa2f7,bb9af7,7dcfff,c0caf5
agg --theme "$theme" --font-size 16 --idle-time-limit 3 --last-frame-duration 2 \
  demo/agora.cast demo/agora.gif
