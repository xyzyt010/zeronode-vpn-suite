pub mod globe;
pub mod db;
pub mod helper;
pub mod ovpn;
pub mod protocols;
mod app;
mod tor_geo;

pub use app::{
    run_desktop, run_desktop_with_auto, run_desktop_with_auto_ex, run_desktop_with_options,
    DesktopAutoConnect,
};

/// Run the privileged helper daemon (`vpn-client --daemon`). Linux only —
/// blocks forever serving `/run/zeronode-vpn.sock`.
pub fn run_helper_daemon() -> anyhow::Result<()> {
    #[cfg(target_os = "linux")]
    {
        helper::run_daemon()
    }
    #[cfg(not(target_os = "linux"))]
    {
        anyhow::bail!("helper daemon is only available on Linux")
    }
}

#[cfg(target_os = "android")]
#[no_mangle]
pub fn android_main(app: winit::platform::android::activity::AndroidApp) {
    if let Err(error) = app::run_android(app) {
        eprintln!("android_main failed: {error:#}");
    }
}

#[cfg(target_os = "android")]
static ANDROID_JVM: std::sync::OnceLock<jni::JavaVM> = std::sync::OnceLock::new();

#[cfg(target_os = "android")]
fn android_register_jvm(env: &jni::JNIEnv) {
    if ANDROID_JVM.get().is_some() {
        return;
    }
    if let Ok(vm) = env.get_java_vm() {
        let _ = ANDROID_JVM.set(vm);
        vpn_platform_android::set_protect_fn(android_protect_fd);
    }
}

/// Called from WireGuard pump threads — must attach to JVM.
#[cfg(target_os = "android")]
fn android_protect_fd(fd: i32) -> bool {
    let Some(vm) = ANDROID_JVM.get() else {
        return false;
    };
    let Ok(mut env) = vm.attach_current_thread() else {
        return false;
    };
    let result = env.call_static_method(
        "io/zeronode/vpn/ZeroNodeVpnService",
        "protectSocket",
        "(I)Z",
        &[jni::objects::JValue::Int(fd)],
    );
    match result {
        Ok(v) => v.z().unwrap_or(false),
        Err(e) => {
            eprintln!("ZeroNode protectSocket JNI failed: {e:?}");
            let _ = env.exception_clear();
            false
        }
    }
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_io_zeronode_vpn_NativeBridge_nativeEnsureProtectBridge<'local>(
    env: jni::JNIEnv<'local>,
    _class: jni::objects::JClass<'local>,
) {
    android_register_jvm(&env);
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_io_zeronode_vpn_NativeBridge_nativePlatformSummary<'local>(
    env: jni::JNIEnv<'local>,
    _class: jni::objects::JClass<'local>,
) -> jni::sys::jstring {
    android_register_jvm(&env);
    let summary = vpn_platform_android::describe_client_platform();
    let text = format!(
        "{}; {}; {}",
        summary.service_model, summary.tunnel_backend, summary.ui_shell
    );
    match env.new_string(text) {
        Ok(value) => value.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_io_zeronode_vpn_NativeBridge_nativeDiscover<'local>(
    mut env: jni::JNIEnv<'local>,
    _class: jni::objects::JClass<'local>,
    hosts: jni::objects::JString<'local>,
) -> jni::sys::jstring {
    let hosts = match env.get_string(&hosts) {
        Ok(value) => value.to_string_lossy().to_string(),
        Err(error) => return java_string(env, format!("ERR\nmessage=invalid hosts: {error}")),
    };
    let result = match android_discover_blocking(hosts) {
        Ok(value) => value,
        Err(error) => format!("ERR\nmessage={error:#}"),
    };
    java_string(env, result)
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_io_zeronode_vpn_NativeBridge_nativeGetStatus<'local>(
    env: jni::JNIEnv<'local>,
    _class: jni::objects::JClass<'local>,
) -> jni::sys::jstring {
    let result = match android_get_status_blocking() {
        Ok(value) => value,
        Err(error) => format!("ERR\nmessage={error:#}"),
    };
    java_string(env, result)
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_io_zeronode_vpn_NativeBridge_nativeConnect<'local>(
    mut env: jni::JNIEnv<'local>,
    _class: jni::objects::JClass<'local>,
    host: jni::objects::JString<'local>,
    password: jni::objects::JString<'local>,
) -> jni::sys::jstring {
    let host = match env.get_string(&host) {
        Ok(value) => value.to_string_lossy().trim().to_owned(),
        Err(error) => return java_string(env, format!("ERR\nmessage=invalid host: {error}")),
    };
    let password = match env.get_string(&password) {
        Ok(value) => {
            let value = value.to_string_lossy().to_string();
            if value.is_empty() {
                None
            } else {
                Some(value)
            }
        }
        Err(error) => return java_string(env, format!("ERR\nmessage=invalid password: {error}")),
    };

    let result = match android_connect_blocking(host, password) {
        Ok(value) => value,
        Err(error) => format!("ERR\nmessage={error:#}"),
    };
    java_string(env, result)
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_io_zeronode_vpn_NativeBridge_nativeDisconnect<'local>(
    env: jni::JNIEnv<'local>,
    _class: jni::objects::JClass<'local>,
) -> jni::sys::jstring {
    let result = match android_disconnect_blocking() {
        Ok(value) => value,
        Err(error) => format!("ERR\nmessage={error:#}"),
    };
    java_string(env, result)
}

#[cfg(target_os = "android")]
fn java_string<'local>(env: jni::JNIEnv<'local>, value: String) -> jni::sys::jstring {
    match env.new_string(value) {
        Ok(value) => value.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

#[cfg(target_os = "android")]
fn android_discover_blocking(hosts: String) -> anyhow::Result<String> {
    use vpn_suite_core::{
        app_paths::client_paths,
        config::load_or_create_client_config,
        control_plane::{discover_servers, discover_servers_on_hosts},
    };

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let paths = client_paths()?;
        let config = load_or_create_client_config(&paths)?;
        let host_list: Vec<String> = hosts
            .split(',')
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect();

        let servers = if host_list.is_empty() {
            discover_servers(&config).await?
        } else {
            discover_servers_on_hosts(&config, &host_list).await?
        };

        if servers.is_empty() {
            return Ok(String::from("OK\ncount=0"));
        }

        let mut result = format!("OK\ncount={}", servers.len());
        for (i, server) in servers.iter().enumerate() {
            result.push_str(&format!(
                "\nserver.{i}.id={}\nserver.{i}.name={}\nserver.{i}.country_code={}\nserver.{i}.country_name={}\nserver.{i}.endpoint={}\nserver.{i}.wireguard_endpoint={}\nserver.{i}.has_password={}\nserver.{i}.public_key={}\nserver.{i}.online={}",
                server.server_id,
                server.name,
                server.country_code,
                server.country_name,
                server.endpoint,
                server.wireguard_endpoint,
                server.has_password,
                server.public_key,
                server.online,
            ));
        }
        Ok(result)
    })
}

#[cfg(target_os = "android")]
fn android_get_status_blocking() -> anyhow::Result<String> {
    use vpn_suite_core::{
        app_paths::client_paths,
        config::{load_or_create_client_config, load_or_create_client_state},
        control_plane::query_server_status,
        unix_now,
    };

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let paths = client_paths()?;
        let config = load_or_create_client_config(&paths)?;
        let state = load_or_create_client_state(&paths)?;
        let Some(active) = state.last_active_connection.as_ref() else {
            return Ok(String::from("OK\nphase=disconnected"));
        };

        let phase_str = match active.phase {
            vpn_suite_core::model::ConnectionPhase::Disconnected => "disconnected",
            vpn_suite_core::model::ConnectionPhase::Connecting => "connecting",
            vpn_suite_core::model::ConnectionPhase::Connected => "connected",
            vpn_suite_core::model::ConnectionPhase::Reconnecting => "reconnecting",
            vpn_suite_core::model::ConnectionPhase::Cooldown => "cooldown",
            vpn_suite_core::model::ConnectionPhase::Error => "error",
        };

        let mut result = format!(
            "OK\nphase={phase_str}\nserver_name={}\nserver_id={}\nendpoint={}",
            active.server_name, active.server_id, active.endpoint
        );

        if let Some(ip) = active.reserved_client_ip.as_deref() {
            result.push_str(&format!("\nclient_ip={ip}"));
        }
        if let Some(ip) = active.server_internal_ip.as_deref() {
            result.push_str(&format!("\nserver_ip={ip}"));
        }
        if let Some(sid) = active.session_id.as_deref() {
            result.push_str(&format!("\nsession_id={sid}"));
        }
        if let Some(at) = active.connected_at_unix {
            let elapsed = unix_now().saturating_sub(at);
            result.push_str(&format!("\nelapsed_secs={elapsed}"));
        }
        if let Some(until) = active.cooldown_until_unix {
            let remaining = until.saturating_sub(unix_now());
            result.push_str(&format!("\ncooldown_remaining_secs={remaining}"));
        }
        if let Some(profile) = active.tunnel_profile_path.as_deref() {
            result.push_str(&format!("\nprofile={profile}"));
        }

        if let Some(session_id) = active.session_id.as_deref() {
            if let Ok(status) = query_server_status(
                &active.endpoint,
                &active.server_id,
                Some(config.client_id.clone()),
                Some(session_id.to_owned()),
            )
            .await
            {
                result.push_str(&format!(
                    "\nserver_locked_down={}\nconnected_peers={}\nuptime_secs={}",
                    status.locked_down, status.connected_peers, status.uptime_secs
                ));
            }
        }

        Ok(result)
    })
}

#[cfg(target_os = "android")]
fn android_connect_blocking(host: String, password: Option<String>) -> anyhow::Result<String> {
    use vpn_suite_core::{
        app_paths::client_paths,
        config::{
            load_or_create_client_config, load_or_create_client_state, save_client_config,
            save_client_state,
        },
        control_plane::{attempt_auth, discover_servers_on_hosts, write_client_tunnel_artifact},
        model::{ActiveConnection, ConnectionPhase},
        unix_now,
        wireguard::build_client_artifact,
    };

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let paths = client_paths()?;
        let mut config = load_or_create_client_config(&paths)?;
        if !config.known_hosts.iter().any(|known| known == &host) {
            config.known_hosts.push(host.clone());
            save_client_config(&paths, &config)?;
        }

        let mut state = load_or_create_client_state(&paths)?;
        let server = discover_servers_on_hosts(&config, std::slice::from_ref(&host))
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("no reachable ZeroNode server at {host}"))?;
        let auth = attempt_auth(&config, &server.endpoint, &server.server_id, password).await?;
        if !auth.accepted {
            anyhow::bail!("{}", auth.message);
        }

        let lease = auth
            .lease
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("server accepted without returning a session lease"))?;
        let artifact = build_client_artifact(&config, &server, lease)?;
        let profile_path =
            write_client_tunnel_artifact(&paths.profiles_dir, &server.server_id, &artifact.contents)?;
        state.last_connected_server_id = Some(server.server_id.clone());
        state.last_tunnel_profile_path = Some(profile_path.clone());
        state.last_active_connection = Some(ActiveConnection {
            server_id: server.server_id.clone(),
            server_name: server.name.clone(),
            endpoint: server.endpoint.clone(),
            protocol: vpn_suite_core::model::VpnProtocol::WireGuard,
            phase: ConnectionPhase::Connected,
            connected_at_unix: Some(unix_now()),
            attempt_count: 0,
            session_id: Some(lease.session_id.clone()),
            reserved_client_ip: Some(lease.reserved_client_ip.clone()),
            server_internal_ip: Some(lease.server_internal_ip.clone()),
            tunnel_profile_path: Some(profile_path.clone()),
            cooldown_until_unix: None,
            tor_exit_info: None,
            country_code: None,
            last_status_unix: Some(unix_now()),
        });
        state.cooldowns.remove(&server.server_id);
        save_client_state(&paths, &state)?;

        Ok(format!(
            "OK\nserver={}\nserver_id={}\ncontrol={}\nwireguard={}\nclient_ip={}\nserver_ip={}\nsession={}\nprofile={}",
            server.name,
            server.server_id,
            server.endpoint,
            server.wireguard_endpoint,
            lease.reserved_client_ip,
            lease.server_internal_ip,
            lease.session_id,
            profile_path
        ))
    })
}

#[cfg(target_os = "android")]
fn android_disconnect_blocking() -> anyhow::Result<String> {
    use vpn_suite_core::{
        app_paths::client_paths,
        config::{load_or_create_client_config, load_or_create_client_state, save_client_state},
        control_plane::send_disconnect_notice,
    };

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let paths = client_paths()?;
        let config = load_or_create_client_config(&paths)?;
        let mut state = load_or_create_client_state(&paths)?;
        let Some(active) = state.last_active_connection.clone() else {
            return Ok(String::from("OK\nmessage=no cached active connection"));
        };
        if let Some(session_id) = active.session_id.as_deref() {
            send_disconnect_notice(
                &active.endpoint,
                &active.server_id,
                &config.client_id,
                session_id,
            )
            .await?;
        }
        state.last_active_connection = None;
        save_client_state(&paths, &state)?;
        Ok(format!(
            "OK\nmessage=disconnected from {}",
            active.server_name
        ))
    })
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_io_zeronode_vpn_NativeBridge_nativeStartPacketPump<'local>(
    mut env: jni::JNIEnv<'local>,
    _class: jni::objects::JClass<'local>,
    tun_fd: jni::sys::jint,
    profile: jni::objects::JString<'local>,
) -> jni::sys::jstring {
    let profile = match env.get_string(&profile) {
        Ok(value) => value.to_string_lossy().to_string(),
        Err(error) => return java_string(env, format!("ERR\nmessage=invalid profile: {error}")),
    };
    let result = match vpn_platform_android::start_wireguard(tun_fd, &profile) {
        Ok(()) => String::from("OK\nmessage=packet pump started"),
        Err(error) => format!("ERR\nmessage={error:#}"),
    };
    java_string(env, result)
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_io_zeronode_vpn_NativeBridge_nativeStopPacketPump<'local>(
    env: jni::JNIEnv<'local>,
    _class: jni::objects::JClass<'local>,
) -> jni::sys::jstring {
    vpn_platform_android::stop_all_tunnels();
    java_string(env, String::from("OK\nmessage=packet pump stop requested"))
}

#[cfg(target_os = "android")]
fn jstring_to_string(env: &mut jni::JNIEnv<'_>, value: &jni::objects::JString<'_>) -> Result<String, String> {
    env.get_string(value)
        .map(|v| v.to_string_lossy().to_string())
        .map_err(|e| e.to_string())
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_io_zeronode_vpn_NativeBridge_nativeStartTunnel<'local>(
    mut env: jni::JNIEnv<'local>,
    _class: jni::objects::JClass<'local>,
    tun_fd: jni::sys::jint,
    kind: jni::objects::JString<'local>,
    profile_or_key: jni::objects::JString<'local>,
    host: jni::objects::JString<'local>,
    port: jni::objects::JString<'local>,
    user: jni::objects::JString<'local>,
    password: jni::objects::JString<'local>,
    method: jni::objects::JString<'local>,
    extra: jni::objects::JString<'local>,
) -> jni::sys::jstring {
    android_register_jvm(&env);
    let kind = match jstring_to_string(&mut env, &kind) {
        Ok(v) => v,
        Err(e) => return java_string(env, format!("ERR\nmessage=invalid kind: {e}")),
    };
    let profile_or_key = jstring_to_string(&mut env, &profile_or_key).unwrap_or_default();
    let host = jstring_to_string(&mut env, &host).unwrap_or_default();
    let port_s = jstring_to_string(&mut env, &port).unwrap_or_default();
    let _user = jstring_to_string(&mut env, &user).unwrap_or_default();
    let password = jstring_to_string(&mut env, &password).unwrap_or_default();
    let method = jstring_to_string(&mut env, &method).unwrap_or_default();
    let extra = jstring_to_string(&mut env, &extra).unwrap_or_default();

    let result = android_start_tunnel(
        tun_fd,
        &kind,
        &profile_or_key,
        &host,
        &port_s,
        &password,
        &method,
        &extra,
    );
    java_string(env, result)
}

#[cfg(target_os = "android")]
fn parse_extra_udp_fd(extra: &str) -> i32 {
    for part in extra.split(|c| c == '\n' || c == ';' || c == ',') {
        let p = part.trim();
        if let Some(v) = p.strip_prefix("udp_fd=") {
            if let Ok(n) = v.trim().parse::<i32>() {
                return n;
            }
        }
    }
    -1
}

#[cfg(target_os = "android")]
fn android_start_tunnel(
    tun_fd: i32,
    kind: &str,
    profile_or_key: &str,
    host: &str,
    port_s: &str,
    password: &str,
    method: &str,
    extra: &str,
) -> String {
    use vpn_platform_android::AndroidTunnelKind;
    let kind = AndroidTunnelKind::from_str_label(kind);
    let udp_fd = parse_extra_udp_fd(extra);
    let outcome = match kind {
        AndroidTunnelKind::WireGuard | AndroidTunnelKind::ZeroNodeWireGuard => {
            vpn_platform_android::start_wireguard_ex(tun_fd, profile_or_key, udp_fd).map(|_| {
                format!("OK\nkind=wireguard\nmessage=wireguard pump started")
            })
        }
        AndroidTunnelKind::Outline => {
            let port: u16 = port_s.parse().unwrap_or(0);
            if host.is_empty() || port == 0 || method.is_empty() || password.is_empty() {
                // Try parsing profile_or_key as ss:// if fields incomplete
                match vpn_suite_core::outline::parse_access_key(profile_or_key) {
                    Ok(ep) => vpn_platform_android::start_outline(
                        &ep.method,
                        &ep.password,
                        &ep.host,
                        ep.port,
                        tun_fd,
                    )
                    .map(|socks| format!("OK\nkind=outline\nsocks_port={socks}")),
                    Err(e) => Err(e),
                }
            } else {
                vpn_platform_android::start_outline(method, password, host, port, tun_fd)
                    .map(|socks| format!("OK\nkind=outline\nsocks_port={socks}"))
            }
        }
        AndroidTunnelKind::Tor => {
            // Tor SOCKS must already be up; attach system tunnel only.
            vpn_platform_android::start_tor_system_tunnel(tun_fd)
                .map(|_| String::from("OK\nkind=tor\nmessage=system tunnel attached"))
        }
        AndroidTunnelKind::Pptp => {
            // GRE raw sockets unavailable in unprivileged apps — never fake ACTIVE.
            vpn_platform_android::start_pptp(host, "", password, tun_fd)
                .map(|_| String::from("OK\nkind=pptp"))
        }
    };
    match outcome {
        Ok(msg) => msg,
        Err(error) => format!("ERR\nmessage={error:#}"),
    }
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_io_zeronode_vpn_NativeBridge_nativeStopTunnel<'local>(
    env: jni::JNIEnv<'local>,
    _class: jni::objects::JClass<'local>,
) -> jni::sys::jstring {
    // Keep Tor SOCKS alive so system-tunnel attach can succeed.
    vpn_platform_android::stop_all_tunnels();
    java_string(env, String::from("OK\nmessage=data planes stopped (tor kept)"))
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_io_zeronode_vpn_NativeBridge_nativeStopEverything<'local>(
    env: jni::JNIEnv<'local>,
    _class: jni::objects::JClass<'local>,
) -> jni::sys::jstring {
    vpn_platform_android::stop_everything();
    java_string(env, String::from("OK\nmessage=all tunnels and tor stopped"))
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_io_zeronode_vpn_NativeBridge_nativeStartTorSocks<'local>(
    mut env: jni::JNIEnv<'local>,
    _class: jni::objects::JClass<'local>,
    tor_home: jni::objects::JString<'local>,
    native_lib_dir: jni::objects::JString<'local>,
) -> jni::sys::jstring {
    let tor_home = match jstring_to_string(&mut env, &tor_home) {
        Ok(v) => v,
        Err(e) => return java_string(env, format!("ERR\nmessage=invalid tor_home: {e}")),
    };
    let native_lib_dir = match jstring_to_string(&mut env, &native_lib_dir) {
        Ok(v) => v,
        Err(e) => return java_string(env, format!("ERR\nmessage=invalid native_lib_dir: {e}")),
    };
    let result = match vpn_platform_android::start_tor_socks(
        std::path::Path::new(&tor_home),
        std::path::Path::new(&native_lib_dir),
    ) {
        Ok(port) => format!("OK\nsocks_port={port}\nmessage=tor socks ready"),
        Err(error) => format!("ERR\nmessage={error:#}"),
    };
    java_string(env, result)
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_io_zeronode_vpn_NativeBridge_nativeAttachTorSystemTunnel<'local>(
    env: jni::JNIEnv<'local>,
    _class: jni::objects::JClass<'local>,
    tun_fd: jni::sys::jint,
) -> jni::sys::jstring {
    let result = match vpn_platform_android::start_tor_system_tunnel(tun_fd) {
        Ok(()) => String::from("OK\nmessage=tor system tunnel attached"),
        Err(error) => format!("ERR\nmessage={error:#}"),
    };
    java_string(env, result)
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_io_zeronode_vpn_NativeBridge_nativeParseOutline<'local>(
    mut env: jni::JNIEnv<'local>,
    _class: jni::objects::JClass<'local>,
    key: jni::objects::JString<'local>,
) -> jni::sys::jstring {
    let key = match jstring_to_string(&mut env, &key) {
        Ok(v) => v,
        Err(e) => return java_string(env, format!("ERR\nmessage={e}")),
    };
    let result = match vpn_suite_core::outline::parse_access_key(&key)
        .or_else(|_| vpn_suite_core::outline::parse_outline_json(&key))
    {
        Ok(ep) => format!(
            "OK\nmethod={}\nhost={}\nport={}\npassword={}\nname={}",
            ep.method,
            ep.host,
            ep.port,
            ep.password,
            ep.name.unwrap_or_default()
        ),
        Err(error) => format!("ERR\nmessage={error:#}"),
    };
    java_string(env, result)
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_io_zeronode_vpn_NativeBridge_nativeParseWireGuard<'local>(
    mut env: jni::JNIEnv<'local>,
    _class: jni::objects::JClass<'local>,
    conf: jni::objects::JString<'local>,
) -> jni::sys::jstring {
    let conf = match jstring_to_string(&mut env, &conf) {
        Ok(v) => v,
        Err(e) => return java_string(env, format!("ERR\nmessage={e}")),
    };
    let mut endpoint = None;
    let mut address = None;
    let mut public_key = None;
    let mut has_private = false;
    for line in conf.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            match k.trim() {
                "PrivateKey" if !v.trim().is_empty() => has_private = true,
                "Endpoint" => endpoint = Some(v.trim().to_string()),
                "Address" => address = Some(v.trim().to_string()),
                "PublicKey" => public_key = Some(v.trim().to_string()),
                _ => {}
            }
        }
    }
    if !has_private || endpoint.is_none() {
        return java_string(
            env,
            String::from("ERR\nmessage=WireGuard config needs PrivateKey and Endpoint"),
        );
    }
    java_string(
        env,
        format!(
            "OK\nendpoint={}\naddress={}\npublic_key={}",
            endpoint.unwrap_or_default(),
            address.unwrap_or_default(),
            public_key.unwrap_or_default()
        ),
    )
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_io_zeronode_vpn_NativeBridge_nativeGetProgress<'local>(
    env: jni::JNIEnv<'local>,
    _class: jni::objects::JClass<'local>,
) -> jni::sys::jstring {
    let p = vpn_platform_android::get_progress();
    java_string(
        env,
        format!(
            "OK\nstage={}\nfraction={}\ndetail={}",
            p.stage, p.fraction, p.detail
        ),
    )
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_io_zeronode_vpn_NativeBridge_nativeTorBootstrap<'local>(
    env: jni::JNIEnv<'local>,
    _class: jni::objects::JClass<'local>,
) -> jni::sys::jstring {
    java_string(env, vpn_platform_android::tor_bootstrap_hint())
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_io_zeronode_vpn_NativeBridge_nativeOutlineSocksPort<'local>(
    env: jni::JNIEnv<'local>,
    _class: jni::objects::JClass<'local>,
) -> jni::sys::jstring {
    let port = vpn_platform_android::outline_socks_port().unwrap_or(0);
    java_string(env, port.to_string())
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_io_zeronode_vpn_NativeBridge_nativeFetchPublicIp<'local>(
    env: jni::JNIEnv<'local>,
    _class: jni::objects::JClass<'local>,
) -> jni::sys::jstring {
    let result = match android_fetch_public_ip() {
        Ok(v) => v,
        Err(error) => format!("ERR\nmessage={error:#}"),
    };
    java_string(env, result)
}

#[cfg(target_os = "android")]
fn android_http_get_or_wget(url: &str) -> Option<String> {
    std::process::Command::new("/system/bin/toybox")
        .args(["wget", "-qO-", url])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .or_else(|| android_http_get(url).ok())
}

#[cfg(target_os = "android")]
fn android_format_geo(v: &serde_json::Value, fallback_ip: &str) -> String {
    let ip = v
        .get("query")
        .and_then(|x| x.as_str())
        .or_else(|| v.get("ip").and_then(|x| x.as_str()))
        .unwrap_or(fallback_ip);
    let country = v
        .get("country")
        .and_then(|x| x.as_str())
        .or_else(|| v.get("country_name").and_then(|x| x.as_str()))
        .unwrap_or("");
    let code = v
        .get("countryCode")
        .and_then(|x| x.as_str())
        .or_else(|| v.get("country_code").and_then(|x| x.as_str()))
        .unwrap_or("");
    let city = v.get("city").and_then(|x| x.as_str()).unwrap_or("");
    let lat = v
        .get("lat")
        .and_then(|x| x.as_f64())
        .or_else(|| v.get("latitude").and_then(|x| x.as_f64()))
        .unwrap_or(0.0);
    let lon = v
        .get("lon")
        .and_then(|x| x.as_f64())
        .or_else(|| v.get("longitude").and_then(|x| x.as_f64()))
        .unwrap_or(0.0);
    let isp = v
        .get("isp")
        .and_then(|x| x.as_str())
        .or_else(|| v.get("org").and_then(|x| x.as_str()))
        .unwrap_or("");
    format!(
        "OK\nip={ip}\ncountry={country}\ncountry_code={code}\ncity={city}\nlat={lat}\nlon={lon}\nisp={isp}"
    )
}

/// Windows-parity public IP: echo the address first, then reverse-geolocate
/// that specific IP so a leaked GeoIP request cannot swap in the device IP.
#[cfg(target_os = "android")]
fn android_fetch_public_ip() -> anyhow::Result<String> {
    let bust = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);

    let mut confirmed = String::new();
    for url in [
        format!("http://api.ipify.org?format=text&_={bust}"),
        format!("http://icanhazip.com?_={bust}"),
        format!("http://ifconfig.me/ip?_={bust}"),
    ] {
        if let Some(body) = android_http_get_or_wget(&url) {
            let ip = body
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim()
                .to_string();
            if ip.contains('.') && !ip.contains('<') {
                confirmed = ip;
                break;
            }
        }
    }

    let geo_urls = if confirmed.is_empty() {
        vec![format!(
            "http://ip-api.com/json/?fields=status,message,query,country,countryCode,city,lat,lon,isp&_={bust}"
        )]
    } else {
        vec![
            format!(
                "http://ip-api.com/json/{confirmed}?fields=status,message,query,country,countryCode,city,lat,lon,isp&_={bust}"
            ),
            format!("http://ipwho.is/{confirmed}?_={bust}"),
        ]
    };

    for url in geo_urls {
        if let Some(body) = android_http_get_or_wget(&url) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(body.trim()) {
                let status = v.get("status").and_then(|s| s.as_str()).unwrap_or("success");
                if status == "fail" {
                    continue;
                }
                let formatted = android_format_geo(&v, &confirmed);
                if formatted.contains("ip=") && !formatted.ends_with("ip=\n") {
                    return Ok(formatted);
                }
            }
        }
    }

    if !confirmed.is_empty() {
        return Ok(format!(
            "OK\nip={confirmed}\ncountry=\ncountry_code=\ncity=\nlat=0\nlon=0\nisp="
        ));
    }
    anyhow::bail!("public IP fetch failed")
}

#[cfg(target_os = "android")]
fn android_http_get(url: &str) -> anyhow::Result<String> {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    // Only supports http://host/path
    let url = url
        .strip_prefix("http://")
        .ok_or_else(|| anyhow::anyhow!("only http supported for fallback"))?;
    let (host_port, path) = url
        .split_once('/')
        .map(|(h, p)| (h, format!("/{p}")))
        .unwrap_or((url, String::from("/")));
    let host = host_port.split(':').next().unwrap_or(host_port);
    let mut stream = TcpStream::connect((host, 80))?;
    stream.set_read_timeout(Some(std::time::Duration::from_secs(8)))?;
    stream.set_write_timeout(Some(std::time::Duration::from_secs(5)))?;
    let req = format!(
        "GET {path} HTTP/1.0\r\nHost: {host}\r\nUser-Agent: ZeroNodeVPN/0.1\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(req.as_bytes())?;
    let mut buf = String::new();
    stream.read_to_string(&mut buf)?;
    let body = buf
        .split("\r\n\r\n")
        .nth(1)
        .or_else(|| buf.split("\n\n").nth(1))
        .unwrap_or(&buf)
        .to_string();
    Ok(body)
}
