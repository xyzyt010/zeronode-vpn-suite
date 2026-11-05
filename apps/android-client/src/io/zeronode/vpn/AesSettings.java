package io.zeronode.vpn;

import android.content.Context;

import java.util.Locale;

/**
 * Client-side AES-256 enforcement.
 *
 * <p><b>Real, protocol-correct scope.</b> There is no meaningful, server-free
 * packet layer that can be "forced" on all three clients at the transport
 * without either breaking the connection or needing the deferred Zeronode
 * server (whose AES-256 double-hop lives on the server side). So "Force
 * AES-256" here means the honest, safe thing the client fully controls:
 * <ul>
 *   <li><b>WireGuard</b> — already AEAD-encrypted (ChaCha20-Poly1305) by
 *       protocol; nothing to add.</li>
 *   <li><b>Outline/Shadowsocks</b> — the one cipher the client chooses; when
 *       enforcement is ON we <b>refuse to connect</b> on a reversible/weak
 *       stream cipher and require an AES-256-class AEAD.</li>
 *   <li><b>Tor</b> — device→relay already encrypted (TLS + onion); AES-256 over
 *       the exit hop is the Zeronode server path, built later.</li>
 * </ul>
 *
 * <p>Default ON (forcing AES-256-class encryption), toggleable in Settings.
 * The strong-cipher test mirrors {@code vpn_suite_core::outline::is_aead}, so
 * it never disagrees with the Rust core that actually runs the tunnel.
 */
final class AesSettings {
    private static final String PREFS = "zeronode_aes256";
    private static final String KEY_ENABLED = "enabled";

    private AesSettings() {}

    /** Enforce AES-256-class encryption. ON by default. */
    static synchronized boolean enabled(Context ctx) {
        return ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getBoolean(KEY_ENABLED, true);
    }

    static synchronized void setEnabled(Context ctx, boolean on) {
        ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putBoolean(KEY_ENABLED, on).apply();
    }

    /**
     * True when {@code method} is an AES-256-class AEAD cipher. Mirrors the
     * Rust core's {@code OutlineEndpoint::is_aead}: any gcm / poly1305 / 2022
     * variant counts (e.g. {@code aes-256-gcm}, {@code chacha20-ietf-poly1305},
     * {@code 2022-blake3-aes-256-gcm}).
     */
    static boolean isStrongCipher(String method) {
        if (method == null) return false;
        String m = method.trim().toLowerCase(Locale.US);
        if (m.isEmpty()) return false;
        return m.contains("gcm")
            || m.contains("poly1305")
            || m.contains("2022");
    }

    /** Human label for the cipher a connection will actually use (for UI). */
    static String cipherLabel(String method) {
        if (method == null) return "";
        String m = method.trim();
        return m.isEmpty() ? "" : m.toUpperCase(Locale.US);
    }

    /**
     * Connect-time guard.
     *
     * @return {@code null} when it's OK to connect, or a user-facing message
     *         explaining the block when AES-256 enforcement is ON and the
     *         requested Outline cipher is not AES-256-class. Only the
     *         Outline/Shadowsocks kind exposes a client-chosen cipher, so
     *         other kinds always pass.
     */
    static String guardCipher(Context ctx, String kind, String method) {
        if (!enabled(ctx)) return null;
        if (kind == null) return null;
        String k = kind.trim().toLowerCase(Locale.US);
        if (!k.equals("outline") && !k.equals("ss") && !k.equals("shadowsocks")) return null;
        if (method == null || method.trim().isEmpty()) return null; // parsed from ss:// later; can't judge here
        if (isStrongCipher(method)) return null;
        return "AES-256 is enforced (Settings \u2192 Encryption): the Outline cipher \""
            + method.trim() + "\" is not AES-256-class. Use aes-256-gcm or "
            + "chacha20-ietf-poly1305, or turn off \"Force AES-256\" in Settings.";
    }
}
