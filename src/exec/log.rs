//! The output log: creating the file, streaming a command's output into it,
//! finishing its header, and keeping the log directory bounded.

use super::ExecContext;
use std::path::{Path, PathBuf};

/// Log files kept in `ExecContext::log_dir`; the oldest are removed after a
/// write so a long-lived server cannot grow that directory without bound.
pub(super) const LOG_KEEP: usize = 200;

/// Milliseconds since the epoch, fixed width, so log file names sort
/// chronologically as plain strings — `prune_logs` relies on that.
fn log_stamp() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// A command line reduced to a file-name-safe slug, e.g.
/// `bitbake -c compile zlib` -> `bitbake--c-compile-zlib`.
fn slug(command: &str) -> String {
    let mut out = String::new();
    for c in command.chars().take(60) {
        out.push(if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
            c
        } else {
            '-'
        });
    }
    out.trim_matches('-').to_string()
}

/// Width of the exit-code field in a log header: long enough for any `i32`.
/// The code is unknown until the command ends but the header precedes the
/// streamed output, so the field is reserved up front and filled in after.
const EXIT_FIELD: usize = 11;

/// An open log file receiving a command's output as it is produced.
pub(super) struct LogFile {
    pub(super) path: PathBuf,
    /// Taken while a stream is being drained into it, then put back.
    pub(super) file: Option<tokio::fs::File>,
    /// Byte offset of the reserved exit-code field.
    pub(super) exit_offset: u64,
}

/// Create a fresh, never-before-used log file in `ctx.log_dir` and write its
/// header. `None` when there is no log dir or it cannot be written: the caller
/// then keeps the output inline, because losing a build's output is worse than
/// a large result.
///
/// `create_new` makes the name a reservation: two calls with the same command
/// in the same millisecond get distinct files (`-1`, `-2`, ...), never one
/// file truncated by the other.
pub(super) async fn open_log(ctx: &ExecContext, name: &str, command: &str, cwd: &str) -> Option<LogFile> {
    use tokio::io::AsyncWriteExt;
    if ctx.log_dir.as_os_str().is_empty() {
        return None;
    }
    tokio::fs::create_dir_all(&ctx.log_dir).await.ok()?;
    let stem = format!("{}-{}", log_stamp(), slug(name));
    let mut attempt = 0u32;
    let (path, mut file) = loop {
        let file_name = if attempt == 0 {
            format!("{stem}.log")
        } else {
            format!("{stem}-{attempt}.log")
        };
        let path = ctx.log_dir.join(file_name);
        match tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .await
        {
            Ok(f) => break (path, f),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && attempt < 10_000 => {
                attempt += 1;
            }
            Err(_) => return None,
        }
    };
    let head = format!("# command: {command}\n# cwd: {cwd}\n# exit_code: ");
    let exit_offset = head.len() as u64;
    let header = format!("{head}{:<EXIT_FIELD$}\n", "-");
    if file.write_all(header.as_bytes()).await.is_err() {
        let _ = tokio::fs::remove_file(&path).await;
        return None;
    }
    Some(LogFile { path, file: Some(file), exit_offset })
}

/// What was seen of one output stream.
pub(super) struct Capture {
    /// Leading bytes kept in memory: everything, or only enough to decide
    /// whether the stream fits `INLINE_CAP` when it is also going to a log.
    pub(super) head: Vec<u8>,
    pub(super) total: u64,
    pub(super) last: Option<u8>,
    /// The sink, if it is still healthy at the end.
    pub(super) sink: Option<tokio::fs::File>,
}

/// Copy `reader` to `sink` (when given) until EOF, keeping the first
/// `head_cap` bytes in memory. If the sink (the file at `sink_path`) fails
/// mid-stream it is dropped and the stream is kept in memory from then on; the
/// bytes between the kept head and the failure point are in that file, so the
/// kept text names the gap and the file instead of silently skipping it.
pub(super) async fn drain<R: tokio::io::AsyncRead + Unpin>(
    mut reader: R,
    mut sink: Option<tokio::fs::File>,
    sink_path: &Path,
    mut head_cap: usize,
) -> Capture {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut cap = Capture { head: Vec::new(), total: 0, last: None, sink: None };
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = match reader.read(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        let chunk = &buf[..n];
        // tokio's File completes a write in the background and reports its
        // error on the next operation, so without the flush a failure of the
        // last chunk would go unseen and the sink would count as healthy.
        if let Some(f) = sink.as_mut()
            && (f.write_all(chunk).await.is_err() || f.flush().await.is_err())
        {
            sink = None;
            head_cap = usize::MAX;
            let missing = cap.total - cap.head.len() as u64;
            if missing > 0 {
                cap.head.extend_from_slice(gap_marker(missing, sink_path).as_bytes());
            }
        }
        let room = head_cap.saturating_sub(cap.head.len());
        cap.head.extend_from_slice(&chunk[..room.min(n)]);
        cap.total += n as u64;
        cap.last = chunk.last().copied();
    }
    cap.sink = sink;
    cap
}

/// Text standing in for `missing` bytes that are only in the file `path`.
fn gap_marker(missing: u64, path: &Path) -> String {
    format!(
        "\n[mcp-yocto: {missing} bytes omitted here; they are in {}]\n",
        path.display()
    )
}

/// One stream as it appears in a result: `None` (key omitted) when empty —
/// nothing to read — or, with a log, longer than `cap` (it is only in the log).
pub(super) fn inline(c: &Capture, in_log: bool, cap: usize) -> Option<String> {
    if c.total == 0 || (in_log && c.total > cap as u64) {
        return None;
    }
    Some(String::from_utf8_lossy(&c.head).into_owned())
}

/// Fill in the exit code, then append stderr (held in `err_path`) after the
/// stdout already in the log. Returns the log's final size.
pub(super) async fn finish_log(
    log: &mut LogFile,
    err_path: &Path,
    exit_code: Option<i32>,
    out: &Capture,
    err: &Capture,
) -> std::io::Result<u64> {
    use std::io::SeekFrom;
    use tokio::io::{AsyncSeekExt, AsyncWriteExt};
    let Some(file) = log.file.as_mut() else {
        return Err(std::io::Error::other("log file missing"));
    };
    let code = exit_code.map(|c| c.to_string()).unwrap_or_else(|| "-".into());
    file.seek(SeekFrom::Start(log.exit_offset)).await?;
    file.write_all(format!("{code:<EXIT_FIELD$}").as_bytes()).await?;
    file.seek(SeekFrom::End(0)).await?;
    if out.total > 0 && out.last != Some(b'\n') {
        file.write_all(b"\n").await?;
    }
    if err.total > 0 {
        file.write_all(b"----- stderr -----\n").await?;
        let mut src = tokio::fs::File::open(err_path).await?;
        tokio::io::copy(&mut src, file).await?;
        if err.last != Some(b'\n') {
            file.write_all(b"\n").await?;
        }
    }
    file.flush().await?;
    Ok(file.metadata().await?.len())
}

/// Keep the newest `keep` log files in `dir` and delete the rest. Names start
/// with a fixed-width timestamp, so a lexicographic sort is chronological.
/// Blocking filesystem work: call it through `spawn_blocking`.
pub(super) fn prune_logs(dir: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".log") || n.ends_with(".err"))
        .collect();
    if names.len() <= keep {
        return;
    }
    names.sort();
    for name in &names[..names.len() - keep] {
        let _ = std::fs::remove_file(dir.join(name));
    }
}

/// Removes a call's log files if the call is dropped (cancelled) before it
/// finishes, so an abandoned request leaves no half-written log behind.
pub(super) struct LogGuard {
    pub(super) paths: Vec<PathBuf>,
}

impl Drop for LogGuard {
    fn drop(&mut self) {
        for p in &self.paths {
            let _ = std::fs::remove_file(p);
        }
    }
}
