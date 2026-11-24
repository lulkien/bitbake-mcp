//! Wire types of the digest: what a caller reads, and the command kind that
//! shapes it. Field order is serialization order.

use serde::Serialize;

/// What a command does, which decides whether a failure can have a stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandKind {
    /// Runs bitbake tasks: `bitbake`, `devtool build`, `devtool modify`,
    /// `devtool build-image`, and `devtool upgrade`.
    Tasks,
    /// Everything else (`devtool deploy-target`, `status`, `reset`).
    Plain,
}

impl CommandKind {
    /// Kind of `program args...`.
    pub fn of(program: &str, args: &[String]) -> Self {
        match (program, args.first().map(String::as_str)) {
            ("bitbake", _) | ("devtool", Some("build" | "modify" | "build-image" | "upgrade")) => {
                Self::Tasks
            }
            _ => Self::Plain,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::CommandKind;

    #[test]
    fn devtool_build_image_is_a_task_command() {
        let args = vec!["build-image".to_string(), "core-image-minimal".to_string()];
        assert_eq!(CommandKind::of("devtool", &args), CommandKind::Tasks);
    }

    #[test]
    fn devtool_upgrade_is_a_task_command() {
        let args = vec!["upgrade".to_string(), "busybox".to_string()];
        assert_eq!(CommandKind::of("devtool", &args), CommandKind::Tasks);
    }
}

#[derive(Debug, Default, Clone, Serialize, PartialEq, Eq)]
pub struct FailedTask {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recipe_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_log: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_error: Option<String>,
    #[serde(skip_serializing_if = "is_zero")]
    pub error_lines: u64,
    /// `first_error` before `LINE_CAP`, so the last-resort shrink still finds
    /// `error:` behind a path longer than the cap.
    #[serde(skip)]
    pub(super) first_error_raw: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct WarningGroup {
    pub count: u64,
    pub sample: String,
}

#[derive(Debug, Default, Clone, Serialize, PartialEq, Eq)]
pub struct Tasks {
    pub attempted: u64,
    pub not_rerun: u64,
    pub failed: u64,
}

#[derive(Debug, Default, Clone, Serialize, PartialEq, Eq)]
pub struct Sstate {
    pub wanted: u64,
    pub local: u64,
    pub mirrors: u64,
    pub missed: u64,
    pub current: u64,
    pub match_pct: u64,
    pub complete_pct: u64,
}

#[derive(Debug, Default, Clone, Serialize, PartialEq, Eq)]
pub struct Parse {
    pub bb_files: u64,
    pub parsed: u64,
    pub errors: u64,
}

/// Per-class line counts; zero counts are omitted.
#[derive(Debug, Default, Clone, Serialize, PartialEq, Eq)]
pub struct Lines {
    pub total: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub noise: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub progress: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub task_cmd: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub task_other: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub diag: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub diag_failed: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub task_failed: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub summary: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub error: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub warning: u64,
}

/// The digest of one command's log (design §3.3). Fields serialize in this
/// order and are omitted when empty.
#[derive(Debug, Default, Clone, Serialize, PartialEq, Eq)]
pub struct Digest {
    pub outcome: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed_stage: Option<&'static str>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub failed_tasks: Vec<FailedTask>,
    /// Failed tasks not listed in `failed_tasks` (over the cap of 4, or cut
    /// by the drop order), so the failure count stays honest.
    #[serde(skip_serializing_if = "is_zero")]
    pub failed_tasks_omitted: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
    #[serde(skip_serializing_if = "is_zero")]
    pub errors_overflow: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<WarningGroup>,
    #[serde(skip_serializing_if = "is_zero")]
    pub warnings_overflow: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub noise_warnings: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tasks: Option<Tasks>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sstate: Option<Sstate>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parse: Option<Parse>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tail: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lines: Option<Lines>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub incomplete: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stderr_file: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub dropped: Vec<&'static str>,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}
