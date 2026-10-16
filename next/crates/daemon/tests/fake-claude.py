#!/usr/bin/env python3
import json, os, sys, threading, time
LOG = os.environ.get("FAKE_LOG", "/tmp/fake-engine.log")
argv = sys.argv[1:]
if "--version" in argv:
    print("fake-claude 1.0")
    sys.exit(0)
session = ""
for i, a in enumerate(argv):
    if a in ("--session-id", "--resume"):
        session = argv[i + 1]
def logline(s):
    with open(LOG, "a", encoding="utf-8") as f:
        f.write(s + "\n")
logline("start " + json.dumps(argv) + " cwd=" + os.getcwd() + " var=" + os.environ.get("MY_VAR", ""))
lock = threading.Lock()
mode = ["default"]
total = [0.0]
def out(o):
    with lock:
        sys.stdout.write(json.dumps(o) + "\n")
        sys.stdout.flush()
def init():
    out({"type": "system", "subtype": "init", "session_id": session, "model": "claude-sonnet-5-5", "permissionMode": mode[0], "skills": ["jw"]})
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
            out({"type": "control_response", "response": {"subtype": "success", "request_id": m["request_id"], "response": {
                "commands": [
                    {"name": "context", "description": "Show context usage", "argumentHint": "", "builtin": True},
                    {"name": "doctor", "description": "Diagnose the install", "argumentHint": "", "builtin": True},
                    {"name": "init", "description": "Create CLAUDE.md", "argumentHint": "", "builtin": True},
                    {"name": "help", "description": "Show help", "argumentHint": "", "builtin": True},
                    {"name": "model", "description": "Set the model", "argumentHint": "", "builtin": True},
                    {"name": "deploy", "description": "publicar la rama en staging", "argumentHint": ""},
                    {"name": "jw", "description": "work with jw (user)", "argumentHint": ""},
                    {"name": "vercel:deploy", "description": "(vercel) deploy", "argumentHint": ""},
                ],
                "models": [
                    {"value": "default", "resolvedModel": "claude-sonnet-5-5"},
                    {"value": "opus", "resolvedModel": "claude-opus-5-5"},
                    {"value": "haiku", "resolvedModel": "claude-haiku-5-5"},
                ],
            }}})
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
    if text.startswith("/"):
        result(0.0, turns=0)
        continue
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
    elif "edit" in text:
        edits += 1
        say("Voy a editar.")
        edit(edits, "session" in text)
    else:
        say("Voy a mirar.")
        result()
    if "monitor" in text:
        threading.Thread(target=auto).start()
