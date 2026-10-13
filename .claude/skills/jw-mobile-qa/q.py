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
    for t, b, c in nodes():
        if text in t:
            return b
    return None


def main(a):
    c = a[0]
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
