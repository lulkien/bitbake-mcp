//! Safe subprocess execution for BitBake/devtool commands.
//!
//! Two paths:
//! - No `init_script` configured: runs with an explicit argument vector and NO
//!   shell, so a recipe/target value can never inject shell syntax.
//! - `init_script` configured (Yocto `oe-init-build-env`): the command is run
//!   through `bash -c` after sourcing the init script so bitbake/devtool are
//!   on PATH with the correct BBPATH, and the command still runs in `cwd`
//!   with `cwd` as the build dir. Every interpolated value (program, args,
//!   paths) is shell-quoted, and tool inputs are already validated to a
//!   metachar-free charset before reaching here, so injection is doubly
//!   prevented.
//!
//! Both paths write the command's complete output to a log file and return
//! only what fits `INLINE_CAP` inline: a bitbake build prints kilobytes of
//! banner and sstate statistics no caller reads, and the caller's context is
//! the scarce resource. The log file is the full record; when a stream is too
//! large to inline, a `digest` derived from it (see `digest`) says what failed.

mod envelope;
mod log;
mod spawn;

use crate::digest::{CommandKind, DigestBuilder};
use log::{LOG_KEEP, drain, finish_log, inline, open_log, prune_logs, read_notes};
use spawn::GroupKill;
use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
};

pub use envelope::ExecResult;
use envelope::build_failure;

/// Longest stdout/stderr (bytes) still returned inline. A longer stream lives
/// only in the log file and its key is omitted from the result.
///
/// 2 KiB keeps list-shaped answers (`devtool status` with a screenful of
/// recipes, a no-op devtool build) inline, while bitbake's own stdout — a
/// ~2.5 KB banner plus sstate statistics before anything actionable — always
/// goes to the log.
const INLINE_CAP: usize = 2048;

/// Environment context for running build commands.
#[derive(Debug, Clone, Default)]
pub struct ExecContext {
    /// Yocto init script (e.g. `/home/worker/atlas/oe-init-build-env`) to
    /// `source` before the command. When unset, commands run directly.
    pub init_script: Option<PathBuf>,
    /// Directory the complete output of every executed command is written to.
    /// Empty means "no log file": results then carry their output inline.
    pub log_dir: PathBuf,
    /// Log paths currently being written; pruning holds this lock while
    /// selecting and deleting old files.
    pub live_logs: Arc<Mutex<BTreeSet<PathBuf>>>,
}

/// Run `program` with `args` in `cwd`.
///
/// Sourced script handling: when `ctx.init_script` is set, the produced shell
/// line is `source <init> .; <program> <args...>` evaluated with `bash -c`,
/// with every value shell-quoted. The init script receives `.` (i.e. `cwd`,
/// which is already the build dir) as its build-dir argument, matching the
/// usual `source oe-init-build-env .` invocation, and a failed source aborts
/// the command instead of running bitbake outside the Yocto environment.
///
/// `kind` only shapes the digest (whether a failure has a `failed_stage`).
pub async fn run(
    ctx: &ExecContext,
    program: &str,
    args: &[String],
    cwd: &str,
    kind: CommandKind,
    log_slot: &OnceLock<PathBuf>,
) -> ExecResult {
    let (cmd, command, command_line) =
        spawn::command(ctx.init_script.as_deref(), program, args, cwd);
    run_child(cmd, &command, &command_line, ctx, cwd, kind, log_slot).await
}

/// Format the exact command display string without spawning a child.
pub fn display_command(ctx: &ExecContext, program: &str, args: &[String], cwd: &str) -> String {
    spawn::command(ctx.init_script.as_deref(), program, args, cwd).1
}

async fn run_child(
    mut cmd: tokio::process::Command,
    command: &str,
    name: &str,
    ctx: &ExecContext,
    cwd: &str,
    kind: CommandKind,
    log_slot: &OnceLock<PathBuf>,
) -> ExecResult {
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return build_failure(format!("failed to spawn: {e}"), command.to_string()),
    };
    let (Some(child_out), Some(child_err)) = (child.stdout.take(), child.stderr.take()) else {
        return build_failure("failed to read output: pipes missing", command.to_string());
    };

    // stdout streams straight into the log; stderr into a sibling file that is
    // appended after it, so the log keeps its stdout-then-stderr layout while
    // neither stream is ever held whole in memory.
    let mut log = open_log(ctx, name, command, cwd).await;
    if let Some(l) = &log {
        let _ = log_slot.set(l.path.clone());
    }
    let mut err_path = PathBuf::new();
    let mut err_sink = None;
    if let Some(l) = &log {
        err_path = l.path.with_extension("err");
        match tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&err_path)
            .await
        {
            Ok(f) => err_sink = Some(f),
            Err(_) => {
                let _ = tokio::fs::remove_file(&l.path).await;
                log = None;
            }
        }
    }
    let head_cap = if log.is_some() {
        INLINE_CAP + 1
    } else {
        usize::MAX
    };
    let out_sink = log.as_mut().and_then(|l| l.file.take());

    let mut group = GroupKill(child.id().map(|pid| pid as i32));
    let out_path = log.as_ref().map(|l| l.path.clone()).unwrap_or_default();
    let (mut out, err, status) = tokio::join!(
        drain(child_out, out_sink, &out_path, head_cap),
        drain(child_err, err_sink, &err_path, head_cap),
        child.wait()
    );
    // Finished normally: leave the group alone (bitbake's server daemon is
    // meant to outlive the command).
    group.0 = None;
    let status = match status {
        Ok(s) => s,
        Err(e) => {
            return build_failure(format!("failed to read output: {e}"), command.to_string());
        }
    };
    let exit_code = status.code();
    // A stream over the inline cap is not inlined when it reached the log (or,
    // after a log write failure, only partly is): those calls get a digest.
    let want_digest = out.total > INLINE_CAP as u64 || err.total > INLINE_CAP as u64;

    let mut digest = None;
    let mut note = read_notes(&out, &err);
    let mut logged: Option<(String, u64)> = None;
    let (mut out_in_log, mut err_in_log) = (false, false);
    if let Some(mut l) = log {
        // `out.sink` is the log's own handle coming back: if it was dropped the
        // stdout write failed. The log is kept either way — it holds what was
        // written before the failure — and the streams that did not make it
        // into it are returned inline instead.
        out_in_log = out.sink.is_some();
        err_in_log = err.sink.is_some();
        l.file = out.sink.take();
        let mut size = None;
        if l.file.is_some() {
            match finish_log(&mut l, &err_path, exit_code, &out, &err).await {
                Ok(n) => size = Some(n),
                Err(e) => note.push_str(&format!("output log not finished ({e}); ")),
            }
        } else {
            note.push_str("stdout could not be written to the log; ");
        }
        if !err_in_log {
            note.push_str("stderr could not be fully written to the log; ");
        }
        // The scratch file is the only complete copy of stderr until it is in
        // the log: delete it only once the log is finished, otherwise keep it
        // and say where it is.
        let mut kept_err = None;
        if size.is_some() && err_in_log {
            let _ = tokio::fs::remove_file(&err_path).await;
        } else if err.total > 0 {
            note.push_str(&format!("stderr is in {}; ", err_path.display()));
            kept_err = Some(err_path.clone());
        } else {
            let _ = tokio::fs::remove_file(&err_path).await;
        }
        let size = match size {
            Some(n) => n,
            None => tokio::fs::metadata(&l.path)
                .await
                .map(|m| m.len())
                .unwrap_or(0),
        };
        // The digest reads only the `.log`, never the `.err`: on success the
        // log already holds stderr, and an incomplete log names the kept
        // `.err` instead of mixing in bytes the log may partly hold.
        let dir = ctx.log_dir.clone();
        let log_path = l.path.clone();
        if let Some(guard) = &mut l.cleanup {
            guard.paths.clear();
        }
        drop(l.live.take());
        let incomplete = !note.is_empty();
        let live_logs = ctx.live_logs.clone();
        digest = tokio::task::spawn_blocking(move || {
            let d = want_digest
                .then(|| {
                    let mut b = DigestBuilder::new(kind);
                    if incomplete {
                        b.mark_incomplete(kept_err.as_deref());
                    }
                    b.feed_file(&log_path).ok()?;
                    Some(b.finish(exit_code))
                })
                .flatten();
            prune_logs(&dir, LOG_KEEP, &live_logs);
            d
        })
        .await
        .ok()
        .flatten();
        logged = Some((l.path.to_string_lossy().into_owned(), size));
    } else if want_digest {
        // No log file: both streams are inline in full; digest them in the
        // log's own order (stdout, then stderr).
        let (o, e) = (out.head.clone(), err.head.clone());
        let incomplete = !note.is_empty();
        digest = tokio::task::spawn_blocking(move || {
            let mut b = DigestBuilder::new(kind);
            if incomplete {
                b.mark_incomplete(None);
            }
            b.feed(o.as_slice()).ok()?;
            b.feed(e.as_slice()).ok()?;
            Some(b.finish(exit_code))
        })
        .await
        .ok()
        .flatten();
    }
    // The log stays, and so does a stderr scratch file that holds the only
    // complete copy: the guard is for cancellation only.
    let mut stderr = inline(&err, err_in_log, INLINE_CAP);
    if !note.is_empty() {
        let incomplete_thing = if logged.is_some() { "log" } else { "output" };
        let n = format!(
            "\n[bitbake-mcp: {}the {incomplete_thing} is incomplete]\n",
            note
        );
        stderr.get_or_insert_with(String::new).push_str(&n);
    }
    ExecResult {
        ok: status.success(),
        exit_code,
        stdout: inline(&out, out_in_log, INLINE_CAP),
        stderr,
        command: command.to_string(),
        log: logged.as_ref().map(|(path, _)| path.clone()),
        log_bytes: logged.map(|(_, size)| size),
        digest,
    }
}
