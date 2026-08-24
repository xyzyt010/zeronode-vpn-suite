//! Embedded WireGuard tunnel for Windows using boringtun + wintun.
//! No external wireguard.exe needed — the tunnel runs entirely in-process.
//! Global lifecycle: one tunnel at a time, stored in a process-wide static.

use anyhow::{bail, Context, Result};
use boringtun::noise::{Tunn, TunnResult};
use boringtun::x25519::{PublicKey, StaticSecret};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

/// 512 KB ring — well under the 15 MB creation budget.
/// (wintun::MAX_RING_CAPACITY is 64 MB, way too large.)
const WINTUN_RING_CAPACITY: u32 = 0x80000;

/// Max IP packet through the tunnel. WireGuard MTU is typically 1420.
/// 2 KB buffer covers any single packet plus WG overhead.
const PKT_BUF_SIZE: usize = 2048;

const ADAPTER_NAME: &str = "ZeroNodeVPN";
const ADAPTER_DESC: &str = "ZeroNode VPN Tunnel";

static GLOBAL_TUNNEL: OnceLock<Mutex<Option<WireGuardTunnel>>> = OnceLock::new();

fn global_tunnel() -> &'static Mutex<Option<WireGuardTunnel>> {
    GLOBAL_TUNNEL.get_or_init(|| Mutex::new(None))
}

/// Configuration for a WireGuard tunnel (client-side).
#[derive(Clone, Debug)]
pub struct TunnelConfig {
    pub private_key: [u8; 32],
    pub server_public_key: [u8; 32],
    /// Optional pre-shared key (PresharedKey in conf). Required when the peer
    /// has PSK enabled — without it the handshake completes but data fails.
    pub preshared_key: Option<[u8; 32]>,
    pub server_endpoint: SocketAddr,
    pub tunnel_ip: IpAddr,
    /// Prefix length from Address= (e.g. 32 for 10.0.0.2/32). Used for netmask.
    pub tunnel_prefix: u8,
    /// Optional IPv6 tunnel address. Reserved for future dual-stack
    /// WireGuard support; currently the v4 `tunnel_ip` is the only address
    /// installed on the interface.
    #[allow(dead_code)]
    pub tunnel_ipv6: Option<IpAddr>,
    pub dns: Vec<IpAddr>,
    pub mtu: u16,
    pub allowed_ips: Vec<String>,
    /// PersistentKeepalive seconds (0 = disabled). Default 25 if omitted in conf.
    pub persistent_keepalive: u16,
}

/// A running WireGuard tunnel using boringtun + wintun.
pub struct WireGuardTunnel {
    running: Arc<AtomicBool>,
    config: TunnelConfig,
}

impl WireGuardTunnel {
    pub fn new(config: TunnelConfig) -> Result<Self> {
        Ok(Self {
            running: Arc::new(AtomicBool::new(false)),
            config,
        })
    }

    pub fn start(&self) -> Result<()> {
        if self.running.swap(true, Ordering::SeqCst) {
            bail!("tunnel already running");
        }

        let wintun = unsafe { wintun::load() }
            .context("failed to load wintun.dll — ensure WireGuard/Wintun driver is installed")?;

        // Reuse a leftover adapter from a previous crash; otherwise create fresh.
        let adapter = match wintun::Adapter::open(&wintun, ADAPTER_NAME) {
            Ok(a) => a,
            Err(_) => wintun::Adapter::create(&wintun, ADAPTER_NAME, ADAPTER_DESC, None)
                .context("failed to create wintun adapter — try running as Administrator")?,
        };

        let if_index = adapter
            .get_adapter_index()
            .context("failed to read Wintun interface index")?;

        let tunnel_ip = match self.config.tunnel_ip {
            IpAddr::V4(ip) => ip,
            IpAddr::V6(_) => bail!("IPv4 tunnel address required for primary"),
        };

        configure_adapter_ip(tunnel_ip, self.config.tunnel_prefix)?;
        set_adapter_mtu(self.config.mtu);
        set_interface_metric(if_index, 1);
        configure_dns(&self.config.dns);
        add_tunnel_routes(
            &self.config.allowed_ips,
            tunnel_ip,
            self.config.tunnel_prefix,
            if_index,
            &self.config.server_endpoint,
        )?;

        let session = Arc::new(
            adapter
                .start_session(WINTUN_RING_CAPACITY)
                .context("failed to start wintun session")?,
        );

        let private_key = StaticSecret::from(self.config.private_key);
        let server_public = PublicKey::from(self.config.server_public_key);
        let keepalive = if self.config.persistent_keepalive == 0 {
            None
        } else {
            Some(self.config.persistent_keepalive)
        };
        let tunnel = Box::new(Tunn::new(
            private_key,
            server_public,
            self.config.preshared_key,
            keepalive,
            0,
            None,
        ));

        let running = Arc::clone(&self.running);
        let server_endpoint = self.config.server_endpoint;

        thread::Builder::new()
            .name("wireguard-pump".into())
            .stack_size(256 * 1024)
            .spawn(move || {
                if let Err(e) = run_packet_pump(tunnel, session, server_endpoint, &running) {
                    eprintln!("wireguard tunnel pump error: {e:#}");
                }
                running.store(false, Ordering::SeqCst);
            })
            .context("failed to spawn tunnel thread")?;

        eprintln!(
            "WireGuard tunnel started: endpoint={server_endpoint} tunnel_ip={tunnel_ip}/{} if={if_index}",
            self.config.tunnel_prefix
        );
        Ok(())
    }

    pub fn stop(&self) -> Result<()> {
        if !self.running.swap(false, Ordering::SeqCst) {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(200));

        remove_tunnel_routes(&self.config.allowed_ips, &self.config.server_endpoint);
        remove_adapter_dns();

        let _ = command_no_window("netsh")
            .args([
                "interface",
                "ipv4",
                "set",
                "address",
                &format!("name={ADAPTER_NAME}"),
                "source=dhcp",
            ])
            .output();

        eprintln!("WireGuard tunnel stopped");
        Ok(())
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}

impl Drop for WireGuardTunnel {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

// ---------------------------------------------------------------------------
// Global lifecycle — one tunnel at a time
// ---------------------------------------------------------------------------

/// Start a tunnel and store the handle globally. Stops any previous tunnel.
pub fn start_global(config: TunnelConfig) -> Result<()> {
    let guard = global_tunnel();
    let mut slot = guard.lock().unwrap();

    if let Some(old) = slot.take() {
        let _ = old.stop();
    }

    let tunnel = WireGuardTunnel::new(config)?;
    tunnel.start()?;
    *slot = Some(tunnel);
    Ok(())
}

/// Stop the globally-held tunnel (if any).
pub fn stop_global() -> Result<()> {
    let guard = global_tunnel();
    let mut slot = guard.lock().unwrap();
    if let Some(tunnel) = slot.take() {
        tunnel.stop()?;
    }
    Ok(())
}

/// Check whether the global tunnel is currently running.
pub fn is_global_running() -> bool {
    let guard = global_tunnel();
    let slot = guard.lock().unwrap();
    slot.as_ref().map(|t| t.is_running()).unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Adapter / route helpers
// ---------------------------------------------------------------------------

fn prefix_to_mask(prefix: u8) -> Ipv4Addr {
    let bits = prefix.min(32) as u32;
    let mask = if bits == 0 {
        0u32
    } else {
        !0u32 << (32 - bits)
    };
    Ipv4Addr::from(mask)
}

fn configure_adapter_ip(ip: Ipv4Addr, prefix: u8) -> Result<()> {
    let ip_str = ip.to_string();
    // /32 is common in WG client confs. Windows needs a broader mask so the
    // peer "gateway" (.1) is on-link when we use classic next-hop routes; for
    // pure IF-index on-link routes a /32 is fine. Prefer the conf prefix, but
    // widen /32 → /24 so .1-style gateways still work as a fallback.
    let mask = if prefix >= 32 {
        Ipv4Addr::new(255, 255, 255, 0) // /24 for peer reachability
    } else {
        prefix_to_mask(prefix)
    };
    let mask_str = mask.to_string();
    let mut last_err = String::new();

    // Retry: Windows needs a moment to register the new interface name.
    for _attempt in 1..=20 {
        let output = command_no_window("netsh")
            .args([
                "interface",
                "ipv4",
                "set",
                "address",
                &format!("name={ADAPTER_NAME}"),
                "source=static",
                &format!("address={ip_str}"),
                &format!("mask={mask_str}"),
            ])
            .output();

        match output {
            Ok(output) if output.status.success() => {
                return Ok(());
            }
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
                last_err = if !stderr.is_empty() {
                    stderr
                } else {
                    stdout
                };
            }
            Err(e) => {
                last_err = e.to_string();
            }
        }
        thread::sleep(Duration::from_millis(150));
    }

    bail!("failed to set tunnel IP address after retries: {last_err}");
}

fn set_adapter_mtu(mtu: u16) {
    let _ = command_no_window("netsh")
        .args([
            "interface",
            "ipv4",
            "set",
            "subinterface",
            ADAPTER_NAME,
            &format!("mtu={mtu}"),
            "store=active",
        ])
        .output();
}

/// Prefer the tunnel interface for default-bound traffic.
fn set_interface_metric(if_index: u32, metric: u32) {
    let _ = command_no_window("netsh")
        .args([
            "interface",
            "ipv4",
            "set",
            "interface",
            &if_index.to_string(),
            &format!("metric={metric}"),
        ])
        .output();
}

fn configure_dns(dns: &[IpAddr]) {
    if dns.is_empty() {
        return;
    }
    let _ = command_no_window("netsh")
        .args([
            "interface",
            "ipv4",
            "set",
            "dns",
            &format!("name={ADAPTER_NAME}"),
            "source=static",
            &format!("address={}", dns[0]),
            "register=primary",
        ])
        .output();

    for extra in dns.iter().skip(1) {
        let _ = command_no_window("netsh")
            .args([
                "interface",
                "ipv4",
                "add",
                "dns",
                &format!("name={ADAPTER_NAME}"),
                &format!("address={extra}"),
                "index=2",
            ])
            .output();
    }
}

fn remove_adapter_dns() {
    let _ = command_no_window("netsh")
        .args([
            "interface",
            "ipv4",
            "set",
            "dns",
            &format!("name={ADAPTER_NAME}"),
            "source=dhcp",
        ])
        .output();
}

fn command_no_window(program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    use std::process::Stdio;
    let mut command = std::process::Command::new(program);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    // Redirect stdio so console-subsystem tools (route, netsh) never flash a window.
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    command
}

/// Add routes for AllowedIPs through the tunnel adapter.
///
/// Full tunnel (`0.0.0.0/0`) uses two /1 routes so we never wipe the system
/// default route. Encrypted traffic to the peer endpoint is pinned via the
/// physical default gateway. All tunnel routes bind to the Wintun `IF` index
/// so Windows does not need a reachable L3 next-hop on the virtual NIC.
fn add_tunnel_routes(
    allowed_ips: &[String],
    tunnel_ip: Ipv4Addr,
    _tunnel_prefix: u8,
    if_index: u32,
    server_endpoint: &SocketAddr,
) -> Result<()> {
    let physical_gw = detect_default_gateway();
    let if_str = if_index.to_string();

    // Host route for the WireGuard server endpoint through the real gateway
    // (must NOT go through the tunnel or we black-hole the encrypted path).
    if let IpAddr::V4(srv_ip) = server_endpoint.ip() {
        if let Some(gw_ip) = physical_gw.as_ref() {
            let _ = command_no_window("route")
                .args([
                    "add",
                    &srv_ip.to_string(),
                    "mask",
                    "255.255.255.255",
                    &gw_ip.to_string(),
                    "metric",
                    "1",
                ])
                .output();
        }
    }

    let is_full_tunnel = allowed_ips.iter().any(|a| {
        let trimmed = a.trim();
        trimmed == "0.0.0.0/0"
            || trimmed.starts_with("0.0.0.0/0")
            || trimmed.split(',').any(|p| p.trim() == "0.0.0.0/0")
    });

    // On-link next hop via the tunnel interface. Prefer classic .1 peer address
    // when it is on the same /24 we installed; fall back to 0.0.0.0 on-link.
    let octets = tunnel_ip.octets();
    let peer_guess = Ipv4Addr::new(octets[0], octets[1], octets[2], 1);
    let next_hop = if peer_guess != tunnel_ip {
        peer_guess.to_string()
    } else {
        String::from("0.0.0.0")
    };

    let add_via_if = |net: &str, mask: &str| {
        // Prefer IF-bound route (works even when next-hop is not ARP-able).
        let r1 = command_no_window("route")
            .args([
                "add",
                net,
                "mask",
                mask,
                &next_hop,
                "metric",
                "1",
                "IF",
                &if_str,
            ])
            .output();
        let ok = r1.as_ref().map(|o| o.status.success()).unwrap_or(false);
        if !ok {
            let _ = command_no_window("route")
                .args(["add", net, "mask", mask, "0.0.0.0", "metric", "1", "IF", &if_str])
                .output();
        }
    };

    if is_full_tunnel || allowed_ips.is_empty() {
        // Two /1 routes cover all IPv4 without replacing 0.0.0.0/0.
        add_via_if("0.0.0.0", "128.0.0.0");
        add_via_if("128.0.0.0", "128.0.0.0");
    } else {
        for cidr in allowed_ips {
            for single in cidr.split(',') {
                let single = single.trim();
                if single.is_empty() || single.contains(':') {
                    continue; // IPv6 — skip for now
                }
                let (net, mask) = cidr_to_net_mask(single);
                add_via_if(&net, &mask);
            }
        }
    }

    Ok(())
}

fn remove_tunnel_routes(allowed_ips: &[String], server_endpoint: &SocketAddr) {
    if let IpAddr::V4(srv_ip) = server_endpoint.ip() {
        let _ = command_no_window("route")
            .args(["delete", &srv_ip.to_string(), "mask", "255.255.255.255"])
            .output();
    }

    let is_full_tunnel = allowed_ips.is_empty()
        || allowed_ips.iter().any(|a| a.trim().contains("0.0.0.0/0"));

    if is_full_tunnel {
        for net in ["0.0.0.0", "128.0.0.0"] {
            let _ = command_no_window("route")
                .args(["delete", net, "mask", "128.0.0.0"])
                .output();
        }
    } else {
        for cidr in allowed_ips {
            for single in cidr.split(',') {
                let single = single.trim();
                if single.is_empty() || single.contains(':') {
                    continue;
                }
                let (net, mask) = cidr_to_net_mask(single);
                let _ = command_no_window("route")
                    .args(["delete", &net, "mask", &mask])
                    .output();
            }
        }
    }
}

/// Detect default gateway without PowerShell (avoids console flashes).
/// Parses `route print -4` with CREATE_NO_WINDOW.
fn detect_default_gateway() -> Option<Ipv4Addr> {
    let output = command_no_window("route")
        .args(["print", "-4"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    // Active routes table: Network Destination | Netmask | Gateway | Interface | Metric
    // Look for 0.0.0.0  0.0.0.0  <gateway>
    for line in text.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        if cols.len() >= 3 && cols[0] == "0.0.0.0" && cols[1] == "0.0.0.0" {
            if let Ok(gw) = cols[2].parse::<Ipv4Addr>() {
                if !gw.is_unspecified() {
                    return Some(gw);
                }
            }
        }
    }
    None
}

fn cidr_to_net_mask(cidr: &str) -> (String, String) {
    if let Some((net, prefix)) = cidr.split_once('/') {
        let bits: u32 = prefix.parse().unwrap_or(32);
        let mask = if bits == 0 {
            0u32
        } else {
            !0u32 << (32 - bits)
        };
        let m = Ipv4Addr::from(mask);
        (net.to_string(), m.to_string())
    } else {
        (cidr.to_string(), "255.255.255.255".to_string())
    }
}

// ---------------------------------------------------------------------------
// Packet pump — handshake immediately, honor timers, flush after decapsulate
// ---------------------------------------------------------------------------

fn run_packet_pump(
    mut tunnel: Box<Tunn>,
    session: Arc<wintun::Session>,
    server_endpoint: SocketAddr,
    running: &AtomicBool,
) -> Result<()> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0")
        .context("failed to bind UDP socket for WireGuard")?;
    socket.set_nonblocking(true)?;
    socket.set_read_timeout(Some(Duration::from_millis(5)))?;
    socket.set_write_timeout(Some(Duration::from_secs(5)))?;
    socket
        .connect(server_endpoint)
        .with_context(|| format!("failed to connect UDP to {server_endpoint}"))?;

    // dst must be ≥ src + 32 and ≥ 148 (handshake size).
    let mut buf = vec![0u8; PKT_BUF_SIZE];
    let mut wg_buf = vec![0u8; PKT_BUF_SIZE];
    let mut idle_streak: u32 = 0;
    let mut last_timer = Instant::now();

    // Immediate handshake so the tunnel is usable without waiting for traffic.
    send_handshake(&mut tunnel, &socket, &mut wg_buf, true);
    let mut handshake_force_at = Instant::now();

    while running.load(Ordering::Relaxed) {
        let mut did_work = false;

        // 1. Drain wintun → encrypt → UDP
        loop {
            match session.try_receive() {
                Ok(Some(packet)) => {
                    let bytes = packet.bytes();
                    if bytes.is_empty() {
                        continue;
                    }
                    match tunnel.encapsulate(bytes, &mut wg_buf) {
                        TunnResult::WriteToNetwork(enc) => {
                            let _ = socket.send(enc);
                        }
                        TunnResult::Err(e) => eprintln!("wg encapsulate error: {e:?}"),
                        _ => {}
                    }
                    did_work = true;
                }
                Ok(None) => break,
                Err(e) => {
                    eprintln!("wintun recv error: {e}");
                    break;
                }
            }
        }

        // 2. Drain UDP → decrypt → wintun (flush WriteToNetwork queue)
        loop {
            match socket.recv(&mut buf) {
                Ok(n) if n > 0 => {
                    handle_udp_datagram(&mut tunnel, &session, &socket, &buf[..n], &mut wg_buf);
                    did_work = true;
                }
                Ok(_) => break,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => break,
                Err(e) => {
                    eprintln!("UDP recv error: {e}");
                    break;
                }
            }
        }

        // 3. Timers (~every 100ms): keepalives, rekey, handshake retry.
        //    Previous code discarded update_timers() — so handshakes never retried
        //    and PersistentKeepalive never fired.
        if last_timer.elapsed() >= Duration::from_millis(100) {
            match tunnel.update_timers(&mut wg_buf) {
                TunnResult::WriteToNetwork(pkt) => {
                    let _ = socket.send(pkt);
                    // Flush any further timer-driven packets.
                    loop {
                        match tunnel.update_timers(&mut wg_buf) {
                            TunnResult::WriteToNetwork(p) => {
                                let _ = socket.send(p);
                            }
                            TunnResult::Done => break,
                            TunnResult::Err(e) => {
                                eprintln!("wg timer error: {e:?}");
                                // ConnectionExpired → force a fresh handshake.
                                send_handshake(&mut tunnel, &socket, &mut wg_buf, true);
                                break;
                            }
                            _ => break,
                        }
                    }
                    did_work = true;
                }
                TunnResult::Err(e) => {
                    eprintln!("wg timer error: {e:?}");
                    send_handshake(&mut tunnel, &socket, &mut wg_buf, true);
                }
                _ => {}
            }
            last_timer = Instant::now();
        }

        // 4. Safety-net re-handshake if nothing established for a while.
        if handshake_force_at.elapsed() >= Duration::from_secs(5) {
            send_handshake(&mut tunnel, &socket, &mut wg_buf, false);
            handshake_force_at = Instant::now();
        }

        // Adaptive sleep: busy when traffic flows, back off when idle.
        if did_work {
            idle_streak = 0;
        } else {
            idle_streak = idle_streak.saturating_add(1);
            let sleep_ms = match idle_streak {
                0..=50 => 0,
                51..=200 => 1,
                _ => 5,
            };
            if sleep_ms > 0 {
                thread::sleep(Duration::from_millis(sleep_ms));
            } else {
                thread::yield_now();
            }
        }
    }

    Ok(())
}

fn send_handshake(
    tunnel: &mut Tunn,
    socket: &std::net::UdpSocket,
    wg_buf: &mut [u8],
    force: bool,
) {
    match tunnel.format_handshake_initiation(wg_buf, force) {
        TunnResult::WriteToNetwork(msg) => {
            if let Err(e) = socket.send(msg) {
                eprintln!("wg handshake send failed: {e}");
            } else {
                eprintln!("wg handshake initiation sent (force={force})");
            }
        }
        TunnResult::Done => {}
        TunnResult::Err(e) => eprintln!("wg handshake error: {e:?}"),
        _ => {}
    }
}

/// Process one UDP datagram from the peer, then flush boringtun's WriteToNetwork queue.
fn handle_udp_datagram(
    tunnel: &mut Tunn,
    session: &Arc<wintun::Session>,
    socket: &std::net::UdpSocket,
    datagram: &[u8],
    wg_buf: &mut [u8],
) {
    let mut first = true;
    loop {
        let result = if first {
            first = false;
            tunnel.decapsulate(None, datagram, wg_buf)
        } else {
            // boringtun: after WriteToNetwork, call again with empty datagram
            // until Done — drains queued packets after handshake response.
            tunnel.decapsulate(None, &[], wg_buf)
        };

        match result {
            TunnResult::WriteToTunnelV4(dec, _) | TunnResult::WriteToTunnelV6(dec, _) => {
                inject_to_wintun(session, dec);
                // Keep flushing; more packets may be queued.
            }
            TunnResult::WriteToNetwork(enc) => {
                let _ = socket.send(enc);
            }
            TunnResult::Done => break,
            TunnResult::Err(e) => {
                eprintln!("wg decapsulate error: {e:?}");
                break;
            }
        }
    }
}

fn inject_to_wintun(session: &Arc<wintun::Session>, data: &[u8]) {
    if data.is_empty() {
        return;
    }
    let len = data.len() as u16;
    match session.allocate_send_packet(len) {
        Ok(mut pkt) => {
            let buf = pkt.bytes_mut();
            let n = data.len().min(buf.len());
            buf[..n].copy_from_slice(&data[..n]);
            session.send_packet(pkt);
        }
        Err(e) => eprintln!("wintun send alloc error: {e}"),
    }
}

// ---------------------------------------------------------------------------
// Config parser
// ---------------------------------------------------------------------------

/// Parse a WireGuard config file and extract tunnel configuration.
pub fn parse_client_config(contents: &str) -> Result<TunnelConfig> {
    let mut private_key = None;
    let mut server_public_key = None;
    let mut preshared_key = None;
    let mut endpoint = None;
    let mut address = None;
    let mut dns = Vec::new();
    let mut allowed_ips = Vec::new();
    let mut mtu = 1420u16;
    let mut persistent_keepalive = 25u16;

    let mut in_interface = false;
    let mut in_peer = false;

    // Strip UTF-8 BOM if present (common when exporting .conf from Windows tools).
    let contents = contents.strip_prefix('\u{feff}').unwrap_or(contents);

    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line.eq_ignore_ascii_case("[Interface]") {
            in_interface = true;
            in_peer = false;
            continue;
        }
        if line.eq_ignore_ascii_case("[Peer]") {
            in_interface = false;
            in_peer = true;
            continue;
        }

        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            let value = value.trim();
            match (key, in_interface, in_peer) {
                ("PrivateKey", true, false) => {
                    private_key = Some(decode_base64_key(value)?);
                }
                ("Address", true, false) => {
                    // Prefer first IPv4 address if multiple are listed (e.g. v4,v6).
                    address = value
                        .split(',')
                        .map(str::trim)
                        .find(|s| {
                            let ip = s.split('/').next().unwrap_or(s);
                            ip.parse::<Ipv4Addr>().is_ok()
                        })
                        .or_else(|| value.split(',').next().map(str::trim))
                        .map(|s| s.to_string());
                }
                ("DNS", true, false) => {
                    for d in value.split(',') {
                        if let Ok(ip) = d.trim().parse::<IpAddr>() {
                            dns.push(ip);
                        }
                    }
                }
                ("MTU", true, false) => {
                    mtu = value.parse().unwrap_or(1420);
                }
                ("PublicKey", false, true) => {
                    server_public_key = Some(decode_base64_key(value)?);
                }
                ("PresharedKey", false, true) => {
                    preshared_key = Some(decode_base64_key(value)?);
                }
                ("Endpoint", false, true) => {
                    endpoint = Some(value.to_string());
                }
                ("AllowedIPs", false, true) => {
                    allowed_ips.push(value.to_string());
                }
                ("PersistentKeepalive", false, true) => {
                    if let Ok(n) = value.parse::<u16>() {
                        persistent_keepalive = n;
                    }
                }
                _ => {}
            }
        }
    }

    let private_key = private_key.context("missing PrivateKey in [Interface]")?;
    let server_public_key = server_public_key.context("missing PublicKey in [Peer]")?;
    let endpoint = endpoint.context("missing Endpoint in [Peer]")?;
    let address = address.context("missing Address in [Interface]")?;

    // Endpoint may be host:port or [ipv6]:port — resolve DNS if needed.
    let server_endpoint = resolve_endpoint(&endpoint)?;

    let (ip_part, prefix) = match address.split_once('/') {
        Some((ip, pfx)) => (ip.trim(), pfx.trim().parse::<u8>().unwrap_or(32)),
        None => (address.as_str(), 32u8),
    };
    let tunnel_ip = ip_part
        .parse::<IpAddr>()
        .with_context(|| format!("invalid tunnel IP address: {ip_part}"))?;

    // Default DNS if none specified (helps name resolution over full tunnel).
    if dns.is_empty() {
        dns.push(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)));
        dns.push(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)));
    }

    // Empty AllowedIPs → treat as full tunnel so the VPN actually carries traffic.
    if allowed_ips.is_empty() {
        allowed_ips.push(String::from("0.0.0.0/0"));
    }

    Ok(TunnelConfig {
        private_key,
        server_public_key,
        preshared_key,
        server_endpoint,
        tunnel_ip,
        tunnel_prefix: prefix.min(32),
        tunnel_ipv6: None,
        dns,
        mtu,
        allowed_ips,
        persistent_keepalive,
    })
}

fn resolve_endpoint(endpoint: &str) -> Result<SocketAddr> {
    let endpoint = endpoint.trim();
    // Fast path: already a SocketAddr
    if let Ok(sa) = endpoint.parse::<SocketAddr>() {
        return Ok(sa);
    }
    // host:port — ToSocketAddrs handles DNS
    let mut addrs = endpoint
        .to_socket_addrs()
        .with_context(|| format!("failed to resolve server endpoint `{endpoint}`"))?;
    // Prefer IPv4 for wider NAT compatibility.
    if let Some(v4) = addrs.find(|a| a.is_ipv4()) {
        return Ok(v4);
    }
    endpoint
        .to_socket_addrs()
        .ok()
        .and_then(|mut a| a.next())
        .with_context(|| format!("no addresses resolved for server endpoint `{endpoint}`"))
}

fn decode_base64_key(s: &str) -> Result<[u8; 32]> {
    use base64::{
        engine::general_purpose::{STANDARD, URL_SAFE, URL_SAFE_NO_PAD, STANDARD_NO_PAD},
        Engine as _,
    };
    let s = s.trim();
    let bytes = STANDARD
        .decode(s)
        .or_else(|_| STANDARD_NO_PAD.decode(s))
        .or_else(|_| URL_SAFE.decode(s))
        .or_else(|_| URL_SAFE_NO_PAD.decode(s))
        .with_context(|| format!("invalid WireGuard key (base64 decode failed)"))?;
    if bytes.len() != 32 {
        bail!(
            "WireGuard key must decode to 32 bytes, got {} bytes",
            bytes.len()
        );
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&bytes);
    Ok(key)
}

/// Check if the Wintun driver is available on this system.
pub fn is_wintun_available() -> bool {
    unsafe { wintun::load().is_ok() }
}
