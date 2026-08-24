//! Privileged helper daemon — ProtonVPN/Mullvad-style split architecture.
//!
//! How real VPNs do it on Linux (ProtonVPN `protonvpn-agent`, Mullvad
//! `mullvad-daemon`, OpenVPN via NetworkManager): a **root systemd service**
//! owns every privileged operation (TUN, routes, firewall). The desktop GUI
//! runs as the normal user and talks to it over a local Unix socket. The GUI
//! therefore *never* relaunches itself, *never* shows a polkit/pkexec dialog,
//! and only one app window ever exists.
//!
//! This module implements both ends:
//!
//! * Server: `vpn-client --daemon` (root, started by `zeronode-vpn-helper.service`).
//!   Listens on `/run/zeronode-vpn.sock`, newline-delimited JSON requests.
//! * Client: [`send`] used by the GUI. Returns `None` when the helper is not
//!   installed/running so callers can fall back to legacy behaviour.
//!
//! Commands (all VPN-scoped, no shell):
//! `ping` · `tor_start{socks_port}` · `tor_stop` · `wg_start{config_path}` ·
//! `wg_stop` · `ovpn_start{profile_path}` · `ovpn_stop` ·
//! `pptp_start{server,username,password}` · `pptp_stop` ·
//! `ss_start{method,password,host,port,system_wide}` → `{port}` · `ss_stop` ·
//! `status`

#[cfg(target_os = "linux")]
mod imp {
    use serde_json::{json, Value};
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    /// Well-known socket path (matches the systemd unit's RuntimeDirectory).
    pub const HELPER_SOCK: &str = "/run/zeronode-vpn.sock";

    // ---------------------------------------------------------------------------
    // Client side (GUI, unprivileged)
    // ---------------------------------------------------------------------------

    /// Send one command to the helper. `Ok(None)` ⇒ helper not running (GUI falls
    /// back to direct/pkexec behaviour); `Some(v)` is the JSON reply.
    pub fn send(method: &str, params: Value) -> Option<Value> {
        let mut stream = UnixStream::connect(HELPER_SOCK).ok()?;
        let _ = stream.set_read_timeout(Some(Duration::from_secs(90)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));

        let req = json!({ "cmd": method, "params": params });
        let mut line = req.to_string();
        line.push('\n');
        stream.write_all(line.as_bytes()).ok()?;

        let mut reader = BufReader::new(stream);
        let mut reply = String::new();
        reader.read_line(&mut reply).ok()?;
        if reply.trim().is_empty() {
            return None;
        }
        serde_json::from_str(reply.trim()).ok()
    }

    /// Convenience: `true` when the helper answers `ping`.
    pub fn available() -> bool {
        matches!(send("ping", json!({})), Some(reply) if reply.get("ok").and_then(Value::as_bool) == Some(true))
    }

    // ---------------------------------------------------------------------------
    // Server side (--daemon, root)
    // ---------------------------------------------------------------------------

    pub fn run_daemon() -> anyhow::Result<()> {
        use std::os::unix::fs::PermissionsExt;
        use std::os::unix::net::UnixListener;
        use std::path::Path;

        let sock_path = Path::new(HELPER_SOCK);
        if sock_path.exists() {
            let _ = std::fs::remove_file(sock_path);
        }
        if let Some(parent) = sock_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let listener = UnixListener::bind(sock_path)
            .map_err(|e| anyhow::anyhow!("cannot bind {HELPER_SOCK}: {e}"))?;
        // 0666 — any local user may ask for VPN operations (same trust level as
        // NetworkManager's default `allow_active=yes`). No shells are executed;
        // each command maps to a fixed function.
        let _ = std::fs::set_permissions(sock_path, std::fs::Permissions::from_mode(0o666));

        tracing::info!("ZeroNode helper daemon listening on {HELPER_SOCK}");
        eprintln!("ZeroNode helper daemon listening on {HELPER_SOCK}");

        // Clean socket on SIGTERM/SIGINT (systemd stop / Ctrl-C).
        unsafe {
            libc::signal(libc::SIGTERM, handle_signal as usize);
            libc::signal(libc::SIGINT, handle_signal as usize);
        }

        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    std::thread::Builder::new()
                        .name("zn-helper-conn".into())
                        .spawn(move || {
                            let _ = handle_connection(stream);
                        })
                        .ok();
                }
                Err(error) => tracing::warn!("helper accept failed: {error}"),
            }
        }
        Ok(())
    }

    extern "C" fn handle_signal(_sig: libc::c_int) {
        let _ = std::fs::remove_file(HELPER_SOCK);
        std::process::exit(0);
    }

    fn handle_connection(stream: UnixStream) -> anyhow::Result<()> {
        let mut writer = stream.try_clone()?;
        let reader = BufReader::new(stream);
        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => break,
            };
            if line.trim().is_empty() {
                continue;
            }
            let reply = dispatch(&line);
            let mut out = reply.to_string();
            out.push('\n');
            if writer.write_all(out.as_bytes()).is_err() {
                break;
            }
        }
        Ok(())
    }

    fn dispatch(line: &str) -> Value {
        let req: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => return json!({ "ok": false, "error": format!("bad request: {e}") }),
        };
        let cmd = req.get("cmd").and_then(Value::as_str).unwrap_or("");
        let params = req.get("params").cloned().unwrap_or_else(|| json!({}));

        let result: std::result::Result<Value, String> = match cmd {
            "ping" => Ok(json!({ "pong": true, "uid": unsafe { libc::geteuid() } })),

            "tor_start" => {
                let port = params.get("socks_port").and_then(Value::as_u64).unwrap_or(9050) as u16;
                vpn_platform_linux::start_tor_system_tunnel(port)
                    .map(|_| json!({ "started": true }))
                    .map_err(|e| format!("{e:#}"))
            }
            "tor_stop" => vpn_platform_linux::stop_tor_system_tunnel()
                .map(|_| json!({ "stopped": true }))
                .map_err(|e| format!("{e:#}")),

            "wg_start" => {
                let path = params.get("config_path").and_then(Value::as_str).unwrap_or("");
                if path.is_empty() {
                    Err("config_path required".into())
                } else {
                    match std::fs::read_to_string(path) {
                        Ok(text) => match vpn_platform_linux::parse_wireguard_config(&text) {
                            Ok(cfg) => vpn_platform_linux::start_wireguard_global(cfg)
                                .map(|_| json!({ "started": true }))
                                .map_err(|e| format!("{e:#}")),
                            Err(e) => Err(format!("parse: {e:#}")),
                        },
                        Err(e) => Err(format!("read {path}: {e}")),
                    }
                }
            }
            "wg_stop" => vpn_platform_linux::stop_wireguard_global()
                .map(|_| json!({ "stopped": true }))
                .map_err(|e| format!("{e:#}")),

            "ovpn_start" => {
                let profile = params.get("profile_path").and_then(Value::as_str).unwrap_or("");
                if profile.is_empty() {
                    Err("profile_path required".into())
                } else {
                    vpn_platform_linux::start_openvpn(profile, None)
                        .map(|_| json!({ "started": true }))
                        .map_err(|e| format!("{e:#}"))
                }
            }
            "ovpn_stop" => vpn_platform_linux::stop_openvpn()
                .map(|_| json!({ "stopped": true }))
                .map_err(|e| format!("{e:#}")),

            "pptp_start" => {
                let server = params.get("server").and_then(Value::as_str).unwrap_or("");
                let user = params.get("username").and_then(Value::as_str).unwrap_or("");
                let pass = params.get("password").and_then(Value::as_str).unwrap_or("");
                if server.is_empty() || user.is_empty() {
                    Err("server/username required".into())
                } else {
                    vpn_platform_linux::start_pptp(server, user, pass)
                        .map(|_| json!({ "started": true }))
                        .map_err(|e| format!("{e:#}"))
                }
            }
            "pptp_stop" => vpn_platform_linux::stop_pptp()
                .map(|_| json!({ "stopped": true }))
                .map_err(|e| format!("{e:#}")),

            "ss_start" => {
                let method = params.get("method").and_then(Value::as_str).unwrap_or("");
                let password = params.get("password").and_then(Value::as_str).unwrap_or("");
                let host = params.get("host").and_then(Value::as_str).unwrap_or("");
                let port = params.get("port").and_then(Value::as_u64).unwrap_or(443) as u16;
                let syswide = params.get("system_wide").and_then(Value::as_bool).unwrap_or(true);
                if host.is_empty() {
                    Err("host required".into())
                } else {
                    vpn_platform_linux::start_outline(method, password, host, port, syswide)
                        .map(|p| json!({ "port": p }))
                        .map_err(|e| format!("{e:#}"))
                }
            }
            "ss_stop" => vpn_platform_linux::stop_outline()
                .map(|_| json!({ "stopped": true }))
                .map_err(|e| format!("{e:#}")),

            "status" => Ok(json!({
                "tor_tunnel": vpn_platform_linux::is_tor_tunnel_running(),
                "wireguard": vpn_platform_linux::is_wireguard_running(),
                "openvpn": vpn_platform_linux::is_openvpn_running(),
                "pptp": vpn_platform_linux::is_pptp_running(),
                "outline": vpn_platform_linux::is_outline_running(),
            })),

            other => Err(format!("unknown command '{other}'")),
        };

        match result {
            Ok(mut v) => {
                if let Some(obj) = v.as_object_mut() {
                    obj.insert("ok".into(), Value::Bool(true));
                }
                v
            }
            Err(error) => json!({ "ok": false, "error": error }),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn dispatch_rejects_unknown_and_bad_json() {
            assert_eq!(dispatch("not json")["ok"], Value::Bool(false));
            assert_eq!(dispatch(r#"{"cmd":"nope"}"#)["ok"], Value::Bool(false));
            assert_eq!(dispatch(r#"{"cmd":"ping"}"#)["ok"], Value::Bool(true));
        }
    }
}

#[cfg(target_os = "linux")]
pub use imp::{available, run_daemon, send, HELPER_SOCK};

/// Non-Linux: helper is unavailable; callers fall back to platform-native paths.
#[cfg(not(target_os = "linux"))]
pub mod fallback_stub {
    use serde_json::{json, Value};

    pub const HELPER_SOCK: &str = "/run/zeronode-vpn.sock";

    pub fn send(_method: &str, _params: Value) -> Option<Value> {
        None
    }

    pub fn available() -> bool {
        false
    }
}

#[cfg(not(target_os = "linux"))]
pub use fallback_stub::{available, send, HELPER_SOCK};
