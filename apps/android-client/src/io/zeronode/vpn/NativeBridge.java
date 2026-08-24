package io.zeronode.vpn;

/**
 * JNI bridge into libmain.so (Rust). All methods are safe when the native
 * library fails to load — they return ERR strings instead of crashing.
 */
final class NativeBridge {
    private static boolean loaded;

    static {
        try {
            System.loadLibrary("main");
            loaded = true;
        } catch (UnsatisfiedLinkError ignored) {
            loaded = false;
        }
    }

    private NativeBridge() {}

    static boolean isLoaded() {
        return loaded;
    }

    static String platformSummary() {
        if (!loaded) return "Rust core unavailable";
        return nativePlatformSummary();
    }

    static String discover(String hosts) {
        if (!loaded) return "ERR\nmessage=Rust core unavailable";
        return nativeDiscover(hosts == null ? "" : hosts);
    }

    static String getStatus() {
        if (!loaded) return "ERR\nmessage=Rust core unavailable";
        return nativeGetStatus();
    }

    static String connect(String host, String password) {
        if (!loaded) return "ERR\nmessage=Rust core unavailable";
        return nativeConnect(host == null ? "" : host, password == null ? "" : password);
    }

    static String disconnect() {
        if (!loaded) return "ERR\nmessage=Rust core unavailable";
        return nativeDisconnect();
    }

    /** @deprecated prefer startTunnel */
    static String startPacketPump(int tunFd, String profilePath) {
        return startTunnel(tunFd, "wireguard", profilePath, "", "", "", "", "", "");
    }

    static String stopPacketPump() {
        return stopTunnel();
    }

    /**
     * Start a protocol data-plane on the VpnService TUN fd.
     *
     * @param kind wireguard | outline | tor | zeronode
     */
    static String startTunnel(
        int tunFd,
        String kind,
        String profileOrKey,
        String host,
        String port,
        String user,
        String password,
        String method,
        String extra
    ) {
        if (!loaded) return "ERR\nmessage=Rust core unavailable";
        return nativeStartTunnel(
            tunFd,
            kind == null ? "" : kind,
            profileOrKey == null ? "" : profileOrKey,
            host == null ? "" : host,
            port == null ? "" : port,
            user == null ? "" : user,
            password == null ? "" : password,
            method == null ? "" : method,
            extra == null ? "" : extra
        );
    }

    /** Stop data planes but keep Tor SOCKS process (needed before re-attach). */
    static String stopTunnel() {
        if (!loaded) return "ERR\nmessage=Rust core unavailable";
        return nativeStopTunnel();
    }

    /** Full teardown including Tor process (user Disconnect). */
    static String stopEverything() {
        if (!loaded) return "ERR\nmessage=Rust core unavailable";
        return nativeStopEverything();
    }

    /** Start Tor SOCKS only (no system tunnel). Paths must be absolute. */
    static String startTorSocks(String torHome, String nativeLibDir) {
        if (!loaded) return "ERR\nmessage=Rust core unavailable";
        return nativeStartTorSocks(
            torHome == null ? "" : torHome,
            nativeLibDir == null ? "" : nativeLibDir
        );
    }

    static String attachTorSystemTunnel(int tunFd) {
        if (!loaded) return "ERR\nmessage=Rust core unavailable";
        return nativeAttachTorSystemTunnel(tunFd);
    }

    static String parseOutline(String key) {
        if (!loaded) return "ERR\nmessage=Rust core unavailable";
        return nativeParseOutline(key == null ? "" : key);
    }

    static String parseWireGuard(String conf) {
        if (!loaded) return "ERR\nmessage=Rust core unavailable";
        return nativeParseWireGuard(conf == null ? "" : conf);
    }

    static String getProgress() {
        if (!loaded) return "OK\nstage=\nfraction=0\ndetail=";
        return nativeGetProgress();
    }

    static String fetchPublicIp() {
        if (!loaded) return "ERR\nmessage=Rust core unavailable";
        return nativeFetchPublicIp();
    }

    static String torBootstrap() {
        if (!loaded) return "STOPPED";
        return nativeTorBootstrap();
    }

    /** Local SOCKS port for Outline (0 if not running) — use for in-app IP via tunnel. */
    static int outlineSocksPort() {
        if (!loaded) return 0;
        try {
            String s = nativeOutlineSocksPort();
            if (s == null) return 0;
            return Integer.parseInt(s.trim());
        } catch (Exception e) {
            return 0;
        }
    }

    /**
     * Register JavaVM + protect callback so WireGuard UDP can bypass the TUN.
     * Safe to call multiple times.
     */
    static void ensureProtectBridge() {
        if (!loaded) return;
        try {
            nativeEnsureProtectBridge();
        } catch (UnsatisfiedLinkError ignored) {
        }
    }

    private static native void nativeEnsureProtectBridge();
    private static native String nativePlatformSummary();
    private static native String nativeDiscover(String hosts);
    private static native String nativeGetStatus();
    private static native String nativeConnect(String host, String password);
    private static native String nativeDisconnect();
    private static native String nativeStartPacketPump(int tunFd, String profilePath);
    private static native String nativeStopPacketPump();
    private static native String nativeStartTunnel(
        int tunFd,
        String kind,
        String profileOrKey,
        String host,
        String port,
        String user,
        String password,
        String method,
        String extra
    );
    private static native String nativeStopTunnel();
    private static native String nativeStopEverything();
    private static native String nativeStartTorSocks(String torHome, String nativeLibDir);
    private static native String nativeAttachTorSystemTunnel(int tunFd);
    private static native String nativeParseOutline(String key);
    private static native String nativeParseWireGuard(String conf);
    private static native String nativeGetProgress();
    private static native String nativeFetchPublicIp();
    private static native String nativeTorBootstrap();
    private static native String nativeOutlineSocksPort();
}
