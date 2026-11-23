//! The result of one command, as it is serialized into a tool response.

use crate::digest;
use serde::Serialize;

/// Result of running a command.
#[derive(Debug, Serialize)]
pub struct ExecResult {
    pub ok: bool,
    /// Command exit code (0 on success). Absent if the process could not be
    /// spawned.
    pub exit_code: Option<i32>,
    /// Inline stdout, present only while it fits `INLINE_CAP`. An absent key
    /// means the output is in `log` — or that there was none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stdout: Option<String>,
    /// Inline stderr, same rule as `stdout`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stderr: Option<String>,
    /// The effective command line, joined for the user's reference.
    pub command: String,
    /// Log file holding this command's complete output. Absent when nothing
    /// was spawned or when the log could not be written.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log: Option<String>,
    /// Size of that log file in bytes: the "is it worth reading?" signal.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_bytes: Option<u64>,
    /// Summary of the output, present only when a stream was too large to
    /// inline. Derived from the log, which stays the source of truth.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub digest: Option<digest::Digest>,
}

/// A result for a command that never produced output (spawn failed, or its
/// output could not be read): no log file exists, so the reason is the whole
/// result and stays inline.
pub(super) fn build_failure(err: impl std::fmt::Display, command: String) -> ExecResult {
    ExecResult {
        ok: false,
        exit_code: None,
        stdout: None,
        stderr: Some(err.to_string()),
        command,
        log: None,
        log_bytes: None,
        digest: None,
    }
}
