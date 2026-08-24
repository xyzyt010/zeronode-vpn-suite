//! Shared helpers for the Linux platform crate.
//!
//! Hosts the command runner used by both server and client flows, PATH
//! resolution, and small filesystem utilities. The client-side modules
//! (`proc`, `elevation`, `client_setup`) build on top of these.

use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Default wall-clock budget for [`run_command`] before the child is killed.
pub const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug)]
pub enum CommandOutcome {
    Success(String),
    Failed(String),
}

impl CommandOutcome {
    /// Used by tunnel modules landing in the next blocks (C–I).
    #[allow(dead_code)]
    pub fn ok(&self) -> bool {
        matches!(self, CommandOutcome::Success(_))
    }
}

/// Resolve an executable name against `PATH` (exact file match per dir).
pub fn find_in_path(name: &str) -> Option<PathBuf> {
    if name.contains('/') {
        let path = PathBuf::from(name);
        return path.is_file().then_some(path);
    }
    let paths = env::var_os("PATH")?;
    env::split_paths(&paths)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

pub fn command_exists(name: &str) -> bool {
    find_in_path(name).is_some()
}

/// Directory containing the running executable. Consumed by asset resolvers
/// in the tunnel blocks (Tor bundle discovery, OpenVPN binary lookup).
#[allow(dead_code)]
pub fn current_exe_dir() -> Option<PathBuf> {
    env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(ToOwned::to_owned))
}

/// Run a program with args, capturing stdout/stderr with a hard timeout
/// (child is SIGKILLed past the deadline). Mirrors the historical
/// `run_command` API from lib.rs so server call sites stay unchanged.
pub fn run_command(program: &str, args: &[&str]) -> CommandOutcome {
    run_command_with_timeout(program, args, COMMAND_TIMEOUT)
}

pub fn run_command_with_timeout(
    program: &str,
    args: &[&str],
    timeout: Duration,
) -> CommandOutcome {
    let mut child = match Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => return CommandOutcome::Failed(format!("could not run {program}: {error}")),
    };

    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let output = match child.wait_with_output() {
                    Ok(output) => output,
                    Err(error) => {
                        return CommandOutcome::Failed(format!(
                            "{program} finished but output could not be read: {error}"
                        ))
                    }
                };
                if status.success() {
                    return CommandOutcome::Success(
                        String::from_utf8_lossy(&output.stdout).trim().to_owned(),
                    );
                }
                let detail = if output.stderr.is_empty() {
                    &output.stdout
                } else {
                    &output.stderr
                };
                return CommandOutcome::Failed(format!(
                    "{program} {} failed: {}",
                    args.join(" "),
                    String::from_utf8_lossy(detail).trim()
                ));
            }
            Ok(None) => {}
            Err(error) => {
                return CommandOutcome::Failed(format!("could not wait for {program}: {error}"))
            }
        }

        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return CommandOutcome::Failed(format!(
                "{program} {} timed out after {} ms",
                args.join(" "),
                timeout.as_millis()
            ));
        }
        std::thread::sleep(Duration::from_millis(15));
    }
}

/// Best-effort capture of a tool's stdout; never fails loudly.
/// Linux analogue of the Windows `silent_cmd::silent_output`.
pub fn silent_output(program: impl AsRef<Path>, args: &[&str]) -> Option<String> {
    let output = Command::new(program.as_ref())
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}
