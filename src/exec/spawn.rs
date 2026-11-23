//! Launching a command and killing it: the shell-quoted `bash -c` wrapper,
//! the command's own process group, and the guard that kills that group.

/// Shell for the sourced path. `oe-init-build-env` is bash-only (it reads
/// `$BASH_SOURCE`) and `source` is itself a bashism, so this must not be a
/// POSIX `sh` such as dash — under dash the init script silently never runs
/// and bitbake/devtool resolve against the wrong PATH.
const INIT_SHELL: &str = "bash";

/// Single-quote a value for `sh`. The input is also validated upstream to a
/// metachar-free charset, so this is defense-in-depth.
pub(super) fn shq(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Build the process for `program args...` in `cwd`: its spawnable command,
/// the effective command line reported in the result, and the plain line
/// (`program args...`) that names its log.
pub(super) fn command(
    init_script: Option<&std::path::Path>,
    program: &str,
    args: &[String],
    cwd: &str,
) -> (tokio::process::Command, String, String) {
    let command_line = {
        let mut parts = vec![program.to_string()];
        parts.extend(args.iter().cloned());
        parts.join(" ")
    };

    let (mut cmd, command) = if let Some(init) = init_script {
        // bash -c "source <init> . || exit 1; <program> <args...>"
        let init = init.to_string_lossy();
        let mut body = format!("source {} . || exit 1; ", shq(&init));
        body.push_str(program);
        for a in args {
            body.push(' ');
            body.push_str(&shq(a));
        }
        let mut cmd = tokio::process::Command::new(INIT_SHELL);
        cmd.arg("-c").arg(&body);
        (cmd, format!("(cd {} && source {init} . && {command_line})", shq(cwd)))
    } else {
        let mut cmd = tokio::process::Command::new(program);
        cmd.args(args);
        (cmd, command_line.clone())
    };

    cmd.current_dir(cwd);
    // A dropped (cancelled) request must not leave bitbake/devtool running.
    cmd.kill_on_drop(true);
    // Own process group, so a cancel can kill the whole tree (the `bash -c`
    // wrapper, bitbake/devtool and their children), not just the direct child.
    cmd.process_group(0);
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    (cmd, command, command_line)
}

/// Kills a command's whole process group when dropped while armed, i.e. when
/// the request is cancelled mid-run. `kill_on_drop` alone reaches only the
/// direct child, which on the `init_script` path is the `bash -c` wrapper.
pub(super) struct GroupKill(pub(super) Option<i32>);

impl Drop for GroupKill {
    fn drop(&mut self) {
        if let Some(pgid) = self.0 {
            // SAFETY: killpg takes plain integers; a stale group id at worst
            // fails with ESRCH.
            unsafe {
                libc::killpg(pgid, libc::SIGKILL);
            }
        }
    }
}
