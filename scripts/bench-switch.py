#!/usr/bin/env python3
"""Times jw's workspace switch (addendum 14, F0): where the milliseconds go.

    cargo build --release -p jw
    scripts/bench-switch.py [N]             N switches (default 30), release build
    JWBIN=target/debug/jw scripts/bench-switch.py 10

Starts an isolated jw (state under $JWB, default /tmp/jwb, 60x240 pty) with
JW_TRACE set, opens three git folders a, b, c, fills each shell pane with ~2.6 MB
of output (the daemon's 2 MiB ring is then full), then jumps `^␣ 1`, `2`, `3`
round and round, once every 1.2 s. It reads the trace the TUI wrote and prints
p50 / p95 / max per phase, in ms. Kills only its own daemon (the pid in its dir),
never `pkill jw`.
"""
import fcntl
import os
import pty
import select
import shutil
import statistics
import struct
import subprocess
import sys
import termios
import time

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
JW = os.path.join(REPO, os.environ.get("JWBIN", "target/release/jw"))
S = os.environ.get("JWB", "/tmp/jwb")
ROWS, COLS = 60, 240
N = int(sys.argv[1]) if len(sys.argv) > 1 else 30
TRACE = S + "/t.csv"
HOME = S + "/home"

shutil.rmtree(S, ignore_errors=True)
os.makedirs(HOME)
for r in "abc":
    d = f"{HOME}/{r}"
    os.makedirs(d)
    git = ["git", "-C", d, "-c", "user.name=t", "-c", "user.email=t@t"]
    subprocess.run(git + ["init", "-q"], check=True)
    subprocess.run(git + ["commit", "-q", "--allow-empty", "-m", "init"], check=True)

env = dict(os.environ, HOME=HOME, XDG_RUNTIME_DIR=S + "/run", XDG_STATE_HOME=S + "/state",
           XDG_CONFIG_HOME=S + "/cfg", TERM="xterm-256color", SHELL="/bin/sh",
           JW_THEME="dark", JW_WELCOME="0", JW_TRACE=TRACE, COLORTERM="truecolor")
env.pop("JW_SOCKET", None)
env.pop("JW_SESSION", None)

pid, fd = pty.fork()
if pid == 0:
    os.chdir(HOME)
    os.execve(JW, ["jw"], env)
fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
out_bytes = 0


def pump(t):
    global out_bytes
    end = time.time() + t
    while time.time() < end:
        r, _, _ = select.select([fd], [], [], 0.02)
        if r:
            try:
                out_bytes += len(os.read(fd, 1 << 20))
            except OSError:
                return


leader_bytes = []


def lead(k):
    global out_bytes
    before = out_bytes
    os.write(fd, b"\x00")
    pump(0.05)
    leader_bytes.append(out_bytes - before)
    os.write(fd, k.encode())


pump(1.5)
for r in "abc":
    lead("o")
    pump(0.6)
    os.write(fd, b"~")
    pump(0.4)
    os.write(fd, r.encode())
    pump(0.4)
    os.write(fd, b"\r")
    pump(1.5)
    os.write(fd, b"seq 1 400000\r")
    pump(3.0)

per_switch_bytes = []
for i in range(N):
    before = out_bytes
    lead(str("123"[i % 3]))
    pump(1.2)
    per_switch_bytes.append(out_bytes - before)

os.write(fd, b"\x00")
os.write(fd, b"q")
pump(0.5)
try:
    os.kill(int(open(S + "/run/jw/jw.pid").read()), 15)
except (OSError, ValueError):
    pass

# --- analysis ------------------------------------------------------------
ev = []
for line in open(TRACE):
    us, name, v = line.strip().split(",")
    ev.append((int(us) / 1000.0, name, int(v)))
starts = [i for i, e in enumerate(ev) if e[1] == "switch_start"]
segs = []
for a, b in zip(starts, starts[1:] + [len(ev)]):
    segs.append(ev[a:b])
opens, switches = segs[:3], segs[3:]


def first(seg, name):
    return next((e for e in seg if e[1] == name), None)


def analyse(seg):
    t0 = seg[0][0]
    m = {}
    for name in ("save_recent", "folders_edit", "resolve", "start_panes", "forget"):
        e = first(seg, name)
        m[name] = e[2] / 1000.0 if e else 0.0
    m["send_max"] = max([e[2] for e in seg if e[1] == "send"] or [0]) / 1000.0
    m["snap_parse_sum"] = sum(e[2] for e in seg if e[1] == "snap_parse") / 1000.0
    m["snap_parse_max"] = max([e[2] for e in seg if e[1] == "snap_parse"] or [0]) / 1000.0
    m["fit_sum"] = sum(e[2] for e in seg if e[1] == "fit") / 1000.0
    m["set_tree"] = sum(e[2] for e in seg if e[1] == "set_tree") / 1000.0
    m["snap_kib"] = sum(e[2] for e in seg if e[1] == "rx_snapshot") / 1024.0
    e = first(seg, "rx_tree")
    m["key>rx_tree"] = e[0] - t0 if e else float("nan")
    e = first(seg, "rx_snapshot")
    m["key>rx_snap1"] = e[0] - t0 if e else float("nan")
    snaps = [e[0] for e in seg if e[1] == "rx_snapshot"]
    m["key>rx_snapN"] = snaps[-1] - t0 if snaps else float("nan")
    inst = first(seg, "snap_installed")
    m["key>content"] = float("nan")
    if inst:
        d = next((e for e in seg if e[1] == "draw_total" and e[0] >= inst[0]), None)
        if d:
            m["key>content"] = d[0] - t0
    win = [e for e in seg if e[1] == "draw_total" and e[0] - t0 < 1000]
    m["frames"] = len(win)
    m["draw_total_max"] = max([e[2] for e in win] or [0]) / 1000.0
    m["draw_build_max"] = max([e[2] for e in seg if e[1] == "draw_build" and e[0] - t0 < 1000] or [0]) / 1000.0
    m["frames>16ms"] = sum(1 for e in win if e[2] > 16000)
    # The slide lasts 240 ms; after it the loop idles on purpose, so only the
    # first 300 ms count for stalls (the first gap runs from the key).
    ends = [t0] + [e[0] for e in win if e[0] - t0 < 300]
    gaps = [y - x for x, y in zip(ends, ends[1:])]
    m["stall_max"] = max(gaps or [0])
    m["gaps>20ms"] = sum(1 for g in gaps if g > 20)
    # Steady cadence of the slide once the content is in (16.7 ms is 60 fps).
    after = [e[0] for e in win if inst and e[0] > inst[0] and e[0] - t0 < 300]
    steady = [y - x for x, y in zip(after, after[1:])]
    m["frame_period"] = statistics.median(steady) if steady else float("nan")
    m["handle_max"] = max([e[2] for e in seg if e[1].startswith("handle") and e[0] - t0 < 1000] or [0]) / 1000.0
    m["handle_term"] = (first(seg, "handle_term") or (0, 0, 0))[2] / 1000.0
    return m


def table(title, segs):
    rows = [analyse(s) for s in segs]
    print(f"\n{title} (n={len(rows)}); ms unless noted")
    print(f"{'phase':<16}{'p50':>9}{'p95':>9}{'max':>9}")
    for k in rows[0]:
        vals = sorted(r[k] for r in rows if r[k] == r[k])
        if not vals:
            continue
        p95 = vals[min(len(vals) - 1, int(len(vals) * 0.95))]
        print(f"{k:<16}{statistics.median(vals):>9.2f}{p95:>9.2f}{vals[-1]:>9.2f}")


print(f"jw: {os.path.relpath(JW, REPO)}  {ROWS}x{COLS}")
table("open a new workspace (first time: Open)", opens)
table("switch to an open workspace (Attach)", switches)
b = sorted(per_switch_bytes)
lb = sorted(leader_bytes[-N:])
print(f"\nterminal bytes for the leader's frame (^␣ alone): p50 {statistics.median(lb) / 1024:.1f} KiB, "
      f"max {lb[-1] / 1024:.1f} KiB")
print(f"\nterminal bytes per switch (1.2 s window): p50 {statistics.median(b) / 1024:.1f} KiB, "
      f"max {b[-1] / 1024:.1f} KiB")
