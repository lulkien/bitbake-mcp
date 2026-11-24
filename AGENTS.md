# bitbake-mcp — agent instructions

Rust MCP server (rmcp + tokio) that runs `bitbake` and `devtool` in a Yocto
build directory, exposed as 7 MCP tools over stdio.

`README.md` is the human-facing contract: config, every tool, flags, error
format, layout, tests. This file is the operational path — what an agent does,
in order.

- **Task 1 — install this server.** Do it when `bitbake-mcp` is not on PATH or
  is not registered with your MCP client.
- **Task 2 — change the server.** Repo layout, invariants, proof.

---

## Task 1 — install and register

Done when all three hold:

1. `command -v bitbake-mcp` prints a path.
2. The probe below prints `7 tools: ...`.
3. Steps 3 and 5 are done: the config file exists, and the usage skill is
   installed.

**Fetch the repo first** (given only a repo link, this is the clone to run):

```bash
git clone https://github.com/lulkien/bitbake-mcp.git
cd bitbake-mcp
```

### 1. Rust toolchain

`Cargo.toml` uses `edition = "2024"`, so Rust **1.85 or newer** is required.

```bash
command -v cargo && cargo --version        # if this prints >= 1.85, skip to step 2
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
. "$HOME/.cargo/env"                       # or the rustup env file your shell uses
cargo --version
```

Do not install a distro `rustc` package — it is usually older than 1.85.

### 2. Build, test, install

```bash
cd <clone of this repo>
cargo install --path . --locked            # -> ~/.cargo/bin/bitbake-mcp
```

`--locked` uses `Cargo.lock` as-is; the binary lands in `~/.cargo/bin`, already
on PATH with rustup (add it if your PATH lacks it). Re-running the same command
later upgrades an existing install in place.

Prove the build is sound on this machine before registering it. The repo's own
suites run against `./target/release/bitbake-mcp`, which `cargo install --path .`
just produced:

```bash
python3 test_exec.py && python3 test_config.py   # exit 0, "TOTAL FAILURES: 0"
```

Prove the installed binary speaks MCP and exposes all 7 tools:

```bash
python3 - <<'PY'
import json, subprocess
p = subprocess.Popen(["bitbake-mcp"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                     stderr=subprocess.DEVNULL)
msgs = [
  {"jsonrpc": "2.0", "id": 1, "method": "initialize",
   "params": {"protocolVersion": "2024-11-05", "capabilities": {},
              "clientInfo": {"name": "probe", "version": "0"}}},
  {"jsonrpc": "2.0", "method": "notifications/initialized"},
  {"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}},
]
p.stdin.write(("\n".join(json.dumps(m) for m in msgs) + "\n").encode())
p.stdin.flush()
for line in p.stdout:
    d = json.loads(line)
    if d.get("id") == 2:
        print(len(d["result"]["tools"]), "tools:",
              ", ".join(t["name"] for t in d["result"]["tools"]))
        break
PY
```

`initialize` first is mandatory — a bare `tools/list` is rejected with
`-32602`.

### 3. Config — required

Write `~/.config/bitbake-mcp.toml` (search order: `BITBAKE_MCP_CONFIG` env var,
`./bitbake-mcp.toml` in the server's cwd, then that file — use the absolute user
path, because a stdio server's cwd is the *client's* cwd):

```toml
init_script = "/abs/path/to/oe-init-build-env"
build_dir   = "/abs/path/to/build"
```

- `init_script` — Yocto env script. When set, every `bitbake`/`devtool` command
  runs as `bash -c "source <init_script> .; <command>"` in `build_dir`, so
  bitbake is on PATH with the right `BBPATH`. Set but missing → every tool
  fails closed (`ok:false`) instead of guessing an environment. Unset → commands
  run directly from PATH (only works if the server was started from a sourced
  env).
- `build_dir` — default for tool calls that omit `build_dir`.

**These two paths are machine facts you cannot infer. Detect them
(`ls ~/*/oe-init-build-env`, existing build dirs) and confirm with the user
before writing the file.** Every tool runs a command, so without this file the
server installs but does nothing useful — say so rather than leaving the user
with tools that only fail.

`bitbake-mcp.example.toml` is a commented copy of this file; `cp` it rather than
retyping.

### 4. Register with the MCP client

The server is a **stdio** MCP server: absolute path to the binary, no
arguments, no env required (`BITBAKE_MCP_CONFIG` optional). Do not wrap it in a
shell.

```bash
hermes mcp add bitbake-mcp --command "$HOME/.cargo/bin/bitbake-mcp"   # Hermes
claude mcp add bitbake-mcp -- "$HOME/.cargo/bin/bitbake-mcp"          # Claude Code
```

For a JSON-config client (Claude Desktop, etc.):

```json
{ "mcpServers": { "bitbake-mcp": { "command": "/home/<you>/.cargo/bin/bitbake-mcp" } } }
```

Verify with the client, then restart it if it caches tools:

```bash
hermes mcp test bitbake-mcp     # "Connected" + "Tools discovered: 7"
```

### 5. Install the usage skill

`skills/bitbake-mcp/SKILL.md` in this repo teaches an agent how to drive these
tools (which tool for which job, workflows, pitfalls). Install it where your
agent loads skills from:

```bash
mkdir -p ~/.hermes/skills/project && cp -R skills/bitbake-mcp ~/.hermes/skills/project/
```

Other agents: point them at `skills/bitbake-mcp/SKILL.md` or copy that directory
into their own skill/instruction directory. A new session picks it up; the
current one has already loaded its skill index.

---

## Task 2 — changing the server

| Concern | File |
|---|---|
| Entry wiring (logging, config, serve) | `src/main.rs` |
| Tool declarations, run path | `src/tools/mod.rs` |
| Request structs, argv building, `ok` envelope, call timing | `src/tools/{requests,argv,envelope,handler}.rs` |
| Trust-boundary validation (recipe, target, task) | `src/validation.rs` |
| Process launch (direct argv, or `bash -c "source <init> .; ..."`), kill | `src/exec/{mod,spawn}.rs` |
| Output logs, result envelope | `src/exec/{log,envelope}.rs` |
| Result `digest` (classes, failure state machine, caps) | `src/digest/` (`classify`, `failure`, `caps`) |
| Config load/discovery | `src/config.rs` |

Invariants — keep every one:

- Fail closed: a missing init script aborts the command instead of guessing an
  environment.
- No shell for build commands. Explicit argv only, every user value (recipe,
  target, task, `build_dir`) validated in `src/validation.rs` before it reaches `exec::run`. Never add a shell path
  that skips either.
- Every result — success, validation error, or rejection — is a JSON text block
  carrying `elapsed_ms`. Logs go to stderr; stdout is the JSON-RPC stream.
- Output is logged, not shipped: `src/exec/` writes every executed command's
  complete stdout/stderr to `$XDG_CACHE_HOME|~/.cache` + `/bitbake-mcp` and
  returns `log` + `log_bytes`, inlining a stream only while it fits
  `INLINE_CAP` (2 KiB). Never drop output: with no writable log dir the result
  must carry it inline. Results that never spawned a command have no log. Logs
  are created with `create_new` (never truncate another call's), streamed not
  buffered, and blocking directory work (prune) runs in `spawn_blocking`.
- A cancelled request (`notifications/cancelled`) drops its dispatch future in
  `call_tool`; commands run in their own process group and `GroupKill` kills the
  whole group (the `bash -c` wrapper and grandchildren included), with
  `kill_on_drop` as fallback, and `LogGuard` removes the half-written log. A
  normally finished command's group is left alone (bitbake's server daemon
  outlives it). If a log write fails, the log is kept and the streams that did
  not reach it come back inline, with a note that the log is incomplete and
  gaps marked with the file that holds the missing bytes; the `.err` scratch
  file is deleted only after stderr is safely in the log. `test_exec.py` drives
  this with RLIMIT_FSIZE fault injection. Sink writes are flushed per chunk:
  tokio reports a background write error only on the next operation, so an
  unflushed last chunk would fail silently. A pipe read error also adds a note
  naming the stream and bytes read; `digest.incomplete` is set when a digest is
  present. Interrupted reads are retried.
- Every payload is `ok`-shaped: `call_tool` wraps rmcp's deserialization errors
  into `ok:false` + `elapsed_ms`, and sets `isError` when `ok` is false.
- The digest is derived, never a replacement: `src/digest/` builds it from
  the finished `.log` (in `spawn_blocking`, after `finish_log`) only when a
  stream is over `INLINE_CAP`; `log`, `log_bytes`, `ok` and `isError` never
  depend on it. It never reads the `.err` scratch file; an incomplete log sets
  `digest.incomplete` and names the kept `.err` in `digest.stderr_file`. Caps
  are on serialized bytes (`DIGEST_CAP` 4096), output is deterministic (no
  clocks, no `HashMap` ordering), and the rules follow
  `docs/log-compression-design.md`. A rule change must regenerate the goldens
  (`UPDATE_GOLDEN=1 cargo test`) and the diff must be reviewed.

Proof for any change:

```bash
cargo build --release
cargo test --release                             # digest unit + golden tests
python3 test_exec.py && python3 test_config.py   # "TOTAL FAILURES: 0"
```

`test_exec.py` asserts exact argv and cwd strings from `shims.py`, so any argv
change must update those expectations in the same commit. It also ends with the
output-log contract (the `flood` shims cross `INLINE_CAP`), so any change to the
result envelope must update that section in the same commit. Finish by
confirming the changed tool's `inputSchema` in `tools/list`.

Doc sync: `README.md` carries the human-facing contract (config, per-tool flag
tables, error codes) — update it with the code, and this file if install,
config, or registration changes. The usage skill `skills/bitbake-mcp/SKILL.md`
documents tool *selection and workflows*, not flags; update it when a tool is
added, renamed, or its job changes.
