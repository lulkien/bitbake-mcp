//! Log digest: a compact, high-signal summary of a finished command's log.
//!
//! The log file stays the complete record; the digest is derived from it so a
//! caller can see what failed (task, task log, first compiler error), what
//! warned, and the build's task/sstate/parse counts without opening a
//! multi-megabyte log. Design and evidence: `docs/log-compression-design.md`.
//!
//! The digest depends only on the log bytes, the `CommandKind` and the exit
//! code: no clocks, no hash-map ordering. Families and failures are `Vec`s in
//! first-seen order, so the same input always serializes to the same bytes.

mod caps;
mod classify;
mod failure;
mod text;
mod types;

#[cfg(test)]
mod test_support;

use caps::enforce_cap;
use classify::{Family, FamilyKind, LineClass};
use failure::{Failure, PreError, attach_pre_errors, log_pn, log_task};
use std::collections::VecDeque;
use std::io::BufRead;
use std::path::Path;
use text::{cut_head, cut_tail, strip_bar};

pub use types::{CommandKind, Digest, FailedTask, Lines, Parse, Sstate, Tasks, WarningGroup};

/// Longest quoted string (bytes) in a digest; lines keep their head, paths
/// their tail.
pub const LINE_CAP: usize = 300;
/// Only this much of a line is classified and decoded.
pub const CLASSIFY_SPAN: usize = 4096;
/// Longest line kept by the reader; the rest is skipped up to the next `\n`.
pub const LINE_READ_CAP: usize = 64 * 1024;
const TAIL_LINES: usize = 15;
const FAILED_TASKS_MAX: usize = 4;
const ERRORS_MAX: usize = 5;
const WARNINGS_MAX: usize = 5;

const LOGFILE_MARK: &str = "ERROR: Logfile of failure stored in: ";
const TASK_MARK: &str = "ERROR: Task (";

/// Incremental digest builder: feed it the log's lines in order, then
/// `finish`.
#[derive(Debug)]
pub struct DigestBuilder {
    kind: CommandKind,
    line_no: u64,
    lines: Lines,
    families: Vec<Family>,
    warnings_overflow: u64,
    errors_overflow: u64,
    noise_warnings: u64,
    pending: Vec<Failure>,
    failed: Vec<Failure>,
    /// The first `ERROR: <pkg> <do_X>: ...` line per `(PN, do_X)`.
    pre_errors: Vec<PreError>,
    tail: VecDeque<String>,
    tasks: Option<Tasks>,
    sstate: Option<Sstate>,
    parse: Option<Parse>,
    ran_tasks: bool,
    in_traceback: bool,
    incomplete: bool,
    stderr_file: Option<String>,
}

impl DigestBuilder {
    pub fn new(kind: CommandKind) -> Self {
        Self {
            kind,
            line_no: 0,
            lines: Lines::default(),
            families: Vec::new(),
            warnings_overflow: 0,
            errors_overflow: 0,
            noise_warnings: 0,
            pending: Vec::new(),
            failed: Vec::new(),
            pre_errors: Vec::new(),
            tail: VecDeque::new(),
            tasks: None,
            sstate: None,
            parse: None,
            ran_tasks: false,
            in_traceback: false,
            incomplete: false,
            stderr_file: None,
        }
    }

    /// Mark the log as incomplete (a log write failed). `stderr_file` is the
    /// kept `.err` file holding the stderr the log may lack; it is reported by
    /// path and never read.
    pub fn mark_incomplete(&mut self, stderr_file: Option<&Path>) {
        self.incomplete = true;
        self.stderr_file = stderr_file.map(|p| p.to_string_lossy().into_owned());
    }

    /// Feed every line of `reader`. A line longer than `LINE_READ_CAP` is
    /// pushed with only its first `LINE_READ_CAP` bytes; a final fragment
    /// without `\n` is still pushed.
    pub fn feed<R: BufRead>(&mut self, mut reader: R) -> std::io::Result<()> {
        let mut line: Vec<u8> = Vec::new();
        let mut partial = false;
        loop {
            let buf = match reader.fill_buf() {
                Ok(b) => b,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            };
            if buf.is_empty() {
                if partial {
                    self.push_line(&line);
                }
                return Ok(());
            }
            let (part, used, end) = match buf.iter().position(|&b| b == b'\n') {
                Some(i) => (&buf[..i], i + 1, true),
                None => (buf, buf.len(), false),
            };
            let room = LINE_READ_CAP.saturating_sub(line.len());
            line.extend_from_slice(&part[..room.min(part.len())]);
            reader.consume(used);
            if end {
                self.push_line(&line);
                line.clear();
                partial = false;
            } else {
                partial = true;
            }
        }
    }

    /// Feed every line of the file at `path` through a 64 KiB buffer.
    pub fn feed_file(&mut self, path: &Path) -> std::io::Result<()> {
        let f = std::fs::File::open(path)?;
        self.feed(std::io::BufReader::with_capacity(64 * 1024, f))
    }

    /// Feed one line (without its `\n`).
    pub fn push_line(&mut self, line: &[u8]) {
        self.line_no += 1;
        self.lines.total += 1;
        let span = &line[..line.len().min(CLASSIFY_SPAN)];
        let text = String::from_utf8_lossy(span);
        let text = text.as_ref();
        let class = self.classify(text);
        match class {
            LineClass::Noise => self.lines.noise += 1,
            LineClass::Progress => self.lines.progress += 1,
            LineClass::TaskCmd => self.lines.task_cmd += 1,
            LineClass::TaskOther => self.lines.task_other += 1,
            LineClass::Diag => self.lines.diag += 1,
            LineClass::DiagFailed => self.lines.diag_failed += 1,
            LineClass::TaskFailed => self.lines.task_failed += 1,
            LineClass::Summary => self.lines.summary += 1,
            LineClass::Error => self.lines.error += 1,
            LineClass::Warning => self.lines.warning += 1,
        }
        match class {
            LineClass::Progress => {
                if text.contains("Running task") {
                    self.ran_tasks = true;
                }
            }
            LineClass::Diag => {
                if let Some(f) = self.pending.last_mut() {
                    f.error_lines += 1;
                    if f.first_diag.is_none() {
                        f.first_diag = Some(strip_bar(text).to_string());
                    }
                }
            }
            LineClass::DiagFailed => {
                if let Some(f) = self.pending.last_mut()
                    && f.failed_fallback.is_none()
                {
                    f.failed_fallback = Some(strip_bar(text).to_string());
                }
            }
            // Paths keep their tail, so these few lines are parsed whole
            // (the reader already bounds a line to `LINE_READ_CAP`).
            LineClass::TaskFailed => self.task_failed(&String::from_utf8_lossy(line)),
            LineClass::Summary => self.summary(text),
            LineClass::Error => self.error(text),
            LineClass::Warning => self.warning(text),
            LineClass::Noise | LineClass::TaskCmd | LineClass::TaskOther => {}
        }
        if !matches!(class, LineClass::Noise | LineClass::TaskCmd | LineClass::Progress) {
            if self.tail.len() == TAIL_LINES {
                self.tail.pop_front();
            }
            self.tail.push_back(cut_head(text, LINE_CAP));
        }
    }

    /// Build the digest. `exit_code` is the command's real exit status
    /// (`None`: killed), never the log header's.
    pub fn finish(mut self, exit_code: Option<i32>) -> Digest {
        let outcome = match exit_code {
            Some(0) => "succeeded",
            Some(_) => "failed",
            None => "killed",
        };
        // Rule 5: failures never closed by a Task line (log cut off, killed).
        for mut f in std::mem::take(&mut self.pending) {
            if let Some(log) = f.task_log.as_deref() {
                f.do_x = log_task(log).map(str::to_string);
                f.pn = log_pn(log).map(str::to_string);
                f.task = f.do_x.as_ref().map(|d| match log_pn(log) {
                    Some(pn) => format!("{pn}:{d}"),
                    None => d.clone(),
                });
            }
            self.failed.push(f);
        }

        // Rule 6: first_error rank. A pre-Logfile line used as a cause is
        // consumed and not repeated under `errors`.
        let mut consumed: Vec<String> = Vec::new();
        let mut failed_tasks = Vec::new();
        let failed_tasks_omitted = self.failed.len().saturating_sub(FAILED_TASKS_MAX) as u64;
        self.failed.truncate(FAILED_TASKS_MAX);
        let pre = attach_pre_errors(&self.pre_errors, &self.failed);
        for (f, pre) in self.failed.into_iter().zip(pre) {
            let first_error = if f.first_diag.is_some() {
                f.first_diag
            } else if let Some(p) = pre {
                consumed.push(p.clone());
                Some(p)
            } else {
                f.failed_fallback
            };
            failed_tasks.push(FailedTask {
                task: f.task.map(|t| cut_head(&t, LINE_CAP)),
                recipe_file: f.recipe_file.map(|p| cut_tail(&p, LINE_CAP)),
                task_log: f.task_log.map(|p| cut_tail(&p, LINE_CAP)),
                first_error: first_error.as_deref().map(|e| cut_head(e, LINE_CAP)),
                error_lines: f.error_lines,
                first_error_raw: first_error,
            });
        }

        let mut errors = Vec::new();
        let mut warns: Vec<&Family> = Vec::new();
        for fam in &self.families {
            match fam.kind {
                FamilyKind::Error => {
                    if consumed.iter().any(|c| *c == fam.sample) && fam.count == 1 {
                        continue;
                    }
                    if errors.len() < ERRORS_MAX {
                        errors.push(cut_head(&fam.sample, LINE_CAP));
                    }
                }
                FamilyKind::Warning => warns.push(fam),
            }
        }
        // Stable: equal counts keep first-seen order.
        warns.sort_by(|a, b| b.count.cmp(&a.count));
        let warnings = warns
            .into_iter()
            .take(WARNINGS_MAX)
            .map(|f| WarningGroup { count: f.count, sample: cut_head(&f.sample, LINE_CAP) })
            .collect();

        let failed_stage = (self.kind == CommandKind::Tasks && outcome == "failed")
            .then_some(if self.ran_tasks { "tasks" } else { "parse" });
        let tail = if outcome == "succeeded" { Vec::new() } else { self.tail.into_iter().collect() };

        let mut d = Digest {
            outcome,
            failed_stage,
            failed_tasks,
            failed_tasks_omitted,
            errors,
            errors_overflow: self.errors_overflow,
            warnings,
            warnings_overflow: self.warnings_overflow,
            noise_warnings: self.noise_warnings,
            tasks: self.tasks,
            sstate: self.sstate,
            parse: self.parse,
            tail,
            lines: Some(self.lines),
            incomplete: self.incomplete,
            stderr_file: self.stderr_file.map(|p| cut_tail(&p, LINE_CAP)),
            dropped: Vec::new(),
        };
        enforce_cap(&mut d);
        d
    }
}

/// Digest a finished, complete log file. Blocking: call it through
/// `spawn_blocking`. The server feeds a builder itself so it can mark an
/// incomplete log first; this is the plain form the tests use.
#[cfg(test)]
pub fn digest_file(path: &Path, kind: CommandKind, exit_code: Option<i32>) -> std::io::Result<Digest> {
    let mut b = DigestBuilder::new(kind);
    b.feed_file(path)?;
    Ok(b.finish(exit_code))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::digest::caps::{serialized_len, DIGEST_CAP};
    use crate::digest::test_support::{digest_lines, fixture, header_facts};

    #[test]
    fn failed_stage_by_kind() {
        let d = digest_lines(CommandKind::Tasks, Some(1), &["ERROR: Parsing halted due to errors"]);
        assert_eq!(d.failed_stage, Some("parse"));
        assert_eq!(d.errors, vec!["ERROR: Parsing halted due to errors".to_string()]);
        let d = digest_lines(CommandKind::Plain, Some(1), &["ERROR: deploy failed"]);
        assert_eq!(d.failed_stage, None);
        assert_eq!(d.outcome, "failed");
        let d = digest_lines(CommandKind::Tasks, Some(0), &["NOTE: Running task 1 of 1 (x)"]);
        assert_eq!((d.outcome, d.failed_stage), ("succeeded", None));
        assert!(d.tail.is_empty());
    }

    #[test]
    fn long_line_and_hostile_bytes() {
        let mut data = vec![b'x'; 100 * 1024];
        data.extend_from_slice(b"\nERROR: \xff\xfe\x01\x02 bad\n");
        data.extend_from_slice(&vec![b'y'; 100 * 1024]);
        let mut b = DigestBuilder::new(CommandKind::Plain);
        b.feed(std::io::Cursor::new(&data)).unwrap();
        let d = b.finish(Some(1));
        assert_eq!(d.lines.as_ref().unwrap().total, 3);
        let json = serde_json::to_vec(&d).unwrap();
        assert!(json.len() <= DIGEST_CAP);
        assert!(serde_json::from_slice::<serde_json::Value>(&json).is_ok());
        assert!(d.errors[0].starts_with("ERROR: \u{fffd}\u{fffd}\u{1}\u{2} bad"));
    }

    #[test]
    fn incomplete_log_never_opens_the_err_file() {
        let text = "# command: bitbake x\n# cwd: /b\n# exit_code: -          \n\
                    NOTE: Running task 1 of 1 (/m/x_1.bb:do_compile)\n\
                    ERROR: Logfile of failure stored in: /w/x/1/temp/log.do_compile.7\n\
                    ERROR: Task (/m/x_1.bb:do_compile) failed with exit code '1'\n\
                    ----- stderr -----\nERROR: half a li";
        let mut b = DigestBuilder::new(CommandKind::Tasks);
        b.mark_incomplete(Some(Path::new("/does/not/exist/1-bitbake-x.err")));
        b.feed(text.as_bytes()).unwrap();
        let d = b.finish(Some(1));
        assert_eq!(d.outcome, "failed");
        assert!(d.incomplete);
        assert_eq!(d.stderr_file.as_deref(), Some("/does/not/exist/1-bitbake-x.err"));
        assert_eq!(d.failed_tasks[0].task_log.as_deref(), Some("/w/x/1/temp/log.do_compile.7"));
        assert_eq!(d.errors, vec!["ERROR: half a li".to_string()]);
    }

    #[test]
    fn killed_fixture_does_not_panic() {
        let p = fixture("logs/1792395887863-bitbake-slow.log");
        let d = digest_file(&p, CommandKind::Tasks, None).unwrap();
        assert_eq!(d.outcome, "killed");
        assert!(!d.tail.is_empty() || d.lines.as_ref().unwrap().total > 0);
    }

    #[test]
    fn determinism() {
        let p = fixture("logs/1791941198301-devtool-build-lean-display-server.log");
        let a = serde_json::to_vec(&digest_file(&p, CommandKind::Tasks, Some(1)).unwrap()).unwrap();
        let b = serde_json::to_vec(&digest_file(&p, CommandKind::Tasks, Some(1)).unwrap()).unwrap();
        assert_eq!(a, b);
    }

    /// Golden digests of real logs. `UPDATE_GOLDEN=1 cargo test` rewrites them.
    #[test]
    fn golden_fixtures() {
        let dir = fixture("logs");
        let mut names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".log"))
            .collect();
        names.sort();
        assert_eq!(names.len(), 8, "{names:?}");
        for name in names {
            let log = dir.join(&name);
            let (kind, code) = header_facts(&std::fs::read_to_string(&log).unwrap());
            let d = digest_file(&log, kind, code).unwrap();
            assert!(serialized_len(&d) <= DIGEST_CAP);
            let got = serde_json::to_string_pretty(&d).unwrap() + "\n";
            let golden = dir.join(name.replace(".log", ".digest.json"));
            if std::env::var_os("UPDATE_GOLDEN").is_some() {
                std::fs::write(&golden, &got).unwrap();
            }
            assert_eq!(got, std::fs::read_to_string(&golden).unwrap(), "{name}");
        }
    }

    /// Size distribution over a directory of real logs:
    /// `DIGEST_CORPUS=~/.cache/bitbake-mcp cargo test --release corpus -- --ignored --nocapture`.
    /// `DIGEST_SHOW=<name>,...` also prints those logs' digests.
    #[test]
    #[ignore]
    fn corpus() {
        let dir = std::env::var("DIGEST_CORPUS").expect("set DIGEST_CORPUS");
        let show: Vec<String> = std::env::var("DIGEST_SHOW")
            .unwrap_or_default()
            .split(',')
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        let mut names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".log"))
            .collect();
        names.sort();
        let mut by: Vec<(&str, Vec<(usize, u64, String)>)> =
            vec![("succeeded", vec![]), ("failed", vec![]), ("killed", vec![])];
        let mut largest = (0u64, 0usize, String::new());
        for name in &names {
            let path = Path::new(&dir).join(name);
            let bytes = std::fs::read(&path).unwrap();
            let (kind, code) = header_facts(&String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]));
            let d = digest_file(&path, kind, code).unwrap();
            let n = serialized_len(&d);
            assert!(n <= DIGEST_CAP, "{name}: {n}");
            if let Some(out) = std::env::var_os("DIGEST_DUMP") {
                std::fs::write(Path::new(&out).join(format!("{name}.digest.json")), serde_json::to_vec(&d).unwrap()).unwrap();
            }
            if bytes.len() as u64 > largest.0 {
                largest = (bytes.len() as u64, n, name.clone());
            }
            if show.iter().any(|s| name.starts_with(s.as_str())) {
                println!("== {name}\n{}", serde_json::to_string_pretty(&d).unwrap());
            }
            let slot = by.iter_mut().find(|(o, _)| *o == d.outcome).unwrap();
            slot.1.push((n, bytes.len() as u64, d.dropped.join(",")));
        }
        println!("logs: {}  DIGEST_CAP: {DIGEST_CAP}", names.len());
        println!("| Outcome | Logs | Log bytes p50 / max | Digest bytes p50 / p90 / max | Hit drop order |");
        println!("|---|---:|---|---|---:|");
        for (outcome, rows) in &by {
            if rows.is_empty() {
                continue;
            }
            let mut sizes: Vec<usize> = rows.iter().map(|r| r.0).collect();
            let mut logb: Vec<u64> = rows.iter().map(|r| r.1).collect();
            sizes.sort();
            logb.sort();
            let pct = |v: &[usize], p: usize| v[(v.len() - 1) * p / 100];
            println!(
                "| {outcome} | {} | {} / {} | {} / {} / {} | {} |",
                rows.len(),
                logb[(logb.len() - 1) / 2],
                logb[logb.len() - 1],
                pct(&sizes, 50),
                pct(&sizes, 90),
                sizes[sizes.len() - 1],
                rows.iter().filter(|r| !r.2.is_empty()).count()
            );
        }
        println!("largest log: {} ({} B) -> digest {} B", largest.2, largest.0, largest.1);
    }
}
