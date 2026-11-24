# Log compression engine — design

Status: design only (board task T49, amended by T51, T55 and T57 after review). No code
under `src/` has changed.
`src/exec.rs` is being modified by T46; line numbers below are from the
working tree on 2026-10-19 and will drift.

Goal: every spawned `bitbake`/`devtool` call returns a compact, high-signal
`digest` next to the existing `log` path, so the calling agent rarely has to
open the log at all. The log file stays the complete source of truth.

## 1. Evidence

Sampled every file in `~/.cache/bitbake-mcp/` (202 `.log`, 2 orphan `.err`,
7.6 MB on disk). Classifier: a 20-rule prefix/substring script run over every
line; nothing was eyeballed from whole logs.

| Command kind | Logs | Lines | Bytes | Median lines | Max lines |
|---|---:|---:|---:|---:|---:|
| `devtool build` | 130 | 18 417 | 7 312 036 | 121 | 1 137 |
| `devtool deploy-target` | 56 | 1 158 | 86 532 | 23 | 23 |
| `bitbake -c …` | 10 | 623 | 40 680 | 79 | 85 |
| `devtool status` | 6 | 78 | 6 477 | 13 | 13 |
| **Total** | **202** | **20 276** | **7 445 725** | | |

Totals are `cat *.log | wc -lc`. One log (`1792395887863-bitbake-slow.log`,
killed mid-write) ends without `\n`; the taxonomy table below counts that
fragment as a line, so its columns sum to 20 277 lines / 7 445 726 B.

Exit codes in the header: 180 × `0`, 20 × `1`, 2 × `-` (cancelled `bitbake slow`
test runs whose `.err` sibling was left behind — see Risks).

Largest logs (`wc -l`, `wc -c`):

```
1137 2339471 1792028141185-devtool-build-lvgl-atlas-lean.log      exit 1
 546  999156 1791898141355-devtool-build-lvgl-atlas-lean.log      exit 1
 534  969278 1791890949211-devtool-build-lvgl-atlas-lean.log      exit 1
 514  914577 1791881994020-devtool-build-lvgl-atlas-lean.log      exit 1
 210  149819 1791890148291-devtool-build-lean-display-server.log  exit 1
```

In the 2.3 MB log the median line is 2 396 B (p90 2 509, max 21 942): it is
the failed task's dumped ninja log, one `| [N/M] clang++ …` command per line.

Line taxonomy over all 202 logs:

| Class | Lines | % lines | Bytes | % bytes | Example |
|---|---:|---:|---:|---:|---|
| `banner` (env, server connect, Build Configuration, parse, "No recipes in default available" + bbappend list) | 10 829 | 53.4 | 708 653 | 9.5 | `Parsing of 6776 .bb files complete (6760 cached, 16 parsed)…` |
| `task_progress` (`Running task N of M`, `task do_X: Started/Succeeded`, `compiling from external source tree`) | 2 697 | 13.3 | 283 442 | 3.8 | `NOTE: Running task 2238 of 2238 (…:do_compile)` |
| `tasklog_cmd` (`\| [N/M] …` compiler/linker command) | 2 383 | 11.8 | 6 034 689 | **81.0** | `\| [1/37] cd …/lean-display-server/0.1.2 …` |
| `sstate` (`Sstate summary`, `stale sstate objects`, setscene) | 982 | 4.8 | 79 214 | 1.1 | `Sstate summary: Wanted 0 Local 0 … (0% match, 100% complete)` |
| `blank` | 624 | 3.1 | 624 | 0.0 | |
| `header` (`# command/cwd/exit_code`) | 606 | 3.0 | 44 604 | 0.6 | |
| `tasklog_other` (`\| DEBUG:`, `\| NOTE:`, cmake chatter) | 591 | 2.9 | 133 584 | 1.8 | `\| DEBUG: Executing python function externalsrc_compile_prefunc` |
| `summary` (`Summary:`, `Tasks Summary`) | 441 | 2.2 | 25 899 | 0.3 | `NOTE: Tasks Summary: Attempted 2238 tasks of which 2237 didn't need to be rerun and all succeeded.` |
| `tasklog_ctx` (compiler source excerpt / caret) | 245 | 1.2 | 13 277 | 0.2 | `\|  1070 \|     { return unique_ptr<_Tp>(…` |
| `warning` | 159 | 0.8 | 33 042 | 0.4 | `WARNING: lean-display-server-0.1.2-r2.1 do_package: [atlas-dac] 1 DAC violations…` |
| `task_failed` (`ERROR: Task (`, `task do_X: Failed`, `Logfile of failure stored in`, `Log data follows:`) | 132 | 0.7 | 12 883 | 0.2 | `ERROR: Logfile of failure stored in: …/temp/log.do_compile.71729` |
| `py_warning` (host-python `DeprecationWarning`/`ResourceWarning` + continuation) | 128 | 0.6 | 15 710 | 0.2 | `… report-error.bbclass:…: DeprecationWarning: codecs.open() is deprecated` |
| `tasklog_diag` (`\| file:L:C: error/warning/note:`) | 119 | 0.6 | 26 823 | 0.4 | `\| …/internal-types.h:31:5: error: unknown type name 'GraphicPlaneRole'` |
| `error_report` (send-error-report boilerplate) | 116 | 0.6 | 12 320 | 0.2 | `NOTE: The errors for this build are stored in …` |
| `devtool` (`INFO: Successfully deployed`, status rows) | 104 | 0.5 | 11 629 | 0.2 | `lvgl-atlas-lean: /…/workspace/sources/lvgl-atlas-lean` |
| `stderr_mark` | 57 | 0.3 | 1 083 | 0.0 | `----- stderr -----` |
| `other` (unclassified; mostly one Python traceback) | 31 | 0.2 | 2 956 | 0.0 | `bb.data_smart.ExpansionError: Failure expanding variable …` |
| `error` (other `ERROR:`) | 23 | 0.1 | 4 029 | 0.1 | `ERROR: lean-display-server-0.1.2-r2.1 do_compile: ExecutionError(…)` |
| `traceback` | 10 | 0.0 | 1 265 | 0.0 | `  File "…/urllib/request.py"…` |

Actionable classes (`error`, `task_failed`, `tasklog_diag` errors, `warning`,
`summary`) are ≈2.5 % of lines and ≈1.3 % of bytes.

Repeated-line families (digits → `N`, paths → `<p>`):

- Parse banner printed twice per log in 129 of 202 logs (devtool runs bitbake
  twice); once in 64; never in 9.
- `WARNING: No recipes in default available for:` 253 × with a fixed 7-path
  bbappend list each time.
- `WARNING: <p> is tainted from a forced run` 228 × (135 of them glued onto
  `Initialising tasks...` on the same line).
- `WARNING: … [atlas-dac] N DAC violations found …` 62 × (61 lean-display-server,
  1 lvgl-atlas-lean).
- `codecs.open() is deprecated` 62 × across 4 prefixes.
- The `Build Configuration` block (≈30 key/layer lines) repeated in 137 logs.
- `| [N/M] …clang… -DLV_CONF_INCLUDE_SIMPLE …` 1 939 × (one family).
- `NOTE: Running task N of M (<p>)` 861 ×.

Failure shape (22 logs with a non-zero or `-` exit): 19 have exactly one
`ERROR: Task (…) failed` — 18 `do_compile`, 1 `do_package` QA (`installed but
not shipped`); 1 is a parse-time `ExpansionError` (23 lines, no task ran), 2
are killed (`exit_code: -`). All 20 exit-1 logs are task-running commands
(`bitbake`, `devtool build`); the 56 `deploy-target` and 6 `status` logs all
exit 0. The event order of a task failure is in §3.1.
The top-level `ERROR: … ExecutionError(…)` is a wrapper; the real cause is the
first `| …: error:` line (1–19 per log) or `| FAILED: <target>` (29 total, 18
`ninja: build stopped`). No log contained `\r` progress bars (non-tty output).
138 lines carry `WARNING:`/`ERROR:`/`NOTE:` mid-line, so markers must not be
matched only at column 0 — but 18 `| ` task-log lines also contain `ERROR:` or
`WARNING:`, so `| ` lines must be classified before marker search (§2).
All 202 logs are valid UTF-8 and contain no control bytes other than tab.


Digest size, measured on **serialized JSON** (compact `serde_json` shape,
UTF-8 bytes) from a Python prototype of the rules in §2–§3, run over all 202
logs (`proto.py`, kept outside the repo):

| Outcome | Logs | Log bytes p50 / max | Digest bytes p50 / p90 / max | Hit drop order |
|---|---:|---|---|---:|
| `succeeded` | 180 | 7 547 / 12 055 | 306 / 603 / 605 | 0 |
| `failed` | 20 | 84 624 / 2 339 471 | 2 132 / 2 534 / 2 598 | 0 |
| `killed` | 2 | 225 / 241 | 119 / 119 / 119 | 0 |

The worst log compresses 2 339 471 → 2 534 B (≈920×). No real log reaches the
cap; a synthetic worst case (50 failed tasks, 200 warning families, 20 errors,
every string padded with 400 control bytes) runs the whole drop order and ends
at 2 176 B.

## 2. Line taxonomy (engine classes)

The engine uses 9 classes, merging the measurement classes above:

| `LineClass` | Covers | Digest use |
|---|---|---|
| `Noise` | banner, blank, header, error_report, stderr_mark, py_warning | counted only |
| `Progress` | task_progress, sstate lines other than the summary | counted |
| `TaskCmd` | tasklog_cmd | counted; never quoted (median 2.4 KB) |
| `TaskOther` | tasklog_other, tasklog_ctx, `\| ninja: build stopped` | `tail` only |
| `Diag` | `\| f:L:C: error:` / `fatal error:` | `first_error` rank 1, `error_lines` |
| `DiagFailed` | `\| FAILED: <target>` | `first_error` rank 3 (fallback) |
| `TaskFailed` | `ERROR: Logfile of failure stored in`, `ERROR: Task (` | opens/closes a failure (§3.1) |
| `Error` | other `ERROR:` anywhere in the line, traceback | `errors`, `first_error` rank 2 |
| `Warning` | `WARNING:` anywhere in the line | grouped |
| `Summary` | `Tasks Summary`, `Sstate summary`, `Parsing of` | parsed into numbers |

**Precedence** — first match wins, checked in this order:

1. Header lines 1–3 and `----- stderr -----` → `Noise`.
2. Line starts with `| ` → task-log dump; it is **never** searched for
   `ERROR:`/`WARNING:` (18 such lines exist). Within it: `[N/M] ` or `: && `
   → `TaskCmd`; `:L(:C)?: (fatal error|error): ` → `Diag`; `FAILED: ` →
   `DiagFailed`; else `TaskOther`.
3. `ERROR: Logfile of failure stored in: ` / `ERROR: Task (` → `TaskFailed`.
4. `Tasks Summary`, `Sstate summary`, `Parsing of N .bb files` → `Summary`.
5. contains `ERROR:` → `Error`; `Traceback` / `  File "` → `Error`.
6. contains `WARNING:` → `Warning`.
7. `Running task`, `task do_X: Started|Succeeded`, setscene → `Progress`.
8. Everything else → `Noise`.

Classification looks only at the first 4 KiB of a line (`CLASSIFY_SPAN`).
Lines are decoded with `String::from_utf8_lossy` on that span only; all 202
real logs are valid UTF-8, so this matters only for hostile input.

**Families.** Key: replace each whitespace-delimited token containing `/`
with `<p>`, each digit run with `N`, truncate to 160 B. The table holds at
most `FAMILY_MAX = 64` keys, shared by warnings and errors, in first-seen
order. A line whose family is new once the table is full is counted in
`warnings_overflow` (or `errors_overflow`) and never quoted. Measured maximum:
3 warning families in one log.

Known-noise warning families (`NOISE_WARNINGS`, counted in `noise_warnings`,
not grouped): `No recipes in default available`, `is tainted from a forced
run`, `DeprecationWarning`, `ResourceWarning`, `Report submitted`.

## 3. The digest

### 3.1 Failure-event order (replayed on two real logs)

Bitbake reports a failed task in this order. `1792028141185-devtool-build-lvgl-atlas-lean.log` (2.3 MB):

```
127   ERROR: lvgl-atlas-lean-9.5.0-r3.0 do_compile: ExecutionError('…/run.do_compile.2239491', 1, …)
137   ERROR: Logfile of failure stored in: …/lvgl-atlas-lean/9.5.0/temp/log.do_compile.2239491
138   Log data follows:
1100  | FAILED: atlas-samples/lvgl-overlay/CMakeFiles/lvgl-drm-overlay.dir/lvgl-overlay.c.o
1102  | …/lvgl-overlay.c:575:9: error: call to undeclared function 'stat'; …
1105, 1108, 1111  | …: error: … (3 more)
1116  | ninja: build stopped: subcommand failed.
1119  NOTE: recipe lvgl-atlas-lean-9.5.0-r3.0: task do_compile: Failed
1120  ERROR: Task (…/lvgl-atlas-lean_9.5.0.bb:do_compile) failed with exit code '1'
1133  Summary: 1 task failed:
```

`1791941198301-devtool-build-lean-display-server.log` has the same order:
ExecutionError 92 → Logfile 102 → `Log data follows:` 103 → `| FAILED:` 126 →
first `| …internal-types.h:31:5: error:` 131 → 18 more `error:` lines and 2 more
`FAILED:` (167, 226) → `ninja: build stopped` 267 → `task do_compile: Failed`
270 → `ERROR: Task (…lean-display-server_0.1.bb:do_compile) failed` 271.

So the Logfile line and every diagnostic come **before** the `ERROR: Task`
line, and `FAILED:` comes before the compiler `error:` it is caused by.

### 3.2 Failure state machine

`DigestBuilder` keeps `pending: Vec<Failure>` (first-seen order),
`pre_errors: Vec<(pn, do_x, line)>` (first-seen order) and
`failures_seen: u32`.

A failure's identity is **recipe + task**, never the task alone. `PN` is
derived the same way everywhere: from an `ERROR: <pkg> <do_X>:` line,
`pkg` minus its last two `-` fields (`PV`, `PR`):
`lvgl-atlas-lean-9.5.0-r3.0` → `lvgl-atlas-lean`; from a recipe file, the
stem before `_` (`lvgl-atlas-lean_9.5.0.bb`); from a `task_log`, the path
component three above the file (`…/lvgl-atlas-lean/9.5.0/temp/log.do_package.N`).
PN is compared for equality, not as a prefix (`lvgl-` must not match
`lvgl-atlas-lean-…`).

1. `Error` line matching `ERROR: <pkg> <do_X>: ` that is not an
   `ExecutionError(` wrapper → remember `(PN, do_X, line)` in `pre_errors`,
   once per `(PN, do_X)` pair — the first line wins (the QA case:
   `ERROR: … do_package: QA Issue: …`, then `Fatal QA errors were found`).
   **Failure mode this prevents:** keying by `do_X` alone. In a keep-going
   build where two recipes fail the same task, the second recipe would inherit
   the first one's cause and its own line would fall through to `errors[]`
   (reproduced against the T53 build, see §6).
2. `ERROR: Logfile of failure stored in: <path>` → push a new pending
   `Failure { task_log: path }`.
3. `Diag` while a pending failure exists → on the **last** pushed one:
   `error_lines += 1`; set `first_error` if unset. `DiagFailed` → set
   `failed_fallback` if unset. Bitbake prints one failure's log block
   contiguously, so the last pushed record owns the `| ` lines.
4. `ERROR: Task (<recipe_file>:<do_X>) failed` → `failures_seen += 1`; close a pending record:
   the oldest whose `task_log` contains `/log.<do_X>.` and `/<PN>/`
   (`PN` = recipe file stem before `_`), else the oldest pending, else a new
   record with no `task_log`. Set `task = "<PN>:<do_X>"` and `recipe_file`;
   move it to `failed_tasks`.
5. At end of input, records still pending (log cut off, killed) are moved to
   `failed_tasks` with `task = "<PN>:<do_X>"` taken from `task_log`, and each
   adds 1 to `failures_seen`.
6. `first_error` rank: compiler `error:`/`fatal error:` (rank 1) → the
   `pre_errors` entry with the **same `(PN, do_X)`** (rank 2); if no entry has
   that PN, a `do_X`-only match is used only when exactly one `pre_errors`
   entry has that `do_X` and it is not already attached to another failure →
   `| FAILED: <target>` (rank 3). `ninja: build stopped` is never a cause.
   A `pre_errors` line attached as a `first_error` is not repeated in
   `errors[]`; an unattached one stays there.
7. **Count.** `failed_tasks` lists at most `FAILED_TASKS_MAX = 4` records,
   first-closed first; every record beyond it is still counted. The digest
   carries `failed_tasks_omitted = failures_seen − len(failed_tasks)` when
   non-zero, including records removed later by drop step 5, so
   `len(failed_tasks) + failed_tasks_omitted` always equals the number of
   failures in the log. `tasks.failed` (from `Tasks Summary`) is reported
   as printed and is not used to fill this count: a killed log has no
   summary line.

With `-k`/keep-going, several failures each open at their own Logfile line
and close at their own Task line; rule 4's match on `PN` + `do_X` pairs them
even if Task lines arrive out of order, and rule 6 gives each its own
pre-Logfile cause.

Corpus check for these rules (`cat *.log`): 2 non-wrapper `ERROR: <pkg>
<do_X>:` lines exist, both `lvgl-atlas-lean-9.5.0-r3.0 do_package` in
1792039192210 (lines 85 and 94, one `(PN, do_X)` pair, the first wins); 19
logs have exactly one `ERROR: Task (` line and 183 have none. The real cache
therefore never exercises two recipes on one task or more than four failures;
both are pinned by synthetic tests only.

Replay with the prototype: all 19 task failures get `task`, `recipe_file`,
`task_log` and `first_error` — 18 from rank 1, the QA `do_package` from rank 2,
none needed rank 3. `error_lines` is 4 for 1792028141185 and 19 for
1791941198301. The 20th failure (parse `ExpansionError`) has no task and
reports through `errors`.

### 3.3 Fields

JSON object, fields in this order; absent when empty. Example: the actual
prototype output for 1792028141185 (paths shortened, `tail` elided; the
prototype folds `Diag` into `task_other`, the engine counts it separately):

```json
"digest": {
  "outcome": "failed",
  "failed_stage": "tasks",
  "failed_tasks": [{
    "task": "lvgl-atlas-lean:do_compile",
    "recipe_file": "…/recipes-graphics/lvgl-atlas/lvgl-atlas-lean_9.5.0.bb",
    "task_log": "…/lvgl-atlas-lean/9.5.0/temp/log.do_compile.2239491",
    "first_error": "…/lvgl-overlay.c:575:9: error: call to undeclared function 'stat'; ISO C99 and later do not support implicit function declarations [-Wimplicit-function-declaration]",
    "error_lines": 4
  }],
  "warnings": [{"count": 1, "sample": "WARNING: lean-display-server-0.1.2-r2.1 do_package: [atlas-dac] 1 DAC violations found …"}],
  "noise_warnings": 7,
  "tasks": {"attempted": 2223, "not_rerun": 2212, "failed": 1},
  "sstate": {"wanted": 8, "local": 2, "mirrors": 0, "missed": 6, "current": 878, "match_pct": 25, "complete_pct": 99},
  "parse": {"bb_files": 6776, "parsed": 16, "errors": 0},
  "tail": ["…", "Summary: There were 7 WARNING messages.", "Summary: There was 1 ERROR message, returning a non-zero exit code."],
  "lines": {"total": 1137, "error": 3, "noise": 128, "progress": 11, "task_cmd": 947, "task_other": 33, "warning": 8}
}
```

- `outcome`: `succeeded` (exit 0), `failed` (exit ≠ 0), `killed` (no exit
  code, header `exit_code: -`). Three values for every command kind.
- `failed_stage`: only for task-running kinds (`CommandKind::Tasks`:
  `bitbake`, `devtool build`, `devtool modify`, `devtool build-image`,
  `devtool upgrade`) when `outcome = failed`:
  `parse` if no `Running task` line was seen, else `tasks`. `deploy-target`,
  `status`, `reset`, and `add` (`CommandKind::Plain`) never carry it — a failed deploy is
  just `failed`. Replaces T49's `parse_failed` outcome.
- `failed_tasks` (≤ 4): §3.2. `failed_tasks_omitted`: how many more failed
  (rule 7); absent when 0; never dropped.
- `errors` (≤ 5, deduped by family): `Error` lines not consumed by §3.2 and
  not `ExecutionError(` wrappers, plus the last line of any traceback.
- `warnings` (≤ 5 groups, count desc then first seen), `warnings_overflow`,
  `noise_warnings`.
- `tasks`, `sstate`, `parse`: integers parsed from `Summary` lines; last
  occurrence wins (devtool runs bitbake twice).
- `tail`: only when `outcome ≠ succeeded`; last 15 lines that are not
  `Noise`/`TaskCmd`/`Progress`.
- `lines`: per-class counts plus `total`.
- `incomplete` / `stderr_file`: only for an incomplete log (§4); never
  dropped.
- `dropped`: names of the drop steps applied, in order.

### 3.4 Caps and drop order

Caps are on **serialized** bytes (`serde_json::to_vec(&digest).len()`).

- `LINE_CAP = 300` B of raw text for every quoted string, **paths included**
  (`task_log`, `recipe_file`). Lines keep their head (`…` suffix); paths keep
  their tail (`…` prefix) so the file name survives. Cuts land on a UTF-8
  boundary.
- `DIGEST_CAP = 4096` B serialized.

Over the cap, apply steps until it fits, appending each name to `dropped`:

1. `tail` 2. `lines` 3. `warnings[2..]` 4. `errors[2..]`
5. `failed_tasks[1..]` 6. `sstate` 7. `parse` 8. `warnings` 9. `errors`
10. last resort: every string in `failed_tasks[0]` shrinks to
    `SHRINK_CAP = 120` B raw with `shrink()` below; `stderr_file`, if
    present, shrinks to its file name.

**`shrink()` for `first_error`.** A head cut does not work on real lines:
they start with `| ` and an absolute workspace path. Over the 18 logs with a
compiler `first_error`, the byte offset of `: error:` in the raw line is
min 108 / median 136 / max 194 (0-based; 109/137/195 counted from 1), 12 of
18 are ≥ 118, and a 120 B head cut keeps `error:` in only **2 of 18**. So for
a `Diag` line (`[| ]<path>:<L>[:<C>]: (fatal error|error): <msg>`):

1. strip `| `;
2. replace `<path>` with its last 2 components (`core/internal-types.h`);
3. if `<short>:<L>:<C>: error: ` alone is over 60 B, keep its tail (`…`
   prefix) to 60 B;
4. append `<msg>` and cut the whole string to 120 B on a UTF-8 boundary
   (`…` suffix).

Any other string keeps the `LINE_CAP` rule (lines head, paths tail) at 120 B.
Replay on all 18 real lines: `error:` kept in **18 of 18**; output 83–120 B;
14 keep the whole message (37–76 B), 4 are cut and keep 62–78 B of it.
Example, `1792028141185` line 1102 (raw 258 B, `: error:` at 116):

```
raw   | /mnt/external/01/kien1.luu/atlas/workspace/sources/lvgl-atlas-lean/atlas-samples/lvgl-overlay/lvgl-overlay.c:575:9: error: call to undeclared function 'stat'; ISO C99 and later do not support implicit function declarations [-Wimplicit-function-declaration]
head  /mnt/external/01/kien1.luu/atlas/workspace/sources/lvgl-atlas-lean/atlas-samples/lvgl-overlay/lvgl-overlay.c:575:9: e…
shrink lvgl-overlay/lvgl-overlay.c:575:9: error: call to undeclared function 'stat'; ISO C99 and later do not support implic…
```

Shortest-message case, `1791890148291` line 174 (raw 182 B, `: error:` at 136;
its head cut ends at `…/adaptor/ldsdbusadapt…`):
`adaptor/ldsdbusadaptor_test.cpp:51:25: error: array type 'int[2]' is not
assignable` (83 B, whole 37 B message kept).

**Bound.** After step 10 the digest holds `outcome`, `failed_stage`, one
failure with 4 strings and `error_lines`, `failed_tasks_omitted`, `tasks`,
counters, `incomplete`,
`stderr_file` (a file name from `slug()`: `[A-Za-z0-9._-]`, ≤ 78 B, never
escaped) and `dropped`. Worst-case escaping is 6 B per raw byte (`\u00XX`):
4 × 120 × 6 = 2 880 B + 78 B + ≤ 500 B of keys, integers and the `dropped`
list ≈ 3 460 B < 4096. Measured with every string 120 control bytes, all
counters (including `failed_tasks_omitted`) at 10⁶–10⁹ and `stderr_file`
present: 3 441 B. The synthetic case
above (no `incomplete`) measured 2 176 B.

## 4. Envelope change

`ExecResult` (`src/exec.rs:54-75`) gains one field:

```rust
#[serde(skip_serializing_if = "Option::is_none")]
pub digest: Option<digest::Digest>,
```

- Present iff a child was spawned and at least one stream is not inline.
  `devtool status` (≤ 1.1 KB) and tiny outputs stay as today.
- `log`, `log_bytes`, `exit_code`, `ok`, `command` unchanged; `elapsed_ms`
  still added by `with_elapsed_ms` (`src/main.rs:234-262`). `ok`/`isError`
  derive from the exit status, never from the digest.
- No writable log dir: streams are inlined in full (existing invariant) and
  the digest is built from the in-memory streams with the same builder.
- `INLINE_CAP` (2 KiB, `src/exec.rs:35`) is unchanged and independent.
- Cancelled request: the dispatch future is dropped and no result is sent, so
  no digest is built. A log left with `exit_code: -` (2 exist today) digests to
  `outcome: "killed"` with a `tail`, if anything ever reads it.
- `build_failure` (`src/exec.rs:80-90`): no log, no digest.
- **Incomplete log** (T50: a log sink or `finish_log` failed; the result
  carries `[bitbake-mcp: … the log is incomplete]` and, when stderr had output,
  `stderr is in <file>.err`). The digest is still built, from the `.log`
  text only, and gains `"incomplete": true` plus `"stderr_file": "<path of
  the kept .err>"` when one was kept. `outcome` comes from the exit status
  passed in, not the header (whose `exit_code` field may still read `-`);
  `failed_tasks`/`errors` reflect only what reached the log. The `.err` file
  is the only place for the missing stderr; the digest does not read it,
  because after a partial `finish_log` some of it may already be in the log
  and digesting both would double-count. Chosen over "no digest" because
  bitbake writes `ERROR: Task (` and the Logfile lines to stdout, so a
  partial log still names the failure.

## 5. Implementation surface

New file `src/digest.rs`, no new crate:

```rust
pub enum CommandKind { Tasks, Plain }          // from program + first arg in src/tools/mod.rs
pub struct DigestBuilder { /* counters, families (Vec, ≤ 64), pending, pre_errors, tail ring (15) */ }
impl DigestBuilder {
    pub fn new(kind: CommandKind) -> Self;
    pub fn push_line(&mut self, line: &[u8]);             // one line, no '\n', ≤ LINE_READ_CAP
    pub fn finish(self, exit_code: Option<i32>) -> Digest; // applies §3.4
}
pub fn digest_file(path: &Path, kind: CommandKind, exit_code: Option<i32>) -> std::io::Result<Digest>; // blocking
```

`CommandKind::of` in `src/digest/types.rs`, called by `src/tools/mod.rs`, maps
`bitbake`, `devtool build`, `devtool modify`, `devtool build-image`, and
`devtool upgrade` to `Tasks`; other devtool subcommands (including `add`) map to `Plain`. It travels in `ExecContext`
or as a `run` argument.

Phase 1 calls `digest_file` on the finished log inside `spawn_blocking`, right
after `finish_log` (`src/exec.rs:253`, called at `:459`). It reads only the
`.log` — never the `.err` scratch file: on success `finish_log` has already
copied it in, and on an incomplete log it is reported by path (§4) — through a 64 KiB `BufReader`. Lines are read with a bounded loop
(`fill_buf` + `memchr`-style scan): at most `LINE_READ_CAP = 64 KiB` of a line
is kept, the rest is skipped up to the next `\n` and only counted. A final
fragment without `\n` (the killed log) is still pushed. Measured longest line:
21 942 B.

Cost per line: ≤ 20 prefix/substring checks over ≤ 4 KiB, one family key
(≤ 160 B) for `Warning`/`Error` only, O(1) counters. Worst measured log
(2.34 MB, 1 137 lines): one sequential read, low milliseconds. Memory bound:
64 × 160 B keys + 15 × 300 B tail + ≤ 4 failures + `pre_errors` (≤ 64
entries, shared with the family cap) + one 64 KiB line buffer
≈ 80 KB.

Determinism: no clocks, no `HashMap` iteration (families and pending records
are `Vec`s in first-seen order, stable sort by count), byte-exact
normalization; the digest depends only on log bytes, `CommandKind` and exit
code.

**Phase 3 (streaming)** keeps a **single** `DigestBuilder` fed in the log's
own order: stdout lines as they arrive, then the stderr scratch file's lines
after stdout reaches EOF — the same sequence `digest_file` sees. The two
streams are never interleaved into one builder and never merged from two
builders, so the tail ring, pending failures and family order match Phase 1
and the Phase-1 goldens must pass unchanged. stdout streams live through a
per-stream `LineSplitter`; stderr is digested from the scratch file in
`finish_log`, which already reads it to append it.

## 6. Tests and docs

Rust unit tests (`#[cfg(test)] mod tests` in `src/digest.rs`, `cargo test`,
added to the proof in `AGENTS.md`):

- Precedence: `| foo ERROR: bar` is `TaskOther`, not `Error`;
  `| [3/9] … ERROR: …` is `TaskCmd`; `Initialising tasks...WARNING: x` is `Warning`.
- Event order (§3.1): ExecutionError → Logfile → `| FAILED:` → `| …: error:`
  → `ERROR: Task (` yields `task_log` set and `first_error` = the `error:`
  line, not the `FAILED:` line.
- Rank 2: QA `ERROR: … do_package: QA Issue` with no `| ` diag → `first_error`
  is the QA line. Rank 3: only `| FAILED:` → `first_error` is it.
- Same task, two recipes (the T53 repro, which mis-paired under a
  `do_X`-only key): input lines
  `ERROR: a-1.0-r0 do_package: QA A-issue`,
  `ERROR: b-1.0-r0 do_package: QA B-issue`,
  `ERROR: Logfile of failure stored in: /w/x/a/1.0/temp/log.do_package.1`,
  `ERROR: Task (/l/a_1.0.bb:do_package) failed with exit code '1'`,
  `ERROR: Logfile of failure stored in: /w/x/b/1.0/temp/log.do_package.2`,
  `ERROR: Task (/l/b_1.0.bb:do_package) failed with exit code '1'`, exit 1 →
  `failed_tasks[0] = {task: "a:do_package", first_error: "ERROR: a-1.0-r0
  do_package: QA A-issue"}`, `failed_tasks[1] = {task: "b:do_package",
  first_error: "ERROR: b-1.0-r0 do_package: QA B-issue"}`, and `errors[]`
  contains neither QA line. Same input with the B lines first (Task b before
  Task a) gives the same pairing. A PN-prefix variant (`lvgl` and
  `lvgl-atlas-lean` both failing `do_package`) pairs by equality, not prefix.
- Over the cap: 6 failures → 4 in `failed_tasks`, `failed_tasks_omitted: 2`;
  the same input forced through drop step 5 → 1 in `failed_tasks`,
  `failed_tasks_omitted: 5`. 0 extra failures → field absent.
- Keep-going: Logfile A, Logfile B, Task B, Task A → each `task_log` pairs
  with its own task. Task line with nothing pending → record without
  `task_log`. Logfile with no Task line at EOF → record with `task` from the
  log name.
- `failed_stage`: `Tasks` kind, exit 1, no `Running task` → `parse`; `Plain`
  kind, exit 1 → no `failed_stage`.
- Killed: header `exit_code: -`, no stderr section, last line without `\n`
  (copy of `1792395887863-bitbake-slow.log`) → `outcome: killed`, no panic.
- Families: 228 `tainted` lines → `noise_warnings`; 62 DAC lines → one group
  `count: 62`; 65 distinct families → 64 grouped + `warnings_overflow: 1`.
- Caps: the synthetic worst case → serialized ≤ 4096, `dropped` equals the
  10 step names in order; a 30 KB path → `task_log` ≤ 300 B ending in the
  file name; a 100 KB line without `\n` → read capped at 64 KiB, no OOM;
  invalid UTF-8 and control bytes → valid JSON ≤ 4096.
- Determinism: digest twice, compare bytes.

Captured samples in `tests/fixtures/logs/` (absolute paths rewritten to
`/build`), each with a golden `*.digest.json`:

- `1792028141185` trimmed to lines **1–140 + last 60** (keeps ExecutionError
  127, Logfile 137, `Log data follows:` 138, and 1078–1137 with `FAILED:`
  1100, the 4 `error:` lines, `ninja: build stopped` 1116, Task line 1120).
- `1791941198301` whole (284 lines, 19 errors, 3 `FAILED:`).
- `1791881693107` (parse `ExpansionError`), `1792039192210` (QA
  `do_package`), one successful `devtool build`, one `deploy-target`, one
  `devtool status`, `1792395887863-bitbake-slow.log` (killed).

`test_exec.py`: shims in `shims.py` that print a fixture and exit 1/0;
assert `digest.outcome`, `failed_tasks[0].task_log` and `first_error`,
absence of `digest` for inline-only output, that the `flood` section still sees
every line in the log, and that after a cancelled call **no `*.err` file
remains** in the log dir. A `.err` kept because `finish_log` failed is
allowed: it is named in the result and, since T50, `prune_logs`
(`src/exec.rs:287`) counts `.log` and `.err` names toward `LOG_KEEP`, so it is
pruned like a log. The 2 orphan `.err` files in today's cache predate T50.

Shrink and incomplete-log tests (Rust unit tests in `src/digest.rs`):

- `shrink()` corpus test: the 18 real compiler `first_error` lines (fixture
  `tests/fixtures/first_errors.txt`, paths rewritten to `/build` but the
  original path depth kept) each shrunk to 120 B must contain `error:` and
  either the whole message or ≥ 40 B of it; output ≤ 120 B and valid UTF-8.
  Pinned cases: 1792028141185:1102 equals the `shrink` line in §3.4;
  1791890148291:174 keeps its full 37 B message.
- A 200 B file name (step 3) still yields `error:` within 120 B.
- Incomplete log: a log truncated mid-stderr with `exit_code: -` in its header,
  digested with `exit_code = Some(1)` and a kept-`.err` path → `outcome:
  failed`, `incomplete: true`, `stderr_file` set, the `.err` never opened
  (pass a path that does not exist), and the field survives the full drop
  order. In `test_exec.py`, extend the existing sink-failure case to assert
  `digest.incomplete` and that `digest.stderr_file` equals the `.err` path in
  the result note.

Docs: `README.md` result envelope (`digest`, drop order, 4 KiB serialized
cap); `AGENTS.md` invariants (digest is derived; the log is the source of
truth; digest never reads `.err`; an incomplete log sets `digest.incomplete`)
and proof (`cargo test`);
`skills/bitbake-mcp/SKILL.md` workflow ("read `digest.failed_tasks[0]` first;
open `task_log` or `log` only if that is not enough").

## 7. Phases

1. **P1 — failure digest, post-pass.** `src/digest.rs` with precedence,
   §3.2 state machine, `outcome`/`failed_stage`, `failed_tasks`, `errors`,
   `tasks`, `sstate`, `tail`, all caps and the full drop order; `CommandKind`
   plumbed from `main.rs`; wired after `finish_log`. Unit tests + the
   1792028141185, 1791941198301 and killed fixtures. Covers all 20 real
   failures.
2. **P2 — warnings and counts.** Families, `NOISE_WARNINGS`, overflow
   counters, `lines`, `parse`, the remaining fixtures, README/SKILL/AGENTS.
3. **P3 — streaming** (after T46 lands). One builder fed stdout then stderr
   (§5); delete the re-read; P1/P2 goldens pass unchanged.

## Risks

- Bitbake wording varies by release; unknown lines fall to `TaskOther`/`Noise`
  and still reach `tail` or the log, never lost.
- Samples come from one tree (scarthgap, two recipes); fetch, `do_rootfs`,
  image and keep-going multi-failure logs are unrepresented, so §3.2 rule 4's
  pairing is tested only on synthetic input. Add fixtures as they appear.
- Compile command lines (81 % of bytes) are never quoted; a cause visible only
  in a command line reaches the caller only via `tail` or the log.
- Rule 3 assumes one failure's `| ` block is contiguous; true in both replayed
  logs, unverified under keep-going.
- Two pre-T50 `.err` orphans and `exit_code: -` logs exist today; since T50
  `prune_logs` counts `.err` too, so they age out with the logs.
- An incomplete log's digest is partial by design (§4); callers must open
  `stderr_file` when `incomplete` is set.
- P1's re-read doubles I/O on multi-MB logs (max 2.3 MB) until P3.
