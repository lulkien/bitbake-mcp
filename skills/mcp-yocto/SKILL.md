---
name: mcp-yocto
description: Drive Yocto builds and devtool workflows via mcp-yocto.
version: 0.1.0
author: Kien Hoang Luu (kien1.luu), Hermes Agent
license: MIT
platforms: [linux]
metadata:
  hermes:
    tags: [yocto, bitbake, devtool, mcp, embedded]
---

# mcp-yocto: BitBake/Yocto MCP server

Six MCP tools that run `bitbake`/`devtool` in a Yocto build directory and return
`{ok, exit_code, command, elapsed_ms, log, log_bytes}` plus `stdout`/`stderr`
while those fit 2 KiB, and a `digest` (≤ 4 KiB) when they do not. **A build's
full output is in the log file, not in the result** — the `digest` says what
failed; open the log only when it is not enough.

## When to Use

- Building a recipe: `bitbake`, a single task, or `devtool build`.
- Modifying a recipe from source: `devtool status` → `devtool modify` →
  `devtool build` → `devtool reset`.
- Getting a build onto a board: `devtool deploy_target`.
- Any task where you would otherwise run `bitbake` or `devtool` in a terminal.

Don't use for: setting up a Yocto build tree or editing recipes (edit files
directly) — see the `yocto-embedded-build` and `yocto-deploy` project skills for
those.

## Prerequisites

- `mcp-yocto` binary installed and registered with the MCP client, tools
  visible as `mcp__mcp_yocto__*` (or plain names for non-Hermes clients). If
  missing: the repo's `AGENTS.md` has the install/register steps.
- `~/.config/mcp-yocto.toml` (or `MCP_YOCTO_CONFIG`) must point `init_script` at
  `oe-init-build-env` and `build_dir` at a build tree. Without it every tool
  fails closed with `ok:false` — that is config, not a broken recipe.

## Tool map

Every tool takes an optional `build_dir` (default from config; pass it
explicitly when several build trees exist):

| tool | arguments | job |
|---|---|---|
| `bitbake_recipe` | `recipe`, `task?`, `force?`, `clear_stamp?`, `runall?`, `runonly?`, `no_setscene?`, `skip_setscene?`, `setscene_only?` | `bitbake [flags] <recipe>`; omit `task` for the recipe's default task |
| `devtool_status` | — | list recipes currently in the workspace (`devtool status`) |
| `devtool_modify` | `recipe` | start modifying a recipe (extracts sources) |
| `devtool_build` | `recipe`, `clean?` | `devtool build [-c] <recipe>`; `clean:true` cleans build results first |
| `devtool_reset` | `recipe`, `no_clean?` | undo a modify; passes `-n` unless `no_clean:false` |
| `devtool_deploy_target` | `recipe`, `target` (`user@host`), `port?`, `strip?`, `no_host_check?`, `no_preserve?`, `no_check_space?` | deploy the recipe's `do_install` output to a board |

## Workflows

**Modify → build → deploy → clean up.** `devtool_status` (what is already
modified) → `devtool_modify` → edit the extracted source tree → `devtool_build`
→ `devtool_deploy_target` with `target: "root@<ip>"` and `port` if non-22 →
`devtool_reset` when done. `devtool_reset` defaults to `-n` (leaves the sysroot
output); pass `no_clean: false` only when the stale sysroot is the problem.

**Ship small.** `devtool_deploy_target` deploys the whole `do_install` output,
debug info included; `strip: true` adds `-S`. For a smaller transfer, build the
packages instead: `bitbake_recipe` with `task: "package_write_ipk"`, then deploy
the IPKs.

**Rebuild when the stamp or sstate is in the way.** `force: true` (`-f`) re-runs
work whose stamp says it already succeeded; `clear_stamp: "compile"` (`-C`)
invalidates that task's stamp and then runs the default task; `runonly: "<task>"`
restricts the run to one task, `runall: "<task>"` widens it to every recipe in
the taskgraph. For sstate: `no_setscene: true` ignores it and builds everything
needed, `skip_setscene: true` skips setscene tasks but keeps what was already
restored, `setscene_only: true` restores without running real tasks.

## Pitfalls

- **`devtool add` has no tool here.** The server exposes modify/build/status/reset/
  deploy-target and bitbake_recipe only, so `devtool add <name> <srctree>` (and any
  other unexposed devtool subcommand) is the one case that runs in `terminal`,
  replicating the server's own invocation exactly:
  `cd <build_dir> && bash -c 'source <init_script> . || exit 1; devtool add <name> <srctree>'`.
  Everything after that — editing the generated `.bb`, building, deploying — goes
  back through the tools.
- **Run `bitbake`/`devtool` through these tools, never `terminal`.** They own
  cwd resolution, the sourced Yocto environment, argument validation, and the
  `elapsed_ms` measurement; a raw terminal call bypasses all four.
- **No shell.** Commands run as an explicit argv. Globs, pipes, redirects, `&&`,
  `$( )` and `;` are not interpreted, and a value containing shell
  metacharacters is rejected as `INVALID_ARGUMENT`. Do not work around a
  rejection by shelling out — split it into tool calls.
- **Read `ok`, `exit_code` and `log` first — not the output.** A bitbake build's
  stdout is kilobytes of banner and sstate statistics, so it is not in the
  result: every call returns `log` (absolute path) and `log_bytes`, and
  `stdout`/`stderr` appear only while they fit 2 KiB (a `devtool_status`
  listing does, a build does not). `ok:false` with `exit_code != 0` means the
  command ran and failed; `ok:false` with no `exit_code` means it never spawned
  (bad cwd or a missing init script) — and then there is no `log`, because there
  was no output.
- **Diagnose a failure from the digest first, then the log.** Read
  `digest.failed_tasks[0]`: `task` (`<PN>:<do_X>`), `first_error` (the first
  compiler error, else the task's own `ERROR:` line such as a QA issue) and
  `task_log` (bitbake's per-task log). That is usually enough to fix the code.
  A keep-going build lists up to 4 failures, each with its own cause;
  `failed_tasks_omitted` counts the rest.
  `failed_stage: "parse"` means no task ran — look at `digest.errors`. Open
  `task_log` or `log` only when the digest is not enough, and then search for
  `^ERROR:` and read that region; `log_bytes` tells you whether reading
  straight through is even sane — a real log reaches megabytes.
- **The digest is a summary, not the record.** `ok`/`exit_code` decide success,
  never the digest. Strings are capped (paths keep their tail, `…` marks a cut)
  and `dropped` lists what was cut for size. `digest.incomplete: true` means a
  log write failed: the missing stderr is in `digest.stderr_file`.
- **Errors are content, not protocol failures.** Every error is
  `{ok:false, error_code, message, elapsed_ms}` with `error_code:
  INVALID_ARGUMENT` (bad recipe name, `user@host` target, task name, a relative
  or missing `build_dir`, port 0, or a field sent with the wrong JSON type such
  as `"port": "2222"`), and the MCP result has `isError: true` whenever `ok` is
  false. Pass `build_dir` as an absolute path and `port` as a number.
- **The first `devtool` call in a fresh build dir mutates the tree**: it creates
  a workspace layer and enables it in `conf/bblayers.conf`. `devtool_status` on
  an empty workspace answers with `ok:true` and no recipes; that is not an
  error.
- Missing required arguments (`recipe`, and `target` for deploy) return
  `INVALID_ARGUMENT` rather than a protocol error — check the argument names in
  the table above.

## Verification

- Environment wiring: a `bitbake_recipe` with `task: "compile"` returns
  `ok:true`, `exit_code: 0`, and a `command` matching
  `bitbake -c compile <recipe>` — if it returns `ok:false` with no `exit_code`,
  the config's `init_script` is wrong or unsourced.
- Build output: a real build's result carries `log` + `log_bytes` + `digest`
  and no `stdout` (over 2 KiB). That is the contract, not a failure — read the
  digest, then the log if needed.
- `elapsed_ms` is present on every result; use it instead of timing builds
  externally.
