#!/usr/bin/env python3
"""Test the six execution tools over MCP stdio.

Uses the PATH-injected shims from shims.py (this script creates them; nothing
has to be prepared by hand) so we can verify exact command construction — argv
and cwd — without a Yocto build.
"""
import json, os, shutil, subprocess, sys, tempfile, time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import shims

REPO = os.path.dirname(os.path.abspath(__file__))
BIN = os.environ.get("BB_BIN", os.path.join(REPO, "target", "release", "mcp-yocto"))
if not os.path.exists(BIN):
    sys.exit(f"missing server binary: {BIN}\nrun: cargo build --release  (or set BB_BIN)")

BASE = tempfile.mkdtemp(prefix="mcp-yocto-exec-")
SH = shims.install(BASE)
BUILD = SH["build_dir"]
GONE = os.path.join(BASE, "does-not-exist")
# log dir the server derives from HOME (shims.server_env isolates HOME under BASE)
LOG_DIR = os.path.join(SH["home"], ".cache", "mcp-yocto")
FLOOD = 300

ENV = shims.server_env(SH)
ENV["FLOOD_LINES"] = str(FLOOD)

proc = subprocess.Popen(
    [BIN], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
    stderr=subprocess.PIPE, env=ENV, cwd=BASE,
)

def send(o):
    proc.stdin.write((json.dumps(o) + "\n").encode()); proc.stdin.flush()

def recv():
    return json.loads(proc.stdout.readline())

def call(name, args, rid=10):
    send({"jsonrpc":"2.0","id":rid,"method":"tools/call",
          "params":{"name":name,"arguments":args}})
    r = recv()
    if r.get("error"):
        return {"ok": False, "error_code": "JSONRPC", "message": str(r["error"])}, r
    return json.loads(r["result"]["content"][0]["text"]), r

# init
send({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
    "protocolVersion":"2024-11-05","capabilities":{},
    "clientInfo":{"name":"t","version":"1"}}})
recv()
send({"jsonrpc":"2.0","method":"notifications/initialized"})

fail = 0
def check(label, cond, detail=""):
    global fail
    if not cond: fail += 1
    print(f"[{'PASS' if cond else 'FAIL'}] {label}" + (f"\n      {detail}" if detail and not cond else ""))

send({"jsonrpc":"2.0","id":2,"method":"tools/call",
      "params":{"name":"unknown_tool","arguments":{}}})
unknown_tool_response = recv()
check("unknown tool returns JSON-RPC -32602",
      unknown_tool_response.get("error", {}).get("code") == -32602,
      str(unknown_tool_response))

# devtool modify
t0 = time.monotonic()
r, _ = call("devtool_modify", {"recipe": "busybox", "build_dir": BUILD})
wall_ms = (time.monotonic() - t0) * 1000
check("devtool_modify runs", r.get("ok") is True, str(r))
check("devtool_modify argv present", "devtool-args:modify busybox" in r.get("stdout",""), str(r.get("stdout")))
check("devtool_modify cwd", f"devtool-cwd:{BUILD}" in r.get("stdout",""), str(r.get("stdout")))
check("devtool_modify command ref", "devtool modify busybox" in r.get("command",""), r.get("command"))
check("devtool_modify elapsed_ms is the call duration",
      isinstance(r.get("elapsed_ms"), int) and 0 <= r["elapsed_ms"] <= wall_ms + 100,
      f"elapsed_ms={r.get('elapsed_ms')} wall_ms={wall_ms:.1f}")

# devtool build (newline pins the end of the argv, so an extra flag on the
# default path would fail this check)
r, _ = call("devtool_build", {"recipe": "myapp", "build_dir": BUILD})
check("devtool_build argv is exactly 'build myapp'",
      "devtool-args:build myapp\n" in r.get("stdout",""), str(r.get("stdout")))

# devtool build -c
r, _ = call("devtool_build", {"recipe": "myapp", "clean": True, "build_dir": BUILD})
check("devtool_build clean=true argv",
      "devtool-args:build -c myapp\n" in r.get("stdout",""), str(r.get("stdout")))
check("devtool_build clean=true command ref",
      r.get("command") == "devtool build -c myapp", r.get("command"))

# clean=false must emit the same argv as an omitted clean
r, _ = call("devtool_build", {"recipe": "myapp", "clean": False, "build_dir": BUILD})
check("devtool_build clean=false argv",
      "devtool-args:build myapp\n" in r.get("stdout",""), str(r.get("stdout")))

# devtool status (no arguments beyond build_dir; the newline pins the end of
# the argv so an appended flag would fail this check)
r, _ = call("devtool_status", {"build_dir": BUILD})
check("devtool_status runs", r.get("ok") is True, str(r))
check("devtool_status argv is exactly 'status'",
      "devtool-args:status\n" in r.get("stdout",""), str(r.get("stdout")))
check("devtool_status cwd", f"devtool-cwd:{BUILD}" in r.get("stdout",""), str(r.get("stdout")))
check("devtool_status command ref", r.get("command") == "devtool status", r.get("command"))

# omitted build_dir -> server cwd, like the other exec tools
r, _ = call("devtool_status", {})
check("devtool_status default cwd", f"devtool-cwd:{BASE}" in r.get("stdout",""), str(r.get("stdout")))

# devtool reset (default -n)
r, _ = call("devtool_reset", {"recipe": "myapp", "build_dir": BUILD})
check("devtool_reset argv (default no_clean)",
      "devtool-args:reset -n myapp" in r.get("stdout",""), str(r.get("stdout")))
r, _ = call("devtool_reset", {"recipe": "myapp", "no_clean": False, "build_dir": BUILD})
check("devtool_reset argv (no_clean=false)",
      "devtool-args:reset myapp" in r.get("stdout",""), str(r.get("stdout")))

# bitbake_recipe: default task, then every task-control option. Each argv
# assertion pins the end of the line with a newline, so a flag emitted after
# the recipe would fail the check.
r, _ = call("bitbake_recipe", {"recipe": "busybox", "build_dir": BUILD})
check("bitbake_recipe runs", r.get("ok") is True, str(r))
check("bitbake_recipe default argv is exactly 'busybox'",
      "bitbake-args:busybox\n" in r.get("stdout",""), str(r.get("stdout")))

r, _ = call("bitbake_recipe", {"recipe": "gtk+3", "build_dir": BUILD})
check("bitbake_recipe accepts plus in recipe name",
      r.get("ok") is True and "bitbake-args:gtk+3\n" in r.get("stdout",""), str(r))

r, _ = call("bitbake_recipe", {"recipe": "busybox", "task": "compile", "build_dir": BUILD})
check("bitbake_recipe -c <task> argv",
      "bitbake-args:-c compile busybox\n" in r.get("stdout",""), str(r.get("stdout")))
check("bitbake_recipe -c command ref",
      r.get("command") == "bitbake -c compile busybox", r.get("command"))

r, _ = call("bitbake_recipe", {"recipe": "busybox", "task": "compile", "force": True, "build_dir": BUILD})
check("bitbake_recipe force=true puts -f first",
      "bitbake-args:-f -c compile busybox\n" in r.get("stdout",""), str(r.get("stdout")))

r, _ = call("bitbake_recipe", {"recipe": "busybox", "force": False, "build_dir": BUILD})
check("bitbake_recipe force=false emits nothing",
      "bitbake-args:busybox\n" in r.get("stdout",""), str(r.get("stdout")))

r, _ = call("bitbake_recipe", {"recipe": "busybox", "clear_stamp": "compile", "build_dir": BUILD})
check("bitbake_recipe -C <task> argv",
      "bitbake-args:-C compile busybox\n" in r.get("stdout",""), str(r.get("stdout")))

r, _ = call("bitbake_recipe", {"recipe": "busybox", "runall": "compile", "build_dir": BUILD})
check("bitbake_recipe --runall argv",
      "bitbake-args:--runall compile busybox\n" in r.get("stdout",""), str(r.get("stdout")))

r, _ = call("bitbake_recipe", {"recipe": "busybox", "runonly": "compile", "build_dir": BUILD})
check("bitbake_recipe --runonly argv",
      "bitbake-args:--runonly compile busybox\n" in r.get("stdout",""), str(r.get("stdout")))

for field, flag in (("no_setscene", "--no-setscene"),
                    ("skip_setscene", "--skip-setscene"),
                    ("setscene_only", "--setscene-only")):
    r, _ = call("bitbake_recipe", {"recipe": "busybox", field: True, "build_dir": BUILD})
    check(f"bitbake_recipe {flag} argv",
          f"bitbake-args:{flag} busybox\n" in r.get("stdout",""), str(r.get("stdout")))
    r, _ = call("bitbake_recipe", {"recipe": "busybox", field: False, "build_dir": BUILD})
    check(f"bitbake_recipe {field}=false emits nothing",
          "bitbake-args:busybox\n" in r.get("stdout",""), str(r.get("stdout")))

# all options at once: bitbake --help order, recipe last
r, _ = call("bitbake_recipe", {"recipe": "busybox", "task": "compile", "force": True,
                               "clear_stamp": "compile", "runall": "compile",
                               "no_setscene": True, "skip_setscene": True,
                               "setscene_only": True, "build_dir": BUILD})
check("bitbake_recipe full argv order",
      "bitbake-args:-f -c compile -C compile --runall compile --no-setscene"
      " --skip-setscene --setscene-only busybox\n" in r.get("stdout",""), str(r.get("stdout")))

# every task-valued option is validated before argv is built
for field in ("task", "clear_stamp", "runall", "runonly"):
    r, _ = call("bitbake_recipe", {"recipe": "busybox", field: "compile; rm -rf /", "build_dir": BUILD})
    check(f"bitbake_recipe bad {field} -> INVALID_ARGUMENT",
          r.get("ok") is False and r.get("error_code") == "INVALID_ARGUMENT", str(r))
    check(f"bitbake_recipe bad {field} never ran",
          "bitbake-args" not in r.get("stdout",""), str(r.get("stdout")))

# devtool deploy-target (defaults: -c -p --no-check-space -P 22 --no-strip)
r, _ = call("devtool_deploy_target", {"recipe": "myapp", "target": "user@192.168.1.5", "build_dir": BUILD})
check("deploy_target default argv",
      "devtool-args:deploy-target -c -p --no-check-space -P 22 --no-strip myapp user@192.168.1.5" in r.get("stdout",""),
      str(r.get("stdout")))

# devtool deploy-target -S / --no-strip
r, _ = call("devtool_deploy_target", {"recipe": "myapp", "target": "user@192.168.1.5", "strip": True, "build_dir": BUILD})
check("deploy_target strip=true argv",
      "devtool-args:deploy-target -c -p --no-check-space -P 22 -S myapp user@192.168.1.5" in r.get("stdout",""),
      str(r.get("stdout")))
r, _ = call("devtool_deploy_target", {"recipe": "myapp", "target": "user@192.168.1.5", "strip": False, "build_dir": BUILD})
check("deploy_target strip=false argv",
      "devtool-args:deploy-target -c -p --no-check-space -P 22 --no-strip myapp user@192.168.1.5" in r.get("stdout",""),
      str(r.get("stdout")))

# devtool deploy-target: port and the on-by-default flags turned off
r, _ = call("devtool_deploy_target", {"recipe": "myapp", "target": "user@192.168.1.5", "port": 2222, "build_dir": BUILD})
check("deploy_target port argv",
      "devtool-args:deploy-target -c -p --no-check-space -P 2222 --no-strip myapp user@192.168.1.5" in r.get("stdout",""),
      str(r.get("stdout")))
r, _ = call("devtool_deploy_target", {"recipe": "myapp", "target": "user@192.168.1.5",
                                      "no_host_check": False, "no_preserve": False,
                                      "no_check_space": False, "build_dir": BUILD})
check("deploy_target flags off argv",
      "devtool-args:deploy-target -P 22 --no-strip myapp user@192.168.1.5" in r.get("stdout",""),
      str(r.get("stdout")))
send({"jsonrpc": "2.0", "id": 11, "method": "tools/call",
      "params": {"name": "devtool_deploy_target",
                 "arguments": {"recipe": "myapp", "target": "user@192.168.1.5", "port": "abc", "build_dir": BUILD}}})
bad_port = recv()["result"]
bad_port_body = json.loads(bad_port["content"][0]["text"])
check("deploy_target bad port type -> ok:false envelope with elapsed_ms",
      bad_port.get("isError") is True and bad_port_body.get("ok") is False
      and bad_port_body.get("error_code") == "INVALID_ARGUMENT"
      and isinstance(bad_port_body.get("elapsed_ms"), int) and "u16" in bad_port_body.get("message", ""),
      str(bad_port))
r, _ = call("devtool_deploy_target", {"recipe": "myapp", "target": "user@host", "port": 0, "build_dir": BUILD})
check("deploy_target port 0 rejected", r.get("ok") is False and r.get("error_code") == "INVALID_ARGUMENT", str(r))
r, _ = call("devtool_status", {"build_dir": 5})
check("non-string build_dir -> envelope with elapsed_ms",
      r.get("ok") is False and r.get("error_code") == "INVALID_ARGUMENT" and "elapsed_ms" in r, str(r))
# validation: bad recipe (shell injection attempt)
r, _ = call("bitbake_recipe", {"recipe": "busybox; rm -rf /", "build_dir": BUILD})
check("injection recipe rejected", r.get("ok") is False and r.get("error_code") == "INVALID_ARGUMENT", str(r))
check("injection did not run", "bitbake-args" not in r.get("stdout",""), str(r.get("stdout")))
check("rejected call still reports elapsed_ms",
      isinstance(r.get("elapsed_ms"), int) and r["elapsed_ms"] >= 0, str(r))

# validation: bad target
r, _ = call("devtool_deploy_target", {"recipe": "myapp", "target": "user@host;ls", "build_dir": BUILD})
check("bad target rejected", r.get("ok") is False and r.get("error_code") == "INVALID_ARGUMENT", str(r))

# validation: bad task
r, _ = call("bitbake_recipe", {"recipe": "busybox", "task": "compile;x", "build_dir": BUILD})
check("bad task rejected", r.get("ok") is False and r.get("error_code") == "INVALID_ARGUMENT", str(r))

# missing recipe
r, _ = call("bitbake_recipe", {"build_dir": BUILD})
check("missing recipe rejected", r.get("ok") is False and r.get("error_code") == "INVALID_ARGUMENT", str(r))

# option-shaped / ambiguous targets must never reach ssh/scp
for bad in ("-F@host", "user@-oProxyCommand", "-user@host", "user@host:/tmp", "user@"):
    r, _ = call("devtool_deploy_target", {"recipe": "myapp", "target": bad, "build_dir": BUILD})
    check(f"target {bad!r} rejected",
          r.get("ok") is False and r.get("error_code") == "INVALID_ARGUMENT" and "stdout" not in r, str(r))
r, _ = call("devtool_deploy_target", {"recipe": "myapp", "target": "root@fe80::1", "build_dir": BUILD})
check("IPv6 literal target accepted", r.get("ok") is True, str(r))
r, _ = call("bitbake_recipe", {"recipe": "busybox", "task": "-x", "build_dir": BUILD})
check("task with leading '-' rejected", r.get("ok") is False and r.get("error_code") == "INVALID_ARGUMENT", str(r))
for dots in (".", "..", "..."):
  r, _ = call("bitbake_recipe", {"recipe": dots, "build_dir": BUILD})
  check(f"recipe {dots!r} rejected", r.get("ok") is False and r.get("error_code") == "INVALID_ARGUMENT", str(r))
r, _ = call("bitbake_recipe", {"recipe": "x", "task": "-", "build_dir": BUILD})
check("task '-' rejected", r.get("ok") is False and r.get("error_code") == "INVALID_ARGUMENT", str(r))
r, _ = call("bitbake_recipe", {"recipe": ".", "build_dir": BUILD})
check("recipe '.' rejected", r.get("ok") is False and r.get("error_code") == "INVALID_ARGUMENT", str(r))

# missing arg -> content-level error (not JSON-RPC)
r, raw = call("devtool_deploy_target", {})
check("missing args content-level", raw.get("error") is None and r.get("ok") is False and r.get("error_code") == "INVALID_ARGUMENT", f"raw={raw}")

# default cwd is the server's own cwd when build_dir is omitted
r, _ = call("bitbake_recipe", {"recipe": "x"})
check("default cwd is server cwd", f"bitbake-cwd:{BASE}" in r.get("stdout",""), str(r.get("stdout")))

# bad build_dir: validated, never spawned; ok:false envelope + elapsed_ms + isError
for label, bd in (("missing", GONE), ("relative", "build"), ("a file", os.path.join(BASE, "oe-init-build-env"))):
    r, raw = call("devtool_reset", {"recipe": "nope", "build_dir": bd})
    check(f"build_dir {label} -> ok:false envelope",
          r.get("ok") is False and r.get("error_code") == "INVALID_ARGUMENT"
          and isinstance(r.get("elapsed_ms"), int) and "stdout" not in r and "log" not in r, str(r))
    check(f"build_dir {label} -> isError", raw["result"].get("isError") is True, str(raw))
r, raw = call("devtool_status", {"build_dir": BUILD})
check("successful command is not isError", not raw["result"].get("isError"), str(raw))

# ---- output logging: over-cap output goes to the log file, not the payload ----
def logs():
    return sorted(os.listdir(LOG_DIR)) if os.path.isdir(LOG_DIR) else []

# small output stays inline and is logged in full
r, _ = call("devtool_status", {"build_dir": BUILD})
log = r.get("log", "")
check("small output still inlined", "devtool-args:status" in r.get("stdout",""), str(r))
check("small output is logged too",
      bool(log) and os.path.isfile(log) and "devtool-args:status" in open(log).read(), str(r))
check("small output log_bytes matches the file",
      r.get("log_bytes") == os.path.getsize(log), str(r))
check("inline-only output carries no digest", "digest" not in r, str(r))

# output over the inline cap: stdout omitted, log holds everything
r, _ = call("bitbake_recipe", {"recipe": "floody", "build_dir": BUILD})
log = r.get("log", "")
text = open(log).read() if log and os.path.isfile(log) else ""
check("over-cap call still ok", r.get("ok") is True and r.get("exit_code") == 0, str(r)[:200])
check("over-cap stdout is not inlined", "stdout" not in r, str(r)[:200])
check("over-cap result stays tiny", len(json.dumps(r)) < 512, str(r)[:200])
check("log holds the whole argv", "bitbake-args:floody" in text, text[:200])
check("log holds every flooded line", text.count("-flood:") == FLOOD, f"count={text.count('-flood:')}")
check("log header names command and cwd",
      "# command: bitbake floody" in text and f"# cwd: {BUILD}" in text, text[:200])
check("over-cap log_bytes matches the file",
      r.get("log_bytes") == os.path.getsize(log), str(r))
check("log lands in the default log dir", os.path.dirname(log) == LOG_DIR, log)

# a failing over-cap call: failure is visible from ok/exit_code + the log
r, _ = call("bitbake_recipe", {"recipe": "floodfail", "build_dir": BUILD})
log = r.get("log", "")
text = open(log).read() if log and os.path.isfile(log) else ""
check("failed over-cap call is ok:false exit 3",
      r.get("ok") is False and r.get("exit_code") == 3, str(r)[:200])
check("failed over-cap call inlines nothing",
      "stdout" not in r and "stderr" not in r, str(r)[:200])
check("failed call's log records the exit code", "# exit_code: 3" in text, text[:200])
check("failed call's log holds the output", text.count("-flood:") == FLOOD, f"count={text.count('-flood:')}")
check("failed over-cap call has a failed digest",
      r.get("digest", {}).get("outcome") == "failed", str(r)[:300])

# ---- digest of real bitbake output (captured logs replayed by the shim) ----
r, raw = call("devtool_build", {"recipe": "fixture-1791941198301", "build_dir": BUILD})
d = r.get("digest", {})
ft = (d.get("failed_tasks") or [{}])[0]
check("real failed build: ok:false, isError, exit 1 (from the exit status)",
      r.get("ok") is False and r.get("exit_code") == 1 and raw["result"].get("isError") is True, str(r)[:300])
check("real failed build: digest outcome and stage",
      d.get("outcome") == "failed" and d.get("failed_stage") == "tasks", str(d)[:300])
check("real failed build: digest names the failed task",
      ft.get("task") == "lean-display-server:do_compile", str(ft))
check("real failed build: digest task_log",
      ft.get("task_log", "").endswith("/lean-display-server/0.1.2/temp/log.do_compile.1435482"), str(ft))
check("real failed build: first_error is the first compiler error",
      ft.get("first_error", "").endswith("internal-types.h:31:5: error: unknown type name 'GraphicPlaneRole'; did you mean 'GraphicPlaneInfo'?")
      and ft.get("error_lines") == 19, str(ft))
check("real failed build: digest within 4096 B serialized",
      len(json.dumps(d, separators=(",", ":"), ensure_ascii=False).encode()) <= 4096, str(len(json.dumps(d))))
check("real failed build: log still complete",
      r.get("log") and open(r["log"]).read().count("| ") > 200 and "stdout" not in r, str(r)[:200])
r, raw = call("devtool_build", {"recipe": "fixture-1791882275218", "build_dir": BUILD})
d = r.get("digest", {})
check("real successful build: digest outcome, tasks, no tail",
      r.get("ok") is True and d.get("outcome") == "succeeded" and "failed_stage" not in d
      and d.get("tasks", {}).get("attempted") == 1701 and "tail" not in d, str(d)[:300])

# a result that never ran a command has no log and stays small
before = logs()
r, _ = call("bitbake_recipe", {"recipe": "busybox; rm -rf /", "build_dir": BUILD})
check("validation error writes no log", "log" not in r and logs() == before, str(r))
r, _ = call("devtool_reset", {"recipe": "nope", "build_dir": GONE})
check("rejected build_dir writes no log", "log" not in r and logs() == before, str(r))

# two identical calls in flight at once: distinct log files, each complete
for i in (0, 1):
    send({"jsonrpc":"2.0","id":50+i,"method":"tools/call",
          "params":{"name":"bitbake_recipe","arguments":{"recipe":"floody","build_dir":BUILD}}})
pair = [json.loads(json.loads(proc.stdout.readline())["result"]["content"][0]["text"]) for _ in (0, 1)]
paths = [x.get("log", "") for x in pair]
check("concurrent identical calls get distinct logs",
      all(paths) and paths[0] != paths[1], str(paths))
check("concurrent calls' logs are both complete",
      all(os.path.isfile(x) and open(x).read().count("-flood:") == FLOOD for x in paths), str(paths))

# log content is byte-exact: non-UTF-8 output is not replaced
raw_shim = os.path.join(SH["path_dir"], "bitbake")
orig = open(raw_shim).read()
open(raw_shim, "w").write("#!/bin/sh\nprintf 'a\\377b\\n'\nprintf 'e\\376\\n' >&2\n")
r, _ = call("bitbake_recipe", {"recipe": "binout", "build_dir": BUILD})
data = open(r["log"], "rb").read() if r.get("log") else b""
check("log keeps non-UTF-8 bytes verbatim", b"a\xffb\n" in data and b"e\xfe\n" in data, repr(data))
check("log has stdout before the stderr section",
      data.find(b"a\xffb") < data.find(b"----- stderr -----") < data.find(b"e\xfe"), repr(data))
open(raw_shim, "w").write(orig)
os.chmod(raw_shim, 0o755)

# a cancelled request must not leave its command running (kill_on_drop)
pidfile = os.path.join(BASE, "slow.pid")
open(raw_shim, "w").write(f"#!/bin/sh\necho $$ > {pidfile}\nexec sleep 30\n")
send({"jsonrpc":"2.0","id":60,"method":"tools/call",
      "params":{"name":"bitbake_recipe","arguments":{"recipe":"slow","build_dir":BUILD}}})
for _ in range(100):
    if os.path.exists(pidfile) and open(pidfile).read().strip():
        break
    time.sleep(0.05)
slow_pid = int(open(pidfile).read().strip()) if os.path.exists(pidfile) else 0
send({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":60}})
gone = False
for _ in range(100):
    try:
        # killed-but-unreaped (zombie) counts as gone
        gone = open(f"/proc/{slow_pid}/stat").read().rsplit(")", 1)[1].split()[0] == "Z"
    except OSError:
        gone = True
    if gone:
        break
    time.sleep(0.05)
if slow_pid and not gone:
    os.kill(slow_pid, 9)
check("cancelled request kills its command", slow_pid > 0 and gone, f"pid={slow_pid}")
# the dropped call's half-written log and .err scratch file are removed
for _ in range(100):
    if not any(n.endswith(".err") or "-slow" in n for n in logs()):
        break
    time.sleep(0.05)
check("cancelled request leaves no .err or log behind",
      not any(n.endswith(".err") or "-slow" in n for n in logs()), str(logs()))
open(raw_shim, "w").write(orig)
os.chmod(raw_shim, 0o755)

# ---- log write failures lose nothing (fault injection: RLIMIT_FSIZE) ----
import re, resource, signal

def limited_server(limit):
    def pre():
        signal.signal(signal.SIGXFSZ, signal.SIG_IGN)
        resource.setrlimit(resource.RLIMIT_FSIZE, (limit, limit))
    p = subprocess.Popen([BIN], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                         stderr=subprocess.PIPE, env=ENV, cwd=BASE, preexec_fn=pre)
    def snd(o):
        p.stdin.write((json.dumps(o) + "\n").encode()); p.stdin.flush()
    snd({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
        "protocolVersion":"2024-11-05","capabilities":{},
        "clientInfo":{"name":"t","version":"1"}}})
    p.stdout.readline()
    snd({"jsonrpc":"2.0","method":"notifications/initialized"})
    def run(recipe):
        snd({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
            "name":"bitbake_recipe","arguments":{"recipe":recipe,"build_dir":BUILD}}})
        return json.loads(json.loads(p.stdout.readline())["result"]["content"][0]["text"])
    return p, run

raw_shim = os.path.join(SH["path_dir"], "bitbake")
orig = open(raw_shim).read()
# 100 stderr lines of exactly 30 bytes = 3000 bytes, stdout one short line.
# Lines 0-79 (2400 bytes) go first; the shim then waits until the server has
# written all of them to this call's .err file (the one holding err-079 but not
# err-099, which rules out a file kept by an earlier case), and emits lines
# 80-99 as one 600-byte printf (atomic on a pipe, so one read). That last write
# is the server's final sink write, with no later write to surface its error:
# only the per-chunk flush in drain() can detect it. No timing assumptions.
open(raw_shim, "w").write(
    "#!/bin/sh\necho out\ni=0\nwhile [ $i -lt 80 ]; do\n"
    "  printf 'err-%03d-xxxxxxxxxxxxxxxxxxxxx\\n' $i >&2\n  i=$((i + 1))\ndone\n"
    f"until grep -l err-079 {LOG_DIR}/*.err 2>/dev/null | xargs -r grep -L err-099 | grep -q .; do\n"
    "  sleep 0.01\ndone\n"
    "tail=''\nwhile [ $i -lt 100 ]; do\n"
    "  tail=\"$tail$(printf 'err-%03d-xxxxxxxxxxxxxxxxxxxxx' $i)\n\"\n  i=$((i + 1))\ndone\n"
    "printf '%s' \"$tail\" >&2\n")
os.chmod(raw_shim, 0o755)

# (a) both sinks healthy, but appending stderr to the log fails: the .err
# scratch file must survive and be named in the result
pa, run = limited_server(3100)
r = run("errflood")
note = r.get("stderr", "")
m = re.search(r"stderr is in (\S+\.err)", note)
errfile = m.group(1) if m else ""
check("finish failure: result names the kept stderr file", bool(errfile), str(r)[:300])
check("finish failure: kept file holds every stderr byte",
      bool(errfile) and os.path.isfile(errfile)
      and open(errfile).read().count("err-") == 100, errfile)
check("finish failure: log kept and reported", os.path.isfile(r.get("log", "")), str(r)[:300])
check("finish failure: result says the log is incomplete", "log is incomplete" in note, note[:300])
d = r.get("digest", {})
check("finish failure: digest marks the log incomplete",
      d.get("incomplete") is True and d.get("outcome") == "succeeded", str(d)[:300])
check("finish failure: digest.stderr_file is the kept .err",
      bool(errfile) and d.get("stderr_file") == errfile, f"{d.get('stderr_file')} vs {errfile}")
pa.stdin.close(); pa.wait(timeout=5)

# (b) stderr sink fails mid-stream: inline text marks the gap and names the file
pb, run = limited_server(2500)
r = run("errflood")
note = r.get("stderr", "")
m = re.search(r"bytes omitted here; they are in (\S+\.err)", note)
check("mid-stream failure: gap marker names the file", bool(m), note[:300])
inline_n = note.count("err-")
file_n = open(m.group(1)).read().count("err-") if m and os.path.isfile(m.group(1)) else 0
check("mid-stream failure: inline + file cover all 100 lines",
      inline_n + file_n >= 100 and "err-099" in note, f"inline={inline_n} file={file_n}")
pb.stdin.close(); pb.wait(timeout=5)

open(raw_shim, "w").write(orig)
os.chmod(raw_shim, 0o755)

# retention: at most 200 log files are kept
for _ in range(210):
    call("devtool_status", {"build_dir": BUILD})
check("log dir is pruned to 200 files", len(logs()) <= 200, f"{len(logs())} files")

# no log dir at all (HOME and XDG_CACHE_HOME unset): nothing may be dropped
env = dict(ENV)
env.pop("HOME", None)
env.pop("XDG_CACHE_HOME", None)
p2 = subprocess.Popen([BIN], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                      stderr=subprocess.PIPE, env=env, cwd=BASE)

def send2(o):
    p2.stdin.write((json.dumps(o) + "\n").encode()); p2.stdin.flush()

send2({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
    "protocolVersion":"2024-11-05","capabilities":{},
    "clientInfo":{"name":"t","version":"1"}}})
json.loads(p2.stdout.readline())
send2({"jsonrpc":"2.0","method":"notifications/initialized"})
send2({"jsonrpc":"2.0","id":40,"method":"tools/call",
       "params":{"name":"bitbake_recipe","arguments":{"recipe":"floody","build_dir":BUILD}}})
r2 = json.loads(json.loads(p2.stdout.readline())["result"]["content"][0]["text"])
check("no log dir -> over-cap output still returned inline",
      "log" not in r2 and r2.get("stdout","").count("-flood:") == FLOOD,
      str(r2)[:200])
p2.stdin.close()
p2.wait(timeout=5)

print()
print("TOTAL FAILURES:", fail)
proc.stdin.close()
proc.wait(timeout=5)
shutil.rmtree(BASE, ignore_errors=True)
sys.exit(1 if fail else 0)
