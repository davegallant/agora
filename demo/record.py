#!/usr/bin/env python3
"""Drive agora in a pseudo-terminal and save the session as an asciinema cast.

usage: record.py <agora-binary> <fake-home> <out.cast>

The keystrokes below are the whole demo; edit SCRIPT to change it.
"""
import codecs
import fcntl
import json
import os
import pty
import select
import struct
import sys
import termios
import time
from pathlib import Path

COLS, ROWS = 120, 30

# (seconds to wait first, keys to send)
SCRIPT = [
    (2.5, ""),         # let the first live snapshot land
    (0.6, "j"), (0.6, "j"), (0.6, "j"), (0.6, "j"),
    (0.8, "k"), (0.6, "k"), (0.6, "k"), (0.6, "k"),
    (1.2, "\t"),       # claude only
    (1.2, "\t"),       # codex only
    (1.2, "\t"),       # pi only
    (1.2, "\t"),       # back to all
    (1.2, "l"),        # live only
    (1.8, "l"),
    (1.2, "/"),
    *[(0.18, c) for c in "redis cache"],
    (2.5, "\x1b[B"),   # matches found in conversation text
    (1.5, "\x1b[B"),
    (1.0, "\r"),       # leave the search box
    (1.2, "N"),        # jump between highlighted matches
    (1.2, "N"),
    (2.0, "\x1b"),     # clear search
    (1.5, "q"),
]


def main():
    # Absolute: the child changes into the fake home before exec.
    binary = str(Path(sys.argv[1]).resolve())
    home, out = Path(sys.argv[2]).resolve(), Path(sys.argv[3])
    shims = Path(__file__).resolve().parent / "bin"
    env = {
        "HOME": str(home),
        "PATH": f"{shims}:/usr/bin:/bin",
        "TERM": "xterm-256color",
        "COLORTERM": "truecolor",
        "LANG": "en_US.UTF-8",
    }

    pid, fd = pty.fork()
    if pid == 0:
        os.chdir(home)
        os.execve(binary, [binary], env)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))

    start = time.monotonic()
    events = []
    # Reads can split a multibyte character; decode incrementally.
    decoder = codecs.getincrementaldecoder("utf-8")("replace")

    def pump(seconds):
        end = time.monotonic() + seconds
        while (left := end - time.monotonic()) > 0:
            r, _, _ = select.select([fd], [], [], left)
            if not r:
                continue
            try:
                data = os.read(fd, 65536)
            except OSError:
                return False
            if not data:
                return False
            # Answer cursor-position queries so the TUI never stalls.
            if b"\x1b[6n" in data:
                os.write(fd, b"\x1b[1;1R")
            text = decoder.decode(data)
            if text:
                events.append([round(time.monotonic() - start, 4), "o", text])
        return True

    for wait, keys in SCRIPT:
        if not pump(wait):
            break
        if keys:
            os.write(fd, keys.encode())
    pump(1.0)
    try:
        _, status = os.waitpid(pid, 0)
    except ChildProcessError:
        status = 0
    # Never write a cast of a crash (or a failed exec) over the good one.
    if os.waitstatus_to_exitcode(status) != 0:
        tail = "".join(e[2] for e in events)[-2000:]
        sys.exit(f"agora exited with status {os.waitstatus_to_exitcode(status)}:\n{tail}")

    header = {"version": 2, "width": COLS, "height": ROWS,
              "timestamp": int(time.time()), "title": "agora",
              "env": {"TERM": env["TERM"], "SHELL": "/bin/sh"}}
    with out.open("w") as f:
        f.write(json.dumps(header) + "\n")
        for e in events:
            f.write(json.dumps(e) + "\n")


main()
