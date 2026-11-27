#!/usr/bin/env python3
"""Drive agora in a real Kitty window on a virtual screen and record it.

usage: record.py <agora-binary> <fake-home> <out.mkv>

Kitty draws agora's images (the agent logos), so the recording shows exactly
what a Kitty user sees. Needs Linux with Xvfb, kitty, xdotool and ffmpeg; the
demo workflow installs them. The keystrokes below are the whole demo; edit
SCRIPT to change it.
"""
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
    (2.5, ""),         # let the first live snapshot land
    (0.6, "j"), (0.6, "j"), (0.6, "j"), (0.6, "j"),
    (0.8, "k"), (0.6, "k"), (0.6, "k"), (0.6, "k"),
    (1.2, "\t"),       # claude only
    (1.2, "\t"),       # codex only
    (1.2, "\t"),       # pi only
    (1.2, "\t"),       # back to all
    (1.2, "c"),        # channel: shop
    (1.2, "c"),        # channel: platform
    (1.2, "c"),        # channel: personal
    (1.0, "C"), (1.0, "C"), (1.0, "C"),  # back to all
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
         "sh", "-c", f'"{binary}"; echo $? > "{status}"'],
        env=env, stdout=subprocess.DEVNULL, stderr=log.open("w"))

    def geometry():
        r = subprocess.run(["xdotool", "search", "--sync", "--class", "agora-demo",
                            "getwindowgeometry", "--shell"],
                           env=x, capture_output=True, text=True, timeout=30)
        g = dict(l.split("=", 1) for l in r.stdout.split() if "=" in l)
        return g if "WIDTH" in g else None

    # Kitty maps a small window first, then grows it to COLS x ROWS: wait
    # until two readings half a second apart agree.
    last = {}

    def settled():
        nonlocal last
        g = geometry()
        time.sleep(0.5)
        same = g is not None and g == last
        last = g or {}
        return g if same else None

    g = wait_for("the kitty window to settle", settled)
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
