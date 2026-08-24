//! WireGuard data plane on Android VpnService TUN using boringtun.
//!
//! Packet loop matches the Windows boringtun pump:
//! - immediate handshake initiation
//! - flush WriteToNetwork queue after every decapsulate
//! - update_timers every ~100ms (keepalive + rekey + handshake retry)
//! - safety-net re-handshake while session not established

use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use boringtun::{
    noise::{Tunn, TunnResult},
    x25519::{PublicKey, StaticSecret},
};
use std::{
    fs,
    fs::File,
    io::{Read, Write},
    net::{ToSocketAddrs, UdpSocket},
    os::fd::{AsRawFd, FromRawFd},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, OnceLock,
    },
    thread,
    time::{Duration, Instant},
};

use crate::progress::set_progress;
use crate::protect::protect_fd;

const PKT_BUF: usize = 2048;

static ACTIVE: OnceLock<Mutex<Option<Arc<AtomicBool>>>> = OnceLock::new();
static BYTES_RX: AtomicU64 = AtomicU64::new(0);
static BYTES_TX: AtomicU64 = AtomicU64::new(0);
static HANDSHAKE_OK: AtomicBool = AtomicBool::new(false);

pub fn is_wireguard_running() -> bool {
    ACTIVE
        .get()
        .and_then(|lock| lock.lock().ok())
        .map(|g| g.is_some())
        .unwrap_or(false)
}

/// True once a decrypted data-plane packet (or handshake completion traffic) was seen.
pub fn is_wireguard_handshake_ok() -> bool {
    HANDSHAKE_OK.load(Ordering::SeqCst) && is_wireguard_running()
}

pub fn wireguard_byte_counts() -> (u64, u64) {
    (BYTES_RX.load(Ordering::Relaxed), BYTES_TX.load(Ordering::Relaxed))
}

pub fn start_wireguard(tun_fd: i32, profile_path: &str) -> Result<()> {
    start_wireguard_ex(tun_fd, profile_path, -1)
}

/// `udp_fd` is an optional already-protected datagram socket from Java
/// (`VpnService.protect`). Pass `-1` to create and protect the socket here.
pub fn start_wireguard_ex(tun_fd: i32, profile_path: &str, udp_fd: i32) -> Result<()> {
    stop_wireguard()?;
    BYTES_RX.store(0, Ordering::Relaxed);
    BYTES_TX.store(0, Ordering::Relaxed);
    HANDSHAKE_OK.store(false, Ordering::SeqCst);
    set_progress("wireguard", 0.15, "parsing profile");
    let mut config = match PumpConfig::from_profile(profile_path) {
        Ok(c) => c,
        Err(e) => {
            close_owned_fd(udp_fd);
            return Err(e);
        }
    };
    config.udp_fd = udp_fd;
    set_progress("wireguard", 0.35, format!("endpoint {}", config.endpoint));

    let duplicated_fd = unsafe { libc::dup(tun_fd) };
    if duplicated_fd < 0 {
        close_owned_fd(udp_fd);
        bail!("could not duplicate Android TUN file descriptor");
    }

    let running = Arc::new(AtomicBool::new(true));
    *ACTIVE
        .get_or_init(|| Mutex::new(None))
        .lock()
        .map_err(|_| anyhow::anyhow!("wireguard lock poisoned"))? = Some(running.clone());

    set_progress("wireguard", 0.5, "starting packet pump");
    let running_for_wait = running.clone();
    thread::Builder::new()
        .name(String::from("zeronode-android-wg-pump"))
        .spawn(move || {
            if let Err(error) = run_pump(duplicated_fd, config, running) {
                eprintln!("ZeroNode Android WireGuard pump stopped: {error:#}");
            }
            // Pump exit ⇒ clear handshake so UI cannot stay "active" on a dead plane.
            HANDSHAKE_OK.store(false, Ordering::SeqCst);
        })
        .context("failed to spawn WireGuard pump")?;

    // Do NOT report Active until the peer answers. Otherwise the UI shows
    // "connected" while the TUN blackholes all traffic (classic no-internet).
    set_progress("wireguard", 0.65, "waiting for handshake");
    let deadline = Instant::now() + Duration::from_secs(22);
    while Instant::now() < deadline {
        if HANDSHAKE_OK.load(Ordering::SeqCst) {
            set_progress("wireguard", 1.0, "handshake ok · active");
            return Ok(());
        }
        if !running_for_wait.load(Ordering::SeqCst) {
            let _ = stop_wireguard();
            bail!("WireGuard pump exited before handshake completed");
        }
        thread::sleep(Duration::from_millis(100));
    }

    let _ = stop_wireguard();
    bail!(
        "WireGuard handshake timed out (22s) — check Endpoint reachability, keys, and that UDP is not blocked"
    )
}

pub fn stop_wireguard() -> Result<()> {
    HANDSHAKE_OK.store(false, Ordering::SeqCst);
    if let Some(lock) = ACTIVE.get() {
        if let Ok(mut active) = lock.lock() {
            if let Some(running) = active.take() {
                running.store(false, Ordering::SeqCst);
            }
        }
    }
    thread::sleep(Duration::from_millis(120));
    Ok(())
}

fn run_pump(fd: i32, config: PumpConfig, running: Arc<AtomicBool>) -> Result<()> {
    // Resolve endpoint on the underlying network (package is disallowed from VPN).
    // Prefer IPv4 for maximum peer compatibility.
    set_progress("wireguard", 0.55, format!("resolving {}", config.endpoint));
    let endpoint = resolve_endpoint(&config.endpoint)?;
    set_progress("wireguard", 0.6, format!("endpoint {endpoint}"));

    // Prefer the Java-protected fd (VpnService.protect on the VpnService
    // instance — no JNI race). Otherwise bind here and protect via JNI.
    let socket = if config.udp_fd >= 0 {
        eprintln!(
            "ZeroNode WG: adopting Java-protected UDP fd {}",
            config.udp_fd
        );
        unsafe { UdpSocket::from_raw_fd(config.udp_fd) }
    } else {
        UdpSocket::bind("0.0.0.0:0")?
    };
    let sock_fd = socket.as_raw_fd();
    let mut protected = protect_fd(sock_fd);
    if !protected {
        thread::sleep(Duration::from_millis(40));
        protected = protect_fd(sock_fd);
    }
    eprintln!("ZeroNode WG: protect({sock_fd}) -> {protected} endpoint={endpoint}");
    if !protected && config.udp_fd < 0 {
        eprintln!(
            "ZeroNode WG: WARNING protect() failed — handshake UDP may loop into TUN"
        );
    }
    socket
        .connect(endpoint)
        .with_context(|| format!("failed to connect UDP to {endpoint}"))?;
    socket.set_nonblocking(true)?;
    let _ = protect_fd(socket.as_raw_fd());

    // TUN: blocking with short select via non-blocking when possible
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags >= 0 {
            let _ = libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
    }

    let mut tun_file = unsafe { File::from_raw_fd(fd) };
    let mut tunnel = Tunn::new(
        config.private_key,
        config.peer_public_key,
        config.preshared_key,
        Some(25), // PersistentKeepalive seconds
        0,
        None,
    );

    let mut plain = vec![0u8; PKT_BUF];
    let mut encrypted = vec![0u8; PKT_BUF];
    let mut udp_buf = vec![0u8; PKT_BUF];

    // Immediate handshake (critical — without this, 0 bytes forever until traffic,
    // and encapsulate of plain IP fails until session exists).
    send_handshake(&mut tunnel, &socket, &mut encrypted, true);
    set_progress("wireguard", 0.75, format!("handshake → {endpoint}"));

    let mut last_timer = Instant::now();
    let mut handshake_force_at = Instant::now();
    let mut idle_streak: u32 = 0;
    let mut handshake_ok = false;

    while running.load(Ordering::SeqCst) {
        let mut did_work = false;

        // 1. TUN → encrypt → UDP (bounded burst so UDP/timers stay responsive)
        for _ in 0..48 {
            match tun_file.read(&mut plain) {
                Ok(0) => break,
                Ok(size) => {
                    match tunnel.encapsulate(&plain[..size], &mut encrypted) {
                        TunnResult::WriteToNetwork(pkt) => {
                            if socket.send(pkt).is_ok() {
                                BYTES_TX.fetch_add(pkt.len() as u64, Ordering::Relaxed);
                            }
                        }
                        TunnResult::Err(e) => {
                            eprintln!("wg encapsulate error: {e:?}");
                        }
                        _ => {}
                    }
                    did_work = true;
                }
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    break;
                }
                Err(e) => {
                    eprintln!("wg tun read error: {e}");
                    break;
                }
            }
        }

        // 2. UDP → decrypt → TUN (flush WriteToNetwork queue after each datagram)
        loop {
            match socket.recv(&mut udp_buf) {
                Ok(n) if n > 0 => {
                    handle_udp(
                        &mut tunnel,
                        &mut tun_file,
                        &socket,
                        &udp_buf[..n],
                        &mut encrypted,
                        &mut handshake_ok,
                    );
                    did_work = true;
                }
                Ok(_) => break,
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    break;
                }
                Err(e) => {
                    eprintln!("wg udp recv error: {e}");
                    break;
                }
            }
        }

        // 3. Timers ~100ms — keepalives, rekey, handshake retry
        if last_timer.elapsed() >= Duration::from_millis(100) {
            match tunnel.update_timers(&mut encrypted) {
                TunnResult::WriteToNetwork(pkt) => {
                    let _ = socket.send(pkt);
                    loop {
                        match tunnel.update_timers(&mut encrypted) {
                            TunnResult::WriteToNetwork(p) => {
                                let _ = socket.send(p);
                            }
                            TunnResult::Done => break,
                            TunnResult::Err(e) => {
                                eprintln!("wg timer error: {e:?}");
                                send_handshake(&mut tunnel, &socket, &mut encrypted, true);
                                break;
                            }
                            _ => break,
                        }
                    }
                    did_work = true;
                }
                TunnResult::Err(e) => {
                    eprintln!("wg timer error: {e:?}");
                    send_handshake(&mut tunnel, &socket, &mut encrypted, true);
                }
                _ => {}
            }
            // Session established once boringtun records a completed handshake.
            if tunnel.time_since_last_handshake().is_some() {
                mark_handshake(&mut handshake_ok);
            }
            last_timer = Instant::now();
        }

        // 4. Safety-net re-handshake until peer answers
        if !handshake_ok && handshake_force_at.elapsed() >= Duration::from_secs(3) {
            send_handshake(&mut tunnel, &socket, &mut encrypted, true);
            handshake_force_at = Instant::now();
            set_progress("wireguard", 0.8, "retrying handshake");
        } else if handshake_ok && handshake_force_at.elapsed() >= Duration::from_secs(15) {
            send_handshake(&mut tunnel, &socket, &mut encrypted, false);
            handshake_force_at = Instant::now();
        }

        if handshake_ok {
            let rx = BYTES_RX.load(Ordering::Relaxed);
            let tx = BYTES_TX.load(Ordering::Relaxed);
            set_progress(
                "wireguard",
                1.0,
                format!("active rx={rx} tx={tx}"),
            );
        }

        if did_work {
            idle_streak = 0;
        } else {
            idle_streak = idle_streak.saturating_add(1);
            let sleep_ms = match idle_streak {
                0..=30 => 0,
                31..=150 => 1,
                _ => 4,
            };
            if sleep_ms > 0 {
                thread::sleep(Duration::from_millis(sleep_ms));
            } else {
                thread::yield_now();
            }
        }
    }

    // Dropping File closes the duplicated fd (original VpnService fd stays open).
    drop(tun_file);
    Ok(())
}

fn close_owned_fd(fd: i32) {
    if fd >= 0 {
        unsafe {
            libc::close(fd);
        }
    }
}

fn send_handshake(tunnel: &mut Tunn, socket: &UdpSocket, wg_buf: &mut [u8], force: bool) {
    match tunnel.format_handshake_initiation(wg_buf, force) {
        TunnResult::WriteToNetwork(msg) => {
            if let Err(e) = socket.send(msg) {
                eprintln!("wg handshake send failed: {e}");
            } else {
                eprintln!("wg handshake initiation sent (force={force}, {} bytes)", msg.len());
            }
        }
        TunnResult::Done => {}
        TunnResult::Err(e) => eprintln!("wg handshake error: {e:?}"),
        _ => {}
    }
}

/// Resolve WireGuard Endpoint=host:port, preferring IPv4.
fn resolve_endpoint(endpoint: &str) -> Result<std::net::SocketAddr> {
    let mut v4 = None;
    let mut v6 = None;
    for addr in endpoint
        .to_socket_addrs()
        .with_context(|| format!("could not resolve {endpoint}"))?
    {
        if addr.is_ipv4() && v4.is_none() {
            v4 = Some(addr);
        } else if addr.is_ipv6() && v6.is_none() {
            v6 = Some(addr);
        }
    }
    v4.or(v6)
        .ok_or_else(|| anyhow::anyhow!("endpoint {endpoint} resolved to no addresses"))
}

/// Process one UDP datagram, then flush boringtun's WriteToNetwork queue
/// (required after handshake response — without this, session never completes).
fn handle_udp(
    tunnel: &mut Tunn,
    tun: &mut File,
    socket: &UdpSocket,
    datagram: &[u8],
    wg_buf: &mut [u8],
    handshake_ok: &mut bool,
) {
    let mut first = true;
    loop {
        let result = if first {
            first = false;
            tunnel.decapsulate(None, datagram, wg_buf)
        } else {
            tunnel.decapsulate(None, &[], wg_buf)
        };

        match result {
            TunnResult::WriteToTunnelV4(dec, _) | TunnResult::WriteToTunnelV6(dec, _) => {
                mark_handshake(handshake_ok);
                BYTES_RX.fetch_add(dec.len() as u64, Ordering::Relaxed);
                let _ = tun.write_all(dec);
                let _ = tun.flush();
            }
            TunnResult::WriteToNetwork(enc) => {
                let _ = socket.send(enc);
                BYTES_TX.fetch_add(enc.len() as u64, Ordering::Relaxed);
            }
            TunnResult::Done => break,
            TunnResult::Err(e) => {
                eprintln!("wg decapsulate error: {e:?}");
                break;
            }
        }
    }
    // Peer Noise handshake completes without inner IP traffic — detect via boringtun.
    if tunnel.time_since_last_handshake().is_some() {
        mark_handshake(handshake_ok);
    }
}

#[inline]
fn mark_handshake(handshake_ok: &mut bool) {
    if !*handshake_ok {
        *handshake_ok = true;
        HANDSHAKE_OK.store(true, Ordering::SeqCst);
        eprintln!("ZeroNode WG: handshake established");
    }
}

struct PumpConfig {
    private_key: StaticSecret,
    peer_public_key: PublicKey,
    preshared_key: Option<[u8; 32]>,
    endpoint: String,
    udp_fd: i32,
}

impl PumpConfig {
    fn from_profile(path: &str) -> Result<Self> {
        let profile = fs::read_to_string(path)
            .with_context(|| format!("could not read WireGuard profile {path}"))?;
        let private_key = required_value(&profile, "PrivateKey")?;
        let peer_public_key = required_value(&profile, "PublicKey")?;
        let endpoint = required_value(&profile, "Endpoint")?;
        let psk = optional_value(&profile, "PresharedKey")
            .map(decode_key)
            .transpose()?;
        Ok(Self {
            private_key: StaticSecret::from(decode_key(private_key)?),
            peer_public_key: PublicKey::from(decode_key(peer_public_key)?),
            preshared_key: psk,
            endpoint: endpoint.to_owned(),
            udp_fd: -1,
        })
    }
}

fn required_value<'a>(profile: &'a str, key: &str) -> Result<&'a str> {
    optional_value(profile, key).ok_or_else(|| anyhow::anyhow!("missing {key} in WireGuard profile"))
}

fn optional_value<'a>(profile: &'a str, key: &str) -> Option<&'a str> {
    // Prefer Peer section PublicKey over any other; first matching key wins which
    // is correct for standard single-peer configs (Interface PrivateKey, Peer PublicKey).
    profile
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with('[') {
                return None;
            }
            line.split_once('=')
                .map(|(left, right)| (left.trim(), right.trim()))
        })
        .find(|(left, _)| left.eq_ignore_ascii_case(key))
        .map(|(_, right)| right)
}

fn decode_key(value: &str) -> Result<[u8; 32]> {
    let cleaned = value.trim().trim_end_matches('\r');
    let decoded = STANDARD
        .decode(cleaned)
        .with_context(|| format!("invalid base64 key"))?;
    decoded
        .try_into()
        .map_err(|_| anyhow::anyhow!("WireGuard key must decode to 32 bytes"))
}
