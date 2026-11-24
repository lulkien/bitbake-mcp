//! Configuration for the MCP server.
//!
//! Loaded from `bitbake-mcp.toml` (near the config discovery convention used
//! by sibling tools): the current dir, `~/.config/bitbake-mcp.toml`, or a path
//! from the `BITBAKE_MCP_CONFIG` env var. All fields optional — if no
//! `init_script` is given, build commands run directly (same as before).

use serde::Deserialize;
use std::path::PathBuf;

/// Server configuration.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Config {
    /// Absolute path to the Yocto environment init script, e.g.
    /// `/home/worker/atlas/oe-init-build-env`. When set, every build command
    /// runs as `bash -c "source <init_script> .; <command>"` with `build_dir`
    /// as the working directory, so bitbake/devtool are on PATH with the
    /// correct BBPATH and BUILDDIR. Optional.
    pub init_script: Option<PathBuf>,
    /// Default build directory for commands when a tool call omits
    /// `build_dir`. Optional; fallback is the current directory.
    pub build_dir: Option<PathBuf>,
}

/// Default directory for the per-call output logs: `$XDG_CACHE_HOME/bitbake-mcp`
/// when that is an absolute path, else `$HOME/.cache/bitbake-mcp`.
///
/// An empty path when neither is usable (`HOME` unset in a container): the
/// caller then writes no log file and results carry their output inline.
pub fn default_log_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
    {
        return dir.join("bitbake-mcp");
    }
    match std::env::var_os("HOME") {
        Some(home) => PathBuf::from(home).join(".cache").join("bitbake-mcp"),
        None => PathBuf::new(),
    }
}

impl Config {
    /// Load config from the first existing source, or the default when none
    /// are present. Logs nothing here; the caller logs the outcome.
    pub fn load() -> Result<Config, anyhow::Error> {
        let mut candidates: Vec<PathBuf> = Vec::new();

        // 1. explicit env override
        if let Ok(p) = std::env::var("BITBAKE_MCP_CONFIG") {
            candidates.push(PathBuf::from(p));
        }
        // 2. current dir
        if let Ok(cwd) = std::env::current_dir() {
            candidates.push(cwd.join("bitbake-mcp.toml"));
        }
        // 3. user config dir
        if let Some(home) = std::env::var_os("HOME") {
            candidates.push(PathBuf::from(home).join(".config").join("bitbake-mcp.toml"));
        }

        for path in &candidates {
            if path.is_file() {
                let raw = std::fs::read_to_string(path)?;
                let cfg: Config = toml::from_str(&raw)?;
                return Ok(cfg);
            }
        }

        Ok(Config::default())
    }
}
