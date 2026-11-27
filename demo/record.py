#!/usr/bin/env python3
"""Drive agora in a real Kitty window on a virtual screen and record it.

usage: record.py <agora-binary> <fake-home> <out.mkv>

Kitty draws agora's images (the agent logos), so the recording shows exactly
what a Kitty user sees. Needs Linux with Xvfb, kitty, xdotool and ffmpeg; the
demo workflow installs them. The keystrokes below are the whole demo; edit
SCRIPT to change it.
"""
import json
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path

COLS, ROWS = 120, 30
DISPLAY = ":99"
FPS = 20

# (seconds to wait first, keys to send)
SCRIPT = [
    (0.4, ""),
    (0.4, "/"),        # search straight away
    *[(0.07, c) for c in "redis cache"],
    (1.6, ""),         # matches found in conversation text
    (0.5, "\r"),       # leave the search box
    (0.7, "j"),
    (0.7, "v"),        # read that conversation full-screen
    (1.4, "\x06"),     # ctrl-f: a page down
    (1.0, "\x06"),
    (1.2, "\x1b"),     # back to the list
    (0.8, "\x1b"),     # clear the search
    (0.8, "\t"),       # claude only
    (0.6, "\t"),       # codex only
    (0.6, "\t"),       # pi only
    (0.6, "\t"),       # back to all
    (0.7, "c"),        # channel: shop
    (0.6, "c"),        # channel: platform
    (0.6, "C"), (0.5, "C"),  # back to all
    (0.7, "l"),        # live only
    (1.0, "l"),
    (0.8, "?"),        # every key
    (2.2, "?"),        # any key closes it
    (0.8, "q"),
]

# Tokyo Night, as the GIF has always used: background, foreground, then the
# 16 ANSI colours.
BG, FG = "#1a1b26", "#c0caf5"
ANSI = ["#15161e", "#f7768e", "#9ece6a", "#e0af68", "#7aa2f7", "#bb9af7",
        "#7dcfff", "#a9b1d6", "#565f89", "#f7768e", "#9ece6a", "#e0af68",
        "#7aa2f7", "#bb9af7", "#7dcfff", "#c0caf5"]


def escaped(keys):
    """`keys` as kitty's send-text reads them: Python escapes."""
    return "".join(c if c.isalnum() or c in " /-" else f"\\x{ord(c):02x}" for c in keys)


def wait_for(what, check, timeout=30):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        if (got := check()) is not None:
            return got
        time.sleep(0.1)
    sys.exit(f"timed out waiting for {what}")


def main():
    binary = str(Path(sys.argv[1]).resolve())
    home, out = Path(sys.argv[2]).resolve(), Path(sys.argv[3]).resolve()
    shims = Path(__file__).resolve().parent / "bin"
    tmp = Path(tempfile.mkdtemp())
    sock = tmp / "kitty.sock"
    status = tmp / "status"
    go = tmp / "go"
    log = tmp / "kitty.log"

    xvfb = subprocess.Popen(
        ["Xvfb", DISPLAY, "-screen", "0", "2560x1440x24", "-nolisten", "tcp"],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    x = {"DISPLAY": DISPLAY, "PATH": "/usr/bin:/bin"}
    wait_for("Xvfb", lambda: True if subprocess.run(
        ["xdotool", "getdisplaygeometry"], env=x, capture_output=True).returncode == 0 else None)

    options = {
        "font_family": "JetBrains Mono",
        "font_size": "13",
        "initial_window_width": f"{COLS}c",
        "initial_window_height": f"{ROWS}c",
        "remember_window_size": "no",
        "window_padding_width": "10",
        "hide_window_decorations": "yes",
        "placement_strategy": "top-left",
        "cursor_blink_interval": "0",
        "allow_remote_control": "yes",
        "confirm_os_window_close": "0",
        "enable_audio_bell": "no",
        "background": BG,
        "foreground": FG,
        **{f"color{i}": c for i, c in enumerate(ANSI)},
    }
    env = {
        **x,
        "HOME": str(home),
        "PATH": f"{shims}:/usr/bin:/bin",
        "LANG": "C.UTF-8",
        "COLORTERM": "truecolor",
        # Kitty needs OpenGL; there is no GPU here.
        "LIBGL_ALWAYS_SOFTWARE": "1",
    }
    kitty = subprocess.Popen(
        ["kitty", "--config", "NONE", "--class", "agora-demo",
         "--listen-on", f"unix:{sock}", "--directory", str(home),
         *[a for k, v in options.items() for a in ("-o", f"{k}={v}")],
         # agora waits for `go`: it starts once the window has its full
         # size, so it never redraws (and drops its images) on a resize.
         "sh", "-c",
         f'while [ ! -e "{go}" ]; do sleep 0.05; done; "{binary}"; echo $? > "{status}"'],
        env=env, stdout=subprocess.DEVNULL, stderr=log.open("w"))

    def geometry():
        r = subprocess.run(["xdotool", "search", "--sync", "--class", "agora-demo",
                            "getwindowgeometry", "--shell"],
                           env=x, capture_output=True, text=True, timeout=30)
        g = dict(l.split("=", 1) for l in r.stdout.split() if "=" in l)
        return g if "WIDTH" in g else None

    def full_size():
        r = subprocess.run(["kitty", "@", "--to", f"unix:{sock}", "ls"],
                           env=x, capture_output=True, text=True)
        if r.returncode != 0:
            return None
        win = json.loads(r.stdout)[0]["tabs"][0]["windows"][0]
        return True if (win["columns"], win["lines"]) == (COLS, ROWS) else None

    # Kitty maps a small window first, then grows it to COLS x ROWS.
    wait_for("the kitty window to reach full size", full_size)
    time.sleep(0.3)
    g = wait_for("the kitty window", geometry)
    go.touch()
    time.sleep(1.0)  # agora's first frames, off camera
    # Even sizes, which the GIF encoder and players prefer.
    w, h = int(g["WIDTH"]) // 2 * 2, int(g["HEIGHT"]) // 2 * 2
    ffmpeg = subprocess.Popen(
        ["ffmpeg", "-loglevel", "error", "-y", "-f", "x11grab", "-draw_mouse", "0",
         "-framerate", str(FPS), "-video_size", f"{w}x{h}",
         "-i", f"{DISPLAY}+{g['X']},{g['Y']}",
         "-c:v", "libx264rgb", "-crf", "0", "-preset", "ultrafast", str(out)],
        env=x, stdin=subprocess.PIPE)

    for wait, keys in SCRIPT:
        time.sleep(wait)
        if kitty.poll() is not None:
            break
        if keys:
            subprocess.run(["kitty", "@", "--to", f"unix:{sock}", "send-text",
                            escaped(keys)], env=x, check=True,
                           stdout=subprocess.DEVNULL)
    try:
        kitty.wait(timeout=10)
    except subprocess.TimeoutExpired:
        kitty.kill()
    time.sleep(0.3)
    ffmpeg.communicate(b"q")
    xvfb.terminate()

    # Never keep a recording of a crash (or a failed start) over the good one.
    code = status.read_text().strip() if status.exists() else "missing"
    if code != "0":
        out.unlink(missing_ok=True)
        err = log.read_text(errors="replace")[-2000:]
        sys.exit(f"agora exited with status {code}\n{err}")


main()
