#!/bin/sh
# Rebuild demo/agora.gif from made-up sessions, recorded in a real Kitty window
# (demo/record.py) so the agent logos show. Never touches real sessions.
# Needs Linux with Xvfb, kitty, xdotool, ffmpeg and the JetBrains Mono font.
set -eu
cd "$(dirname "$0")/.."

bin=${AGORA_BIN:-target/release/agora}
[ -n "${AGORA_BIN:-}" ] || cargo build --release --locked
home=$(mktemp -d)
rec=$(mktemp -d)
trap 'rm -rf "$home" "$rec"' EXIT

python3 demo/seed.py "$home"
python3 demo/record.py "$bin" "$home" "$rec/agora.mkv"
# One palette for the whole clip; no dithering keeps text crisp and small.
ffmpeg -loglevel error -y -i "$rec/agora.mkv" -vf \
  "fps=10,split[a][b];[a]palettegen=max_colors=64:stats_mode=full[p];[b][p]paletteuse=dither=none" \
  -loop 0 demo/agora.gif
