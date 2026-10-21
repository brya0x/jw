#!/usr/bin/env python3
"""Do the daemon, the terminal client and the phone agree? Drives all three.

    cargo build --release -p jwn && (cd android && ./gradlew :app:assembleDebug)
    scripts/e2e-live.py            # a phone on a cable, USB debugging on, screen unlocked

It starts its own daemon in /tmp/jwn-e2e (own socket, own state, loopback port 7871), runs
the real `jwn tui` in a pty with a keyboard, and reads the phone through `uiautomator`. It
touches only the app dev.jw.next (reinstalls it so its data starts empty) and stops only the daemon it
started, by the pid it recorded. Never pkill.
"""
import fcntl, json, os, pty, re, shutil, signal, struct, subprocess, sys, termios, threading, time
import xml.etree.ElementTree as ET

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
JWN = os.path.join(ROOT, "target/release/jwn")
APK = os.path.join(ROOT, "android/app/build/outputs/apk/debug/app-debug.apk")
ADB = os.path.expanduser("~/Library/Android/sdk/platform-tools/adb")
PKG, PORT, DIR = "dev.jw.next", 7871, "/tmp/jwn-e2e"
ENV = dict(os.environ, JWN_STATE_DIR=DIR + "/state", JWN_SOCKET=DIR + "/s.sock", JWN_TCP=f"127.0.0.1:{PORT}",
           JWN_TUI_DUMP=DIR + "/tui.txt", SHELL="/bin/sh", TERM="xterm-256color")
results = []

def adb(*a, check=True):
    return subprocess.run([ADB, *a], capture_output=True, text=True, check=check).stdout

def cmd(obj):
    out = subprocess.run([JWN, "cmd", json.dumps(obj)], env=ENV, capture_output=True, text=True)
    return json.loads(out.stdout) if out.stdout.strip() else {"ok": False, "error": out.stderr}

def write(pane, data):
    cmd({"cmd": "write", "pane": pane, "data": data + "\r"})

def mark(code):  # what a shell integration prints: command done with an exit code
    return "printf '\\033]133;D;%d\\007'" % code

def started():  # a command starts: the mark that ends a shell_done
    return "printf '\\033]133;C\\007'"

# ---- the phone, read through the accessibility tree ----
def phone_nodes():
    adb("shell", "uiautomator", "dump", "/sdcard/jwn-e2e.xml")
    xml = adb("exec-out", "cat", "/sdcard/jwn-e2e.xml")
    nodes = []
    for n in ET.fromstring(xml[xml.index("<"):]).iter("node"):
        t = n.get("text") or ""
        if t:
            x1, y1, x2, y2 = map(int, re.findall(r"\d+", n.get("bounds")))
            nodes.append((t, ((x1 + x2) // 2, (y1 + y2) // 2)))
    return nodes

def phone_edit():
    """The centre of the phone's input field, if one is on screen."""
    adb("shell", "uiautomator", "dump", "/sdcard/jwn-e2e.xml")
    xml = adb("exec-out", "cat", "/sdcard/jwn-e2e.xml")
    for n in ET.fromstring(xml[xml.index("<"):]).iter("node"):
        if n.get("class") == "android.widget.EditText":
            x1, y1, x2, y2 = map(int, re.findall(r"\d+", n.get("bounds")))
            return ((x1 + x2) // 2, (y1 + y2) // 2)
    return None

def phone_text():
    return "\n".join(t for t, _ in phone_nodes())

def phone_tap(label, nth=0, timeout=10):
    """Tap the nth node with this text, waiting for it to appear (the dump can lag a frame)."""
    end = time.time() + timeout
    while time.time() < end:
        hits = [c for t, c in phone_nodes() if t == label]
        if len(hits) > nth:
            adb("shell", "input", "tap", str(hits[nth][0]), str(hits[nth][1]))
            return
        time.sleep(0.5)
    raise AssertionError(f"nothing on the phone says {label!r}: {phone_text()!r}")

# ---- the terminal client, in a pty with a keyboard ----
class Tui:
    def __init__(self):
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.execve(JWN, [JWN, "tui"], ENV)
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", 32, 120, 0, 0))
        # Read what the client draws all the time: a full pty buffer would stall it.
        threading.Thread(target=self.drain, daemon=True).start()

    def keys(self, s):
        os.write(self.fd, s.encode())
        time.sleep(0.3)

    def nav(self, keys):  # keys of the client itself: the leader (Ctrl-Space) first
        self.keys("\x00" + keys)

    def click(self, x, y):  # cells are 0-based here; the terminal's mouse protocol counts from 1
        self.keys(f"\x1b[<0;{x + 1};{y + 1}M\x1b[<0;{x + 1};{y + 1}m")

    def wheel(self, down, x, y):
        self.keys(f"\x1b[<{65 if down else 64};{x + 1};{y + 1}M")

    def drain(self):
        while True:
            try:
                if not os.read(self.fd, 65536):
                    return
            except OSError:
                return

    def text(self):
        try:
            return open(DIR + "/tui.txt").read()
        except FileNotFoundError:
            return ""

    def stop(self):
        try:
            os.kill(self.pid, signal.SIGTERM)
            os.waitpid(self.pid, 0)
        except (ProcessLookupError, ChildProcessError):
            pass

def until(fn, what, timeout=8):
    end = time.time() + timeout
    while time.time() < end:
        try:
            if fn():
                return True
        except Exception:
            pass
        time.sleep(0.4)
    return False

def check(name, ok, detail=""):
    results.append((name, ok))
    print(("PASS  " if ok else "FAIL  ") + name + ("" if ok else f"   {detail}"), flush=True)

def sidebar(t, n):  # the n-th sidebar row of the terminal client (1-based)
    rows = [l for l in t.split("\n")]
    return rows[1 + n][:27] if len(rows) > 1 + n else ""

def avisos_count_tui(t):
    m = re.search(r"avisos (\d+)", t.split("\n")[-1])
    return int(m.group(1)) if m else 0

def avisos_count_phone(txt):
    m = re.search(r"^avisos\s*(\d*)$", txt, re.M)
    if m is None:
        m = re.search(r"avisos\n(\d+)", txt)
    return int(m.group(1)) if m and m.group(1) else 0

def main():
    if not adb("devices").count("\tdevice"):
        sys.exit("no phone attached")
    shutil.rmtree(DIR, ignore_errors=True)
    os.makedirs(DIR + "/work")
    daemon = subprocess.Popen([JWN, "daemon"], env=ENV, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    tui = None
    try:
        assert until(lambda: os.path.exists(DIR + "/s.sock"), "daemon"), "the daemon did not start"
        for d in "abc":
            os.makedirs(f"{DIR}/work/{d}")
        cmd({"cmd": "rename_session", "name": "e2e"})
        ws = {d: cmd({"cmd": "open_folder", "path": f"{DIR}/work/{d}"})["result"]["workspace"] for d in "ab"}
        pa = cmd({"cmd": "new_pane", "workspace": ws["a"]})["result"]["pane"]
        pb1 = cmd({"cmd": "new_pane", "workspace": ws["b"]})["result"]["pane"]
        pb2 = cmd({"cmd": "new_pane", "workspace": ws["b"]})["result"]["pane"]

        adb("reverse", f"tcp:{PORT}", f"tcp:{PORT}")
        # A fresh install: the app's own data (its seen record) starts empty.
        adb("uninstall", PKG, check=False)
        adb("install", APK)
        adb("shell", "am", "force-stop", PKG)
        adb("shell", "am", "start", "-n", f"{PKG}/{PKG}.app.MainActivity", "-e", "live", f"127.0.0.1:{PORT}")
        tui = Tui()
        until(lambda: "e2e" in tui.text() and "e2e" in phone_text(), "both connected", 15)

        print("\n-- both draw the same workspaces")
        check("terminal lists a and b", "1 ● a" in tui.text() and "2 ● b" in tui.text(), tui.text()[:300])
        txt = phone_text()
        check("phone lists a and b with pane counts", "1 panel" in txt and "2 paneles" in txt, txt)

        print("\n-- a signal raised by the daemon reaches both")
        write(pa, mark(3))
        check("terminal marks a with a failure and counts one aviso",
              until(lambda: "✗" in sidebar(tui.text(), 1) and avisos_count_tui(tui.text()) == 1, "tui"), sidebar(tui.text(), 1))
        check("phone shows salió 3 and one aviso", until(lambda: "salió 3" in phone_text() and avisos_count_phone(phone_text()) == 1, "phone"), phone_text())
        write(pb2, mark(0))
        check("terminal marks b with a green prompt and counts two",
              until(lambda: "❯" in sidebar(tui.text(), 2) and avisos_count_tui(tui.text()) == 2, "tui"), sidebar(tui.text(), 2))
        check("phone shows both and counts two", until(lambda: "salió 0" in phone_text() and avisos_count_phone(phone_text()) == 2, "phone"), phone_text())

        print("\n-- clearing on one client clears it on the other")
        phone_tap("ir ›", 0)  # the phone goes to a's pane
        check("the phone opened the pane", until(lambda: "hello" in phone_text() or "sh-3" in phone_text() or "printf" in phone_text(), "nav"), phone_text())
        check("the phone now has one waiting", until(lambda: "avisos" not in phone_text() or avisos_count_phone(phone_text()) <= 1, "phone"), phone_text())
        check("the terminal dropped the same aviso", until(lambda: avisos_count_tui(tui.text()) == 1, "tui"), tui.text().split("\n")[-1])
        check("the terminal's row for a is clear", until(lambda: "✗" not in sidebar(tui.text(), 1), "tui"), sidebar(tui.text(), 1))
        adb("shell", "input", "keyevent", "KEYCODE_BACK")
        tui.nav("a")
        tui.nav("\r")  # the terminal goes to the remaining aviso (b)
        check("the terminal has nothing waiting now", until(lambda: avisos_count_tui(tui.text()) == 0, "tui"), tui.text().split("\n")[-1])
        check("the phone dropped it too", until(lambda: avisos_count_phone(phone_text()) == 0 and "salió 0" not in phone_text(), "phone"), phone_text())

        print("\n-- the daemon can also end an attention for everyone")
        write(pa, mark(7))
        check("a signal raised again shows on both", until(lambda: avisos_count_tui(tui.text()) == 1 and avisos_count_phone(phone_text()) == 1, "both"), phone_text())
        write(pa, started())
        check("the terminal has nothing waiting", until(lambda: avisos_count_tui(tui.text()) == 0, "tui"), tui.text().split("\n")[-1])
        check("the phone has nothing waiting", until(lambda: avisos_count_phone(phone_text()) == 0, "phone"), phone_text())

        print("\n-- a new signal is unseen again, even where it was seen before")
        write(pa, mark(5))
        check("terminal sees the new failure", until(lambda: avisos_count_tui(tui.text()) == 1, "tui"), tui.text().split("\n")[-1])
        check("phone sees the new failure", until(lambda: "salió 5" in phone_text(), "phone"), phone_text())
        # and the other way: the terminal looks, the phone drops it
        tui.nav("a")
        tui.nav("\r")
        check("the terminal looked at it", until(lambda: avisos_count_tui(tui.text()) == 0, "tui"), tui.text().split("\n")[-1])
        check("the phone dropped it because the terminal looked", until(lambda: avisos_count_phone(phone_text()) == 0, "phone"), phone_text())

        print("\n-- the mouse in the terminal client")
        write(pa, mark(9))
        check("a new failure is waiting on both", until(lambda: avisos_count_tui(tui.text()) == 1 and avisos_count_phone(phone_text()) == 1, "both"), tui.text().split("\n")[-1])
        tui.click(113, 31)  # "avisos 1" in the status bar
        check("clicking avisos in the status bar opens the list", until(lambda: "AVISOS" in tui.text().split("\n")[-1], "tui"), tui.text().split("\n")[-1])
        tui.click(52, 6)  # the first item of the list
        check("clicking an item goes there and clears it on the terminal", until(lambda: avisos_count_tui(tui.text()) == 0 and "TERM" in tui.text().split("\n")[-1], "tui"), tui.text().split("\n")[-1])
        check("and on the phone", until(lambda: avisos_count_phone(phone_text()) == 0, "phone"), phone_text())
        tui.click(5, 3)  # the second workspace row
        check("clicking a workspace row opens it", until(lambda: tui.text().split("\n")[0].split("│")[1].strip().startswith("b") and "TERM" in tui.text().split("\n")[-1], "tui"), tui.text().split("\n")[0])
        tui.wheel(False, 5, 3)  # the wheel over the sidebar moves the highlight up
        check("the wheel over the sidebar moves the highlight", until(lambda: tui.text().split("\n")[0].split("│")[1].strip().startswith("a") and "MOVE" in tui.text().split("\n")[-1], "tui"), tui.text().split("\n")[0])

        print("\n-- what a pane prints reaches both")
        write(pa, "echo hello-e2e")
        tui.nav("k")
        tui.nav("\r")  # open a in the terminal
        check("terminal draws the pane's output", until(lambda: "hello-e2e" in tui.text(), "tui"), tui.text()[:200])
        phone_tap("a", 0)  # the workspace row: nothing waits there any more, so no "ir ›"
        check("phone draws the pane's output", until(lambda: "hello-e2e" in phone_text(), "phone"), phone_text())
        adb("shell", "input", "keyevent", "KEYCODE_BACK")

        print("\n-- typing into a pane, from the terminal and from the phone")
        tui.nav("\r")  # open workspace a again: the keyboard goes to its shell
        tui.keys("echo from-tui\r")
        check("what the terminal types runs in the shell", until(lambda: "from-tui" in tui.text() and "echo from-tui" in tui.text(), "tui"), tui.text()[:300])
        phone_tap("a", 0)
        check("and the phone shows it", until(lambda: "from-tui" in phone_text(), "phone"), phone_text())
        edit = phone_edit()
        check("the phone has an input field", edit is not None)
        if edit:
            adb("shell", "input", "tap", str(edit[0]), str(edit[1]))
            adb("shell", "input", "text", "echo%sfrom-phone")
            adb("shell", "input", "keyevent", "KEYCODE_ENTER")
            check("what the phone types runs in the shell", until(lambda: "from-phone" in phone_text(), "phone"), phone_text())
            check("and the terminal shows it", until(lambda: "from-phone" in tui.text(), "tui"), tui.text()[:300])
        adb("shell", "input", "keyevent", "KEYCODE_BACK")
        adb("shell", "input", "keyevent", "KEYCODE_BACK")
        tui.keys("sleep 30\r")
        tui.keys("\x03")  # Ctrl-C reaches the program, not the client
        tui.keys("echo after-interrupt\r")
        check("Ctrl-C from the terminal stops the program and the shell answers", until(lambda: "after-interrupt" in tui.text(), "tui"), tui.text()[:300])
        tui.nav("")  # give the keyboard back to the client for what follows

        print("\n-- structure changes reach both")
        c = cmd({"cmd": "open_folder", "path": f"{DIR}/work/c"})["result"]["workspace"]
        check("terminal gets a third workspace", until(lambda: "3 ○ c" in tui.text(), "tui"), tui.text()[:200])
        check("phone gets a third workspace", until(lambda: "workspaces · 3" in phone_text(), "phone"), phone_text())
        cmd({"cmd": "remove_workspace", "workspace": c})
        check("both lose it again", until(lambda: "3 ○ c" not in tui.text() and "workspaces · 2" in phone_text(), "both"), phone_text())
        write(pb1, "exit")
        check("a shell that exits takes its pane away on both",
              until(lambda: "1 panel" in phone_text() and phone_text().count("1 panel") >= 2, "phone"), phone_text())

        print("\n-- the daemon restarts")
        subprocess.run([JWN, "stop"], env=ENV)
        daemon.wait(timeout=10)
        daemon = subprocess.Popen([JWN, "daemon"], env=ENV, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        check("phone reconnects by itself and shows the same workspaces",
              until(lambda: "workspaces · 2" in phone_text() and "conectado" in phone_text(), "phone", 20), phone_text())
        alive = tui.text() != "" and os.waitpid(tui.pid, os.WNOHANG) == (0, 0)
        check("terminal client survives the restart", alive and until(lambda: "e2e" in tui.text(), "tui", 6), "the terminal client ended")
    finally:
        if tui:
            tui.stop()
        try:
            subprocess.run([JWN, "stop"], env=ENV, capture_output=True)
        except Exception:
            pass
        adb("reverse", "--remove", f"tcp:{PORT}", check=False)
    bad = [n for n, ok in results if not ok]
    print(f"\n{len(results) - len(bad)}/{len(results)} passed")
    sys.exit(1 if bad else 0)

main()
