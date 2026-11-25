#!/usr/bin/env python3
"""Test the eighteen execution tools over MCP stdio.

Uses the PATH-injected shims from shims.py (this script creates them; nothing
has to be prepared by hand) so we can verify exact command construction — argv
and cwd — without a Yocto build.
"""
import json, os, shutil, subprocess, sys, tempfile, time, select, signal

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import shims

REPO = os.path.dirname(os.path.abspath(__file__))
BIN = os.environ.get("BB_BIN", os.path.join(REPO, "target", "release", "bitbake-mcp"))
if not os.path.exists(BIN):
    sys.exit(f"missing server binary: {BIN}\nrun: cargo build --release  (or set BB_BIN)")

BASE = tempfile.mkdtemp(prefix="bitbake-mcp-exec-")
SH = shims.install(BASE)
BUILD = SH["build_dir"]
GONE = os.path.join(BASE, "does-not-exist")
# log dir the server derives from HOME (shims.server_env isolates HOME under BASE)
LOG_DIR = os.path.join(SH["home"], ".cache", "bitbake-mcp")
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

# Tool metadata is public MCP behavior: keep this table in sync with the
# annotations and warning sentences on the Rust tool declarations.
send({"jsonrpc":"2.0","id":3,"method":"tools/list","params":{}})
listed_tools = recv().get("result", {}).get("tools", [])
expected_annotations = {
    "bitbake_recipe": {"destructiveHint": True, "idempotentHint": False, "openWorldHint": True},
    "devtool_status": {"readOnlyHint": True, "openWorldHint": False},
    "devtool_modify": {"destructiveHint": False, "idempotentHint": False, "openWorldHint": True},
    "devtool_build": {"destructiveHint": True, "idempotentHint": False, "openWorldHint": True},
    "devtool_build_image": {"destructiveHint": False, "idempotentHint": False, "openWorldHint": True},
    "devtool_reset": {"destructiveHint": True, "idempotentHint": True, "openWorldHint": False},
    "devtool_deploy_target": {"destructiveHint": True, "idempotentHint": True, "openWorldHint": True},
    "devtool_undeploy_target": {"destructiveHint": True, "idempotentHint": True, "openWorldHint": True},
    "devtool_search": {"readOnlyHint": True, "openWorldHint": False},
    "devtool_find_recipe": {"readOnlyHint": True, "openWorldHint": False},
    "devtool_check_upgrade_status": {"readOnlyHint": True, "openWorldHint": True},
    "devtool_configure_help": {"destructiveHint": False, "idempotentHint": True, "openWorldHint": True},
    "devtool_add": {"destructiveHint": False, "idempotentHint": False, "openWorldHint": True},
    "devtool_upgrade": {"destructiveHint": False, "idempotentHint": False, "openWorldHint": True},
    "devtool_update_recipe": {"destructiveHint": True, "idempotentHint": False, "openWorldHint": False},
    "devtool_finish": {"destructiveHint": True, "idempotentHint": False, "openWorldHint": False},
    "job_wait": {"readOnlyHint": True, "idempotentHint": True, "openWorldHint": False},
    "job_cancel": {"destructiveHint": True, "idempotentHint": True, "openWorldHint": False},
}
listed_by_name = {tool["name"]: tool for tool in listed_tools}
check("tools/list has exactly 18 tools", set(listed_by_name) == set(expected_annotations),
      ", ".join(sorted(listed_by_name)))
for tool_name, expected in expected_annotations.items():
    tool = listed_by_name.get(tool_name, {})
    check(f"annotations {tool_name}", tool.get("annotations") == expected,
          f"expected {expected}, got {tool.get('annotations')}")
    if expected.get("readOnlyHint"):
        check(f"description {tool_name} says read-only",
              "read-only" in tool.get("description", "").lower(), tool.get("description", ""))
    elif expected.get("destructiveHint"):
        check(f"description {tool_name} warns destructive",
              "Destructive:" in tool.get("description", ""), tool.get("description", ""))

wait_schema = listed_by_name.get("job_wait", {}).get("inputSchema", {})
check("job_wait schema requires job_id and allows optional wait_secs",
      wait_schema.get("required") == ["job_id"]
      and "wait_secs" in wait_schema.get("properties", {})
      and "wait_secs" not in wait_schema.get("required", []), str(wait_schema))

# The sourced command display must quote an init path exactly as the shell does.
space_dir = os.path.join(BASE, "init path")
os.makedirs(space_dir)
space_init = os.path.join(space_dir, "oe-init-build-env")
shutil.copy(SH["init_script"], space_init)
space_config = os.path.join(BASE, "space-config.toml")
with open(space_config, "w") as fh:
    fh.write(f'init_script = "{space_init}"\nbuild_dir = "{BUILD}"\n')
space_env = dict(ENV, BITBAKE_MCP_CONFIG=space_config)
space_proc = subprocess.Popen([BIN], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                              stderr=subprocess.PIPE, env=space_env, cwd=BASE)
def space_send(o):
    space_proc.stdin.write((json.dumps(o) + "\n").encode()); space_proc.stdin.flush()
space_send({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
    "protocolVersion":"2024-11-05","capabilities":{},
    "clientInfo":{"name":"t","version":"1"}}})
json.loads(space_proc.stdout.readline())
space_send({"jsonrpc":"2.0","method":"notifications/initialized"})
space_send({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
    "name":"bitbake_recipe","arguments":{"recipe":"busybox"}}})
space_result = json.loads(json.loads(space_proc.stdout.readline())[
    "result"]["content"][0]["text"])
check("sourced command quotes init path with spaces",
      f"source '{space_init}' ." in space_result.get("command", ""),
      space_result.get("command"))
space_proc.stdin.close(); space_proc.wait(timeout=5)

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
r, _ = call("devtool_modify", {"recipe": "busybox", "debug_build": True, "build_dir": BUILD})
check("devtool_modify --debug-build argv",
      "devtool-args:modify --debug-build busybox\n" in r.get("stdout", ""),
      str(r.get("stdout")))

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
r, _ = call("devtool_build", {"recipe": "myapp", "disable_parallel_make": True,
                              "build_dir": BUILD})
check("devtool_build -s argv",
      "devtool-args:build -s myapp\n" in r.get("stdout", ""), str(r.get("stdout")))

r, _ = call("devtool_build_image", {"image": "core-image-minimal",
                                     "add_packages": "a,b", "build_dir": BUILD})
check("devtool_build_image -p argv",
      "devtool-args:build-image -p a,b core-image-minimal\n" in r.get("stdout", ""),
      str(r.get("stdout")))
r, _ = call("devtool_build_image", {"image": "core-image-minimal",
                                     "add_packages": "a;b", "build_dir": BUILD})
check("devtool_build_image rejects invalid add_packages",
      r.get("ok") is False and "devtool-args" not in r.get("stdout", ""), str(r))

# Read-only devtool queries: exact argv and validation before exec.
r, _ = call("devtool_search", {"keyword": "openssl", "build_dir": BUILD})
check("devtool_search argv", "devtool-args:search openssl\n" in r.get("stdout", ""), str(r))
r, _ = call("devtool_search", {"keyword": "-x", "build_dir": BUILD})
check("devtool_search rejects leading option keyword",
      r.get("ok") is False and r.get("error_code") == "INVALID_ARGUMENT"
      and "devtool-args" not in r.get("stdout", ""), str(r))
r, _ = call("devtool_find_recipe", {"recipe": "busybox", "build_dir": BUILD})
check("devtool_find_recipe argv",
      "devtool-args:find-recipe busybox\n" in r.get("stdout", ""), str(r))
r, _ = call("devtool_check_upgrade_status", {"recipes": ["r1", "r2"], "build_dir": BUILD})
check("devtool_check_upgrade_status recipes argv",
      "devtool-args:check-upgrade-status r1 r2\n" in r.get("stdout", ""), str(r))
r, _ = call("devtool_check_upgrade_status", {"all": True, "build_dir": BUILD})
check("devtool_check_upgrade_status all argv",
      "devtool-args:check-upgrade-status -a\n" in r.get("stdout", ""), str(r))
r, _ = call("devtool_check_upgrade_status", {"recipes": [f"r{i}" for i in range(33)],
                                               "build_dir": BUILD})
check("devtool_check_upgrade_status rejects 33 recipes",
      r.get("ok") is False and r.get("error_code") == "INVALID_ARGUMENT"
      and "devtool-args" not in r.get("stdout", ""), str(r))
r, _ = call("devtool_configure_help", {"recipe": "busybox", "build_dir": BUILD})
check("devtool_configure_help always disables pager",
      "devtool-args:configure-help -p busybox\n" in r.get("stdout", ""), str(r))

# Workspace creators validate values before spawning.
r, _ = call("devtool_add", {"recipe": "foo", "fetch_uri": "git://example.org/foo",
                             "version": "1.2", "srcrev": "abc123", "srcbranch": "release/1",
                             "build_dir": BUILD})
check("devtool_add exact argv",
      "devtool-args:add -V 1.2 -S abc123 -B release/1 foo git://example.org/foo\n" in r.get("stdout", ""), str(r))
r, _ = call("devtool_add", {"recipe": "foo", "fetch_uri": "https://example.org/foo", "autorev": True, "build_dir": BUILD})
check("devtool_add autorev argv", "devtool-args:add -a foo https://example.org/foo\n" in r.get("stdout", ""), str(r))
for label, args in (("bad scheme", {"fetch_uri": "bad://host/repo"}),
                    ("leading dash URI", {"fetch_uri": "-git://host/repo"}),
                    ("bad version", {"fetch_uri": "git://host/repo", "version": "bad value"}),
                    ("srcrev with autorev", {"fetch_uri": "git://host/repo", "srcrev": "abc", "autorev": True}),
                    ("branch traversal", {"fetch_uri": "git://host/repo", "srcbranch": "a..b"})):
    r, _ = call("devtool_add", {"recipe": "foo", **args, "build_dir": BUILD})
    check(f"devtool_add rejects {label}", r.get("ok") is False and "devtool-args" not in r.get("stdout", ""), str(r))
r, _ = call("devtool_upgrade", {"recipe": "foo", "version": "2.0", "srcrev": "deadbeef",
                                 "srcbranch": "stable/2", "no_patch": True, "build_dir": BUILD})
check("devtool_upgrade exact argv",
      "devtool-args:upgrade -V 2.0 -S deadbeef -B stable/2 --no-patch foo\n" in r.get("stdout", ""), str(r))
r, _ = call("devtool_upgrade", {"recipe": "foo", "srcrev": "bad;rev", "build_dir": BUILD})
check("devtool_upgrade rejects invalid srcrev", r.get("ok") is False and "devtool-args" not in r.get("stdout", ""), str(r))

# Layer writers expose only the reviewed, non-destructive options.
r, _ = call("devtool_update_recipe", {"recipe": "foo", "mode": "patch", "append": BUILD,
                                       "dry_run": True, "no_remove": True, "build_dir": BUILD})
check("devtool_update_recipe exact argv",
      f"devtool-args:update-recipe -m patch -a {BUILD} -N -n foo\n" in r.get("stdout", ""), str(r))
for label, append in (("layer name", "meta-foo"), ("relative directory", "rel/dir")):
    r, _ = call("devtool_update_recipe", {"recipe": "foo", "append": append, "build_dir": BUILD})
    check(f"devtool_update_recipe rejects {label}",
          r.get("ok") is False and r.get("error_code") == "INVALID_ARGUMENT"
          and "devtool-args" not in r.get("stdout", ""), str(r))
r, _ = call("devtool_update_recipe", {"recipe": "foo", "append": GONE, "build_dir": BUILD})
check("devtool_update_recipe rejects missing append directory",
      r.get("ok") is False and "devtool-args" not in r.get("stdout", ""), str(r))
r, _ = call("devtool_finish", {"recipe": "foo", "destination": "meta-foo", "mode": "srcrev",
                               "dry_run": True, "no_clean": True, "build_dir": BUILD})
check("devtool_finish exact argv",
      "devtool-args:finish -m srcrev -N -n foo meta-foo\n" in r.get("stdout", ""), str(r))
for destination in ("rel/dir", "-x"):
    r, _ = call("devtool_finish", {"recipe": "foo", "destination": destination, "build_dir": BUILD})
    check(f"devtool_finish rejects destination {destination}",
          r.get("ok") is False and r.get("error_code") == "INVALID_ARGUMENT"
          and "devtool-args" not in r.get("stdout", ""), str(r))

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

r, _ = call("bitbake_recipe", {"recipe": "busybox", "continue_on_error": True,
                               "dry_run": True, "dump_signatures": "printdiff",
                               "build_dir": BUILD})
check("bitbake_recipe -k -n -S printdiff argv",
      "bitbake-args:-k -n -S printdiff busybox\n" in r.get("stdout", ""),
      str(r.get("stdout")))

r, _ = call("bitbake_recipe", {"recipe": "busybox", "dump_signatures": "bogus",
                               "build_dir": BUILD})
check("bitbake_recipe rejects unknown dump_signatures",
      r.get("ok") is False and "bitbake-args" not in r.get("stdout", ""), str(r))

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
r, _ = call("devtool_deploy_target", {"recipe": "myapp", "target": "user@192.168.1.5",
                                     "dry_run": True, "build_dir": BUILD})
check("deploy_target -n argv",
      "devtool-args:deploy-target -c -n -p --no-check-space -P 22 --no-strip myapp user@192.168.1.5"
      in r.get("stdout", ""), str(r.get("stdout")))

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

# devtool undeploy-target flags follow deploy.py's parser order: -c, -a, -n, -P.
r, _ = call("devtool_undeploy_target", {"recipe": "myapp", "target": "user@192.168.1.5",
                                         "no_host_check": True, "dry_run": True,
                                         "port": 2222, "build_dir": BUILD})
check("undeploy_target recipe argv",
      "devtool-args:undeploy-target -c -n -P 2222 myapp user@192.168.1.5\n"
      in r.get("stdout", ""), str(r.get("stdout")))
r, _ = call("devtool_undeploy_target", {"all": True, "target": "user@192.168.1.5",
                                         "no_host_check": True, "dry_run": True,
                                         "port": 2222, "build_dir": BUILD})
check("undeploy_target all argv",
      "devtool-args:undeploy-target -c -a -n -P 2222 user@192.168.1.5\n"
      in r.get("stdout", ""), str(r.get("stdout")))
for label, selectors in (("both", {"recipe": "myapp", "all": True}), ("neither", {})):
    r, _ = call("devtool_undeploy_target", {**selectors, "target": "user@192.168.1.5",
                                             "build_dir": BUILD})
    check(f"undeploy_target {label} selector rejected",
          r.get("ok") is False and r.get("error_code") == "INVALID_ARGUMENT"
          and "devtool-args" not in r.get("stdout", ""), str(r))
r, _ = call("devtool_undeploy_target", {"recipe": "myapp", "target": "user@host;ls",
                                         "build_dir": BUILD})
check("undeploy_target invalid target rejected",
      r.get("ok") is False and r.get("error_code") == "INVALID_ARGUMENT"
      and "devtool-args" not in r.get("stdout", ""), str(r))
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

r, _ = call("devtool_search", {"keyword": "flood", "build_dir": BUILD})
log = r.get("log", "")
text = open(log).read() if log and os.path.isfile(log) else ""
check("devtool_search flood crosses INLINE_CAP and returns log",
      r.get("ok") is True and "stdout" not in r and bool(log)
      and text.count("devtool-flood:") == FLOOD, str(r)[:250])

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

# ---- wait-then-detach job lifecycle --------------------------------------
job_env = dict(ENV)
job_env["BITBAKE_MCP_DETACH_AFTER_MS"] = "300"
job_env["SHIM_PIDDIR"] = BASE

def start_job_server(env=job_env):
    p = subprocess.Popen([BIN], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                         stderr=subprocess.PIPE, env=env, cwd=BASE)
    def send_job(obj):
        p.stdin.write((json.dumps(obj) + "\n").encode()); p.stdin.flush()
    send_job({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
        "protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"jobs","version":"0"}}})
    json.loads(p.stdout.readline())
    send_job({"jsonrpc":"2.0","method":"notifications/initialized"})
    return p, send_job

def recv_job_ids(p, wanted, timeout=6):
    replies = {}
    deadline = time.monotonic() + timeout
    while wanted - replies.keys() and time.monotonic() < deadline:
        ready, _, _ = select.select([p.stdout], [], [], max(0, deadline - time.monotonic()))
        if not ready: break
        line = p.stdout.readline()
        if not line: break
        reply = json.loads(line)
        if reply.get("id") in wanted: replies[reply["id"]] = reply
    return replies

def job_call(p, send_job, name, args, rid):
    send_job({"jsonrpc":"2.0","id":rid,"method":"tools/call",
              "params":{"name":name,"arguments":args}})
    reply = recv_job_ids(p, {rid}).get(rid, {})
    if reply.get("error"):
        return {"ok":False,"error_code":"JSONRPC"}, reply
    try:
        return json.loads(reply["result"]["content"][0]["text"]), reply
    except (KeyError, IndexError, TypeError, json.JSONDecodeError):
        return {"ok":False,"error_code":"NO_REPLY"}, reply

def pid_gone(path):
    try:
        pid = int(open(path).read().strip())
    except (OSError, ValueError):
        return False
    try:
        return open(f"/proc/{pid}/stat").read().rsplit(")", 1)[1].split()[0] == "Z"
    except OSError:
        return True

def wait_pid_gone(path, seconds=3):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if pid_gone(path): return True
        time.sleep(.03)
    return pid_gone(path)

def launch_sleep(p, send_job, name, recipe, rid):
    pidfile = os.path.join(BASE, recipe + ".pid")
    try: os.unlink(pidfile)
    except FileNotFoundError: pass
    result, raw = job_call(p, send_job, name, {"recipe":recipe,"build_dir":BUILD}, rid)
    return result, raw, pidfile

jp, jsend = start_job_server()
slow, _, slow_pidfile = launch_sleep(jp, jsend, "bitbake_recipe", "sleep-5", 100)
check("slow command detaches with a job id and log",
      slow.get("ok") is True and slow.get("status") == "running" and slow.get("job_id")
      and slow.get("command") and os.path.isfile(slow.get("log", "")), str(slow))
job_id = slow.get("job_id", 0)
logs_before_busy = set(logs())
busy, _ = job_call(jp, jsend, "bitbake_recipe", {"recipe":"sleep-5","build_dir":BUILD}, 101)
busy_slash, _ = job_call(jp, jsend, "bitbake_recipe", {"recipe":"sleep-5","build_dir":BUILD + "/"}, 102)
check("detached same build dir returns BUSY with the existing job id",
      busy.get("error_code") == "BUSY" and busy.get("job_id") == job_id
      and busy_slash.get("error_code") == "BUSY" and busy_slash.get("job_id") == job_id
      and set(logs()) == logs_before_busy, f"{busy} / {busy_slash}")
other, _ = job_call(jp, jsend, "bitbake_recipe", {"recipe":"otherbuild","build_dir":SH["other_build_dir"]}, 103)
check("other build dir is not BUSY", other.get("ok") is True and other.get("status") != "running", str(other))
still_running, _ = job_call(jp, jsend, "job_wait", {"job_id":job_id,"wait_secs":1}, 104)
check("job_wait timeout returns running", still_running.get("status") == "running" and still_running.get("job_id") == job_id, str(still_running))
check("job_wait running includes command and build_dir",
      still_running.get("command") == "bitbake sleep-5"
      and still_running.get("build_dir") == BUILD, str(still_running))

# Cancelling the wait request itself must not cancel the server-owned job.
jsend({"jsonrpc":"2.0","id":105,"method":"tools/call","params":{"name":"job_wait","arguments":{"job_id":job_id,"wait_secs":30}}})
time.sleep(.1)
jsend({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":105}})
still_running, _ = job_call(jp, jsend, "job_wait", {"job_id":job_id,"wait_secs":1}, 106)
check("cancelling job_wait leaves the command running", still_running.get("status") == "running" and not pid_gone(slow_pidfile), str(still_running))

cancelled, cancel_raw = job_call(jp, jsend, "job_cancel", {"job_id":job_id}, 107)
gone = wait_pid_gone(slow_pidfile)
log_path = slow.get("log", "")
for _ in range(100):
    if not (os.path.exists(log_path) or os.path.exists(os.path.splitext(log_path)[0] + ".err")): break
    time.sleep(.02)
check("job_cancel kills the process group and removes partial logs",
      cancelled.get("status") == "cancelled" and gone and not os.path.exists(log_path)
      and not os.path.exists(os.path.splitext(log_path)[0] + ".err"), str(cancelled))
cancel_wait, cancel_wait_raw = job_call(jp, jsend, "job_wait", {"job_id":job_id,"wait_secs":1}, 108)
cancel_repeat, _ = job_call(jp, jsend, "job_cancel", {"job_id":job_id}, 109)
check("cancelled job_wait is an isError CANCELLED and cancel is idempotent",
      cancel_wait.get("error_code") == "CANCELLED" and cancel_wait_raw.get("result",{}).get("isError") is True
      and cancel_repeat.get("status") == "cancelled", f"{cancel_wait} / {cancel_repeat}")

# A wait already in flight is woken by job_cancel; concurrent cancels both settle.
concurrent, _, concurrent_pid = launch_sleep(jp, jsend, "bitbake_recipe", "sleep-5", 110)
concurrent_id = concurrent.get("job_id", 0)
jsend({"jsonrpc":"2.0","id":111,"method":"tools/call","params":{"name":"job_wait","arguments":{"job_id":concurrent_id,"wait_secs":30}}})
jsend({"jsonrpc":"2.0","id":112,"method":"tools/call","params":{"name":"job_cancel","arguments":{"job_id":concurrent_id}}})
both = recv_job_ids(jp, {111,112})
wait_payload = json.loads(both[111]["result"]["content"][0]["text"]) if 111 in both else {}
cancel_payload = json.loads(both[112]["result"]["content"][0]["text"]) if 112 in both else {}
check("job_cancel wakes an in-flight job_wait", wait_payload.get("error_code") == "CANCELLED" and cancel_payload.get("status") == "cancelled", f"{wait_payload} / {cancel_payload}")
check("job_cancel kills while job_wait is pending", wait_pid_gone(concurrent_pid), concurrent_pid)

double, _, double_pid = launch_sleep(jp, jsend, "bitbake_recipe", "sleep-5", 120)
double_id = double.get("job_id", 0)
jsend({"jsonrpc":"2.0","id":121,"method":"tools/call","params":{"name":"job_cancel","arguments":{"job_id":double_id}}})
jsend({"jsonrpc":"2.0","id":122,"method":"tools/call","params":{"name":"job_cancel","arguments":{"job_id":double_id}}})
double_replies = recv_job_ids(jp, {121,122})
double_payloads = [json.loads(double_replies[i]["result"]["content"][0]["text"]) for i in (121,122) if i in double_replies]
check("back-to-back job_cancel calls both settle", len(double_payloads) == 2 and all(x.get("status") == "cancelled" for x in double_payloads) and wait_pid_gone(double_pid), str(double_payloads))

finished_start, _, finished_pid = launch_sleep(jp, jsend, "bitbake_recipe", "sleep-1", 130)
finished, _ = job_call(jp, jsend, "job_wait", {"job_id":finished_start.get("job_id"),"wait_secs":5}, 131)
finished_again, _ = job_call(jp, jsend, "job_wait", {"job_id":finished_start.get("job_id"),"wait_secs":1}, 132)
finished_cancel, _ = job_call(jp, jsend, "job_cancel", {"job_id":finished_start.get("job_id")}, 133)
compare_a, compare_b = dict(finished), dict(finished_again)
compare_a.pop("elapsed_ms", None); compare_b.pop("elapsed_ms", None)
check("job_wait returns finished result with stable payload and run_ms", finished.get("status") == "finished" and finished.get("ok") is True and finished.get("exit_code") == 0 and finished.get("run_ms", 0) >= 1000 and compare_a == compare_b, str(finished))
check("job_cancel on a finished job returns its stored result", finished_cancel.get("status") == "finished" and finished_cancel.get("run_ms") == finished.get("run_ms"), str(finished_cancel))

missing, _ = job_call(jp, jsend, "job_wait", {"job_id":1,"wait_secs":1}, 134)
bad0, bad0_raw = job_call(jp, jsend, "job_wait", {"job_id":1,"wait_secs":0}, 135)
bad51, bad51_raw = job_call(jp, jsend, "job_wait", {"job_id":1,"wait_secs":51}, 136)
badid, badid_raw = job_call(jp, jsend, "job_wait", {"job_id":"abc"}, 137)
check("unknown job returns NOT_FOUND", missing.get("error_code") == "NOT_FOUND", str(missing))
check("wait_secs 0/51 and malformed job id return INVALID_ARGUMENT", all(x.get("error_code") == "INVALID_ARGUMENT" for x in (bad0,bad51,badid)) and all(r.get("result",{}).get("isError") is True for r in (bad0_raw,bad51_raw,badid_raw)), f"{bad0} / {bad51} / {badid}")

retained_ids = []
for index in range(17):
    result, _, _ = launch_sleep(jp, jsend, "bitbake_recipe", "sleep-0.4", 160 + index * 2)
    retained_ids.append(result.get("job_id"))
    completed, _ = job_call(jp, jsend, "job_wait", {"job_id":result.get("job_id"),"wait_secs":2}, 161 + index * 2)
    if completed.get("status") != "finished": break
oldest, _ = job_call(jp, jsend, "job_wait", {"job_id":retained_ids[0],"wait_secs":1}, 200)
newest, _ = job_call(jp, jsend, "job_wait", {"job_id":retained_ids[-1],"wait_secs":1}, 201)
check("finished jobs retain the newest 16 in FIFO order",
      len(retained_ids) == 17 and oldest.get("error_code") == "NOT_FOUND"
      and newest.get("status") == "finished", f"{oldest} / {newest}")

# The server owns detached jobs across request cancellation, but not across its own exit.
eof_result, _, eof_pid = launch_sleep(jp, jsend, "bitbake_recipe", "sleep-30", 140)
eof_log = eof_result.get("log", "")
jp.stdin.close()
try: jp.wait(timeout=5)
except subprocess.TimeoutExpired: jp.kill(); jp.wait(); check("stdin EOF aborts detached jobs before exit", False, "server did not exit")
else: check("stdin EOF aborts detached jobs before exit", wait_pid_gone(eof_pid), eof_pid)
check("stdin EOF removes detached job partial log", not eof_log or not os.path.exists(eof_log), eof_log)
jp.stderr.read()

sp, ssend = start_job_server()
sig_result, _, sig_pid = launch_sleep(sp, ssend, "bitbake_recipe", "sleep-30", 150)
sp.terminate()
try: sp.wait(timeout=5)
except subprocess.TimeoutExpired: sp.kill(); sp.wait(); check("SIGTERM aborts detached jobs before exit", False, "server did not exit")
else: check("SIGTERM aborts detached jobs before exit", wait_pid_gone(sig_pid), sig_pid)
check("SIGTERM removes detached job partial log", not sig_result.get("log") or not os.path.exists(sig_result["log"]), str(sig_result))
sp.stderr.read()

print()
print("TOTAL FAILURES:", fail)
proc.stdin.close()
proc.wait(timeout=5)
shutil.rmtree(BASE, ignore_errors=True)
sys.exit(1 if fail else 0)
