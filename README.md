# mcp-yocto

A deterministic [MCP (Model Context Protocol)](https://modelcontextprotocol.io/) server for BitBake/Yocto, written in Rust: it runs `bitbake` and `devtool` commands in a build directory and returns their output as structured JSON.

```bash
git clone http://civsl.lge.com/gitea/kien1.luu/mcp-yocto.git
```

Clone over http (Gitea) — no key needed. The ssh remote
(`ssh://git@10.218.140.120:2222/kien1.luu/mcp-yocto.git`) is owner-only.

## Install

```bash
cargo install --path . --locked    # -> ~/.cargo/bin/mcp-yocto
```

Register that binary as a **stdio** MCP server (absolute path, no arguments).
`AGENTS.md` is the full agent-followable install: toolchain, config file,
client registration, verification, and the usage skill. `skills/mcp-yocto/SKILL.md`
teaches an agent how to drive the tools:

```bash
cp -R skills/mcp-yocto ~/.hermes/skills/project/
```

**Design rules:**
- Runs `bitbake`/`devtool` with an explicit argv — no shell. Every value that
  reaches it is validated first
- Returns structured JSON for all responses
- Logs the output instead of shipping it: a call's complete stdout/stderr goes
  to a log file, and only a stream under 2 KiB is also inlined in the result
  (see [Output logs](#output-logs))
- Digests what it does not inline: when a stream is over 2 KiB the result
  carries a `digest` of at most 4 KiB naming the failed task, its task log and
  the first compiler error (see [Digest](#digest))
- Fails closed: a missing init script aborts the command instead of guessing an
  environment
- Every tool result carries `elapsed_ms`: the wall-clock duration of that call
  in milliseconds, measured at the dispatch boundary. A `bitbake` build reports
  its own duration next to its output, and a rejected call still reports how
  long the rejection took.

## Tools

Runs `bitbake` or `devtool` in a build directory:
`bitbake_recipe`, `devtool_status`, `devtool_modify`,
`devtool_build`, `devtool_reset`, `devtool_deploy_target`

Each tool is documented in full below.

## Build

```bash
cargo build --release
```

The binary is `target/release/mcp-yocto`.

## Claude Desktop configuration

```json
{
  "mcpServers": {
    "mcp-yocto": {
      "command": "/absolute/path/to/mcp-yocto/target/release/mcp-yocto"
    }
  }
}
```

Config file location:
- **macOS:** `~/Library/Application Support/Claude/claude_desktop_config.json`
- **Windows:** `%APPDATA%\Claude\claude_desktop_config.json`
- **Linux:** `~/.config/Claude/claude_desktop_config.json`

Restart Claude Desktop after editing.

## Configuration

The commands need a working Yocto environment (bitbake/devtool on PATH with the
correct BBPATH). Load config from the first existing source:

1. `MCP_YOCTO_CONFIG` env var → path to a config file
2. `./mcp-yocto.toml` in the working directory
3. `~/.config/mcp-yocto.toml`

`mcp-yocto.example.toml` in this repo is a commented copy of that file:

```bash
cp mcp-yocto.example.toml mcp-yocto.toml
```

```toml
# mcp-yocto.toml
init_script = "/home/worker/atlas/oe-init-build-env"
build_dir  = "/home/worker/atlas/build"
```

- `init_script` (optional): absolute path to the Yocto init script. When set,
  every bitbake/devtool command runs as
  `bash -c "source <init_script> .; <command>"` with `build_dir` as the working
  directory, so the tools find bitbake/devtool with a correct BBPATH/BUILDDIR.
  A missing init script fails closed: the command is not run and the tool
  result is `ok:false`. If unset, commands run directly from PATH (only works
  if the server was started from a sourced env).
- `build_dir` (optional): default working directory for build commands when a
  tool call omits `build_dir`. Defaults to the current directory.

`bash` is required for the sourced path — `oe-init-build-env` is bash-only.
All values are shell-quoted and tool inputs are validated, so injection is
blocked.

## Tool reference

Run BitBake and devtool commands in a Yocto build directory. Every command runs
with an explicit argument vector and **no shell**, so a recipe or target value
can never inject shell syntax. Inputs are validated (recipe name, `user@host`
target, task name) and rejected with `INVALID_ARGUMENT` on anything suspicious.

Commands run in `build_dir` (default: the configured `build_dir`, else the
server's current directory). A `build_dir` must be an absolute path to an
existing directory; anything else is rejected with `INVALID_ARGUMENT` before a
command is spawned. Returns:

```json
{
  "ok": true,            // false if the command exited nonzero / could not spawn
  "exit_code": 0,
  "stdout": "...",       // omitted when the stream is empty or over 2 KiB
  "stderr": "...",       // same rule
  "command": "devtool modify busybox",
  "log": "/home/you/.cache/mcp-yocto/1790308139198-devtool-modify-busybox.log",
  "log_bytes": 6378,     // size of that log: the "is it worth reading?" signal
  "digest": { ... },     // only when a stream is over 2 KiB; see Digest
  "elapsed_ms": 128430   // wall-clock duration of this tool call, milliseconds
}
```

A long build's duration is therefore visible in the result itself — no clock
needed on the caller's side. `log` and `log_bytes` are absent when no command
was spawned (a bad `build_dir`, a missing init script), because then there is no
output to log.

### `devtool_status`

`devtool status` — list the recipes currently in the workspace, i.e. those set
up for modification with `devtool modify`. Takes no arguments except
`build_dir`:

```json
{ "build_dir": "/path/to/build" }
```

`stdout` carries one line per recipe, `recipe: <source tree> (<recipefile>)` —
a workspace listing is small, so it is returned in the result itself rather than
only in the log. An empty workspace is not an error: `devtool` logs a "No
recipes currently in your workspace" note instead and the call still returns
`ok: true`. As with every `devtool` command, the first call in a build directory
that has no workspace layer yet creates it and enables it in
`conf/bblayers.conf`.

### `devtool_modify`

`devtool modify <recipe>` — start modifying a recipe.

```json
{ "recipe": "busybox", "build_dir": "/path/to/build" }
```

### `devtool_build`

`devtool build [-c] <recipe>` — build a recipe.

```json
{ "recipe": "myapp", "build_dir": "/path/to/build" }
{ "recipe": "myapp", "clean": true, "build_dir": "/path/to/build" }
```

`clean` defaults to false and emits nothing; `true` appends `-c`, which cleans
up the recipe's build results before building. `-s`
(`--disable-parallel-make`) is not exposed.

### `devtool_reset`

`devtool reset [-n] <recipe>` — undo a `devtool modify`.

```json
{ "recipe": "myapp", "build_dir": "/path/to/build" }
{ "recipe": "myapp", "no_clean": false, "build_dir": "/path/to/build" }
```

`no_clean` defaults to true, so `-n` is always passed unless it is set to
false: reset leaves the recipe's sysroot output in place. Set
`no_clean: false` to let devtool clean the sysroot as well.

### `devtool_deploy_target`

`devtool deploy-target <recipe> <user@host> [options]` — deploy a recipe to a target.

```json
{ "recipe": "myapp", "target": "root@192.168.1.5", "build_dir": "/path/to/build" }
{ "recipe": "myapp", "target": "root@192.168.1.5", "port": 2222, "strip": true,
  "no_host_check": false, "no_preserve": false, "no_check_space": false }
```

| argument | flag | default |
|---|---|---|
| `port` | `-P <port>` | `22` |
| `no_host_check` | `-c` | `true` |
| `no_preserve` | `-p` | `true` |
| `no_check_space` | `--no-check-space` | `true` |
| `strip` | `-S` when true, `--no-strip` when false | `false` |

`strip: true` strips the build output before it is deployed so debug info
never crosses the link. Every boolean resolves to a concrete value, so the
emitted argv does not depend on a `[Deploy]` default in `conf/devtool.conf`.
`show_status` (`-s`) is not exposed. This deploys the recipe's whole
`do_install` output, not just its main package — when a recipe carries a large
debug/ptest payload, shipping the packages instead (`bitbake <recipe> -c
package_write_ipk`) is far smaller.

### `bitbake_recipe`

`bitbake [-f] [-c <task>] [-C <task>] [--runall <task>] [--runonly <task>]
[sstate flags] <recipe>` — build a recipe, or one task of it.

```json
{ "recipe": "busybox", "build_dir": "/path/to/build" }
{ "recipe": "busybox", "task": "compile" }
{ "recipe": "busybox", "task": "compile", "force": true }
{ "recipe": "busybox", "clear_stamp": "compile" }
{ "recipe": "busybox", "runall": "compile" }
{ "recipe": "busybox", "no_setscene": true }
```

| argument | flag | default | meaning |
|---|---|---|---|
| `task` | `-c <task>` | omitted | run one task (e.g. `compile`); omit to run the recipe's default task |
| `force` | `-f` | `false` | force the targets/tasks to run, invalidating any existing stamp file |
| `clear_stamp` | `-C <task>` | omitted | invalidate that task's stamp, then run the default task |
| `runall` | `--runall <task>` | omitted | run that task for any recipe in the taskgraph, even if it would not otherwise have run |
| `runonly` | `--runonly <task>` | omitted | run only that task within the taskgraph (plus its task dependencies) |
| `no_setscene` | `--no-setscene` | `false` | ignore sstate and build everything needed |
| `skip_setscene` | `--skip-setscene` | `false` | skip setscene tasks that would run; keep what was already restored from sstate |
| `setscene_only` | `--setscene-only` | `false` | run only setscene tasks |

Flags are emitted in the order `bitbake --help` lists them, with the recipe
last — the reported `command` mirrors a command line a human would write. Task
values (`task`, `clear_stamp`, `runall`, `runonly`) may contain only ASCII
letters, digits, `_` and `-`, and cannot start with `-`. The sstate flags have
no off form, so `false` or an omitted value emits nothing and bitbake's own
default applies.

The other options `bitbake` offers (`-e`, `-S`, `-g`, `-k`, `-n`, …) are not
exposed.

## Error format

All errors return:

```json
{ "ok": false, "error_code": "...", "message": "...", "elapsed_ms": 0 }
```

Error code: `INVALID_ARGUMENT` — a rejected recipe name, `user@host` target,
task name, `build_dir` or `port` (0), or an argument that is missing or has the
wrong JSON type (`recipe` and `target` are required in the tool schema; `port`
is a number, the flags are booleans). A target or task starting with `-`, and a
target host containing `:` that is not an IPv6 literal, are rejected. A command that ran and failed is not reported this way: it returns
`ok: false` with an `exit_code`, its `log`, and `stderr` unless that is too long
to inline.

Tool errors are returned as text content (with `ok: false` and `elapsed_ms`),
and the MCP result carries `isError: true` whenever the payload is `ok: false` —
including a command that ran and failed. Two protocol-level cases return
JSON-RPC errors instead: an unknown tool name (`-32602`) and a cancelled
request.

## Output logs

Every executed command's complete stdout and stderr is written to a log file,
and the result points at it with `log` (absolute path) and `log_bytes` (size).
The same streams are also returned inline while they fit 2 KiB, so short answers
arrive ready to read while a build's banner and sstate statistics do not:

- **directory** — `$XDG_CACHE_HOME/mcp-yocto`, else `$HOME/.cache/mcp-yocto`
- **name** — `<epoch-millis>-<command-slug>.log` (the name sorts
  chronologically, which is what retention relies on):
  `1790308139198-bitbake--c-compile-zlib.log`. The file is created exclusively;
  if two identical calls land in the same millisecond the later one gets a
  `-1`, `-2`, ... suffix, so no call's log is ever overwritten
- **contents** — a three-line header (`# command:`, `# cwd:`, `# exit_code:`;
  the code is padded to a fixed width), then stdout, then `----- stderr -----`
  and stderr when there is any. Output is streamed to the file byte-for-byte as
  the command runs, never buffered whole
- **retention** — the newest 200 logs are kept; older ones are removed after a
  write, so the directory cannot grow without bound
- **never lost** — if the log cannot be written (no `HOME`, unwritable
  directory) the result carries the output inline regardless of size. If a write
  fails mid-run the partial log is kept, the affected stream is returned
  inline with a note that the log is incomplete, a gap in that text is marked
  `[mcp-yocto: N bytes omitted here; they are in <file>]`, and a stderr scratch
  file (`<log>.err`) that holds the only complete copy is kept and named in the
  note. `.err` files count toward the 200-file retention
- **cancellation** — a cancelled request kills the command's whole process
  group and leaves no log behind

A failure is diagnosed from the log, not from the result: bitbake prints the
failing command, the failed task's own log path and the exit code in it. Search
for `^ERROR:` first and read that region rather than the whole file:

```bash
grep -n '^ERROR:' /home/you/.cache/mcp-yocto/1790308139198-bitbake--c-compile-zlib.log
```

Usually the `digest` already answers that question.

## Digest

When a command was spawned and its stdout or stderr is over 2 KiB (so it is
not inlined), the result carries `digest`: a summary derived from the log after
the log is finished. The log stays the complete record; `ok`, `isError`,
`exit_code`, `log` and `log_bytes` come from the command, never from the digest.
A small result (`devtool status`, short output) has no `digest`.

```json
"digest": {
  "outcome": "failed",
  "failed_stage": "tasks",
  "failed_tasks": [{
    "task": "lvgl-atlas-lean:do_compile",
    "recipe_file": "/build/meta-atlas/meta-atlas/recipes-graphics/lvgl-atlas/lvgl-atlas-lean_9.5.0.bb",
    "task_log": "/build/tmp/work/armv8-2a-atlas-linux/lvgl-atlas-lean/9.5.0/temp/log.do_compile.2239491",
    "first_error": "/build/workspace/sources/lvgl-atlas-lean/atlas-samples/lvgl-overlay/lvgl-overlay.c:575:9: error: call to undeclared function 'stat'; ...",
    "error_lines": 4
  }],
  "warnings": [{"count": 1, "sample": "WARNING: lean-display-server-0.1.2-r2.1 do_package: [atlas-dac] 1 DAC violations found ..."}],
  "noise_warnings": 7,
  "tasks": {"attempted": 2223, "not_rerun": 2212, "failed": 1},
  "sstate": {"wanted": 8, "local": 2, "mirrors": 0, "missed": 6, "current": 878, "match_pct": 25, "complete_pct": 99},
  "parse": {"bb_files": 6776, "parsed": 16, "errors": 0},
  "tail": ["...", "Summary: There was 1 ERROR message, returning a non-zero exit code."],
  "lines": {"total": 1137, "noise": 101, "progress": 37, "task_cmd": 947, "task_other": 28, "diag": 4, "diag_failed": 1, "task_failed": 2, "summary": 8, "error": 1, "warning": 8}
}
```

Fields appear in this order and are omitted when empty:

| Field | Meaning |
|---|---|
| `outcome` | `succeeded` (exit 0), `failed` (non-zero exit), `killed` (no exit code) |
| `failed_stage` | `bitbake`, `devtool build`, `devtool modify` that failed: `parse` if no task ran, else `tasks` |
| `failed_tasks` | up to 4: `task` (`<PN>:<do_X>`), `recipe_file`, `task_log` (bitbake's per-task log), `first_error`, `error_lines` (compiler error lines in that task's log block) |
| `failed_tasks_omitted` | failed tasks not listed (over 4, or cut by the size cap): listed + omitted is always the number of failures |
| `errors`, `errors_overflow` | up to 5 other `ERROR:` lines (one per family; `ExecutionError(...)` wrappers and a line already used as `first_error` are left out), plus the last line of a Python traceback |
| `warnings`, `warnings_overflow` | up to 5 warning families, most frequent first, each with `count` and one `sample` |
| `noise_warnings` | count of known-noise warnings (`No recipes in default available`, `is tainted from a forced run`, `DeprecationWarning`, `ResourceWarning`, `Report submitted`) |
| `tasks`, `sstate`, `parse` | numbers from bitbake's `Tasks Summary`, `Sstate summary` and `Parsing of N .bb files` lines (last one wins) |
| `tail` | failed or killed only: the last 15 lines that are not banner, progress or compile commands |
| `lines` | line counts per class plus `total` |
| `incomplete`, `stderr_file` | the log is incomplete (a log write failed); `stderr_file` is the kept `.err` holding the stderr the log may lack — open it |
| `dropped` | size-cap steps applied, in order |

`first_error` is the first compiler `error:`/`fatal error:` line in the failed
task's log block; failing that, the task's own `ERROR: <pkg> do_X: ...` line
(e.g. a QA error), matched by recipe and task so two recipes failing the same
task each keep their own; failing that, the first `| FAILED: <target>`. `ninja: build
stopped` is never a cause.

**Caps.** Every quoted string is at most 300 bytes: lines keep their head and
paths (`task_log`, `recipe_file`) keep their tail, marked with `…`. The whole
digest is at most 4096 bytes as compact JSON. Over that, these are dropped in
order until it fits, each named in `dropped`: `tail`, `lines`,
`warnings[2..]`, `errors[2..]`, `failed_tasks[1..]`, `sstate`, `parse`,
`warnings`, `errors`, and last `shrink` (every string in `failed_tasks[0]` cut
to 120 bytes, a compiler error to `<dir>/<file>:<L>:<C>: error: <message>`;
`stderr_file` to its file name). `outcome`, `failed_tasks[0]`,
`failed_tasks_omitted`, `tasks`, `incomplete` and `stderr_file` are never
dropped. Over the 202 logs of a real
tree the largest digest is 3 177 bytes (a 2.3 MB log digests to 2 352 bytes)
and no digest needed the drop order.

## Logging

Uses `fern` + `log`. Logs are written to **stderr** so the MCP JSON-RPC stream on stdout is never corrupted. Set `RUST_LOG` to a bare level (`error`, `warn`, `info`, `debug`, `trace`, or `off`) to configure logging; unset or invalid values default to `info`.

## Layout

- `src/main.rs` — entry wiring: logging, config, stdio serve
- `src/tools/` — the six tools: `mod.rs` (server type, tool declarations, run
  path), `requests.rs` (request structs), `argv.rs` (argument building),
  `envelope.rs` (`ok` envelope, `elapsed_ms`), `handler.rs` (`ServerHandler`,
  call timing)
- `src/config.rs` — config file loading (init_script, default build_dir, log dir)
- `src/exec/` — safe subprocess execution: `mod.rs` (`run`, orchestration),
  `spawn.rs` (command build, process-group kill), `log.rs` (output logs,
  sinks, prune), `envelope.rs` (`ExecResult`)
- `src/digest/` — the result `digest` built from a finished log: `mod.rs`
  (builder), `types.rs`, `classify.rs` (line classes, families), `failure.rs`
  (task failures, `first_error`), `caps.rs` (caps, drop order, shrink),
  `text.rs`; unit tests sit in each file
- `tests/fixtures/` — captured real logs (paths rewritten to `/build`), their
  golden `*.digest.json`, and real compiler error lines for `shrink`
- `src/validation.rs` — recipe/target/task validation
- `mcp-yocto.example.toml` — commented config template
- `AGENTS.md` — agent-facing install/register path and repo conventions
- `skills/mcp-yocto/SKILL.md` — usage skill (tool map, workflows, pitfalls)
- `shims.py` — shared test fixtures (fake bitbake/devtool/oe-init-build-env)
- `test_exec.py`, `test_config.py` — stdio test suites (see Testing)

## Testing

```bash
cargo build --release               # tests use target/release/mcp-yocto (override with BB_BIN)
cargo test --release    # digest unit tests + golden fixtures (UPDATE_GOLDEN=1 rewrites them)
python3 test_exec.py    # argv/cwd of every execution tool + the output-log contract
python3 test_config.py  # init_script sourcing via MCP_YOCTO_CONFIG
```

Digest size over a directory of real logs (prints p50/p90/max per outcome and
fails if any digest is over 4096 bytes):

```bash
DIGEST_CORPUS=~/.cache/mcp-yocto cargo test --release corpus -- --ignored --nocapture
```

The shim-based tests create their own fixtures in a temp dir (`shims.py`:
fake `bitbake`/`devtool`, a stand-in `oe-init-build-env`, an empty `HOME`) and
remove them again, so they need no manual setup and cannot be affected by the
host's config or PATH.

`test_exec.py` ends with the output-log contract: a command line containing
`flood` makes a shim emit `FLOOD_LINES` lines of stdout, which is how the
over-cap path (stream omitted, `log`/`log_bytes` returned, log content complete)
and log retention are tested without a Yocto tree. An argument
`fixture-<stamp>` makes a shim replay a captured log from `tests/fixtures/logs`
with its exit code, which is how the `digest` is tested end to end. `LOG_DIR` there follows the
isolated `HOME`, so the test never touches the real `~/.cache/mcp-yocto`.

## License

MIT
