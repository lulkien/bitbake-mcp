//! Task failures (design §3.2): pairing `Logfile` and `Task` lines, the
//! task's own pre-`Logfile` `ERROR:` line, and the `first_error` ranking.

use super::classify::FamilyKind;
use super::{DigestBuilder, LOGFILE_MARK, TASK_MARK};

/// A task failure being assembled (design §3.2). Strings are raw (bounded by
/// `CLASSIFY_SPAN`); caps are applied when the digest is finished.
#[derive(Debug, Default)]
pub(super) struct Failure {
    pub(super) task: Option<String>,
    /// Recipe name, from the recipe file (or the task log path at EOF).
    pub(super) pn: Option<String>,
    pub(super) do_x: Option<String>,
    pub(super) recipe_file: Option<String>,
    pub(super) task_log: Option<String>,
    /// Rank 1: the first compiler `error:` / `fatal error:` in its log block.
    pub(super) first_diag: Option<String>,
    /// Rank 3: the first `| FAILED: <target>`.
    pub(super) failed_fallback: Option<String>,
    pub(super) error_lines: u64,
}

/// A task's own `ERROR:` line seen before its Logfile line (design §3.2
/// rule 1), keyed by recipe and task so two recipes failing the same task
/// each keep their own line.
#[derive(Debug)]
pub(super) struct PreError {
    pub(super) pn: String,
    pub(super) do_x: String,
    pub(super) line: String,
}

impl DigestBuilder {
    pub(super) fn task_failed(&mut self, text: &str) {
        if let Some(i) = text.find(LOGFILE_MARK) {
            let path = text[i + LOGFILE_MARK.len()..].trim();
            self.pending.push(Failure { task_log: Some(path.to_string()), ..Failure::default() });
            return;
        }
        let Some(i) = text.find(TASK_MARK) else { return };
        let rest = &text[i + TASK_MARK.len()..];
        let inner = rest.find(") failed").map_or(rest, |j| &rest[..j]);
        let (recipe_file, do_x) = match inner.rfind(':') {
            Some(j) => (&inner[..j], &inner[j + 1..]),
            None => (inner, ""),
        };
        let pn = recipe_pn(recipe_file);
        let log_mark = format!("/log.{do_x}.");
        let pn_mark = format!("/{pn}/");
        let pos = self
            .pending
            .iter()
            .position(|f| {
                f.task_log
                    .as_deref()
                    .is_some_and(|l| l.contains(&log_mark) && l.contains(&pn_mark))
            })
            .or(if self.pending.is_empty() { None } else { Some(0) });
        let mut f = match pos {
            Some(p) => self.pending.remove(p),
            None => Failure::default(),
        };
        f.task = Some(format!("{pn}:{do_x}"));
        f.pn = Some(pn.to_string());
        f.do_x = Some(do_x.to_string());
        f.recipe_file = Some(recipe_file.to_string());
        self.failed.push(f);
    }

    pub(super) fn error(&mut self, text: &str) {
        let Some(i) = text.find("ERROR:") else {
            return;
        };
        let msg = &text[i..];
        if let Some((pkg, do_x, body)) = task_error(msg) {
            if body.starts_with("ExecutionError(") {
                return;
            }
            let pn = pkg_pn(pkg);
            if !self.pre_errors.iter().any(|p| p.pn == pn && p.do_x == do_x) {
                self.pre_errors.push(PreError {
                    pn: pn.to_string(),
                    do_x: do_x.to_string(),
                    line: msg.to_string(),
                });
            }
        }
        self.add_family(FamilyKind::Error, msg);
    }
}

/// `ERROR: <pkg> <do_X>: <body>` -> `(pkg, do_X, body)`.
pub(super) fn task_error(msg: &str) -> Option<(&str, &str, &str)> {
    let rest = msg.strip_prefix("ERROR: ")?;
    let (pkg, rest) = rest.split_once(' ')?;
    let (task, body) = rest.split_once(": ")?;
    (task.starts_with("do_") && !task.contains(' ')).then_some((pkg, task, body))
}

/// `PN` of a `PN-PV-PR` package string: the string minus its last two `-`
/// fields (`lvgl-atlas-lean-9.5.0-r3.0` -> `lvgl-atlas-lean`); the string
/// itself when it has fewer.
pub(super) fn pkg_pn(pkg: &str) -> &str {
    let mut parts = pkg.rsplitn(3, '-');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(_pr), Some(_pv), Some(pn)) if !pn.is_empty() => pn,
        _ => pkg,
    }
}

/// Rank-2 cause of each failure (design §3.2 rule 6): the `pre_errors` entry
/// with the same `(PN, do_X)`. A failure with no such entry falls back to the
/// only entry for its `do_X`, when exactly one exists and no other failure
/// has it already.
pub(super) fn attach_pre_errors(pre: &[PreError], failed: &[Failure]) -> Vec<Option<String>> {
    let exact: Vec<Option<usize>> = failed
        .iter()
        .map(|f| {
            let (pn, do_x) = (f.pn.as_deref()?, f.do_x.as_deref()?);
            pre.iter().position(|p| p.pn == pn && p.do_x == do_x)
        })
        .collect();
    let mut taken: Vec<usize> = exact.iter().flatten().copied().collect();
    let mut out = Vec::new();
    for (f, hit) in failed.iter().zip(exact) {
        let hit = hit.or_else(|| {
            let do_x = f.do_x.as_deref()?;
            let mut same = pre.iter().enumerate().filter(|(_, p)| p.do_x == do_x);
            match (same.next(), same.next()) {
                (Some((i, _)), None) if !taken.contains(&i) => {
                    taken.push(i);
                    Some(i)
                }
                _ => None,
            }
        });
        out.push(hit.map(|i| pre[i].line.clone()));
    }
    out
}

/// Recipe name from a recipe file: the file stem before `_` (`zlib_1.3.bb` ->
/// `zlib`).
pub(super) fn recipe_pn(recipe_file: &str) -> &str {
    let name = recipe_file.rsplit('/').next().unwrap_or(recipe_file);
    let stem = name.strip_suffix(".bb").unwrap_or(name);
    stem.split('_').next().unwrap_or(stem)
}

/// `do_X` from `.../temp/log.do_X.<pid>`.
pub(super) fn log_task(log: &str) -> Option<&str> {
    let name = log.rsplit('/').next()?;
    let rest = name.strip_prefix("log.")?;
    let task = rest.split('.').next()?;
    task.starts_with("do_").then_some(task)
}

/// `PN` from `.../<PN>/<PV>/temp/log.do_X.<pid>`.
pub(super) fn log_pn(log: &str) -> Option<&str> {
    let comps: Vec<&str> = log.split('/').collect();
    let n = comps.len();
    (n >= 4 && comps[n - 2] == "temp").then(|| comps[n - 4]).filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use crate::digest::test_support::{LOGFILE, TASK, digest_lines};
    use crate::digest::CommandKind;
    use super::pkg_pn;

    #[test]
    fn event_order_prefers_compiler_error() {
        let d = digest_lines(CommandKind::Tasks, Some(1), &[
            "NOTE: Running task 1 of 1 (/b/meta/lvgl_9.5.bb:do_compile)",
            "ERROR: lvgl-9.5-r0 do_compile: ExecutionError('/b/run.do_compile.11', 1, None, None)",
            LOGFILE,
            "Log data follows:",
            "| FAILED: obj/a.o",
            "| /src/a.c:5:9: error: first",
            "| /src/a.c:6:9: error: second",
            "| ninja: build stopped: subcommand failed.",
            TASK,
        ]);
        let f = &d.failed_tasks[0];
        assert_eq!(f.task.as_deref(), Some("lvgl:do_compile"));
        assert_eq!(f.recipe_file.as_deref(), Some("/b/meta/lvgl_9.5.bb"));
        assert_eq!(f.task_log.as_deref(), Some("/b/tmp/work/a/lvgl/9.5/temp/log.do_compile.11"));
        assert_eq!(f.first_error.as_deref(), Some("/src/a.c:5:9: error: first"));
        assert_eq!(f.error_lines, 2);
        assert_eq!(d.failed_stage, Some("tasks"));
        assert!(d.errors.is_empty(), "ExecutionError is a wrapper: {:?}", d.errors);
    }

    #[test]
    fn rank_two_and_three() {
        let qa = "ERROR: lvgl-9.5-r0 do_package: QA Issue: installed but not shipped";
        let d = digest_lines(CommandKind::Tasks, Some(1), &[
            qa,
            "ERROR: Logfile of failure stored in: /b/lvgl/9.5/temp/log.do_package.3",
            "ERROR: Task (/b/meta/lvgl_9.5.bb:do_package) failed with exit code '1'",
        ]);
        assert_eq!(d.failed_tasks[0].first_error.as_deref(), Some(qa));
        assert!(d.errors.is_empty(), "consumed cause is not repeated: {:?}", d.errors);

        let d = digest_lines(CommandKind::Tasks, Some(1), &[LOGFILE, "| FAILED: obj/a.o", "| ninja: build stopped: x", TASK]);
        assert_eq!(d.failed_tasks[0].first_error.as_deref(), Some("FAILED: obj/a.o"));
    }

    /// Two recipes failing the same task: each keeps its own pre-Logfile
    /// line, and neither line is repeated under `errors`.
    #[test]
    fn pre_errors_pair_by_recipe_and_task() {
        let d = digest_lines(CommandKind::Tasks, Some(1), &[
            "NOTE: Running task 1 of 2 (/m/a_1.0.bb:do_package)",
            "ERROR: a-1.0-r0 do_package: QA A-issue",
            "ERROR: b-1.0-r0 do_package: QA B-issue",
            "ERROR: Logfile of failure stored in: /w/a/1.0/temp/log.do_package.1",
            "ERROR: Task (/m/a_1.0.bb:do_package) failed with exit code '1'",
            "ERROR: Logfile of failure stored in: /w/b/1.0/temp/log.do_package.2",
            "ERROR: Task (/m/b_1.0.bb:do_package) failed with exit code '1'",
        ]);
        let got: Vec<_> = d
            .failed_tasks
            .iter()
            .map(|f| (f.task.as_deref().unwrap(), f.first_error.as_deref().unwrap()))
            .collect();
        assert_eq!(got, vec![
            ("a:do_package", "ERROR: a-1.0-r0 do_package: QA A-issue"),
            ("b:do_package", "ERROR: b-1.0-r0 do_package: QA B-issue"),
        ]);
        assert!(d.errors.is_empty(), "{:?}", d.errors);

        // Reversed Task order, and b's failure never closed (killed): the
        // pairing still follows the recipe, not the order.
        let d = digest_lines(CommandKind::Tasks, None, &[
            "ERROR: b-1.0-r0 do_package: QA B-issue",
            "ERROR: a-1.0-r0 do_package: QA A-issue",
            "ERROR: Logfile of failure stored in: /w/a/1.0/temp/log.do_package.1",
            "ERROR: Task (/m/a_1.0.bb:do_package) failed with exit code '1'",
            "ERROR: Logfile of failure stored in: /w/b/1.0/temp/log.do_package.2",
        ]);
        let got: Vec<_> = d.failed_tasks.iter().map(|f| f.first_error.as_deref().unwrap()).collect();
        assert_eq!(got, vec!["ERROR: a-1.0-r0 do_package: QA A-issue", "ERROR: b-1.0-r0 do_package: QA B-issue"]);

        // B lines first, Task b before Task a: same pairing.
        let d = digest_lines(CommandKind::Tasks, Some(1), &[
            "ERROR: b-1.0-r0 do_package: QA B-issue",
            "ERROR: a-1.0-r0 do_package: QA A-issue",
            "ERROR: Logfile of failure stored in: /w/b/1.0/temp/log.do_package.2",
            "ERROR: Task (/m/b_1.0.bb:do_package) failed with exit code '1'",
            "ERROR: Logfile of failure stored in: /w/a/1.0/temp/log.do_package.1",
            "ERROR: Task (/m/a_1.0.bb:do_package) failed with exit code '1'",
        ]);
        let got: Vec<_> = d
            .failed_tasks
            .iter()
            .map(|f| (f.task.as_deref().unwrap(), f.first_error.as_deref().unwrap()))
            .collect();
        assert_eq!(got, vec![
            ("b:do_package", "ERROR: b-1.0-r0 do_package: QA B-issue"),
            ("a:do_package", "ERROR: a-1.0-r0 do_package: QA A-issue"),
        ]);
        assert!(d.errors.is_empty(), "{:?}", d.errors);
    }

    #[test]
    fn pkg_pn_strips_version_and_revision() {
        assert_eq!(pkg_pn("lvgl-atlas-lean-9.5.0-r3.0"), "lvgl-atlas-lean");
        assert_eq!(pkg_pn("lvgl-9.5-r0"), "lvgl");
        assert_eq!(pkg_pn("weird"), "weird");
    }

    /// PN is compared for equality: `lvgl` does not take `lvgl-atlas-lean`'s
    /// line, and a do_X-only fallback is not handed out twice.
    #[test]
    fn pre_errors_fallback_is_exclusive() {
        let d = digest_lines(CommandKind::Tasks, Some(1), &[
            "ERROR: lvgl-atlas-lean-9.5.0-r3.0 do_package: QA lean-issue",
            "ERROR: Task (/m/lvgl_9.5.bb:do_package) failed with exit code '1'",
            "ERROR: Task (/m/lvgl-atlas-lean_9.5.0.bb:do_package) failed with exit code '1'",
        ]);
        let got: Vec<_> = d.failed_tasks.iter().map(|f| f.first_error.as_deref()).collect();
        assert_eq!(got, vec![None, Some("ERROR: lvgl-atlas-lean-9.5.0-r3.0 do_package: QA lean-issue")]);

        // Package named differently from the recipe: the single entry is the
        // fallback for the first failure only.
        let d = digest_lines(CommandKind::Tasks, Some(1), &[
            "ERROR: other-1-r0 do_install: odd",
            "ERROR: Task (/m/x_1.bb:do_install) failed with exit code '1'",
            "ERROR: Task (/m/y_1.bb:do_install) failed with exit code '1'",
        ]);
        let got: Vec<_> = d.failed_tasks.iter().map(|f| f.first_error.as_deref()).collect();
        assert_eq!(got, vec![Some("ERROR: other-1-r0 do_install: odd"), None]);
        assert!(d.errors.is_empty());
    }

    #[test]
    fn keep_going_pairs_by_task_and_pn() {
        let d = digest_lines(CommandKind::Tasks, Some(1), &[
            "ERROR: Logfile of failure stored in: /w/aaa/1.0/temp/log.do_compile.1",
            "| /a.c:1:1: error: in a",
            "ERROR: Logfile of failure stored in: /w/bbb/2.0/temp/log.do_install.2",
            "| /b.c:1:1: error: in b",
            "ERROR: Task (/m/bbb_2.0.bb:do_install) failed with exit code '1'",
            "ERROR: Task (/m/aaa_1.0.bb:do_compile) failed with exit code '1'",
        ]);
        let got: Vec<_> = d
            .failed_tasks
            .iter()
            .map(|f| (f.task.clone().unwrap(), f.task_log.clone().unwrap(), f.first_error.clone().unwrap()))
            .collect();
        assert_eq!(got, vec![
            ("bbb:do_install".into(), "/w/bbb/2.0/temp/log.do_install.2".into(), "/b.c:1:1: error: in b".into()),
            ("aaa:do_compile".into(), "/w/aaa/1.0/temp/log.do_compile.1".into(), "/a.c:1:1: error: in a".into()),
        ]);
    }

    #[test]
    fn unpaired_task_and_logfile() {
        let d = digest_lines(CommandKind::Tasks, Some(1), &[TASK]);
        assert_eq!(d.failed_tasks[0].task.as_deref(), Some("lvgl:do_compile"));
        assert_eq!(d.failed_tasks[0].task_log, None);

        let d = digest_lines(CommandKind::Tasks, None, &[LOGFILE, "| /a.c:1:1: error: x"]);
        assert_eq!(d.outcome, "killed");
        assert_eq!(d.failed_tasks[0].task.as_deref(), Some("lvgl:do_compile"));
        assert_eq!(d.failed_tasks[0].first_error.as_deref(), Some("/a.c:1:1: error: x"));
    }
}
