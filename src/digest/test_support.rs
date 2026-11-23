//! Shared fixtures for the digest tests.

use super::{CommandKind, Digest, DigestBuilder};
use std::path::Path;

pub(crate) fn fixture(rel: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(rel)
}

pub(crate) fn digest_lines(kind: CommandKind, exit: Option<i32>, lines: &[&str]) -> Digest {
    let mut b = DigestBuilder::new(kind);
    for l in lines {
        b.push_line(l.as_bytes());
    }
    b.finish(exit)
}

/// Exit code and kind from a fixture's header, as the server would know.
pub(crate) fn header_facts(text: &str) -> (CommandKind, Option<i32>) {
    let cmd = text.lines().next().unwrap_or("");
    let kind = if cmd.contains("bitbake ") || cmd.contains("devtool build") || cmd.contains("devtool modify") {
        CommandKind::Tasks
    } else {
        CommandKind::Plain
    };
    let code = text
        .lines()
        .nth(2)
        .and_then(|l| l.strip_prefix("# exit_code: "))
        .and_then(|c| c.trim().parse().ok());
    (kind, code)
}

pub(crate) const LOGFILE: &str = "ERROR: Logfile of failure stored in: /b/tmp/work/a/lvgl/9.5/temp/log.do_compile.11";
pub(crate) const TASK: &str = "ERROR: Task (/b/meta/lvgl_9.5.bb:do_compile) failed with exit code '1'";
