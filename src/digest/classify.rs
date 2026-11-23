//! Line classes (design §2), diagnostics parsing, summary lines and the
//! warning/error families.

use super::text::floor_boundary;
use super::types::{Parse, Sstate, Tasks};
use super::{DigestBuilder, LOGFILE_MARK, TASK_MARK};

/// Warning and error families tracked, shared by both.
pub const FAMILY_MAX: usize = 64;
/// Family keys are cut to this many bytes.
const FAMILY_KEY_CAP: usize = 160;

/// Warning families that are counted in `noise_warnings` and never grouped.
const NOISE_WARNINGS: [&str; 5] = [
    "No recipes in default available",
    "is tainted from a forced run",
    "DeprecationWarning",
    "ResourceWarning",
    "Report submitted",
];

/// Engine line classes (design §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LineClass {
    Noise,
    Progress,
    TaskCmd,
    TaskOther,
    Diag,
    DiagFailed,
    TaskFailed,
    Error,
    Warning,
    Summary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FamilyKind {
    Warning,
    Error,
}

#[derive(Debug)]
pub(super) struct Family {
    pub(super) kind: FamilyKind,
    pub(super) key: String,
    pub(super) count: u64,
    pub(super) sample: String,
}

impl DigestBuilder {
    /// Precedence: first match wins (design §2).
    pub(super) fn classify(&mut self, text: &str) -> LineClass {
        if (self.line_no <= 3
            && (text.starts_with("# command:")
                || text.starts_with("# cwd:")
                || text.starts_with("# exit_code:")))
            || text == "----- stderr -----"
        {
            return LineClass::Noise;
        }
        if let Some(rest) = text.strip_prefix("| ") {
            self.in_traceback = false;
            return if is_task_cmd(rest) {
                LineClass::TaskCmd
            } else if diag_split(rest).is_some() {
                LineClass::Diag
            } else if rest.starts_with("FAILED: ") {
                LineClass::DiagFailed
            } else {
                LineClass::TaskOther
            };
        }
        if text.contains(LOGFILE_MARK) || text.contains(TASK_MARK) {
            self.in_traceback = false;
            return LineClass::TaskFailed;
        }
        if text.contains("Tasks Summary")
            || text.contains("Sstate summary")
            || text.starts_with("Summary: ")
            || (text.starts_with("Parsing of ") && text.contains(" .bb files"))
        {
            return LineClass::Summary;
        }
        if text.contains("ERROR:") {
            return LineClass::Error;
        }
        if text.contains("Traceback (most recent call last)") {
            self.in_traceback = true;
            return LineClass::Error;
        }
        if text.starts_with("  File \"") {
            return LineClass::Error;
        }
        if self.in_traceback && !text.is_empty() && !text.starts_with(char::is_whitespace) {
            // The line after a traceback's frames names the exception.
            self.in_traceback = false;
            self.add_family(FamilyKind::Error, text);
            return LineClass::Error;
        }
        if text.contains("WARNING:") {
            return LineClass::Warning;
        }
        if text.contains("Running task")
            || ((text.contains(": Started") || text.contains(": Succeeded")) && text.contains("task do_"))
            || text.contains("setscene")
            || text.contains("compiling from external source tree")
        {
            return LineClass::Progress;
        }
        LineClass::Noise
    }

    pub(super) fn summary(&mut self, text: &str) {
        if text.contains("Tasks Summary") {
            self.tasks = Some(Tasks {
                attempted: num_after(text, "Attempted ").unwrap_or(0),
                not_rerun: num_after(text, "of which ").unwrap_or(0),
                failed: num_before(text, " failed").unwrap_or(0),
            });
        } else if text.contains("Sstate summary") {
            self.sstate = Some(Sstate {
                wanted: num_after(text, "Wanted ").unwrap_or(0),
                local: num_after(text, "Local ").unwrap_or(0),
                mirrors: num_after(text, "Mirrors ").unwrap_or(0),
                missed: num_after(text, "Missed ").unwrap_or(0),
                current: num_after(text, "Current ").unwrap_or(0),
                match_pct: num_before(text, "% match").unwrap_or(0),
                complete_pct: num_before(text, "% complete").unwrap_or(0),
            });
        } else if text.starts_with("Parsing of ") {
            self.parse = Some(Parse {
                bb_files: num_after(text, "Parsing of ").unwrap_or(0),
                parsed: num_before(text, " parsed").unwrap_or(0),
                errors: num_before(text, " errors").unwrap_or(0),
            });
        }
    }

    pub(super) fn warning(&mut self, text: &str) {
        let Some(i) = text.find("WARNING:") else {
            return;
        };
        let msg = &text[i..];
        if NOISE_WARNINGS.iter().any(|n| msg.contains(n)) {
            self.noise_warnings += 1;
            return;
        }
        self.add_family(FamilyKind::Warning, msg);
    }

    pub(super) fn add_family(&mut self, kind: FamilyKind, text: &str) {
        let key = family_key(text);
        if let Some(f) = self.families.iter_mut().find(|f| f.kind == kind && f.key == key) {
            f.count += 1;
            return;
        }
        if self.families.len() == FAMILY_MAX {
            match kind {
                FamilyKind::Warning => self.warnings_overflow += 1,
                FamilyKind::Error => self.errors_overflow += 1,
            }
            return;
        }
        self.families.push(Family { kind, key, count: 1, sample: text.to_string() });
    }
}

/// `[N/M] ...` or a `: && ` link line inside a task-log dump.
pub(super) fn is_task_cmd(rest: &str) -> bool {
    if rest.contains(": && ") {
        return true;
    }
    let Some(r) = rest.strip_prefix('[') else {
        return false;
    };
    let Some(end) = r.find("] ") else {
        return false;
    };
    let mut parts = r[..end].splitn(2, '/');
    let all_digits = |s: Option<&str>| s.is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()));
    all_digits(parts.next()) && all_digits(parts.next())
}

/// Split `<path>:<L>[:<C>]: (fatal error|error): <msg>` into
/// `(<path>:<L>[:<C>], "error"|"fatal error", <msg>)`.
pub(super) fn diag_split(s: &str) -> Option<(&str, &'static str, &str)> {
    for (mark, kind) in [(": fatal error: ", "fatal error"), (": error: ", "error")] {
        let mut from = 0;
        while let Some(off) = s[from..].find(mark) {
            let i = from + off;
            let loc = &s[..i];
            let (path, linecol) = split_linecol(loc);
            if !linecol.is_empty() && !path.is_empty() {
                return Some((loc, kind, &s[i + mark.len()..]));
            }
            from = i + mark.len();
        }
    }
    None
}

/// Split `<path>:<L>[:<C>]` into `(<path>, ":<L>[:<C>]")`; the second part is
/// empty when `loc` does not end in a line number.
pub(super) fn split_linecol(loc: &str) -> (&str, &str) {
    let mut cut = loc.len();
    for _ in 0..2 {
        let head = &loc[..cut];
        let digits = head.bytes().rev().take_while(u8::is_ascii_digit).count();
        if digits == 0 || head.len() == digits || head.as_bytes()[head.len() - digits - 1] != b':' {
            break;
        }
        cut = head.len() - digits - 1;
    }
    (&loc[..cut], &loc[cut..])
}

/// Integer right after `key`.
fn num_after(text: &str, key: &str) -> Option<u64> {
    let i = text.find(key)? + key.len();
    let digits: String = text[i..].chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// Integer right before `key`.
fn num_before(text: &str, key: &str) -> Option<u64> {
    let i = text.find(key)?;
    let head = &text[..i];
    let n = head.bytes().rev().take_while(u8::is_ascii_digit).count();
    head[head.len() - n..].parse().ok()
}

/// Family key: tokens containing `/` become `<p>`, digit runs `N`, cut to
/// `FAMILY_KEY_CAP` bytes.
pub(super) fn family_key(text: &str) -> String {
    let mut out = String::new();
    for (i, tok) in text.split(' ').enumerate() {
        if i > 0 {
            out.push(' ');
        }
        if tok.contains('/') {
            out.push_str("<p>");
            continue;
        }
        let mut in_digits = false;
        for c in tok.chars() {
            if c.is_ascii_digit() {
                if !in_digits {
                    out.push('N');
                }
                in_digits = true;
            } else {
                out.push(c);
                in_digits = false;
            }
        }
        if out.len() >= FAMILY_KEY_CAP {
            break;
        }
    }
    let end = floor_boundary(&out, FAMILY_KEY_CAP);
    out.truncate(end);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::digest::test_support::digest_lines;
    use crate::digest::{CommandKind, WARNINGS_MAX};

    fn class_of(line: &str) -> LineClass {
        let mut b = DigestBuilder::new(CommandKind::Tasks);
        b.line_no = 10;
        b.classify(line)
    }

    #[test]
    fn precedence() {
        assert_eq!(class_of("| foo ERROR: bar"), LineClass::TaskOther);
        assert_eq!(class_of("| [3/9] cc -c x.c ERROR: y"), LineClass::TaskCmd);
        assert_eq!(class_of("| : && clang -o a b.o"), LineClass::TaskCmd);
        assert_eq!(class_of("Initialising tasks...WARNING: x"), LineClass::Warning);
        assert_eq!(class_of("| /s/a.c:3:4: error: boom"), LineClass::Diag);
        assert_eq!(class_of("| /s/a.c:3: fatal error: x.h: No such file"), LineClass::Diag);
        assert_eq!(class_of("| FAILED: obj/a.o"), LineClass::DiagFailed);
        assert_eq!(class_of("| ninja: build stopped: subcommand failed."), LineClass::TaskOther);
        assert_eq!(class_of("| /s/a.c:3:4: warning: meh"), LineClass::TaskOther);
        assert_eq!(class_of("ERROR: Logfile of failure stored in: /t/log.do_x.1"), LineClass::TaskFailed);
        assert_eq!(class_of("ERROR: Task (/r/a_1.bb:do_x) failed with exit code '1'"), LineClass::TaskFailed);
        assert_eq!(class_of("NOTE: Tasks Summary: Attempted 3 tasks of which 2 didn't need to be rerun and 1 failed."), LineClass::Summary);
        assert_eq!(class_of("ERROR: a-1 do_x: boom"), LineClass::Error);
        assert_eq!(class_of("  File \"/x.py\", line 3, in f"), LineClass::Error);
        assert_eq!(class_of("NOTE: Running task 1 of 2 (/r/a.bb:do_x)"), LineClass::Progress);
        assert_eq!(class_of("NOTE: recipe a-1-r0: task do_x: Started"), LineClass::Progress);
        assert_eq!(class_of("Loading cache...done."), LineClass::Noise);
        assert_eq!(class_of("----- stderr -----"), LineClass::Noise);
    }

    #[test]
    fn traceback_last_line_is_an_error() {
        let d = digest_lines(CommandKind::Tasks, Some(1), &[
            "WARNING: STDERR: Traceback (most recent call last):",
            "  File \"/x.py\", line 3, in f",
            "    f()",
            "urllib.error.URLError: <urlopen error refused>",
            "NOTE: after",
        ]);
        assert_eq!(d.errors, vec!["urllib.error.URLError: <urlopen error refused>".to_string()]);
    }

    #[test]
    fn families_and_noise() {
        let mut lines: Vec<String> = Vec::new();
        for i in 0..228 {
            lines.push(format!("WARNING: /m/r{i}_1.bb:do_fetch is tainted from a forced run"));
        }
        for i in 0..62 {
            lines.push(format!("WARNING: lds-0.1.2-r2.{i} do_package: [atlas-dac] {i} DAC violations found. See /t/{i}/dac.json"));
        }
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let d = digest_lines(CommandKind::Tasks, Some(0), &refs);
        assert_eq!(d.noise_warnings, 228);
        assert_eq!(d.warnings.len(), 1);
        assert_eq!(d.warnings[0].count, 62);

        let lines: Vec<String> = (0..65).map(|i| format!("WARNING: family {} here", "x".repeat(i + 1))).collect();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let d = digest_lines(CommandKind::Tasks, Some(0), &refs);
        assert_eq!(d.warnings_overflow, 1);
        assert_eq!(d.warnings.len(), WARNINGS_MAX);
        assert_eq!(d.lines.as_ref().unwrap().warning, 65);
    }

    #[test]
    fn warnings_sort_by_count_then_first_seen() {
        let d = digest_lines(CommandKind::Tasks, Some(0), &[
            "WARNING: alpha", "WARNING: beta", "WARNING: beta", "WARNING: gamma", "WARNING: gamma",
        ]);
        let s: Vec<_> = d.warnings.iter().map(|w| (w.count, w.sample.as_str())).collect();
        assert_eq!(s, vec![(2, "WARNING: beta"), (2, "WARNING: gamma"), (1, "WARNING: alpha")]);
    }

    #[test]
    fn summaries_parse_and_last_wins() {
        let d = digest_lines(CommandKind::Tasks, Some(0), &[
            "Parsing of 10 .bb files complete (9 cached, 1 parsed). 5 targets, 0 skipped, 0 masked, 0 errors.",
            "Parsing of 6776 .bb files complete (6760 cached, 16 parsed). 9254 targets, 836 skipped, 10 masked, 2 errors.",
            "Sstate summary: Wanted 8 Local 2 Mirrors 0 Missed 6 Current 878 (25% match, 99% complete)",
            "NOTE: Tasks Summary: Attempted 2223 tasks of which 2212 didn't need to be rerun and all succeeded.",
        ]);
        assert_eq!(d.parse, Some(Parse { bb_files: 6776, parsed: 16, errors: 2 }));
        assert_eq!(d.sstate, Some(Sstate { wanted: 8, local: 2, mirrors: 0, missed: 6, current: 878, match_pct: 25, complete_pct: 99 }));
        assert_eq!(d.tasks, Some(Tasks { attempted: 2223, not_rerun: 2212, failed: 0 }));
    }
}
