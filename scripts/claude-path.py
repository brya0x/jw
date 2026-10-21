#!/usr/bin/env python3
"""The Claude test path: jwn's conversation against the REAL `claude`, stage by stage.

    scripts/claude-path.py --list
    scripts/claude-path.py            # every stage that is checked (about 15 turns of haiku, a dollar or so)
    scripts/claude-path.py turn tool  # only these
    scripts/claude-path.py survey     # what each built-in command of the engine answers when it is sent as typed

Not part of `just ci`: it costs tokens, needs the person's own `claude` login, and what a model
says is not the same twice, so it checks structure (an ask of this kind, a tool item of this name,
the status back to idle), never wording. It runs in a private daemon (/tmp/jwn-path: own socket, own
state) on a throwaway git repo in /tmp, with the person's own `~/.claude` (the transcripts land in
`~/.claude/projects/-private-tmp-jwn-path-repo`). It stops only the daemon it started, by the pid it
recorded. Never pkill. Spec: docs/test-path.md, part 2.
"""
import json, os, shutil, signal, subprocess, sys, time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
JWN = os.environ.get("JWN", shutil.which("jwn") or os.path.join(ROOT, "target/release/jwn"))
DIR = "/tmp/jwn-path"
REPO = os.path.join(DIR, "repo")
ENV = dict(os.environ, JWN_STATE_DIR=DIR + "/state", JWN_SOCKET=DIR + "/s.sock")
for k in ("JWN_TCP", "JWN_TUI_DUMP", "JWN_PANE", "JWN_WORKSPACE", "JWN_SESSION"):
    ENV.pop(k, None)
MODEL = os.environ.get("PATH_MODEL", "haiku")
results = []  # (stage, check, status, detail)  status: ok | FAIL | seen
daemon = None


# ---- the daemon and its commands -------------------------------------------------------
def start_daemon():
    global daemon
    shutil.rmtree(DIR, ignore_errors=True)
    os.makedirs(REPO)
    for c in (["init", "-q"], ["config", "user.email", "t@t"], ["config", "user.name", "t"]):
        subprocess.run(["git", "-C", REPO, *c], check=True)
    open(os.path.join(REPO, "README.md"), "w").write("# path\n")
    subprocess.run(["git", "-C", REPO, "add", "."], check=True)
    subprocess.run(["git", "-C", REPO, "commit", "-q", "-m", "init"], check=True)
    daemon = subprocess.Popen([JWN, "daemon"], env=ENV, stdout=open(DIR + "/daemon.log", "w"), stderr=subprocess.STDOUT)
    open(DIR + "/daemon.pid", "w").write(str(daemon.pid))
    end = time.time() + 15
    while time.time() < end and not os.path.exists(ENV["JWN_SOCKET"]):
        time.sleep(0.1)
    assert os.path.exists(ENV["JWN_SOCKET"]), "the daemon did not start: " + open(DIR + "/daemon.log").read()


def stop_daemon():
    global daemon
    if daemon and daemon.poll() is None:
        daemon.send_signal(signal.SIGTERM)  # the pid this script started, no other
        try:
            daemon.wait(timeout=10)
        except subprocess.TimeoutExpired:
            daemon.kill()
    daemon = None


def cmd(obj):
    out = subprocess.run([JWN, "cmd", json.dumps(obj)], env=ENV, capture_output=True, text=True)
    try:
        r = json.loads(out.stdout)
    except ValueError:
        return {"ok": False, "error": {"message": out.stderr or out.stdout}}
    return r


def result(r):
    assert r.get("ok") or r.get("outcome") == "ok" or "result" in r, f"command failed: {r}"
    return r.get("result") or {}


def snap(node):
    """The node as the daemon holds it now: the first line of `jwn watch`."""
    p = subprocess.Popen([JWN, "watch", node], env=ENV, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
    try:
        line = p.stdout.readline()
    finally:
        p.kill()
        p.wait()
    return json.loads(line)["node"]


def wait(node, what, pred, timeout=120):
    end = time.time() + timeout
    n = None
    while time.time() < end:
        n = snap(node)
        if pred(n):
            return n
        time.sleep(0.4)
    raise AssertionError(f"timed out waiting for {what}: status={n and n.get('status')} ask={n and n.get('ask') and n['ask'].get('kind')}")


def items(n, t):
    return [i for i in n.get("items", []) if i.get("t") == t]


def idle(n):
    return n.get("status") == "idle"


class Conv:
    """A conversation in a pane of the throwaway repo."""

    def __init__(self, args=(), resume=None):
        if not hasattr(Conv, "ws"):
            Conv.ws = result(cmd({"cmd": "open_folder", "path": REPO}))["workspace"]
        pane = result(cmd({"cmd": "new_pane", "workspace": Conv.ws}))["pane"]
        a = {"cmd": "open_conversation", "pane": pane, "args": ["--model", MODEL, *args]}
        if resume:
            a["resume"] = resume
        r = result(cmd(a))
        self.pane, self.node, self.session = pane, r["node"], r["session"]

    def now(self):
        return snap(self.node)

    def send(self, text):
        return cmd({"cmd": "conversation_send", "node": self.node, "text": text})

    def turn(self, text, timeout=120):
        """Send and wait for the turn to end (a result in the items, status idle)."""
        before = len(items(self.now(), "result"))
        self.send(text)
        return wait(self.node, "the turn to end", lambda n: idle(n) and len(items(n, "result")) > before, timeout)

    def answer(self, ask, key, text=None):
        a = {"cmd": "conversation_answer", "node": self.node, "ask": ask["id"], "key": key}
        if text:
            a["text"] = text
        return cmd(a)

    def set(self, **kw):
        return cmd({"cmd": "conversation_set", "node": self.node, **kw})

    def close(self):
        cmd({"cmd": "conversation_close", "node": self.node})


# ---- the stages ------------------------------------------------------------------------
STAGES = {}


def stage(name, about):
    def deco(f):
        STAGES[name] = (f, about)
        return f
    return deco


def check(stage_name, what, ok, detail=""):
    results.append((stage_name, what, "ok" if ok else "FAIL", detail))
    print(("  ok    " if ok else "  FAIL  ") + what + (f"  [{detail}]" if detail and not ok else ""), flush=True)


def seen(stage_name, what, detail):
    results.append((stage_name, what, "seen", detail))
    print(f"  seen  {what}: {detail}", flush=True)


@stage("turn", "a message, the answer streams, the turn ends with its result (REQ-378, 380, 382)")
def s_turn():
    c = Conv()
    n = c.turn("responde solo con la palabra ok")
    check("turn", "status is idle again", idle(n))
    check("turn", "Claude's text is an item", any("ok" in i.get("text", "").lower() for i in items(n, "text")))
    check("turn", "the turn's result is an item", len(items(n, "result")) == 1)
    check("turn", "the person's message is an item", any("ok" in i.get("text", "") for i in items(n, "user")))
    check("turn", "the session id is the engine's", bool(c.session))
    check("turn", "the model is haiku", "haiku" in (n.get("model") or ""), n.get("model", ""))
    c.close()


@stage("tool", "a tool asks permission, an allow runs it, a deny says so (REQ-384 to 387)")
def s_tool():
    c = Conv()
    # A command that writes: `echo` alone is allowed without asking, by the engine's own rules.
    c.send("ejecuta con la herramienta Bash exactamente: printf hola-path > salida-path.txt. No hagas nada más.")
    n = wait(c.node, "a permission ask", lambda n: n.get("ask") is not None)
    ask = n["ask"]
    check("tool", "the ask is a permission for Bash", ask["kind"] == "permission" and ask["tool"] == "Bash", f"{ask['kind']} {ask['tool']}")
    check("tool", "the ask names the command", "salida-path" in (ask.get("command") or ""), ask.get("command", ""))
    check("tool", "the options include yes and no", {"yes", "no"} <= {o["key"] for o in ask["options"]}, str([o["key"] for o in ask["options"]]))
    c.answer(ask, "yes")
    n = wait(c.node, "the turn to end", idle)
    tools = [t for t in items(n, "tool") if t["name"] == "Bash"]
    check("tool", "the Bash tool ran and the file is written", bool(tools) and tools[-1]["state"] == "ok" and os.path.exists(os.path.join(REPO, "salida-path.txt")), str(tools[-1:]))
    check("tool", "no ask is left", n.get("ask") is None)
    c.send("ejecuta con la herramienta Bash exactamente: printf otra > otra-path.txt. No hagas nada más.")
    n = wait(c.node, "the second ask", lambda n: n.get("ask") is not None)
    c.answer(n["ask"], "no", "mejor no")
    n = wait(c.node, "the turn to end", idle)
    tools = [t for t in items(n, "tool") if t["name"] == "Bash"]
    check("tool", "a refused tool is denied and nothing is written", len(tools) >= 2 and tools[-1]["state"] in ("failed", "denied") and not os.path.exists(os.path.join(REPO, "otra-path.txt")), str(tools[-1:]))
    c.close()


@stage("modes", "the four modes change what is asked (REQ-389, 390, 418)")
def s_modes():
    c = Conv()
    for mode in ("acceptEdits", "plan", "manual"):
        c.set(mode=mode)
        n = wait(c.node, f"mode {mode}", lambda n, m=mode: n.get("mode") == m, 30)
        check("modes", f"the engine confirms {mode}", n["mode"] == mode)
    c.set(mode="acceptEdits")
    wait(c.node, "acceptEdits", lambda n: n.get("mode") == "acceptEdits", 30)
    path = os.path.join(REPO, "aceptado.txt")
    c.send("crea con la herramienta Write el archivo aceptado.txt con el texto hola. No hagas nada más.")
    n = wait(c.node, "the turn to end or an ask", lambda n: idle(n) or n.get("ask") is not None)
    check("modes", "acceptEdits writes with no ask", n.get("ask") is None and os.path.exists(path), str(n.get("ask") and n["ask"]["tool"]))
    if n.get("ask"):
        c.answer(n["ask"], "yes")
    c.close()


@stage("plan", "a plan is asked for, shown and answered (REQ-436)")
def s_plan():
    c = Conv()
    c.send("/plan escribe un plan de dos pasos para añadir una función suma a un archivo math.py, y no lo implementes")
    n = wait(c.node, "the plan ask", lambda n: n.get("ask") is not None and n["ask"]["kind"] == "plan", 150)
    ask = n["ask"]
    check("plan", "the ask is a plan with its text", ask["kind"] == "plan" and len(ask.get("plan", [])) > 0, f"{len(ask.get('plan', []))} lines")
    keys = [o["key"] for o in ask["options"]]
    check("plan", "the options offer to accept and to keep asking", len(keys) >= 2, str(keys))
    c.answer(ask, keys[-1], "mejor no por ahora")
    n = wait(c.node, "the turn to end", idle, 90)
    check("plan", "a refused plan ends the turn", idle(n))
    c.close()


@stage("question", "AskUserQuestion becomes an ask of questions (REQ-435)")
def s_question():
    c = Conv()
    c.send("usa la herramienta AskUserQuestion para preguntarme si prefiero rojo o azul. No hagas nada más.")
    n = wait(c.node, "the question", lambda n: n.get("ask") is not None, 90)
    ask = n["ask"]
    check("question", "the ask is a question with options", ask["kind"] == "question" and len(ask["options"]) >= 2, f"{ask['kind']} {len(ask['options'])}")
    c.answer(ask, ask["options"][0]["key"])
    n = wait(c.node, "the turn to end", idle, 90)
    check("question", "the answer reached Claude and the turn ended", idle(n))
    c.close()


@stage("interrupt", "a long turn is stopped and the engine goes on (REQ-383)")
def s_interrupt():
    c = Conv()
    c.send("cuenta del 1 al 400, un número por línea, sin decir nada más")
    wait(c.node, "streaming", lambda n: n.get("status") in ("streaming", "thinking"), 60)
    time.sleep(1.5)
    cmd({"cmd": "conversation_interrupt", "node": c.node})
    n = wait(c.node, "idle after the interrupt", idle, 60)
    check("interrupt", "the turn stopped", idle(n))
    check("interrupt", "a note says it was interrupted", any("nterrump" in (i.get("text") or "") for i in items(n, "note")), str(items(n, "note")[-1:]))
    n = c.turn("responde solo con la palabra ok")
    check("interrupt", "the engine answers again after it", any("ok" in i.get("text", "").lower() for i in items(n, "text")))
    c.close()


@stage("local", "the commands jwn answers itself or the engine answers with no model call (REQ-418 to 421)")
def s_local():
    c = Conv()
    c.turn("responde solo con la palabra ok")
    c.send("/rename camino")
    n = wait(c.node, "the new name", lambda n: n.get("name") == "camino", 30)
    check("local", "/rename changes the name", n["name"] == "camino")
    c.send("/color red")
    n = wait(c.node, "the colour", lambda n: n.get("accent") == "red", 30)
    check("local", "/color changes the colour", n["accent"] == "red")
    c.send("/effort low")
    n = wait(c.node, "the effort", lambda n: n.get("effort") == "low", 30)
    check("local", "/effort changes the effort", n["effort"] == "low")
    c.send("/mode plan")
    n = wait(c.node, "the mode", lambda n: n.get("mode") == "plan", 30)
    check("local", "/mode changes the mode", n["mode"] == "plan")
    c.send("/cost")
    n = wait(c.node, "the cost", lambda n: any(i.get("t") == "out" and any("$" in l for l in i.get("lines", [])) for i in n["items"]), 30)
    check("local", "/cost says what it cost", True)
    before = len(c.now()["items"])
    c.send("/context")
    n = wait(c.node, "the context", lambda n: idle(n) and len(n["items"]) > before, 60)
    check("local", "/context answers with no model call", idle(n), str(n["items"][-1])[:120])
    c.send("/compact")
    n = wait(c.node, "the compaction", lambda n: idle(n) and any(i.get("t") == "divider" and i.get("kind") == "compact" for i in n["items"]), 150)
    check("local", "/compact leaves a divider", True)
    c.send("/clear")
    n = wait(c.node, "the clear", lambda n: idle(n) and len([i for i in n["items"] if i.get("t") in ("text", "user")]) == 0, 60)
    check("local", "/clear empties the conversation", True)
    c.close()


@stage("tasks", "a background shell is a task and can be stopped (REQ-428, 432)")
def s_tasks():
    c = Conv(["--permission-mode", "acceptEdits"])
    c.send("ejecuta con la herramienta Bash en segundo plano (run_in_background) el comando: sleep 60. Luego responde solo 'listo'.")
    n = wait(c.node, "a task", lambda n: len(n.get("tasks", [])) > 0, 120)
    task = n["tasks"][0]
    check("tasks", "the task is listed as running", task.get("state") == "run", str(task))
    r = cmd({"cmd": "conversation_stop_task", "node": c.node, "task": task["id"]})
    n = wait(c.node, "the task to stop", lambda n: all(t.get("state") != "run" for t in n.get("tasks", [])), 60)
    check("tasks", "stopping it ends it", True)
    n = wait(c.node, "the turn to end", idle, 120)
    check("tasks", "the conversation is idle", idle(n))
    c.close()


@stage("edit", "an edit asks with its diff and the tool keeps the diff (REQ-379, 384, 556)")
def s_edit():
    c = Conv()
    open(os.path.join(REPO, "edita.txt"), "w").write("uno\ndos\ntres\n")
    c.send("usa la herramienta Edit en edita.txt para cambiar la línea 'dos' por 'DOS'. No hagas nada más.")
    n = wait(c.node, "the ask", lambda n: n.get("ask") is not None or idle(n), 120)
    if n.get("ask") and n["ask"]["tool"] == "Read":  # Claude reads the file first
        c.answer(n["ask"], "yes")
        n = wait(c.node, "the edit ask", lambda n: n.get("ask") is not None, 120)
    ask = n.get("ask")
    check("edit", "the ask is for Edit and carries a diff", bool(ask) and ask["tool"] == "Edit" and len(ask.get("diff", [])) > 0, str(ask and (ask["tool"], len(ask.get("diff", [])))))
    if ask:
        c.answer(ask, "yes")
    n = wait(c.node, "the turn to end", idle, 120)
    tool = [t for t in items(n, "tool") if t["name"] == "Edit"]
    check("edit", "the tool item has the diff", bool(tool) and len(tool[-1].get("diff", [])) > 0, str(tool[-1:])[:200])
    check("edit", "the file changed", "DOS" in open(os.path.join(REPO, "edita.txt")).read())
    c.close()


@stage("agent", "a subagent (Task) is a card and its work is counted (REQ-429, 430)")
def s_agent():
    c = Conv(["--permission-mode", "acceptEdits"])
    c.send("usa la herramienta Task con un subagente general-purpose que responda solo 'hola'. Luego dime lo que respondió.")
    n = wait(c.node, "the turn to end", lambda n: idle(n) and len(items(n, "result")) > 0, 240)
    agents = items(n, "agent")
    check("agent", "an agent card is an item", bool(agents), str([i.get("t") for i in n["items"]]))
    check("agent", "the turn ended with Claude's answer", any(i.get("text") for i in items(n, "text")[-1:]))
    c.close()


@stage("queue", "a message sent during a turn waits and goes after it (REQ-382)")
def s_queue():
    c = Conv()
    c.send("cuenta del 1 al 150, un número por línea, sin decir nada más")
    wait(c.node, "a turn", lambda n: n.get("status") in ("streaming", "thinking"), 60)
    c.send("ahora responde solo con la palabra segundo")
    n = c.now()
    check("queue", "the second message waits in the queue", len(n.get("queue", [])) == 1, str(n.get("queue")))
    n = wait(c.node, "both turns", lambda n: idle(n) and len(items(n, "result")) >= 2 and not n.get("queue"), 240)
    check("queue", "the queued message was answered", any("segundo" in (i.get("text") or "").lower() for i in items(n, "text")[-1:]))
    c.close()


@stage("shell", "a `!` line runs in the folder (REQ-394)")
def s_shell():
    c = Conv()
    r = cmd({"cmd": "conversation_shell", "node": c.node, "command": "echo desde-shell"})
    n = wait(c.node, "the shell item to end", lambda n: any(i.get("t") == "shell" and i.get("state") != "run" for i in n["items"]), 30)
    sh = items(n, "shell")[-1]
    check("shell", "its output is in the item", "desde-shell" in " ".join(sh.get("output", [])), str(sh))
    c.close()


@stage("resume", "a conversation comes back after the daemon restarts (REQ-391, 540)")
def s_resume():
    global daemon
    c = Conv()
    c.turn("recuerda la palabra clave: manzana. Responde solo ok")
    session = c.session
    stop_daemon()
    start_keep_state()
    d = Conv(resume=session)
    n = wait(d.node, "the old items", lambda n: len(n.get("items", [])) > 0, 60)
    check("resume", "the old messages are there", any("manzana" in (i.get("text") or "") for i in n["items"]))
    n = d.turn("¿cuál era la palabra clave? Responde solo la palabra")
    check("resume", "Claude remembers (the engine resumed the session)", any("manzana" in (i.get("text") or "").lower() for i in items(n, "text")[-1:]))
    d.close()


def start_keep_state():
    global daemon
    daemon = subprocess.Popen([JWN, "daemon"], env=ENV, stdout=open(DIR + "/daemon.log", "a"), stderr=subprocess.STDOUT)
    open(DIR + "/daemon.pid", "w").write(str(daemon.pid))
    end = time.time() + 15
    while time.time() < end and not os.path.exists(ENV["JWN_SOCKET"]):
        time.sleep(0.1)
    time.sleep(0.5)


# What a built-in command of the engine answers when it is sent as typed. Skipped: the ones that
# send or spend something outside the repo.
SKIP = {"__remote-workflow", "workflow-launch-exec", "heapdump", "design-consent", "design-revoke", "team-onboarding",
        "extra-usage", "usage-credits", "import", "import-memory", "skill-creator", "docs", "docx", "pdf", "pptx", "xlsx",
        "google-workspace", "morning", "reload-plugins", "reload-skills", "init", "security-review", "commit",
        "vercel:deploy", "vercel:env", "vercel:status", "goal", "list-agents", "agents", "mcp", "config", "advisor",
        "autocompact", "clear", "compact", "context", "fast", "focus", "output-style", "recap", "usage", "skill-doctor", "auto-mode-setup"}


@stage("survey", "what each command that jwn sends as typed answers from the real engine (not a pass or fail)")
def s_survey():
    c = Conv()
    c.turn("responde solo con la palabra ok")
    names = ["advisor", "agents", "autocompact", "config", "context", "fast", "focus", "goal", "mcp", "output-style", "recap", "reload-plugins", "reload-skills", "usage", "skill-doctor", "list-agents", "auto-mode-setup"]
    for name in names:
        before = len(c.now()["items"])
        c.send(f"/{name}")
        t0 = time.time()
        try:
            n = wait(c.node, name, lambda n: idle(n) and len(n["items"]) > before, 40)
            shown = [i for i in n["items"][before:] if i.get("t") in ("text", "note", "out")]
            words = lambda i: i.get("text") or " ".join(i.get("lines", []))
            tail = words(shown[-1]).replace("\n", " ")[:110] if shown else "NOTHING is drawn (only the turn's result)"
            seen("survey", f"/{name}", f"{time.time() - t0:.0f} s · {tail}")
        except AssertionError as e:
            seen("survey", f"/{name}", f"no answer in 40 s ({e})")
            cmd({"cmd": "conversation_interrupt", "node": c.node})
            time.sleep(2)
    c.close()


# ---- main -------------------------------------------------------------------------------
def main(argv):
    if "--list" in argv:
        for k, (_, about) in STAGES.items():
            print(f"{k:10} {about}")
        return 0
    names = [a for a in argv if not a.startswith("-")] or [k for k in STAGES if k != "survey"]
    unknown = [n for n in names if n not in STAGES]
    if unknown:
        print("no such stage:", unknown, "(--list)")
        return 2
    start_daemon()
    try:
        for name in names:
            print(f"\n[{name}] {STAGES[name][1]}", flush=True)
            try:
                STAGES[name][0]()
            except AssertionError as e:
                check(name, "the stage ran to its end", False, str(e)[:300])
            except Exception as e:  # a crash is a failure of the stage, not of the others
                check(name, "the stage ran to its end", False, f"{type(e).__name__}: {e}"[:300])
    finally:
        stop_daemon()
    ok = sum(1 for r in results if r[2] == "ok")
    bad = [r for r in results if r[2] == "FAIL"]
    print(f"\n{ok} ok, {len(bad)} failed, {sum(1 for r in results if r[2] == 'seen')} seen")
    json.dump(results, open(DIR + "/results.json", "w"), indent=1, ensure_ascii=False)
    for r in bad:
        print(f"  FAIL [{r[0]}] {r[1]}  {r[3]}")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
