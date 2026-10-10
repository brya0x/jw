#!/usr/bin/env python3
"""Drives the jw TUI in a pseudo-terminal and prints its screen as text.

    cargo build --workspace --examples
    scripts/drive.py /tmp/jq1 lead:o 'type:~' type:demo key:enter wait:1.5 dump

<dir> isolates everything: HOME, XDG_* state, config and the socket live
under it, so the real jw state is never touched. Keep it short (a unix
socket path has ~100 characters). The daemon it starts keeps running:
kill it with `kill $(cat <dir>/run/jw/jw.pid)`, never `pkill jw`.

Steps:
  lead:<k>      Ctrl-Space, then <k> (`space` for ␣, or a key: name)
  type:<text>   types text
  key:<name>    enter esc tab up down left right bs
  wait:<s>      reads output for <s> seconds
  sh:<cmd>      runs a shell command (edit a settings file, …)
  clear         forgets the output so far
  dump          prints the screen
  colors        prints the first background colours seen (themes)

Env: JWARGS (arguments to jw), CWD (where it starts), JWT (JW_THEME; empty
leaves it unset).
"""
import fcntl
import os
import pty
import re
import select
import struct
import subprocess
import sys
import termios
import time

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
JW = os.path.join(REPO, "target/debug/jw")
SCREEN = os.path.join(REPO, "target/debug/examples/screen")
ROWS, COLS = 35, 120
KEYS = {"enter": b"\r", "esc": b"\x1b", "tab": b"\t", "down": b"\x1b[B", "up": b"\x1b[A",
        "right": b"\x1b[C", "left": b"\x1b[D", "bs": b"\x7f"}

S = sys.argv[1]
HOME = S + "/home"
os.makedirs(HOME, exist_ok=True)
env = dict(os.environ, HOME=HOME, XDG_RUNTIME_DIR=S + "/run", XDG_STATE_HOME=S + "/state",
           XDG_CONFIG_HOME=S + "/cfg", TERM="xterm-256color", SHELL="/bin/sh",
           JW_THEME=os.environ.get("JWT", "dark"))
if not env["JW_THEME"]:
    env.pop("JW_THEME")
env.pop("JW_SOCKET", None)
env.pop("JW_SESSION", None)

pid, fd = pty.fork()
if pid == 0:
    os.chdir(os.environ.get("CWD", HOME))
    os.execve(JW, ["jw"] + os.environ.get("JWARGS", "").split(), env)
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
buf = b""


def pump(t):
    global buf
    end = time.time() + t
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.05)
        if r:
            try:
                buf += os.read(fd, 65536)
            except OSError:
                return


def dump():
    out = subprocess.run([SCREEN, str(ROWS), str(COLS)], input=buf, capture_output=True).stdout
    print("\n".join(l.rstrip() for l in out.decode().split("\n")))
    print("-" * COLS)


pump(1.5)
for step in sys.argv[2:]:
    k, _, v = step.partition(":")
    if k == "lead":
        os.write(fd, b"\x00")
        time.sleep(0.05)
        os.write(fd, b" " if v == "space" else KEYS.get(v, v.encode()))
        pump(0.6)
    elif k == "type":
        os.write(fd, v.encode())
        pump(0.4)
    elif k == "key":
        os.write(fd, KEYS[v])
        pump(0.4)
    elif k == "wait":
        pump(float(v))
    elif k == "sh":
        subprocess.run(v, shell=True)
    elif k == "clear":
        buf = b""
    elif k == "dump":
        dump()
    elif k == "colors":
        print(sorted(set(re.findall(rb"48;2;(\d+);(\d+);(\d+)", buf)))[:6])
# Detach: the daemon and its panes keep running.
os.write(fd, b"\x00")
os.write(fd, b"q")
pump(0.5)
