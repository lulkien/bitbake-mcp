//! Size caps: the serialized-size cap, its 10-step drop order (design §3.4)
//! and the last-resort shrink.

use super::classify::{diag_split, split_linecol};
use super::text::{cut_head, cut_tail, strip_bar};
use super::types::Digest;

/// Serialized size cap of the whole digest (compact JSON, UTF-8 bytes).
pub const DIGEST_CAP: usize = 4096;
/// Cap for each `failed_tasks[0]` string in the last-resort drop step.
pub const SHRINK_CAP: usize = 120;
/// The location part of a shrunk compiler error keeps at most this much.
const SHRINK_LOC_CAP: usize = 60;

/// Serialized size of a digest in bytes.
pub fn serialized_len(d: &Digest) -> usize {
    serde_json::to_vec(d).map(|v| v.len()).unwrap_or(usize::MAX)
}

/// Apply the drop order (design §3.4) until the digest fits `DIGEST_CAP`.
/// `outcome`, `failed_tasks[0]`, `tasks`, `incomplete` and `stderr_file` are
/// never dropped.
pub(super) fn enforce_cap(d: &mut Digest) {
    type Step = (&'static str, fn(&mut Digest));
    const STEPS: [Step; 10] = [
        ("tail", |d| d.tail.clear()),
        ("lines", |d| d.lines = None),
        ("warnings[2..]", |d| d.warnings.truncate(2)),
        ("errors[2..]", |d| d.errors.truncate(2)),
        ("failed_tasks[1..]", |d| {
            d.failed_tasks_omitted += d.failed_tasks.len().saturating_sub(1) as u64;
            d.failed_tasks.truncate(1)
        }),
        ("sstate", |d| d.sstate = None),
        ("parse", |d| d.parse = None),
        ("warnings", |d| d.warnings.clear()),
        ("errors", |d| d.errors.clear()),
        ("shrink", shrink_last_resort),
    ];
    for (name, step) in STEPS {
        if serialized_len(d) <= DIGEST_CAP {
            return;
        }
        step(d);
        d.dropped.push(name);
    }
}

/// Step 10: every string in `failed_tasks[0]` to `SHRINK_CAP`, and
/// `stderr_file` to its file name.
pub(super) fn shrink_last_resort(d: &mut Digest) {
    if let Some(f) = d.failed_tasks.first_mut() {
        f.task = f.task.as_deref().map(|s| cut_head(s, SHRINK_CAP));
        f.recipe_file = f.recipe_file.as_deref().map(|s| cut_tail(s, SHRINK_CAP));
        f.task_log = f.task_log.as_deref().map(|s| cut_tail(s, SHRINK_CAP));
        let raw = f.first_error_raw.take().or(f.first_error.take());
        f.first_error = raw.as_deref().map(shrink);
    }
    if let Some(p) = d.stderr_file.as_mut() {
        let name = p.rsplit('/').next().unwrap_or(p).to_string();
        *p = cut_tail(&name, SHRINK_CAP);
    }
}

/// Shrink a quoted line to `SHRINK_CAP` bytes. A compiler diagnostic keeps
/// `<last 2 path components>:<L>:<C>: error: <msg>` so `error:` survives a
/// long absolute path; any other line keeps its head.
pub fn shrink(line: &str) -> String {
    let s = strip_bar(line);
    let Some((loc, kind, msg)) = diag_split(s) else {
        return cut_head(s, SHRINK_CAP);
    };
    let (path, linecol) = split_linecol(loc);
    let comps: Vec<&str> = path.rsplit('/').take(2).collect();
    let short = comps.into_iter().rev().collect::<Vec<_>>().join("/");
    let head = cut_tail(&format!("{short}{linecol}: {kind}: "), SHRINK_LOC_CAP);
    cut_head(&format!("{head}{msg}"), SHRINK_CAP)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::digest::test_support::{TASK, digest_lines, fixture};
    use crate::digest::{CommandKind, DigestBuilder, FailedTask, Tasks, FAILED_TASKS_MAX, LINE_CAP, LOGFILE_MARK};
    use crate::digest::text::{ELLIPSIS, strip_bar};
    use std::path::Path;

    /// Six failures: four listed, two counted; the drop order keeps counting.
    #[test]
    fn failed_tasks_over_cap_are_counted() {
        let mut lines = Vec::new();
        for i in 0..6 {
            lines.push(format!("ERROR: Task (/m/r{i}_1.bb:do_compile) failed with exit code '1'"));
        }
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let d = digest_lines(CommandKind::Tasks, Some(1), &refs);
        assert_eq!(d.failed_tasks.len(), FAILED_TASKS_MAX);
        assert_eq!(d.failed_tasks_omitted, 2);
        assert_eq!(d.failed_tasks[3].task.as_deref(), Some("r3:do_compile"));

        // The same six forced through drop step 5: one listed, five counted.
        let pad = "\u{1}".repeat(280);
        let mut lines = Vec::new();
        for i in 0..6 {
            lines.push(format!("{LOGFILE_MARK}/w/r{i}/1/temp/log.do_compile.{i}"));
            lines.push(format!("| /s/f{i}.c:1:2: error: {pad}"));
            lines.push(format!("ERROR: Task (/m/r{i}_1.bb:do_compile) failed with exit code '1'"));
        }
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let d = digest_lines(CommandKind::Tasks, Some(1), &refs);
        assert!(d.dropped.contains(&"failed_tasks[1..]"), "{:?}", d.dropped);
        assert_eq!((d.failed_tasks.len(), d.failed_tasks_omitted), (1, 5));
        assert!(serialized_len(&d) <= DIGEST_CAP);

        let d = digest_lines(CommandKind::Tasks, Some(1), &[TASK]);
        assert_eq!(d.failed_tasks_omitted, 0);
        assert!(!serde_json::to_string(&d).unwrap().contains("failed_tasks_omitted"));
    }

    #[test]
    fn caps_on_paths_and_lines() {
        let long = format!("/{}/temp/log.do_compile.42", "d".repeat(30_000));
        let d = digest_lines(CommandKind::Tasks, Some(1), &[&format!("{LOGFILE_MARK}{long}")]);
        let log = d.failed_tasks[0].task_log.as_deref().unwrap();
        assert!(log.len() <= LINE_CAP && log.ends_with("/log.do_compile.42"), "{log}");
        assert!(serialized_len(&d) <= DIGEST_CAP);

        let e = format!("ERROR: {}", "é".repeat(400));
        let d = digest_lines(CommandKind::Plain, Some(1), &[&e]);
        assert!(d.errors[0].len() <= LINE_CAP && d.errors[0].ends_with(ELLIPSIS));
    }

    /// 50 failed tasks, 200 warning families, 20 errors, every string padded
    /// with 400 control bytes: the whole drop order runs, the result fits.
    #[test]
    fn synthetic_worst_case_runs_whole_drop_order() {
        let pad = "\u{1}".repeat(400);
        let mut b = DigestBuilder::new(CommandKind::Tasks);
        b.mark_incomplete(Some(Path::new(&format!("/c/{pad}/1792-bitbake-x.err"))));
        b.push_line(b"NOTE: Running task 1 of 9 (x)");
        for i in 0..50 {
            b.push_line(format!("{LOGFILE_MARK}/w{pad}/r{i}/1/temp/log.do_compile.{i}").as_bytes());
            b.push_line(format!("| /s{pad}/f{i}.c:1:2: error: {pad}").as_bytes());
            b.push_line(format!("ERROR: Task (/m{pad}/r{i}_1.bb:do_compile) failed with exit code '1'").as_bytes());
        }
        for i in 0..200 {
            b.push_line(format!("WARNING: w{}{pad}", "q".repeat(i + 1)).as_bytes());
        }
        for i in 0..20 {
            b.push_line(format!("ERROR: e{}{pad}", "z".repeat(i + 1)).as_bytes());
        }
        b.push_line(b"Sstate summary: Wanted 1 Local 1 Mirrors 0 Missed 0 Current 1 (100% match, 100% complete)");
        b.push_line(b"Parsing of 1 .bb files complete (1 cached, 0 parsed). 1 targets, 0 skipped, 0 masked, 0 errors.");
        let d = b.finish(Some(1));
        let n = serialized_len(&d);
        assert!(n <= DIGEST_CAP, "{n}");
        assert_eq!(d.dropped, vec![
            "tail", "lines", "warnings[2..]", "errors[2..]", "failed_tasks[1..]",
            "sstate", "parse", "warnings", "errors", "shrink",
        ]);
        assert!(d.incomplete && d.stderr_file.as_deref() == Some("1792-bitbake-x.err"));
        assert_eq!(d.failed_tasks.len(), 1);
        assert_eq!(d.failed_tasks_omitted, 49, "46 over the cap + 3 cut by the drop order");
        let first = d.failed_tasks[0].first_error.as_deref().unwrap();
        assert!(first.len() <= SHRINK_CAP && first.contains(": error: "), "{first}");
    }

    /// Every counter huge, every string 120 control bytes: still under the cap.
    #[test]
    fn bound_after_last_resort() {
        let s = "\u{1}".repeat(SHRINK_CAP);
        let mut d = Digest {
            outcome: "failed",
            failed_stage: Some("parse"),
            failed_tasks: vec![FailedTask {
                task: Some(s.clone()),
                recipe_file: Some(s.clone()),
                task_log: Some(s.clone()),
                first_error: Some(s.clone()),
                error_lines: 999_999_999,
                ..FailedTask::default()
            }],
            errors_overflow: 999_999_999,
            warnings_overflow: 999_999_999,
            noise_warnings: 999_999_999,
            tasks: Some(Tasks { attempted: 999_999_999, not_rerun: 999_999_999, failed: 999_999_999 }),
            incomplete: true,
            stderr_file: Some(format!("1792395887863-{}.err", "a".repeat(60))),
            dropped: vec![
                "tail", "lines", "warnings[2..]", "errors[2..]", "failed_tasks[1..]",
                "sstate", "parse", "warnings", "errors", "shrink",
            ],
            ..Digest::default()
        };
        shrink_last_resort(&mut d);
        assert!(serialized_len(&d) <= DIGEST_CAP, "{}", serialized_len(&d));
    }

    #[test]
    fn shrink_pinned_cases() {
        let raw = "| /mnt/external/01/kien1.luu/atlas/workspace/sources/lvgl-atlas-lean/atlas-samples/lvgl-overlay/lvgl-overlay.c:575:9: error: call to undeclared function 'stat'; ISO C99 and later do not support implicit function declarations [-Wimplicit-function-declaration]";
        assert_eq!(
            shrink(raw),
            "lvgl-overlay/lvgl-overlay.c:575:9: error: call to undeclared function 'stat'; ISO C99 and later do not support implic…"
        );
        let long_name = format!("| /b/dir/{}.c:1:2: error: short message", "n".repeat(200));
        let s = shrink(&long_name);
        assert!(s.len() <= SHRINK_CAP && s.contains("error: short message"), "{s}");
        assert_eq!(shrink("ERROR: plain"), "ERROR: plain");
    }

    #[test]
    fn shrink_corpus() {
        let text = std::fs::read_to_string(fixture("first_errors.txt")).unwrap();
        let mut n = 0;
        for line in text.lines().filter(|l| !l.is_empty()) {
            n += 1;
            let s = shrink(line);
            let (_, _, msg) = diag_split(strip_bar(line)).unwrap();
            assert!(s.len() <= SHRINK_CAP, "{s}");
            assert!(s.contains("error:"), "{s}");
            let kept = s.split_once("error: ").map_or("", |(_, m)| m.trim_end_matches(ELLIPSIS));
            assert!(kept == msg || kept.len() >= 40, "{s}");
            if line.contains("ldsdbusadaptor_test.cpp:51:25") {
                assert!(s.ends_with("array type 'int[2]' is not assignable"), "{s}");
            }
        }
        assert_eq!(n, 18);
    }
}
