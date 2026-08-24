package io.zeronode.vpn;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.content.Context;
import android.content.Intent;
import android.net.ConnectivityManager;
import android.net.Network;
import android.net.NetworkCapabilities;
import android.net.VpnService;
import android.os.Build;
import android.os.ParcelFileDescriptor;
import android.util.Log;

import java.io.BufferedReader;
import java.io.BufferedWriter;
import java.io.File;
import java.io.FileReader;
import java.io.FileWriter;
import java.io.IOException;
import java.net.DatagramSocket;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.util.List;

/**
 * System VPN service. Establishes a full-tunnel TUN and hands the fd to Rust
 * for WireGuard / Outline / Tor (tun2proxy) data planes.
 */
public final class ZeroNodeVpnService extends VpnService {
    private static final String TAG = "ZeroNodeVpnService";
    static final String ACTION_START = "io.zeronode.vpn.START";
    static final String ACTION_STOP = "io.zeronode.vpn.STOP";
    /** Broadcast when tunnel state changes so UI stays in sync (notification disconnect). */
    static final String ACTION_STATE = "io.zeronode.vpn.STATE";
    static final String EXTRA_STATE = "io.zeronode.vpn.STATE_VALUE";
    static final String EXTRA_MESSAGE = "io.zeronode.vpn.STATE_MESSAGE";

    private static final String CHANNEL_ID = "zeronode_vpn";
    private static final int NOTIF_ID = 7;

    private static volatile ZeroNodeVpnService live;
    private static volatile String lastStatus = "IDLE";
    private static volatile String lastKind = "";
    private static volatile boolean runningFlag;

    static final String EXTRA_KIND = "io.zeronode.vpn.KIND";
    static final String EXTRA_SESSION = "io.zeronode.vpn.SESSION";
    static final String EXTRA_CLIENT_ADDRESS = "io.zeronode.vpn.CLIENT_ADDRESS";
    static final String EXTRA_DNS = "io.zeronode.vpn.DNS";
    static final String EXTRA_PROFILE = "io.zeronode.vpn.PROFILE";
    static final String EXTRA_HOST = "io.zeronode.vpn.HOST";
    static final String EXTRA_PORT = "io.zeronode.vpn.PORT";
    static final String EXTRA_USER = "io.zeronode.vpn.USER";
    static final String EXTRA_PASSWORD = "io.zeronode.vpn.PASSWORD";
    static final String EXTRA_METHOD = "io.zeronode.vpn.METHOD";
    static final String EXTRA_EXTRA = "io.zeronode.vpn.EXTRA";
    static final String EXTRA_MTU = "io.zeronode.vpn.MTU";

    private ParcelFileDescriptor tunnel;
    /** Kept alive so DatagramSocket.close() cannot kill the Rust-owned UDP fd. */
    private DatagramSocket wgUdpKeepalive;

    static Intent startIntent(
        Context context,
        String kind,
        String sessionName,
        String clientAddress,
        String dns,
        String profileOrKey,
        String host,
        String port,
        String user,
        String password,
        String method,
        String extra
    ) {
        Intent intent = new Intent(context, ZeroNodeVpnService.class);
        intent.setAction(ACTION_START);
        intent.putExtra(EXTRA_KIND, kind);
        intent.putExtra(EXTRA_SESSION, sessionName);
        intent.putExtra(EXTRA_CLIENT_ADDRESS, clientAddress);
        intent.putExtra(EXTRA_DNS, dns);
        intent.putExtra(EXTRA_PROFILE, profileOrKey);
        intent.putExtra(EXTRA_HOST, host);
        intent.putExtra(EXTRA_PORT, port);
        intent.putExtra(EXTRA_USER, user);
        intent.putExtra(EXTRA_PASSWORD, password);
        intent.putExtra(EXTRA_METHOD, method);
        intent.putExtra(EXTRA_EXTRA, extra);
        return intent;
    }

    static Intent stopIntent(Context context) {
        Intent intent = new Intent(context, ZeroNodeVpnService.class);
        intent.setAction(ACTION_STOP);
        return intent;
    }

    /** Called from Rust (JNI) to bypass TUN. */
    public static boolean protectSocket(int fd) {
        ZeroNodeVpnService svc = live;
        if (svc == null || fd < 0) {
            return false;
        }
        return svc.protect(fd);
    }

    static boolean isRunning() {
        return runningFlag && live != null && tunnelOpen();
    }

    static String lastStatus() {
        return lastStatus == null ? "IDLE" : lastStatus;
    }

    static String lastKind() {
        return lastKind == null ? "" : lastKind;
    }

    private static boolean tunnelOpen() {
        ZeroNodeVpnService svc = live;
        return svc != null && svc.tunnel != null;
    }

    private void broadcastState(String state, String message) {
        Intent i = new Intent(ACTION_STATE);
        i.setPackage(getPackageName());
        i.putExtra(EXTRA_STATE, state);
        i.putExtra(EXTRA_MESSAGE, message == null ? "" : message);
        i.putExtra(EXTRA_KIND, lastKind);
        sendBroadcast(i);
    }

    @Override
    public void onCreate() {
        super.onCreate();
        live = this;
    }

    @Override
    public int onStartCommand(Intent intent, int flags, int startId) {
        live = this;
        String action = intent == null ? ACTION_START : intent.getAction();
        if (ACTION_STOP.equals(action)) {
            fullStop("Disconnected");
            return START_NOT_STICKY;
        }

        final String kind = extra(intent, EXTRA_KIND, "wireguard");
        final String session = extra(intent, EXTRA_SESSION, "ZeroNode VPN");
        final String clientAddress = extra(intent, EXTRA_CLIENT_ADDRESS, "10.7.0.2");
        final String dns = extra(intent, EXTRA_DNS, "1.1.1.1");
        final String profile = extra(intent, EXTRA_PROFILE, "");
        final String host = extra(intent, EXTRA_HOST, "");
        final String port = extra(intent, EXTRA_PORT, "");
        final String user = extra(intent, EXTRA_USER, "");
        final String password = extra(intent, EXTRA_PASSWORD, "");
        final String method = extra(intent, EXTRA_METHOD, "");
        final String extraVal = extra(intent, EXTRA_EXTRA, "");

        // startForeground must be on the main thread and return quickly.
        startForeground(NOTIF_ID, notification("Connecting (" + kind + ")…", true));

        // CRITICAL: never block the main thread. WireGuard waits for handshake.
        // Doing that in onStartCommand
        // causes ANR and kills the service mid-connect ("Active" never sticks,
        // or traffic never comes up).
        lastKind = kind;
        lastStatus = "CONNECTING";
        runningFlag = false;
        broadcastState("connecting", kind);
        final int cmdId = startId;
        new Thread(new Runnable() {
            @Override
            public void run() {
                startTunnel(
                    kind, session, clientAddress, dns, profile,
                    host, port, user, password, method, extraVal, cmdId
                );
            }
        }, "zn-vpn-start").start();
        return START_STICKY;
    }

    @Override
    public void onDestroy() {
        // Ensure data planes die even if OS kills us without ACTION_STOP.
        stopDataPlanes(true);
        closeTunnelFd();
        runningFlag = false;
        lastStatus = "IDLE";
        if (live == this) {
            live = null;
        }
        super.onDestroy();
    }

    @Override
    public void onRevoke() {
        fullStop("VPN revoked by system");
        super.onRevoke();
    }

    private void fullStop(String reason) {
        stopDataPlanes(true);
        closeTunnelFd();
        runningFlag = false;
        lastStatus = "IDLE";
        lastKind = "";
        broadcastState("disconnected", reason);
        try {
            stopForeground(true);
        } catch (Exception ignored) {
        }
        stopSelf();
        if (live == this) {
            live = null;
        }
    }

    private void startTunnel(
        String kind,
        String session,
        String clientAddress,
        String dns,
        String profile,
        String host,
        String port,
        String user,
        String password,
        String method,
        String extraVal,
        int startId
    ) {
        stopDataPlanes(!"tor".equalsIgnoreCase(kind));
        try {
            String result;
            result = startUserspaceTunnel(
                kind, session, clientAddress, dns, profile,
                host, port, user, password, method, extraVal
            );

            NotificationManager manager = getSystemService(NotificationManager.class);
            if (result != null && result.startsWith("OK")) {
                runningFlag = true;
                lastStatus = "OK\nkind=" + kind + "\nmessage=active";
                if (manager != null) {
                    manager.notify(NOTIF_ID, notification("Active · " + session + " · " + kind, true));
                }
                broadcastState("connected", kind);
            } else {
                String msg = firstLine(result);
                lastStatus = "ERR\nmessage=" + msg;
                runningFlag = false;
                if (manager != null) {
                    manager.notify(NOTIF_ID, notification("Failed: " + msg, false));
                }
                stopDataPlanes(true);
                closeTunnelFd();
                broadcastState("error", msg);
                stopSelf(startId);
            }
        } catch (Exception error) {
            lastStatus = "ERR\nmessage=" + error.getMessage();
            runningFlag = false;
            NotificationManager manager = getSystemService(NotificationManager.class);
            if (manager != null) {
                manager.notify(NOTIF_ID, notification("VPN failed: " + error.getMessage(), false));
            }
            stopDataPlanes(true);
            closeTunnelFd();
            broadcastState("error", error.getMessage());
            stopSelf(startId);
        }
    }

    /**
     * WireGuard / Outline / Tor system tunnel: establish TUN then attach data plane.
     * Never inject a fake IPv6 address/route unless the profile actually has IPv6 —
     * dual-stack Happy Eyeballs + blackhole IPv6 is the #1 "connected, no internet"
     * failure mode on Android.
     */
    private String startUserspaceTunnel(
        String kind,
        String session,
        String clientAddress,
        String dns,
        String profile,
        String host,
        String port,
        String user,
        String password,
        String method,
        String extraVal
    ) throws Exception {
        boolean isWg = "wireguard".equalsIgnoreCase(kind) || "zeronode".equalsIgnoreCase(kind);
        // WireGuard: resolve Endpoint hostname on clearnet BEFORE the TUN steals
        // default route. Without this, skipping addDisallowedApplication (required
        // for correct Refresh-IP) makes getaddrinfo blackhole into the empty TUN.
        String wgProfile = profile;
        if (isWg && profile != null && profile.length() > 0) {
            try {
                wgProfile = materializeWgProfileResolvedEndpoint(profile);
            } catch (Exception e) {
                Log.w(TAG, "WG endpoint pre-resolve: " + e.getMessage());
                wgProfile = profile;
            }
        }
        WgTunConfig wgCfg = isWg ? WgTunConfig.fromProfile(wgProfile, clientAddress, dns) : null;

        String resolvedDns;
        if (wgCfg != null && wgCfg.dns != null && wgCfg.dns.length() > 0) {
            resolvedDns = wgCfg.dns;
        } else if (dns != null && dns.length() > 0) {
            resolvedDns = dns;
        } else {
            resolvedDns = "1.1.1.1";
        }

        String addr;
        int prefix;
        int mtu;
        if (wgCfg != null) {
            addr = wgCfg.ipv4;
            prefix = wgCfg.ipv4Prefix;
            mtu = wgCfg.mtu;
        } else {
            addr = clientAddress == null ? "10.7.0.2" : clientAddress.trim();
            int slash = addr.indexOf('/');
            if (slash > 0) {
                try {
                    prefix = Integer.parseInt(addr.substring(slash + 1).trim());
                } catch (NumberFormatException e) {
                    prefix = 32;
                }
                addr = addr.substring(0, slash).trim();
            } else {
                prefix = 32;
            }
            if (addr.isEmpty() || addr.contains(":")) {
                addr = "10.7.0.2";
                prefix = 32;
            }
            mtu = 1280;
        }
        if (prefix < 0 || prefix > 32) prefix = 32;
        if (mtu < 576 || mtu > 9000) mtu = 1280;

        Builder builder = new Builder()
            .setSession(session != null ? session : "ZeroNode VPN")
            .setMtu(mtu)
            .addAddress(addr, prefix)
            .allowFamily(android.system.OsConstants.AF_INET);

        // Routes: WireGuard AllowedIPs when present; otherwise full tunnel.
        boolean hasV4Default = false;
        if (wgCfg != null && !wgCfg.routes.isEmpty()) {
            for (int i = 0; i < wgCfg.routes.size(); i++) {
                RouteCidr r = wgCfg.routes.get(i);
                try {
                    builder.addRoute(r.network, r.prefix);
                    if (!r.ipv6 && "0.0.0.0".equals(r.network) && r.prefix == 0) {
                        hasV4Default = true;
                    }
                    // 0.0.0.0/1 + 128.0.0.0/1 is also full tunnel
                    if (!r.ipv6 && r.prefix == 1
                        && ("0.0.0.0".equals(r.network) || "128.0.0.0".equals(r.network))) {
                        hasV4Default = true;
                    }
                } catch (Exception e) {
                    Log.w(TAG, "skip route " + r.network + "/" + r.prefix + ": " + e.getMessage());
                }
            }
        }
        if (!hasV4Default) {
            builder.addRoute("0.0.0.0", 0);
        }

        // DNS — add all listed servers when possible
        boolean anyDns = false;
        if (wgCfg != null && wgCfg.allDns != null) {
            for (String d : wgCfg.allDns) {
                if (d == null || d.isEmpty()) continue;
                try {
                    builder.addDnsServer(d);
                    anyDns = true;
                } catch (Exception ignored) {
                }
            }
        }
        if (!anyDns) {
            try {
                builder.addDnsServer(resolvedDns);
            } catch (Exception e) {
                builder.addDnsServer("1.1.1.1");
            }
            try {
                builder.addDnsServer("8.8.8.8");
            } catch (Exception ignored) {
            }
        }

        // IPv6 ONLY when the profile actually provisions it. Fake fd00:: + ::/0
        // blackholes Happy-Eyeballs dual-stack traffic → "No internet".
        if (wgCfg != null && wgCfg.ipv6 != null && wgCfg.ipv6.length() > 0) {
            try {
                builder.allowFamily(android.system.OsConstants.AF_INET6);
                builder.addAddress(wgCfg.ipv6, wgCfg.ipv6Prefix);
                boolean hasV6Default = false;
                for (int i = 0; i < wgCfg.routes.size(); i++) {
                    RouteCidr r = wgCfg.routes.get(i);
                    if (r.ipv6) {
                        try {
                            builder.addRoute(r.network, r.prefix);
                            if ("::".equals(r.network) && r.prefix == 0) hasV6Default = true;
                        } catch (Exception ignored) {
                        }
                    }
                }
                if (!hasV6Default) {
                    builder.addRoute("::", 0);
                }
            } catch (Exception e) {
                Log.w(TAG, "IPv6 skipped: " + e.getMessage());
            }
        }
        // Outline/Tor: IPv4-only TUN is correct (SOCKS is typically v4).

        // Outline / Tor: exclude this package so UI + local SOCKS do not loop
        // into the TUN (traffic is driven by tun2proxy, not by app sockets).
        //
        // WireGuard: do NOT exclude the app. Official WireGuard-Android model:
        // protect() the UDP endpoint socket only; all other app sockets
        // (including Refresh IP HTTP) ride the TUN and see the real exit IP.
        // Handshake UDP is created + protect()'d in Java below (no JNI race).
        // API 33+: also excludeRoute the peer /32 so handshake cannot hairpin.
        boolean split = applySplitTunnel(builder);
        if (!split && !isWg) {
            try {
                builder.addDisallowedApplication(getPackageName());
            } catch (Exception ignored) {
            }
        }
        if (isWg) {
            excludeWgEndpointFromTunnel(builder, wgProfile);
        }

        // Create + protect the WG UDP socket BEFORE establish() so the first
        // handshake datagram never enters the TUN even if JNI protect races.
        ProtectedUdp wgUdp = isWg ? createProtectedUdpSocket() : ProtectedUdp.none();
        if (isWg && !wgUdp.protectedOk && Build.VERSION.SDK_INT < 33) {
            // Last-resort: exclude the app so handshake can leave the device.
            // Refresh IP still binds to the VPN Network explicitly.
            try {
                builder.addDisallowedApplication(getPackageName());
                Log.w(TAG, "WG protect() failed on API < 33 — falling back to disallow");
            } catch (Exception ignored) {
            }
        }

        closeTunnelFd();
        tunnel = builder.establish();
        if (tunnel == null) {
            wgUdp.closeQuietly();
            throw new IllegalStateException("Android did not establish the VPN interface");
        }
        Log.i(TAG, "TUN up kind=" + kind + " ip=" + addr + "/" + prefix
            + " mtu=" + mtu + " dns=" + resolvedDns
            + " v6=" + (wgCfg != null && wgCfg.ipv6 != null)
            + " disallowApp=" + !isWg
            + " wgUdpFd=" + wgUdp.fd
            + " wgProtected=" + wgUdp.protectedOk);

        if ("tor".equalsIgnoreCase(kind)) {
            wgUdp.closeQuietly();
            return NativeBridge.attachTorSystemTunnel(tunnel.getFd());
        }
        if ("pptp".equalsIgnoreCase(kind)) {
            wgUdp.closeQuietly();
            return "ERR\nmessage=PPTP is not supported on Android";
        }
        NativeBridge.ensureProtectBridge();
        String extra = extraVal == null ? "" : extraVal;
        if (isWg && wgUdp.fd >= 0) {
            extra = (extra.length() > 0 ? extra + "\n" : "") + "udp_fd=" + wgUdp.fd;
        }
        return NativeBridge.startTunnel(
            tunnel.getFd(),
            kind,
            isWg ? wgProfile : profile,
            host,
            port,
            user,
            password,
            method,
            extra
        );
    }

    /**
     * Rewrite WireGuard {@code Endpoint = host:port} to a literal IP so the
     * userspace pump never needs DNS after the full-tunnel TUN is up.
     */
    private String materializeWgProfileResolvedEndpoint(String profilePath) throws IOException {
        File src = new File(profilePath);
        if (!src.isFile()) return profilePath;

        StringBuilder out = new StringBuilder((int) Math.max(256, src.length()));
        BufferedReader br = new BufferedReader(new FileReader(src));
        String line;
        boolean rewrote = false;
        while ((line = br.readLine()) != null) {
            String trim = line.trim();
            if (trim.regionMatches(true, 0, "Endpoint", 0, 8) && trim.contains("=")) {
                int eq = trim.indexOf('=');
                String val = trim.substring(eq + 1).trim();
                String host;
                String portPart = "";
                // [ipv6]:port or host:port or bare host
                if (val.startsWith("[")) {
                    int close = val.indexOf(']');
                    if (close > 1) {
                        host = val.substring(1, close);
                        portPart = val.substring(close + 1); // includes :port
                    } else {
                        host = val;
                    }
                } else {
                    int colon = val.lastIndexOf(':');
                    if (colon > 0 && val.indexOf(':') == colon) {
                        // single colon → host:port (IPv4 or name)
                        host = val.substring(0, colon).trim();
                        portPart = val.substring(colon);
                    } else if (colon > 0 && looksLikeIpv6(val)) {
                        host = val;
                    } else if (colon > 0) {
                        // hostname unlikely to have multiple colons; treat as IPv6 without port
                        host = val;
                    } else {
                        host = val;
                    }
                }
                if (!isLiteralIp(host)) {
                    try {
                        InetAddress[] addrs = InetAddress.getAllByName(host);
                        InetAddress chosen = null;
                        for (InetAddress a : addrs) {
                            if (a instanceof java.net.Inet4Address) {
                                chosen = a;
                                break;
                            }
                            if (chosen == null) chosen = a;
                        }
                        if (chosen != null) {
                            String ip = chosen.getHostAddress();
                            if (ip != null) {
                                // Strip IPv6 zone id if present
                                int zone = ip.indexOf('%');
                                if (zone > 0) ip = ip.substring(0, zone);
                                String rebuilt;
                                if (ip.contains(":")) {
                                    rebuilt = "[" + ip + "]" + (portPart.startsWith(":") ? portPart : portPart);
                                } else {
                                    rebuilt = ip + portPart;
                                }
                                out.append("Endpoint = ").append(rebuilt).append('\n');
                                rewrote = true;
                                Log.i(TAG, "WG Endpoint " + host + " → " + rebuilt);
                                continue;
                            }
                        }
                    } catch (Exception e) {
                        Log.w(TAG, "WG resolve " + host + ": " + e.getMessage());
                    }
                }
            }
            out.append(line).append('\n');
        }
        br.close();
        if (!rewrote) return profilePath;

        File dir = new File(getCacheDir(), "wg");
        if (!dir.exists() && !dir.mkdirs()) return profilePath;
        File dest = new File(dir, "runtime-resolved.conf");
        BufferedWriter bw = new BufferedWriter(new FileWriter(dest, false));
        bw.write(out.toString());
        bw.close();
        return dest.getAbsolutePath();
    }

    /**
     * Apply exclusive per-app VPN. {@code addAllowedApplication} and
     * {@code addDisallowedApplication} cannot be mixed on one Builder.
     *
     * @return true when an allowed-app list was installed (caller must not
     *         also call addDisallowedApplication).
     */
    private boolean applySplitTunnel(Builder builder) {
        if (AppSplitStore.allApps(this)) return false;
        List<String> pkgs = AppSplitStore.selectedPackages(this);
        boolean any = false;
        for (int i = 0; i < pkgs.size(); i++) {
            String pkg = pkgs.get(i);
            if (pkg == null || pkg.length() == 0) continue;
            try {
                builder.addAllowedApplication(pkg);
                any = true;
            } catch (Exception e) {
                Log.w(TAG, "skip allowed app " + pkg + ": " + e.getMessage());
            }
        }
        if (!any) {
            // Empty exclusive list would otherwise become "all apps". Pin the
            // tunnel to this package only so other apps stay on clearnet.
            try {
                builder.addAllowedApplication(getPackageName());
                any = true;
            } catch (Exception ignored) {
            }
        }
        Log.i(TAG, "per-app VPN allowed=" + (any ? pkgs : "self-only"));
        return any;
    }

    /**
     * Exclude the WireGuard peer /32 (or /128) from the TUN so handshake UDP
     * cannot hairpin even if protect() is late. API 33+.
     */
    private void excludeWgEndpointFromTunnel(Builder builder, String profilePath) {
        if (Build.VERSION.SDK_INT < 33) return;
        String host = readWgEndpointHost(profilePath);
        if (host == null || host.length() == 0 || !isLiteralIp(host)) return;
        try {
            InetAddress addr = InetAddress.getByName(host);
            int bits = addr instanceof java.net.Inet6Address ? 128 : 32;
            builder.excludeRoute(new android.net.IpPrefix(addr, bits));
            Log.i(TAG, "WG excludeRoute " + host + "/" + bits);
        } catch (Throwable t) {
            Log.w(TAG, "excludeRoute: " + t.getMessage());
        }
    }

    private static String readWgEndpointHost(String profilePath) {
        if (profilePath == null || profilePath.length() == 0) return null;
        File src = new File(profilePath);
        if (!src.isFile()) return null;
        BufferedReader br = null;
        try {
            br = new BufferedReader(new FileReader(src));
            String line;
            while ((line = br.readLine()) != null) {
                String trim = line.trim();
                if (!trim.regionMatches(true, 0, "Endpoint", 0, 8) || !trim.contains("=")) {
                    continue;
                }
                String val = trim.substring(trim.indexOf('=') + 1).trim();
                if (val.startsWith("[")) {
                    int close = val.indexOf(']');
                    return close > 1 ? val.substring(1, close) : val;
                }
                int colon = val.lastIndexOf(':');
                if (colon > 0 && val.indexOf(':') == colon) {
                    return val.substring(0, colon).trim();
                }
                return val;
            }
        } catch (Exception ignored) {
        } finally {
            if (br != null) {
                try { br.close(); } catch (IOException ignored) {}
            }
        }
        return null;
    }

    /**
     * Bind a datagram socket and mark it with {@link #protect(java.net.Socket)}
     * on this VpnService instance (no JNI). The detached fd is handed to Rust.
     */
    private ProtectedUdp createProtectedUdpSocket() {
        ProtectedUdp out = new ProtectedUdp();
        DatagramSocket sock = null;
        try {
            if (wgUdpKeepalive != null) {
                try { wgUdpKeepalive.close(); } catch (Exception ignored) {}
                wgUdpKeepalive = null;
            }
            sock = new DatagramSocket(null);
            sock.setReuseAddress(true);
            sock.bind(new InetSocketAddress(0));
            out.protectedOk = protect(sock);
            Log.i(TAG, "WG protect(DatagramSocket)=" + out.protectedOk
                + " local=" + sock.getLocalPort());
            try {
                ConnectivityManager cm =
                    (ConnectivityManager) getSystemService(CONNECTIVITY_SERVICE);
                if (cm != null && Build.VERSION.SDK_INT >= 23) {
                    Network[] nets = cm.getAllNetworks();
                    if (nets != null) {
                        for (Network n : nets) {
                            NetworkCapabilities caps = cm.getNetworkCapabilities(n);
                            if (caps == null) continue;
                            if (caps.hasTransport(NetworkCapabilities.TRANSPORT_VPN)) continue;
                            if (caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI)
                                || caps.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR)
                                || caps.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET)) {
                                n.bindSocket(sock);
                                Log.i(TAG, "WG UDP bound to underlying network " + n);
                                break;
                            }
                        }
                    }
                }
            } catch (Exception e) {
                Log.w(TAG, "WG bindSocket underlying: " + e.getMessage());
            }
            // Dup the fd for Rust. Do NOT close `sock` — DatagramSocket.close()
            // shuts the socket down and would kill the handshake.
            ParcelFileDescriptor pfd = ParcelFileDescriptor.fromDatagramSocket(sock);
            out.fd = pfd.detachFd();
            wgUdpKeepalive = sock;
            sock = null;
        } catch (Exception e) {
            Log.e(TAG, "createProtectedUdpSocket", e);
            out.closeQuietly();
            if (sock != null) {
                try { sock.close(); } catch (Exception ignored) {}
            }
        }
        return out;
    }

    private static final class ProtectedUdp {
        int fd = -1;
        boolean protectedOk;

        static ProtectedUdp none() {
            return new ProtectedUdp();
        }

        void closeQuietly() {
            if (fd >= 0) {
                try {
                    ParcelFileDescriptor.adoptFd(fd).close();
                } catch (Exception ignored) {
                }
                fd = -1;
            }
        }
    }

    private static boolean isLiteralIp(String host) {
        if (host == null || host.isEmpty()) return false;
        if (host.indexOf(':') >= 0) return looksLikeIpv6(host);
        String[] p = host.split("\\.");
        if (p.length != 4) return false;
        try {
            for (String s : p) {
                int v = Integer.parseInt(s);
                if (v < 0 || v > 255) return false;
            }
            return true;
        } catch (NumberFormatException e) {
            return false;
        }
    }

    private static boolean looksLikeIpv6(String s) {
        return s != null && s.indexOf(':') >= 0;
    }

    private void stopDataPlanes(boolean killTor) {
        try {
            if (killTor) {
                NativeBridge.stopEverything();
            } else {
                NativeBridge.stopTunnel();
            }
        } catch (Exception ignored) {
        }
    }

    private void closeTunnelFd() {
        if (tunnel != null) {
            try {
                tunnel.close();
            } catch (IOException ignored) {
            }
            tunnel = null;
        }
        if (wgUdpKeepalive != null) {
            try { wgUdpKeepalive.close(); } catch (Exception ignored) {}
            wgUdpKeepalive = null;
        }
    }

    private Notification notification(String text, boolean ongoing) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            NotificationChannel channel = new NotificationChannel(
                CHANNEL_ID,
                "ZeroNode VPN",
                NotificationManager.IMPORTANCE_LOW
            );
            channel.setDescription("VPN connection status");
            NotificationManager manager = getSystemService(NotificationManager.class);
            if (manager != null) manager.createNotificationChannel(channel);
        }

        Intent launch = new Intent(this, MainActivity.class);
        launch.setFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP | Intent.FLAG_ACTIVITY_CLEAR_TOP);
        PendingIntent contentIntent = PendingIntent.getActivity(
            this,
            0,
            launch,
            PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT
        );

        Intent stop = stopIntent(this);
        PendingIntent stopPi = PendingIntent.getService(
            this,
            1,
            stop,
            PendingIntent.FLAG_IMMUTABLE | PendingIntent.FLAG_UPDATE_CURRENT
        );

        Notification.Builder builder = Build.VERSION.SDK_INT >= Build.VERSION_CODES.O
            ? new Notification.Builder(this, CHANNEL_ID)
            : new Notification.Builder(this);

        builder
            .setSmallIcon(android.R.drawable.ic_lock_lock)
            .setContentTitle("ZeroNode VPN")
            .setContentText(text)
            .setContentIntent(contentIntent)
            .setOngoing(ongoing)
            .setOnlyAlertOnce(true)
            .addAction(android.R.drawable.ic_menu_close_clear_cancel, "Disconnect", stopPi);

        if (Build.VERSION.SDK_INT >= 31) {
            builder.setForegroundServiceBehavior(Notification.FOREGROUND_SERVICE_IMMEDIATE);
        }
        return builder.build();
    }

    private static String extra(Intent intent, String key, String fallback) {
        if (intent == null) return fallback;
        String v = intent.getStringExtra(key);
        return v == null || v.length() == 0 ? fallback : v;
    }

    private static String firstLine(String value) {
        if (value == null) return "no native status";
        int newline = value.indexOf('\n');
        String line = newline >= 0 ? value.substring(0, newline) : value;
        if (line.startsWith("ERR")) {
            int msg = value.indexOf("message=");
            if (msg >= 0) {
                int end = value.indexOf('\n', msg);
                return end >= 0 ? value.substring(msg + 8, end) : value.substring(msg + 8);
            }
        }
        return line;
    }

    /** Simple CIDR for VpnService routes. */
    static final class RouteCidr {
        final String network;
        final int prefix;
        final boolean ipv6;

        RouteCidr(String network, int prefix, boolean ipv6) {
            this.network = network;
            this.prefix = prefix;
            this.ipv6 = ipv6;
        }
    }

    /**
     * Parsed WireGuard Interface/Peer knobs needed to build a correct TUN
     * (Address, DNS, MTU, AllowedIPs) — not just a dummy 10.x /32 + fake IPv6.
     */
    static final class WgTunConfig {
        String ipv4 = "10.7.0.2";
        int ipv4Prefix = 32;
        String ipv6 = null;
        int ipv6Prefix = 128;
        String dns = "1.1.1.1";
        java.util.ArrayList<String> allDns = new java.util.ArrayList<>();
        int mtu = 1280;
        java.util.ArrayList<RouteCidr> routes = new java.util.ArrayList<>();

        static WgTunConfig fromProfile(String path, String fallbackAddress, String fallbackDns) {
            WgTunConfig c = new WgTunConfig();
            if (fallbackAddress != null && fallbackAddress.length() > 0) {
                applyAddressList(c, fallbackAddress);
            }
            if (fallbackDns != null && fallbackDns.length() > 0) {
                c.dns = fallbackDns.split(",")[0].trim();
            }
            if (path == null || path.length() == 0) return c;
            try {
                java.io.BufferedReader br = new java.io.BufferedReader(new java.io.FileReader(path));
                String line;
                String section = "";
                while ((line = br.readLine()) != null) {
                    line = line.trim();
                    if (line.isEmpty() || line.startsWith("#") || line.startsWith(";")) continue;
                    if (line.startsWith("[") && line.endsWith("]")) {
                        section = line.substring(1, line.length() - 1).trim().toLowerCase();
                        continue;
                    }
                    int eq = line.indexOf('=');
                    if (eq <= 0) continue;
                    String key = line.substring(0, eq).trim();
                    String val = line.substring(eq + 1).trim();
                    if ("interface".equals(section)) {
                        if ("Address".equalsIgnoreCase(key)) {
                            applyAddressList(c, val);
                        } else if ("DNS".equalsIgnoreCase(key)) {
                            c.allDns.clear();
                            for (String part : val.split(",")) {
                                String d = part.trim();
                                if (d.isEmpty()) continue;
                                c.allDns.add(d);
                            }
                            if (!c.allDns.isEmpty()) c.dns = c.allDns.get(0);
                        } else if ("MTU".equalsIgnoreCase(key)) {
                            try {
                                c.mtu = Integer.parseInt(val.trim());
                            } catch (NumberFormatException ignored) {
                            }
                        }
                    } else if ("peer".equals(section)) {
                        if ("AllowedIPs".equalsIgnoreCase(key)) {
                            for (String part : val.split(",")) {
                                RouteCidr r = parseCidr(part.trim());
                                if (r != null) c.routes.add(r);
                            }
                        }
                    }
                }
                br.close();
            } catch (Exception e) {
                Log.w(TAG, "WG profile parse: " + e.getMessage());
            }
            if (c.routes.isEmpty()) {
                c.routes.add(new RouteCidr("0.0.0.0", 0, false));
            }
            return c;
        }

        private static void applyAddressList(WgTunConfig c, String val) {
            if (val == null) return;
            for (String part : val.split(",")) {
                String a = part.trim();
                if (a.isEmpty()) continue;
                int slash = a.indexOf('/');
                int pfx = -1;
                String ip = a;
                if (slash > 0) {
                    ip = a.substring(0, slash).trim();
                    try {
                        pfx = Integer.parseInt(a.substring(slash + 1).trim());
                    } catch (NumberFormatException ignored) {
                    }
                }
                if (ip.contains(":")) {
                    c.ipv6 = ip;
                    c.ipv6Prefix = pfx >= 0 ? pfx : 128;
                } else if (ip.indexOf('.') > 0) {
                    c.ipv4 = ip;
                    c.ipv4Prefix = pfx >= 0 ? pfx : 32;
                }
            }
        }

        private static RouteCidr parseCidr(String s) {
            if (s == null || s.isEmpty()) return null;
            boolean v6 = s.contains(":");
            int slash = s.indexOf('/');
            String net;
            int pfx;
            if (slash > 0) {
                net = s.substring(0, slash).trim();
                try {
                    pfx = Integer.parseInt(s.substring(slash + 1).trim());
                } catch (NumberFormatException e) {
                    pfx = v6 ? 128 : 32;
                }
            } else {
                net = s.trim();
                pfx = v6 ? 128 : 32;
            }
            if (net.isEmpty()) return null;
            if (v6) {
                if (pfx < 0 || pfx > 128) pfx = 128;
            } else {
                if (pfx < 0 || pfx > 32) pfx = 32;
            }
            return new RouteCidr(net, pfx, v6);
        }
    }
}
