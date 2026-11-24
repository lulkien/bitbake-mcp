//! Argument validation for the MCP tool boundary.
//!
//! Every user value that reaches an argv — recipe name, `user@host` deploy
//! target, task name — is checked here before it is handed to `exec::run`.

#[derive(Debug, Clone, PartialEq)]
pub struct ValidationError {
    pub error_code: &'static str,
    pub message: String,
}

pub type ValidationResult = Result<(), ValidationError>;

/// Validate an SSH port for `devtool deploy-target`.
pub fn validate_port(port: u16) -> ValidationResult {
    if port == 0 {
        Err(ValidationError {
            error_code: "INVALID_ARGUMENT",
            message: "Invalid port: 0".into(),
        })
    } else {
        Ok(())
    }
}

/// Validate a recipe name (e.g. `busybox` or `busybox_1.36.1`) used in
/// `bitbake <recipe>` / `devtool <recipe>` commands. Rejects anything that
/// could carry shell syntax or path traversal.
pub fn validate_recipe_name(name: &str) -> ValidationResult {
    // BitBake recipe names: letters, digits, underscore, hyphen, dot, plus.
    // No spaces, no slashes, no shell metacharacters.
    let ok = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '+'))
        && !name.contains("..")
        && !name.starts_with(['-', '.']);
    if ok {
        Ok(())
    } else {
        Err(ValidationError {
            error_code: "INVALID_ARGUMENT",
            message: format!("Invalid recipe name: {name}"),
        })
    }
}

/// Validate a comma-separated list of recipe names passed to build-image `-p`.
pub fn validate_package_list(packages: &str) -> ValidationResult {
    if packages
        .split(',')
        .all(|package| validate_recipe_name(package).is_ok())
    {
        Ok(())
    } else {
        Err(ValidationError {
            error_code: "INVALID_ARGUMENT",
            message: format!("Invalid package list: {packages}"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{validate_package_list, validate_recipe_name};

    #[test]
    fn package_lists_accept_recipe_names() {
        assert!(validate_package_list("a,b").is_ok());
        assert!(validate_package_list("gtk+3,libsigc++-3").is_ok());
    }

    #[test]
    fn package_lists_reject_invalid_recipe_names() {
        for packages in ["a;b", "a,,b", "", "-x,b", "a,../b"] {
            assert!(validate_package_list(packages).is_err(), "accepted {packages:?}");
        }
    }

    #[test]
    fn recipe_names_accept_plus_and_existing_safe_characters() {
        assert!(validate_recipe_name("gtk+3").is_ok());
        assert!(validate_recipe_name("libsigc++-3").is_ok());
        assert!(validate_recipe_name("busybox_1.36.1").is_ok());
    }

    #[test]
    fn recipe_names_reject_shell_syntax_and_invalid_forms() {
        for name in ["a;b", "-x", "../x", "a b", "", "name..suffix"] {
            assert!(validate_recipe_name(name).is_err(), "accepted {name:?}");
        }
    }
}

/// Validate an `user@host` target for `devtool deploy-target`. The host part
/// must be a simple hostname/IP (letters, digits, `-`, `.`, or an IPv6
/// literal), and the user part a plain name. Neither may start with `-`. No spaces or shell metacharacters.
pub fn validate_target(target: &str) -> ValidationResult {
    if let Some((user, host)) = target.split_once('@') {
        // A leading `-` would reach ssh/scp as an option (`-F@host`).
        let user_ok = !user.is_empty()
            && !user.starts_with('-')
            && user.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
        // `:` is only valid in an IPv6 literal; scp reads `host:path` as a
        // remote file.
        let host_ok = !host.is_empty()
            && !host.starts_with('-')
            && if host.contains(':') {
                host.parse::<std::net::Ipv6Addr>().is_ok()
            } else {
                host.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
            };
        if user_ok && host_ok && !target.contains(' ') {
            return Ok(());
        }
    }
    Err(ValidationError {
        error_code: "INVALID_ARGUMENT",
        message: format!("Invalid deploy target (expected user@host): {target}"),
    })
}

/// Validate a task name for `-c <command>` (e.g. `configure`, `compile`).
/// Plain identifier, no spaces or metacharacters.
pub fn validate_task(task: &str) -> ValidationResult {
    let ok = !task.is_empty()
        && !task.starts_with('-')
        && task
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if ok {
        Ok(())
    } else {
        Err(ValidationError {
            error_code: "INVALID_ARGUMENT",
            message: format!("Invalid task name: {task}"),
        })
    }
}

/// Validate the directory a command runs in: absolute (a stdio server's cwd is
/// the client's, so a relative path would resolve somewhere unintended), no NUL
/// byte, and an existing directory — the init script creates `conf/` in it, so
/// it must not be conjured by a typo.
pub fn validate_build_dir(dir: &str) -> ValidationResult {
    let path = std::path::Path::new(dir);
    let problem = if dir.contains('\0') || !path.is_absolute() {
        Some("must be an absolute path")
    } else if !path.is_dir() {
        Some("is not an existing directory")
    } else {
        None
    };
    match problem {
        None => Ok(()),
        Some(why) => Err(ValidationError {
            error_code: "INVALID_ARGUMENT",
            message: format!("Invalid build_dir ({why}): {}", dir.escape_debug()),
        }),
    }
}
