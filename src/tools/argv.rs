//! Argument vectors of the seven tools. Pure: no I/O, no shell. User values are
//! validated before these run, except task names, which `bitbake_recipe`
//! validates as it emits them.

use super::requests::DumpSignatures;
use crate::validation::{self, ValidationError};

/// Push the setscene-selection flags shared by the bitbake tools.
///
/// None of the three has an off form, so an omitted or false value emits
/// nothing and bitbake's own default (run setscene tasks normally) applies.
fn push_setscene_flags(
    args: &mut Vec<String>,
    no_setscene: Option<bool>,
    skip_setscene: Option<bool>,
    setscene_only: Option<bool>,
) {
    if no_setscene.unwrap_or(false) {
        args.push("--no-setscene".to_string());
    }
    if skip_setscene.unwrap_or(false) {
        args.push("--skip-setscene".to_string());
    }
    if setscene_only.unwrap_or(false) {
        args.push("--setscene-only".to_string());
    }
}

/// `devtool build [-c] [-s]`; the caller appends the recipe.
pub(super) fn devtool_build(
    clean: Option<bool>,
    disable_parallel_make: Option<bool>,
) -> Vec<String> {
    let mut args = vec!["build".to_string()];
    if clean.unwrap_or(false) {
        args.push("-c".to_string());
    }
    if disable_parallel_make.unwrap_or(false) {
        args.push("-s".to_string());
    }
    args
}

/// `devtool modify [--debug-build]`; the caller appends the recipe.
pub(super) fn devtool_modify(debug_build: Option<bool>) -> Vec<String> {
    let mut args = vec!["modify".to_string()];
    if debug_build.unwrap_or(false) {
        args.push("--debug-build".to_string());
    }
    args
}

/// `devtool build-image [-p <packages>]`; the caller appends the image.
pub(super) fn devtool_build_image(
    add_packages: Option<String>,
) -> Result<Vec<String>, ValidationError> {
    let mut args = vec!["build-image".to_string()];
    if let Some(packages) = add_packages {
        validation::validate_package_list(&packages)?;
        args.push("-p".to_string());
        args.push(packages);
    }
    Ok(args)
}

/// `devtool reset [-n]`; the caller appends the recipe.
pub(super) fn devtool_reset(no_clean: Option<bool>) -> Vec<String> {
    if no_clean.unwrap_or(true) {
        vec!["reset".to_string(), "-n".to_string()]
    } else {
        vec!["reset".to_string()]
    }
}

/// `bitbake [-f] [-c <task>] [-C <task>] [--runall <task>]
/// [--runonly <task>] [--no-setscene] [--skip-setscene] [--setscene-only]
/// [-k] [-n] [-S <none|printdiff>]`;
/// the caller appends the recipe. A task name that fails validation is the
/// error.
pub(super) fn bitbake_recipe(
    force: Option<bool>,
    task: Option<String>,
    clear_stamp: Option<String>,
    runall: Option<String>,
    runonly: Option<String>,
    no_setscene: Option<bool>,
    skip_setscene: Option<bool>,
    setscene_only: Option<bool>,
    continue_on_error: Option<bool>,
    dry_run: Option<bool>,
    dump_signatures: Option<DumpSignatures>,
) -> Result<Vec<String>, ValidationError> {
    let mut argv: Vec<String> = Vec::new();
    if force.unwrap_or(false) {
        argv.push("-f".to_string());
    }
    // Task-valued options, emitted in bitbake's own --help order so the
    // reported `command` mirrors a command line a human would write.
    for (flag, value) in [
        ("-c", task),
        ("-C", clear_stamp),
        ("--runall", runall),
        ("--runonly", runonly),
    ] {
        let Some(value) = value else { continue };
        validation::validate_task(&value)?;
        argv.push(flag.to_string());
        argv.push(value);
    }
    push_setscene_flags(&mut argv, no_setscene, skip_setscene, setscene_only);
    if continue_on_error.unwrap_or(false) {
        argv.push("-k".to_string());
    }
    if dry_run.unwrap_or(false) {
        argv.push("-n".to_string());
    }
    if let Some(signatures) = dump_signatures {
        argv.push("-S".to_string());
        argv.push(match signatures {
            DumpSignatures::None => "none",
            DumpSignatures::Printdiff => "printdiff",
        }.to_string());
    }
    Ok(argv)
}

/// `devtool deploy-target [-c] [-p] [--no-check-space] [-P <port>]
/// [-S|--no-strip] <recipe> <target>`.
pub(super) fn deploy_target(
    recipe: String,
    target: String,
    strip: Option<bool>,
    no_host_check: Option<bool>,
    no_preserve: Option<bool>,
    no_check_space: Option<bool>,
    port: Option<u16>,
) -> Vec<String> {
    let mut args = vec!["deploy-target".to_string()];
    // Each boolean resolves to a concrete value, so the emitted argv does
    // not depend on a `[Deploy]` default in `conf/devtool.conf`.
    let flag = |v: Option<bool>, default: bool| v.unwrap_or(default);
    if flag(no_host_check, true) {
        args.push("-c".to_string());
    }
    if flag(no_preserve, true) {
        args.push("-p".to_string());
    }
    if flag(no_check_space, true) {
        args.push("--no-check-space".to_string());
    }
    args.push("-P".to_string());
    args.push(port.unwrap_or(22).to_string());
    args.push(if flag(strip, false) { "-S" } else { "--no-strip" }.to_string());
    args.push(recipe);
    args.push(target);
    args
}
