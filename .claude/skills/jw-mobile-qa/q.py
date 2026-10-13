#!/usr/bin/env python3
"""adb helpers for the jw mobile QA. Needs ANDROID_SERIAL. Prints one line per call.

  q.py shot OUT.png            screenshot
  q.py dump OUT.xml            uiautomator dump (pulled)
  q.py texts                   every visible text / content-desc, one per line, with bounds
  q.py tap "TEXT"              tap the centre of the first node whose text or desc contains TEXT
  q.py wait "TEXT" [SECS]      poll until TEXT is visible; prints the ms it took (exit 1 on timeout)
  q.py gone "TEXT" [SECS]      poll until TEXT is no longer visible
  q.py type "TEXT"             adb input text (spaces as %s)
  q.py key NAME_OR_CODE        adb input keyevent
  q.py swipe X1 Y1 X2 Y2 [MS]
  q.py guard                   exit 1 unless the screen is the QA session's (see QA_SESSION, QA_ALLOW)

Real-daemon mode: set QA_SESSION (session name), QA_ALLOW (regex of the workspace names that may be open) and QA_PATH (folder prefix of QA workspaces).
With QA_SESSION set, tap/type/key/swipe call guard first and refuse to act anywhere else.
"""
import os, subprocess, sys, tempfile, time, xml.etree.ElementTree as ET

SER = os.environ["ANDROID_SERIAL"]
ADB = ["adb", "-s", SER]


def adb(*a, out=False):
    r = subprocess.run(ADB + list(a), capture_output=True, check=True)
    return r.stdout if out else None


def nodes():
    p = os.path.join(tempfile.gettempdir(), f"q-{SER}.xml")
    adb("shell", "uiautomator", "dump", "/sdcard/q.xml")
    adb("pull", "/sdcard/q.xml", p)
    for n in ET.parse(p).iter("node"):
        t = n.get("text") or n.get("content-desc") or ""
        b = n.get("bounds") or "[0,0][0,0]"
        x1, y1, x2, y2 = [int(v) for v in b.replace("][", ",").strip("[]").split(",")]
        yield t, (x1, y1, x2, y2), n.get("clickable") == "true"


def find(text):
    """Substring match; a leading "=" asks for the whole text ("=esc" is the key, not "escribe a claude")."""
    exact = text.startswith("=")
    for t, b, c in nodes():
        if (t == text[1:]) if exact else (text in t):
            return b
    return None


def guard():
    """Home screen: the session chip must name QA_SESSION. Workspace screen: its title must match QA_ALLOW."""
    import re
    sess, allow = os.environ.get("QA_SESSION"), os.environ.get("QA_ALLOW", "")
    if not sess:
        return
    ts = [t for t, _, _ in nodes() if t]
    if "sesión" in ts:  # home: the chip is the node after "sesión"
        i = ts.index("sesión")
        if i + 1 < len(ts) and sess in ts[i + 1]:
            return
        sys.exit(f"GUARD: home shows session {ts[i + 1:i + 2]}, not {sess}")
    path = os.environ.get("QA_PATH")
    if path and any(path in t for t in ts):
        return  # a workspace screen shows its folder: it is a QA one when the folder is under QA_PATH
    title = next((t for t in ts if re.search(r"\S ›\s*\S", t)), "")
    if title and re.search(allow, title.split("›")[-1].strip()):
        return
    neutral = ("cambios", "ajustes", "avisos", "archivos", "abrir", "sesiones", "nuevo worktree", "nueva sesión")
    if any(any(x in t for t in ts) for x in neutral) and not title:
        return  # Settings, Avisos, diff, Files and the sheets of the session act on nothing outside it
    sys.exit(f"GUARD: screen {ts[:3]} is not {sess} / {allow}")


def main(a):
    c = a[0]
    if c in ("tap", "type", "key", "swipe") and os.environ.get("QA_SESSION"):
        guard()
    if c == "shot":
        open(a[1], "wb").write(adb("exec-out", "screencap", "-p", out=True))
    elif c == "dump":
        nodes_ = list(nodes())
        os.replace(os.path.join(tempfile.gettempdir(), f"q-{SER}.xml"), a[1])
        print(len(nodes_), "nodes")
    elif c == "texts":
        for t, b, _ in nodes():
            if t:
                print(b, t.replace("\n", "⏎"))
    elif c == "guard":
        guard()
        print("ok")
    elif c == "tap":
        b = find(a[1])
        if not b:
            sys.exit(f"not found: {a[1]}")
        x, y = (b[0] + b[2]) // 2, (b[1] + b[3]) // 2
        adb("shell", "input", "tap", str(x), str(y))
        print("tap", x, y)
    elif c in ("wait", "gone"):
        t0, lim = time.time(), float(a[2]) if len(a) > 2 else 10
        while time.time() - t0 < lim:
            if (find(a[1]) is not None) == (c == "wait"):
                print(int((time.time() - t0) * 1000), "ms")
                return
        sys.exit(f"timeout {lim}s: {c} {a[1]}")
    elif c == "type":
        adb("shell", "input", "text", a[1].replace(" ", "%s"))
    elif c == "key":
        adb("shell", "input", "keyevent", a[1])
    elif c == "swipe":
        adb("shell", "input", "swipe", *a[1:5], *(a[5:6] or ["300"]))
    else:
        sys.exit(__doc__)


main(sys.argv[1:] or ["?"])
