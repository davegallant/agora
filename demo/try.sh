#!/bin/sh
# Try the working tree's agora against made-up sessions, without touching your
# real ones. Seeds a throwaway $HOME (demo/seed.py) plus one long conversation
# for scrolling and match-jumping, then runs a debug build in it.
#
#   demo/try.sh          # then try: / redis, enter, n/N, J/K, ctrl-f/ctrl-b
#
# Set AGORA_BIN to skip the build. Needs cargo and python3.
set -eu
cd "$(dirname "$0")/.."

bin=${AGORA_BIN:-target/debug/agora}
[ -n "${AGORA_BIN:-}" ] || cargo build --quiet
home=$(mktemp -d)
trap 'rm -rf "$home"' EXIT

python3 demo/seed.py "$home"
python3 - "$home" <<'EOF'
import json, sys, time
from pathlib import Path

home = Path(sys.argv[1])
sid = "9a1b2c3d-4e5f-4a6b-8c7d-0e1f2a3b4c5d"
cwd = "/home/demo/src/webshop"
topics = ["session store", "Redis cache eviction", "connection pool sizing",
          "retry backoff", "cache stampede on deploy", "key naming scheme"]
rows = [{"type": "ai-title", "aiTitle": "Long Redis cache investigation"}]
for n in range(30):
    t = topics[n % len(topics)]
    rows.append({"type": "user", "cwd": cwd, "message": {"role": "user", "content":
        f"Step {n + 1}: what about the {t}? Check whether the redis client handles it."}})
    rows.append({"type": "assistant", "cwd": cwd, "message": {"role": "assistant", "content": [
        {"type": "text", "text":
            f"Looked into the {t}. The Redis client wraps every call in a 2 second timeout, "
            "which is fine for reads but too tight for the bulk writes the nightly job does.\n\n"
            "- raised the write timeout to 10 seconds\n"
            "- added a histogram of command latency so we can see it next time\n"
            "- left reads alone\n\n"
            "Nothing else in this path talks to the cache directly."}]}})
path = home / ".claude/projects/-home-demo-src-webshop" / f"{sid}.jsonl"
path.parent.mkdir(parents=True, exist_ok=True)
path.write_text("".join(json.dumps(r) + "\n" for r in rows))
EOF

HOME=$home PATH="$PWD/demo/bin:$PATH" "$bin" "$@"
