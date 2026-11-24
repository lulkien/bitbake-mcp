//! Request payloads of the eight tools. Required arguments are declared as
//! such so the tool schema says so; a missing or wrongly typed argument fails
//! deserialization before a handler runs.

use rmcp::schemars;
use serde::Deserialize;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(super) struct DevtoolModifyReq {
    pub(super) recipe: String,
    /// `true` passes `--debug-build` to add DEBUG_BUILD=1 to the workspace
    /// append, enabling on-target debugging.
    #[serde(default)]
    pub(super) debug_build: Option<bool>,
    /// Absolute directory in which to run the command (e.g. a Yocto build
    /// dir). Defaults to the configured `build_dir`.
    #[serde(default)]
    pub(super) build_dir: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(super) struct DeployTargetReq {
    pub(super) recipe: String,
    /// `user@host` deploy target.
    pub(super) target: String,
    /// `true` passes `-S` (strip executables before deploying, so debug info
    /// is not shipped). Defaults to false, which passes `--no-strip`.
    #[serde(default)]
    pub(super) strip: Option<bool>,
    /// `true` passes `-c` (disable ssh host key checking). Defaults to true.
    #[serde(default)]
    pub(super) no_host_check: Option<bool>,
    /// `true` passes `-n` to list files without deploying them.
    #[serde(default)]
    pub(super) dry_run: Option<bool>,
    /// `true` passes `-p` (overwrite target files instead of preserving
    /// them). Defaults to true.
    #[serde(default)]
    pub(super) no_preserve: Option<bool>,
    /// `true` passes `--no-check-space` (skip the free-space check before
    /// deploying). Defaults to true.
    #[serde(default)]
    pub(super) no_check_space: Option<bool>,
    /// SSH port for the connection (`-P`), 1-65535. Defaults to 22.
    #[serde(default)]
    #[schemars(range(min = 1))]
    pub(super) port: Option<u16>,
    #[serde(default)]
    pub(super) build_dir: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(super) struct DevtoolUndeployTargetReq {
    /// Recipe previously deployed to the target; mutually exclusive with `all`.
    #[serde(default)]
    pub(super) recipe: Option<String>,
    /// `true` passes `-a` to undeploy every recipe from the target.
    #[serde(default)]
    pub(super) all: Option<bool>,
    /// `user@host` target from which to remove deployed files.
    pub(super) target: String,
    /// `true` passes `-c` to disable ssh host key checking.
    #[serde(default)]
    pub(super) no_host_check: Option<bool>,
    /// `true` passes `-n` to list files without removing them.
    #[serde(default)]
    pub(super) dry_run: Option<bool>,
    /// SSH port for the connection (`-P`), 1-65535. Defaults to 22.
    #[serde(default)]
    #[schemars(range(min = 1))]
    pub(super) port: Option<u16>,
    #[serde(default)]
    pub(super) build_dir: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(super) struct DevtoolResetReq {
    pub(super) recipe: String,
    /// `true` passes `-n` (do not clean the sysroot of the recipe's output).
    /// Defaults to true.
    #[serde(default)]
    pub(super) no_clean: Option<bool>,
    #[serde(default)]
    pub(super) build_dir: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(super) struct DevtoolBuildReq {
    pub(super) recipe: String,
    /// `true` passes `-c` (clean up the recipe's build results before
    /// building). Defaults to false.
    #[serde(default)]
    pub(super) clean: Option<bool>,
    /// `true` passes `-s` to disable parallel make for this build.
    #[serde(default)]
    pub(super) disable_parallel_make: Option<bool>,
    #[serde(default)]
    pub(super) build_dir: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(super) struct DevtoolBuildImageReq {
    /// Image recipe to build; required by `devtool build-image`.
    pub(super) image: String,
    /// Comma-separated recipe names to add to the image with `-p`.
    #[serde(default)]
    pub(super) add_packages: Option<String>,
    #[serde(default)]
    pub(super) build_dir: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(super) struct BuildDirReq {
    #[serde(default)]
    pub(super) build_dir: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub(super) enum DumpSignatures {
    None,
    Printdiff,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub(super) struct BitbakeRecipeReq {
    pub(super) recipe: String,
    /// Task to run with `-c <task>` (e.g. `compile`, `populate_sysroot`).
    /// Omit it to run the recipe's default task.
    #[serde(default)]
    pub(super) task: Option<String>,
    /// `true` passes `-f`: force the specified targets/tasks to run,
    /// invalidating any existing stamp file. Defaults to false.
    #[serde(default)]
    pub(super) force: Option<bool>,
    /// Task whose stamp `-C <task>` invalidates before the default task runs
    /// (e.g. `compile`).
    #[serde(default)]
    pub(super) clear_stamp: Option<String>,
    /// `--runall <task>`: run that task for any recipe in the taskgraph of the
    /// target, even if it would not otherwise have run.
    #[serde(default)]
    pub(super) runall: Option<String>,
    /// `--runonly <task>`: run only that task within the target's taskgraph
    /// (plus the task dependencies it may have).
    #[serde(default)]
    pub(super) runonly: Option<String>,
    /// `true` passes `--no-setscene`: sstate is ignored and everything needed
    /// is built. Defaults to false.
    #[serde(default)]
    pub(super) no_setscene: Option<bool>,
    /// `true` passes `--skip-setscene`: skip setscene tasks that would be
    /// executed, keeping what was already restored from sstate. Defaults to
    /// false.
    #[serde(default)]
    pub(super) skip_setscene: Option<bool>,
    /// `true` passes `--setscene-only`: run only setscene tasks, no real
    /// tasks. Defaults to false.
    #[serde(default)]
    pub(super) setscene_only: Option<bool>,
    /// `true` passes `-k` (continue as much as possible after an error).
    #[serde(default)]
    pub(super) continue_on_error: Option<bool>,
    /// `true` passes `-n` (show the task execution plan without running it).
    #[serde(default)]
    pub(super) dry_run: Option<bool>,
    /// `-S` signature diagnostics: `none` or `printdiff`.
    #[serde(default)]
    pub(super) dump_signatures: Option<DumpSignatures>,
    #[serde(default)]
    pub(super) build_dir: Option<String>,
}
