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

/// Require exactly one undeploy selector: a recipe name or `all=true`.
pub fn validate_undeploy_target_selector(
    recipe: Option<&str>,
    all: Option<bool>,
) -> ValidationResult {
    let all = all.unwrap_or(false);
    match (recipe, all) {
        (Some(recipe), false) => validate_recipe_name(recipe),
        (None, true) => Ok(()),
        _ => Err(ValidationError {
            error_code: "INVALID_ARGUMENT",
            message: "Exactly one of recipe or all=true is required".into(),
        }),
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

/// Validate a single devtool search keyword.
pub fn validate_search_keyword(keyword: &str) -> ValidationResult {
    let ok = !keyword.is_empty()
        && keyword.len() <= 128
        && !keyword.starts_with('-')
        && keyword.is_ascii()
        && keyword.bytes().all(|b| (0x21..=0x7e).contains(&b));
    if ok {
        Ok(())
    } else {
        Err(ValidationError {
            error_code: "INVALID_ARGUMENT",
            message: format!("Invalid search keyword: {keyword}"),
        })
    }
}

/// Validate `devtool check-upgrade-status` recipe selectors.
pub fn validate_upgrade_status_selector(
    recipes: Option<&[String]>,
    all: Option<bool>,
) -> ValidationResult {
    match (recipes, all.unwrap_or(false)) {
        (Some(recipes), false) if !recipes.is_empty() && recipes.len() <= 32 => {
            for recipe in recipes {
                validate_recipe_name(recipe)?;
            }
            Ok(())
        }
        (None, true) => Ok(()),
        _ => Err(ValidationError {
            error_code: "INVALID_ARGUMENT",
            message: "Provide 1 to 32 recipes or set all=true".into(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        validate_package_list, validate_recipe_name, validate_search_keyword,
        validate_undeploy_target_selector, validate_upgrade_status_selector,
    };

    #[test]
    fn undeploy_selector_requires_exactly_one_recipe_or_all() {
        assert!(validate_undeploy_target_selector(Some("busybox"), None).is_ok());
        assert!(validate_undeploy_target_selector(None, Some(true)).is_ok());
        assert!(validate_undeploy_target_selector(Some("busybox"), Some(true)).is_err());
        assert!(validate_undeploy_target_selector(None, None).is_err());
        assert!(validate_undeploy_target_selector(None, Some(false)).is_err());
        assert!(validate_undeploy_target_selector(Some("-bad"), None).is_err());
    }

    #[test]
    fn package_lists_accept_recipe_names() {
        assert!(validate_package_list("a,b").is_ok());
        assert!(validate_package_list("gtk+3,libsigc++-3").is_ok());
    }

    #[test]
    fn search_keyword_validation() {
        assert!(validate_search_keyword("openssl").is_ok());
        for keyword in ["", "-x", "a b", "é", "x".repeat(129).as_str()] {
            assert!(
                validate_search_keyword(keyword).is_err(),
                "accepted {keyword:?}"
            );
        }
    }

    #[test]
    fn upgrade_status_selector_validation() {
        assert!(validate_upgrade_status_selector(Some(&["a".into(), "b".into()]), None).is_ok());
        assert!(validate_upgrade_status_selector(None, Some(true)).is_ok());
        let too_many: Vec<String> = (0..33).map(|i| format!("r{i}")).collect();
        assert!(validate_upgrade_status_selector(Some(&too_many), None).is_err());
        assert!(validate_upgrade_status_selector(Some(&["-x".into()]), None).is_err());
        assert!(validate_upgrade_status_selector(Some(&[]), None).is_err());
        assert!(validate_upgrade_status_selector(Some(&["a".into()]), Some(true)).is_err());
    }

    #[test]
    fn package_lists_reject_invalid_recipe_names() {
        for packages in ["a;b", "a,,b", "", "-x,b", "a,../b"] {
            assert!(
                validate_package_list(packages).is_err(),
                "accepted {packages:?}"
            );
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
            && user
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
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
