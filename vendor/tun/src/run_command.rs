/// Runs a command and returns an error if the command fails, just convenience for users.
///
/// ZeroNode patch: on Windows, spawn with CREATE_NO_WINDOW so route/netsh
/// never flash a console during tunnel setup/teardown.
#[doc(hidden)]
#[allow(dead_code)]
pub fn run_command(command: &str, args: &[&str]) -> std::io::Result<Vec<u8>> {
    let full_cmd = format!("{} {}", command, args.join(" "));
    log::debug!("Running command: \"{full_cmd}\"...");
    let mut cmd = std::process::Command::new(command);
    cmd.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
    }
    let out = match cmd.output() {
        Ok(out) => out,
        Err(e) => {
            log::error!("Run command: \"{full_cmd}\" failed with: {e}");
            return Err(e);
        }
    };
    if !out.status.success() {
        let err = String::from_utf8_lossy(if out.stderr.is_empty() {
            &out.stdout
        } else {
            &out.stderr
        });
        let info = format!("Run command: \"{full_cmd}\" failed with {err}");
        log::error!("{info}");
        return Err(std::io::Error::other(info));
    }
    Ok(out.stdout)
}
