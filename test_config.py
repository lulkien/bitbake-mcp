#!/usr/bin/env python3
"""Test config-driven init_script sourcing over MCP stdio.

Creates its own shims (see shims.py), config files and HOME, so the three
config modes can be exercised without touching the host environment:

  1. init_script + build_dir set   -> command runs as `bash -c "source <init> .; ..."`
  2. init_script missing           -> fails closed, the command never runs
  3. no config at all              -> command runs directly from PATH
"""
import time, json, os, shutil, subprocess, sys, tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import shims

REPO = os.path.dirname(os.path.abspath(__file__))
BIN = os.environ.get("BB_BIN", os.path.join(REPO, "target", "release", "bitbake-mcp"))
if not os.path.exists(BIN):
    sys.exit(f"missing server binary: {BIN}\nrun: cargo build --release  (or set BB_BIN)")

BASE = tempfile.mkdtemp(prefix="bitbake-mcp-config-")
SH = shims.install(BASE)
BUILD = SH["build_dir"]
OTHER_BUILD = SH["other_build_dir"]


def write_config(name, **fields):
    path = os.path.join(BASE, name)
    with open(path, "w") as fh:
        for key, value in fields.items():
            fh.write(f'{key} = "{value}"\n')
    return path


def start(env):
    """Start the server, initialize it, and return (proc, call)."""
    proc = subprocess.Popen(
        [BIN], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
        stderr=subprocess.PIPE, env=env, cwd=BASE,
    )

    def send(o):
        proc.stdin.write((json.dumps(o) + "\n").encode()); proc.stdin.flush()

    def recv():
        return json.loads(proc.stdout.readline())

    def call(name, args, rid=20):
        send({"jsonrpc":"2.0","id":rid,"method":"tools/call",
              "params":{"name":name,"arguments":args}})
        r = recv()
        if r.get("error"):
            return {"ok": False, "error_code": "JSONRPC", "message": str(r["error"])}, r
        return json.loads(r["result"]["content"][0]["text"]), r

    send({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
        "protocolVersion":"2024-11-05","capabilities":{},
        "clientInfo":{"name":"t","version":"1"}}})
    recv()
    send({"jsonrpc":"2.0","method":"notifications/initialized"})
    return proc, call


def stop(proc):
    proc.stdin.close()
    proc.wait(timeout=5)
    return proc.stderr.read().decode()


fail = 0
def check(label, cond, detail=""):
    global fail
    if not cond: fail += 1
    print(f"[{'PASS' if cond else 'FAIL'}] {label}" + (f"\n      {detail}" if detail and not cond else ""))


# --- logging level -------------------------------------------------------
env = shims.server_env(SH)
env.pop("RUST_LOG", None)
proc, _ = start(env)
stderr = stop(proc)
check("unset RUST_LOG defaults to info", "starting bitbake-mcp" in stderr, stderr)

env = shims.server_env(SH)
env["RUST_LOG"] = "warn"
proc, _ = start(env)
stderr = stop(proc)
check("RUST_LOG=warn filters info startup log", "starting bitbake-mcp" not in stderr, stderr)

env = shims.server_env(SH)
env["RUST_LOG"] = "not-a-level"
proc, _ = start(env)
stderr = stop(proc)
check("invalid RUST_LOG defaults to info", "starting bitbake-mcp" in stderr, stderr)


# --- 1. init_script + build_dir from config -------------------------------
env = shims.server_env(SH)
env["BITBAKE_MCP_CONFIG"] = write_config(
    "cfg.toml", init_script=SH["init_script"], build_dir=BUILD,
)
proc, call = start(env)

# bitbake_recipe with no build_dir -> config default build_dir, init sourced,
# so bitbake resolves through the PATH the init script installed.
r, _ = call("bitbake_recipe", {"recipe": "busybox"})
check("init sourced (INIT-SOURCED present)", "INIT-SOURCED" in r.get("stdout",""), str(r.get("stdout")))
check("init receives build dir arg", "INIT-SOURCED builddir='.'" in r.get("stdout",""), str(r.get("stdout")))
check("uses init-path bitbake", "init-path bitbake-args:busybox" in r.get("stdout",""), str(r.get("stdout")))
check("config build_dir is the cwd", f"init-path bitbake-cwd:{BUILD}" in r.get("stdout",""), str(r.get("stdout")))
check("command ok", r.get("ok") is True, str(r))
check("command names the source", f"source {SH['init_script']} ." in r.get("command",""), r.get("command"))

# explicit build_dir overrides the config default
r, _ = call("devtool_modify", {"recipe": "myapp", "build_dir": OTHER_BUILD})
check("devtool_modify runs via source", r.get("ok") is True, str(r))
check("devtool source path present", "INIT-SOURCED" in r.get("stdout",""), str(r.get("stdout")))
check("explicit build_dir wins", f"init-path devtool-cwd:{OTHER_BUILD}" in r.get("stdout",""), str(r.get("stdout")))

# a tool that takes no recipe still runs through the sourced environment
r, _ = call("devtool_status", {})
check("devtool_status sources init", "init-path devtool-args:status\n" in r.get("stdout",""), str(r.get("stdout")))
check("devtool_status uses config build_dir", f"init-path devtool-cwd:{BUILD}" in r.get("stdout",""), str(r.get("stdout")))
stop(proc)

# --- 2. init_script points at a missing file -> fail closed ---------------
env = shims.server_env(SH)
env["BITBAKE_MCP_CONFIG"] = write_config(
    "cfg_missing.toml",
    init_script=os.path.join(BASE, "not-a-real-oe-init-build-env"),
    build_dir=BUILD,
)
proc, call = start(env)
r, _ = call("bitbake_recipe", {"recipe": "busybox"})
check("missing init_script -> not ok", r.get("ok") is False, str(r))
check("missing init_script -> bitbake never ran", "bitbake-args" not in r.get("stdout",""), str(r.get("stdout")))
check("missing init_script -> nonzero exit", r.get("exit_code") not in (None, 0), str(r))
stop(proc)

# --- 3. no config -> direct execution, no sourcing -----------------------
proc, call = start(shims.server_env(SH))
r, _ = call("bitbake_recipe", {"recipe": "busybox", "build_dir": BUILD})
check("no config -> runs directly", r.get("ok") is True, str(r))
check("no config -> plain bitbake shim", "bitbake-args:busybox" in r.get("stdout",""), str(r.get("stdout")))
check("no config -> nothing sourced", "INIT-SOURCED" not in r.get("stdout",""), str(r.get("stdout")))
stop(proc)

# --- 4. cancelling a call kills the whole tree on the init_script path ----
# The direct child is `bash -c "source <init> . ...; devtool ..."`; the shim is
# a non-exec script with a backgrounded grandchild, so only a process-group
# kill reaches everything.
def alive(pid):
    try:
        return open(f"/proc/{pid}/stat").read().rsplit(")", 1)[1].split()[0] != "Z"
    except OSError:
        return False

pidfile = os.path.join(BASE, "tree.pids")
with open(os.path.join(SH["init_dir"], "devtool"), "w") as fh:
    fh.write(f"#!/bin/sh\nsleep 30 &\necho \"$$ $!\" > {pidfile}\nwait\n")
os.chmod(os.path.join(SH["init_dir"], "devtool"), 0o755)
cfg = write_config("cancel.toml", init_script=SH["init_script"], build_dir=BUILD)
env = shims.server_env(SH)
env["BITBAKE_MCP_CONFIG"] = cfg
proc, call = start(env)
proc.stdin.write((json.dumps({"jsonrpc":"2.0","id":70,"method":"tools/call",
    "params":{"name":"devtool_status","arguments":{}}}) + "\n").encode())
proc.stdin.flush()
pids = []
for _ in range(100):
    if os.path.exists(pidfile) and open(pidfile).read().strip():
        pids = [int(x) for x in open(pidfile).read().split()]
        break
    time.sleep(0.05)
proc.stdin.write((json.dumps({"jsonrpc":"2.0","method":"notifications/cancelled",
    "params":{"requestId":70}}) + "\n").encode())
proc.stdin.flush()
for _ in range(100):
    if pids and not any(alive(x) for x in pids):
        break
    time.sleep(0.05)
survivors = [x for x in pids if alive(x)]
for x in survivors:
    os.kill(x, 9)
check("cancel (init_script path) kills the shim and its grandchild",
      len(pids) == 2 and not survivors, f"pids={pids} survivors={survivors}")
proc.kill()
proc.wait(timeout=5)

print()
print("TOTAL FAILURES:", fail)
shutil.rmtree(BASE, ignore_errors=True)
sys.exit(1 if fail else 0)
