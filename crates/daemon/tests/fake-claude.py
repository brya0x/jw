#!/usr/bin/env python3
import json, os, sys, tempfile, threading, time
LOG = os.environ.get("FAKE_LOG", "/tmp/fake-engine.log")
argv = sys.argv[1:]
if "--version" in argv:
    # With an `installed` file beside it, the stand-in is that version of Claude Code.
    installed = os.path.join(os.path.dirname(os.path.abspath(__file__)), "installed")
    print(open(installed).read().strip() + " (Claude Code)" if os.path.exists(installed) else "fake-claude 1.0")
    sys.exit(0)
session = ""
for i, a in enumerate(argv):
    if a in ("--session-id", "--resume"):
        session = argv[i + 1]
def logline(s):
    with open(LOG, "a", encoding="utf-8") as f:
        f.write(s + "\n")
if os.environ.get("FAKE_PIDFILE"):
    with open(os.environ["FAKE_PIDFILE"], "w", encoding="utf-8") as f:
        f.write(str(os.getpid()))
logline("start " + json.dumps(argv) + " cwd=" + os.getcwd() + " var=" + os.environ.get("MY_VAR", ""))
# With FAKE_CLAUDE_DIR the engine keeps a transcript like Claude's: `<dir>/projects/<folder>/<id>.jsonl`, written
# at the first message; a reused `--session-id` and a `--resume` of nothing end with code 1, as they do there.
claude_dir = os.environ.get("FAKE_CLAUDE_DIR")
def on_disk(id):
    projects = os.path.join(claude_dir, "projects")
    return os.path.isdir(projects) and any(os.path.exists(os.path.join(projects, d, id + ".jsonl")) for d in os.listdir(projects))
if claude_dir:
    if "--session-id" in argv and on_disk(session):
        sys.stderr.write("Error: Session ID %s is already in use.\n" % session)
        sys.exit(1)
    if "--resume" in argv and not on_disk(session):
        sys.stderr.write("No conversation found with session ID: %s\n" % session)
        sys.exit(1)
def write_transcript(text):
    folder = os.path.join(claude_dir, "projects", os.getcwd().replace("/", "-").replace(".", "-"))
    os.makedirs(folder, exist_ok=True)
    with open(os.path.join(folder, session + ".jsonl"), "a", encoding="utf-8") as f:
        f.write(json.dumps({"type": "user", "message": {"role": "user", "content": text}}, ensure_ascii=False) + "\n")
lock = threading.Lock()
mode = ["default"]
reloaded = [None]
outputs = {}
total = [0.0]
def out(o):
    with lock:
        sys.stdout.write(json.dumps(o) + "\n")
        sys.stdout.flush()
def init():
    out({"type": "system", "subtype": "init", "session_id": session, "model": "claude-sonnet-5-5", "permissionMode": mode[0], "skills": ["jw"],
         "claude_code_version": os.environ.get("FAKE_VERSION", "2.1.309")})
def say(text):
    out({"type": "stream_event", "event": {"type": "message_start"}})
    out({"type": "stream_event", "event": {"type": "content_block_start", "content_block": {"type": "text"}}})
    half = len(text) // 2
    for part in (text[:half], text[half:]):
        out({"type": "stream_event", "event": {"type": "content_block_delta", "delta": {"type": "text_delta", "text": part}}})
    out({"type": "assistant", "message": {"content": [{"type": "text", "text": text}]}})
def result(cost=0.06, turns=1, aborted=False):
    total[0] += cost
    out({"type": "result", "subtype": "error_during_execution" if aborted else "success", "is_error": aborted, "num_turns": turns,
         "total_cost_usd": total[0], "duration_ms": 120, "session_id": session,
         "terminal_reason": "aborted_streaming" if aborted else "completed"})
def wait_response(request_id):
    for line in sys.stdin:
        m = json.loads(line)
        if m.get("type") == "control_response" and m["response"]["request_id"] == request_id:
            return m["response"]["response"]
        if m.get("type") == "control_request" and m["request"]["subtype"] == "interrupt":
            out({"type": "control_response", "response": {"subtype": "success", "request_id": m["request_id"], "response": {"still_queued": []}}})
            return {"behavior": "interrupt"}
    logline("eof")
    sys.exit(0)
def edit(n, suggested):
    tid = "toolu_e%d" % n
    inp = {"file_path": "src/auth.rs", "old_string": "a < b", "new_string": "a <= b"}
    out({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": tid, "name": "Edit", "input": inp}]}})
    rid = "req-%d" % n
    out({"type": "control_request", "request_id": rid, "request": {"subtype": "can_use_tool", "tool_name": "Edit", "input": inp,
         "tool_use_id": tid, "permission_suggestions": [{"type": "addRules"}] if suggested else []}})
    r = wait_response(rid)
    if r["behavior"] == "allow":
        out({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": tid, "content": "ok"}]}})
        say("Hecho.")
        result()
    elif r["behavior"] == "deny":
        out({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": tid, "is_error": True, "content": r["message"]}]}})
        say("Entendido.")
        result()
    else:
        out({"type": "user", "message": {"content": "[Request interrupted by user]"}})
        result(0.01, aborted=True)
def asking(tool, inp, rid, tid):
    out({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": tid, "name": tool, "input": inp}]}})
    out({"type": "control_request", "request_id": rid, "request": {"subtype": "can_use_tool", "tool_name": tool, "input": inp, "tool_use_id": tid}})
    return wait_response(rid)
def questions():
    inp = {"questions": [
        {"question": "¿Qué lenguajes?", "header": "Lenguajes", "multiSelect": True, "options": [{"label": "Rust", "description": "sistemas"}, {"label": "Kotlin", "description": "JVM"}, {"label": "Go", "description": ""}]},
        {"question": "¿Qué framework?", "header": "Framework", "multiSelect": False, "options": [{"label": "A", "description": ""}, {"label": "B", "description": ""}]}]}
    r = asking("AskUserQuestion", inp, "req-q", "toolu_q")
    if r["behavior"] == "allow":
        logline("answers " + json.dumps(r["updatedInput"].get("answers"), ensure_ascii=False))
        out({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "toolu_q", "content": "answered"}]}})
        say("Anotado.")
    else:
        logline("question denied " + r.get("message", ""))
        out({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "toolu_q", "is_error": True, "content": r.get("message", "")}]}})
    result()
def plan(long=False):
    # A plan of 300 lines, more than the box keeps, for the viewer that shows it whole.
    body = "# Plan\n" + "\n".join("- paso %d de un plan largo" % n for n in range(300)) if long else "# Plan\n1. mirar\n2. editar"
    r = asking("ExitPlanMode", {"plan": body, "planFilePath": "/x/plan.md"}, "req-p", "toolu_p")
    if r["behavior"] == "allow":
        logline("plan allowed")
        out({"type": "system", "subtype": "status", "status": None, "permissionMode": "default"})
        out({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "toolu_p", "content": "approved"}]}})
        say("Manos a la obra.")
    else:
        logline("plan denied " + r.get("message", ""))
        out({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "toolu_p", "is_error": True, "content": r.get("message", "")}]}})
    result()
def agent():
    tid = "toolu_a1"
    out({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": tid, "name": "Task", "input": {"description": "buscar los usos", "subagent_type": "Explore", "prompt": "x"}}]}})
    out({"type": "system", "subtype": "task_started", "task_id": "tsk1", "tool_use_id": tid, "description": "buscar los usos", "subagent_type": "Explore", "is_backgrounded": False, "task_type": "local_agent"})
    out({"type": "assistant", "parent_tool_use_id": tid, "message": {"content": [{"type": "tool_use", "id": "toolu_n1", "name": "Grep", "input": {"pattern": "verify"}}]}})
    out({"type": "system", "subtype": "task_progress", "task_id": "tsk1", "tool_use_id": tid, "description": "Grep verify", "usage": {"total_tokens": 22024, "tool_uses": 3, "duration_ms": 6000}, "last_tool_name": "Read"})
    out({"type": "system", "subtype": "task_updated", "task_id": "tsk1", "patch": {"status": "completed", "end_time": 1}})
    out({"type": "system", "subtype": "task_notification", "task_id": "tsk1", "tool_use_id": tid, "status": "completed", "summary": "3 usos"})
    out({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": tid, "content": "3 usos"}]}, "tool_use_result": {"totalDurationMs": 6000, "totalTokens": 22024, "totalToolUseCount": 3}})
    say("El subagente terminó.")
    result()
def task_file(task, body):
    """The file the engine writes the output of a background task to, shaped as the real one."""
    d = os.path.join(os.environ.get("FAKE_TASKS_DIR", tempfile.gettempdir()), "claude-fake", "proj", session, "tasks")
    os.makedirs(d, exist_ok=True)
    path = os.path.join(d, task + ".output")
    with open(path, "w", encoding="utf-8") as f:
        f.write(body)
    outputs[task] = path
    return path
def background():
    tid = "toolu_b1"
    path = task_file("bg1", "uno\n\x1b[32mdos\x1b[0m\n")
    out({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": tid, "name": "Bash", "input": {"command": "sleep 45", "run_in_background": True}}]}})
    out({"type": "system", "subtype": "background_tasks_changed", "tasks": [{"task_id": "bg1", "task_type": "local_bash", "description": "dormir 45 s"}]})
    out({"type": "system", "subtype": "task_started", "task_id": "bg1", "tool_use_id": tid, "description": "dormir 45 s", "is_backgrounded": True, "task_type": "local_bash"})
    out({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": tid, "content": "Command running in background with ID: bg1. Output is being written to: " + path + ". You will be notified when it completes. To check interim output, use Read on that file path."}]}})
    say("Corre en segundo plano.")
    result()
def monitor():
    """A monitor is `local_bash` for the engine too, and its result does not say where its output is."""
    tid = "toolu_m1"
    task_file("mon1", "ERROR uno\n")
    out({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": tid, "name": "Monitor", "input": {"command": "tail -f app.log | grep ERROR", "description": "errores del log", "timeout_ms": 60000}}]}})
    out({"type": "system", "subtype": "task_started", "task_id": "mon1", "tool_use_id": tid, "description": "errores del log", "is_backgrounded": True, "task_type": "local_bash"})
    out({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": tid, "content": "Monitor started (task mon1, expires in 1m unless the source ends first). You will be notified on each event."}]}})
    say("Vigilo el log.")
    result()
def todo():
    out({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "toolu_t1", "name": "TodoWrite", "input": {"todos": [
        {"content": "Buscar", "status": "completed", "activeForm": "Buscando"}, {"content": "Compilar", "status": "in_progress", "activeForm": "Compilando"}]}}]}})
    out({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "toolu_t1", "content": "ok"}]}})
    say("Lista hecha.")
    result()
def push():
    out({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "toolu_n", "name": "PushNotification", "input": {"message": "tests listos", "status": "proactive"}}]}})
    out({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "toolu_n", "content": "Not sent"}]}})
    result()
def auto():
    time.sleep(0.4)
    out({"type": "user", "isReplay": True, "message": {"role": "user", "content": "<task-notification><event>tick 1</event></task-notification>"}})
    init()
    say("Línea 1 del log.")
    result(0.02)
edits = 0
for line in sys.stdin:
    m = json.loads(line)
    if m.get("type") == "control_request":
        if m["request"]["subtype"] == "initialize":
            logline("control initialize")
            if reloaded[0] == "fail":
                out({"type": "control_response", "response": {"subtype": "error", "request_id": m["request_id"], "error": "boom"}})
                continue
            if reloaded[0] == "nolist":
                out({"type": "control_response", "response": {"subtype": "success", "request_id": m["request_id"], "response": {}}})
                continue
            commands = [
                {"name": "context", "description": "Show context usage", "argumentHint": "", "builtin": True},
                {"name": "doctor", "description": "Diagnose the install", "argumentHint": "", "builtin": True},
                {"name": "init", "description": "Create CLAUDE.md", "argumentHint": "", "builtin": True},
                {"name": "model", "description": "Set the model", "argumentHint": "", "builtin": True},
                {"name": "deploy", "description": "publicar la rama en staging", "argumentHint": ""},
                {"name": "jw", "description": "work with jw (user)", "argumentHint": ""},
                {"name": "vercel:deploy", "description": "(vercel) deploy", "argumentHint": ""},
            ]
            if reloaded[0] == "ok":
                # what a plugin installed after the engine started brings: only a reload shows it
                commands += [
                    {"name": "ponytail:ponytail", "description": "lazy senior dev", "argumentHint": ""},
                    {"name": "ponytail:ponytail-review", "description": "review a change", "argumentHint": ""},
                ]
            out({"type": "control_response", "response": {"subtype": "success", "request_id": m["request_id"], "response": {
                "commands": commands,
                "models": [
                    {"value": "default", "resolvedModel": "claude-sonnet-5-5"},
                    {"value": "opus", "resolvedModel": "claude-opus-5-5"},
                    {"value": "haiku", "resolvedModel": "claude-haiku-5-5"},
                ],
            }}})
            continue
        if m["request"]["subtype"] == "stop_task":
            logline("control stop_task " + m["request"]["task_id"])
            out({"type": "control_response", "response": {"subtype": "success", "request_id": m["request_id"], "response": {}}})
            if m["request"]["task_id"] in outputs:
                with open(outputs[m["request"]["task_id"]], "a", encoding="utf-8") as f:
                    f.write("tres\n\n[exited with code 137]\n")
            out({"type": "system", "subtype": "task_updated", "task_id": m["request"]["task_id"], "patch": {"status": "killed", "end_time": 2}})
            out({"type": "system", "subtype": "task_notification", "task_id": m["request"]["task_id"], "status": "stopped", "summary": "dormir 45 s"})
            out({"type": "system", "subtype": "background_tasks_changed", "tasks": []})
            continue
        if m["request"]["subtype"] == "set_permission_mode":
            logline("control set_permission_mode " + m["request"]["mode"])
            mode[0] = "default" if m["request"]["mode"] == "manual" else m["request"]["mode"]
            out({"type": "control_response", "response": {"subtype": "success", "request_id": m["request_id"], "response": {"mode": m["request"]["mode"]}}})
            out({"type": "system", "subtype": "status", "status": None, "permissionMode": m["request"]["mode"], "session_id": session})
        continue
    if m.get("type") != "user":
        continue
    text = m["message"]["content"]
    if isinstance(text, list):
        text = " ".join(b.get("text", "[image]") for b in text)
    logline("user " + json.dumps(text, ensure_ascii=False))
    if text.startswith(("/reload-plugins", "/reload-skills")):
        # the word after the line says what the next `initialize` answers: nothing (ok), `fail` or `nolist`
        reloaded[0] = text.split(" ", 1)[1] if " " in text else "ok"
    if text.startswith("/"):
        result(0.0, turns=0)
        continue
    if claude_dir:
        write_transcript(text)
    if "die" in text:
        sys.stderr.write("boom\n")
        sys.stderr.flush()
        sys.exit(3)
    init()
    if "slow" in text:
        say("Pensando…")
        wait_response("never")
        out({"type": "user", "message": {"content": "[Request interrupted by user]"}})
        result(0.01, aborted=True)
    elif "pregunta" in text:
        questions()
    elif "plan" in text:
        plan("largo" in text)
    elif "agente" in text:
        agent()
    elif "fondo" in text:
        background()
    elif "vigila" in text:
        monitor()
    elif "todo" in text:
        todo()
    elif "push" in text:
        push()
    elif "edit" in text or text.startswith("Sí, adelante con Edit"):
        edits += 1
        say("Voy a editar.")
        edit(edits, "session" in text)
    else:
        say("Voy a mirar.")
        result()
    if "monitor" in text:
        threading.Thread(target=auto).start()
logline("eof")
# A stubborn engine does not end when its input does (a turn or a background task still runs).
if os.environ.get("FAKE_STUBBORN"):
    time.sleep(600)
