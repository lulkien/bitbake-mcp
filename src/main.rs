//! Deterministic BitBake/Yocto build MCP server.
//!
//! Eighteen tools for running and managing `bitbake`/`devtool` jobs.
//! Every tool response — success and error alike — is a structured
//! `ok:true/false` JSON text block that also carries `elapsed_ms`, the
//! wall-clock duration of that call, folded in at the dispatch boundary
//! (see `tools::envelope::with_elapsed_ms`).
//!
//! Command output is logged, not shipped: a call's complete stdout/stderr goes
//! to a log file and only the streams small enough to read are inlined (see
//! `exec`). Results carry `log` and `log_bytes` instead of a build's banner and
//! sstate statistics.

mod config;
mod digest;
mod exec;
mod jobs;
mod tools;
mod validation;

use rmcp::ServiceExt;
use std::str::FromStr;
use tools::BitBakeServer;

const SERVER_NAME: &str = "bitbake-mcp";
/// Track the crate version so the reported server version can't drift from
/// `Cargo.toml`.
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Log to stderr so the MCP JSON-RPC stream on stdout stays clean.
    let level = std::env::var("RUST_LOG")
        .ok()
        .and_then(|value| log::LevelFilter::from_str(&value).ok())
        .unwrap_or(log::LevelFilter::Info);
    fern::Dispatch::new()
        .format(|out, message, record| {
            out.finish(format_args!(
                "[{}][{}] {}",
                record.level(),
                record.target(),
                message
            ))
        })
        .level(level)
        .chain(std::io::stderr())
        .apply()
        .map_err(|e| anyhow::anyhow!("failed to init logging: {e}"))?;

    log::info!("starting bitbake-mcp {}", SERVER_VERSION);

    let cfg = config::Config::load()?;
    if let Some(init) = &cfg.init_script {
        if init.is_file() {
            log::info!("init_script: {}", init.to_string_lossy());
        } else {
            // Every build/devtool call will fail closed on this; say why here
            // rather than leaving only bash's error in each tool result.
            log::warn!(
                "init_script not found: {} — build/devtool tools will fail",
                init.to_string_lossy()
            );
        }
    }
    if let Some(bd) = &cfg.build_dir {
        log::info!("default build_dir: {}", bd.to_string_lossy());
    }
    // Say where a call's full output can be read: results only point at it.
    let log_dir = config::default_log_dir();
    if log_dir.as_os_str().is_empty() {
        log::warn!("no log dir (HOME and XDG_CACHE_HOME unset): output returned inline");
    } else {
        log::info!("output logs: {}", log_dir.to_string_lossy());
    }

    let server = BitBakeServer::new(cfg);
    let jobs = server.jobs();
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let service = server.serve(rmcp::transport::stdio()).await?;
    let cancellation = service.cancellation_token();
    let mut waiting = tokio::spawn(service.waiting());
    let ended = tokio::select! {
        result = &mut waiting => Some(result),
        _ = tokio::signal::ctrl_c() => None,
        _ = terminate.recv() => None,
    };
    let signaled = ended.is_none();
    let service_result: anyhow::Result<()> = match ended {
        Some(Ok(Ok(_))) => Ok(()),
        Some(Ok(Err(error))) => Err(error.into()),
        Some(Err(error)) => Err(error.into()),
        None => {
            cancellation.cancel();
            match waiting.await {
                Ok(Ok(_)) => Ok(()),
                Ok(Err(error)) => Err(error.into()),
                Err(error) => Err(error.into()),
            }
        }
    };
    jobs.shutdown().await;
    service_result?;
    if signaled {
        // Tokio's stdin reader uses a blocking worker that cannot be interrupted
        // while the client keeps stdin open. All service and job cleanup is
        // complete here, so exit instead of waiting for that reader on runtime
        // teardown.
        std::process::exit(0);
    }
    Ok(())
}
