package io.zeronode.vpn;

import android.content.Context;
import android.content.SharedPreferences;
import android.net.InetAddresses;
import android.util.AtomicFile;
import android.util.Base64;

import org.json.JSONArray;
import org.json.JSONObject;

import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;

final class BridgeStore {
    static final String MODE_OFF = "off";
    static final String MODE_OBFS4 = "obfs4";
    static final String MODE_SNOWFLAKE = "snowflake";
    static final String TELEGRAM_BOT = "https://t.me/GetBridgesBot";

    private static final String PREFS = "zeronode_bridges";
    private static final String KEY_MODE = "mode";
    private static final String KEY_CUSTOM = "custom_lines";

    private BridgeStore() {}

    static SharedPreferences prefs(Context ctx) {
        return ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE);
    }

    static String mode(Context ctx) {
        String m = prefs(ctx).getString(KEY_MODE, MODE_OFF);
        return m == null ? MODE_OFF : m;
    }

    static boolean enabled(Context ctx) {
        return !MODE_OFF.equals(mode(ctx));
    }

    static void setMode(Context ctx, String mode) {
        if (mode == null) mode = MODE_OFF;
        if (!MODE_OFF.equals(mode) && !MODE_OBFS4.equals(mode) && !MODE_SNOWFLAKE.equals(mode)) {
            throw new IllegalArgumentException("Unsupported bridge mode");
        }
        prefs(ctx).edit().putString(KEY_MODE, mode).apply();
    }

    static String customLines(Context ctx) {
        String s = prefs(ctx).getString(KEY_CUSTOM, "");
        return s == null ? "" : s;
    }

    static void setCustomLines(Context ctx, String lines) {
        String value = lines == null ? "" : lines;
        parseObfs4(value);
        prefs(ctx).edit().putString(KEY_CUSTOM, value).apply();
    }

    static String summary(Context ctx) {
        String m = mode(ctx);
        if (MODE_OFF.equals(m)) return "Direct (no bridge)";
        try {
            List<String> lines = activeBridgeLines(ctx);
            if (MODE_SNOWFLAKE.equals(m)) return "Snowflake · automatic (built-in)";
            return "obfs4 · " + lines.size() + " configured bridges";
        } catch (IllegalArgumentException e) {
            return MODE_SNOWFLAKE.equals(m) ? "Snowflake · bundle unavailable" : "obfs4 · valid configuration required";
        }
    }

    static String maskBlurb(String mode) {
        if (MODE_OBFS4.equals(mode)) {
            return "obfs4 disguises Tor traffic as random bytes, not HTTPS. It needs a complete bridge address, fingerprint, certificate and iat-mode. Bundled bridges are used unless overridden below.";
        }
        if (MODE_SNOWFLAKE.equals(mode)) {
            return "Snowflake automatically uses the bundled broker, WebRTC and volunteer proxies. No bridge needs to be pasted. Saved obfs4 lines are ignored in this mode. Availability depends on your network.";
        }
        return "Connect directly to public Tor relays without bridges. Saved obfs4 lines are kept but not used. Some networks block known Tor relays.";
    }

    static List<String> builtinBridges(Context ctx, String kind) {
        List<String> out = new ArrayList<>();
        try {
            JSONObject root = TorBundle.transportConfig(ctx);
            JSONObject plugins = root.optJSONObject("pluggableTransports");
            String pluginKey = MODE_SNOWFLAKE.equals(kind) ? MODE_SNOWFLAKE : "lyrebird";
            String plugin = plugins == null ? "" : plugins.optString(pluginKey, "");
            if (!plugin.endsWith(" exec ${pt_path}lyrebird")) {
                throw new IOException("Bundled Lyrebird transport is not configured");
            }
            String[] declaration = plugin.split("\\s+");
            boolean supported = false;
            if (declaration.length == 4 && "ClientTransportPlugin".equals(declaration[0])) {
                for (String transport : declaration[1].split(",")) {
                    if (kind.equals(transport)) supported = true;
                }
            }
            if (!supported) throw new IOException("Transport missing from bundle");
            JSONObject bridges = root.optJSONObject("bridges");
            JSONArray arr = bridges == null ? null : bridges.optJSONArray(kind);
            if (arr != null) {
                for (int i = 0; i < arr.length(); i++) {
                    String line = normalize(arr.getString(i));
                    validateBridge(line, kind);
                    if (!out.contains(line)) out.add(line);
                }
            }
        } catch (Exception e) {
            throw new IllegalArgumentException("Bundled " + kind + " configuration is unavailable or invalid", e);
        }
        if (out.isEmpty()) throw new IllegalArgumentException("No bundled " + kind + " bridges available");
        return out;
    }

    static List<String> activeBridgeLines(Context ctx) {
        String m = mode(ctx);
        if (MODE_OFF.equals(m)) return new ArrayList<>();
        if (MODE_SNOWFLAKE.equals(m)) return builtinBridges(ctx, MODE_SNOWFLAKE);
        if (!MODE_OBFS4.equals(m)) throw new IllegalArgumentException("Unsupported bridge mode");
        List<String> lines = parseObfs4(customLines(ctx));
        return lines.isEmpty() ? builtinBridges(ctx, MODE_OBFS4) : lines;
    }

    private static List<String> parseObfs4(String value) {
        List<String> lines = new ArrayList<>();
        if (value.length() > 65536) throw new IllegalArgumentException("Bridge configuration is too large");
        int number = 0;
        for (String raw : value.split("\\r?\\n")) {
            number++;
            String line = normalize(raw);
            if (line.isEmpty() || line.startsWith("#")) continue;
            try {
                validateBridge(line, MODE_OBFS4);
            } catch (IllegalArgumentException e) {
                throw new IllegalArgumentException("Invalid obfs4 bridge on line " + number
                    + ". Include address:port, fingerprint, cert and iat-mode.");
            }
            if (!lines.contains(line)) lines.add(line);
        }
        return lines;
    }

    private static String normalize(String value) {
        String line = value.trim();
        if (line.matches("(?i)^bridge\\s+.*")) line = line.substring(6).trim();
        return line;
    }

    private static void validateBridge(String line, String kind) {
        if (line.length() > 4096) throw new IllegalArgumentException("Bridge line is too long");
        for (int i = 0; i < line.length(); i++) {
            char c = line.charAt(i);
            if (Character.isISOControl(c) && c != '\t') {
                throw new IllegalArgumentException("Invalid bridge characters");
            }
        }
        String[] parts = line.split("[ \\t]+");
        if (parts.length < 5 || !kind.equals(parts[0]) || !parts[2].matches("[0-9A-Fa-f]{40}")) {
            throw new IllegalArgumentException("Invalid bridge header");
        }
        int colon = parts[1].lastIndexOf(':');
        if (colon < 1) throw new IllegalArgumentException("Invalid bridge address");
        String host = parts[1].substring(0, colon);
        boolean ipv6 = host.startsWith("[") && host.endsWith("]");
        if (ipv6) host = host.substring(1, host.length() - 1);
        if ((!ipv6 && host.contains(":")) || !InetAddresses.isNumericAddress(host)) {
            throw new IllegalArgumentException("Invalid bridge IP");
        }
        String portText = parts[1].substring(colon + 1);
        if (!portText.matches("[0-9]{1,5}")) throw new IllegalArgumentException("Invalid bridge port");
        int port = Integer.parseInt(portText);
        if (port < 1 || port > 65535) throw new IllegalArgumentException("Invalid bridge port");
        if (MODE_OBFS4.equals(kind)) {
            if (parts.length != 5) throw new IllegalArgumentException("Invalid obfs4 options");
            String cert = option(parts, "cert");
            if (!cert.matches("[A-Za-z0-9+/]{70}(==)?")
                || Base64.decode(cert, Base64.NO_WRAP).length != 52
                || !option(parts, "iat-mode").matches("[012]")) {
                throw new IllegalArgumentException("Invalid obfs4 certificate or iat-mode");
            }
        } else if (!parts[2].equalsIgnoreCase(option(parts, "fingerprint"))
            || !option(parts, "url").startsWith("https://")
            || (option(parts, "fronts").isEmpty() && option(parts, "front").isEmpty())
            || !option(parts, "ice").startsWith("stun:")) {
            throw new IllegalArgumentException("Incomplete Snowflake configuration");
        }
    }

    private static String option(String[] parts, String key) {
        String value = "";
        for (int i = 3; i < parts.length; i++) {
            if (parts[i].startsWith(key + "=")) {
                if (!value.isEmpty()) throw new IllegalArgumentException("Duplicate bridge option");
                value = parts[i].substring(key.length() + 1);
            }
        }
        return value;
    }

    static synchronized void writeTorrcExtra(Context ctx, File torHome) throws IOException {
        if (torHome == null) throw new IOException("Tor home is missing");
        if (!torHome.isDirectory() && !torHome.mkdirs()) throw new IOException("Cannot create Tor home");
        StringBuilder body = new StringBuilder();
        if (!MODE_OFF.equals(mode(ctx))) {
            List<String> lines;
            try {
                lines = activeBridgeLines(ctx);
            } catch (IllegalArgumentException e) {
                throw new IOException(e.getMessage(), e);
            }
            TorBundle.requireBridgeTransport(ctx, torHome);
            body.append("UseBridges 1\n");
            for (String line : lines) body.append("Bridge ").append(line).append('\n');
        } else {
            body.append("UseBridges 0\n");
        }
        AtomicFile extra = new AtomicFile(new File(torHome, "user-bridges.conf"));
        FileOutputStream out = null;
        try {
            out = extra.startWrite();
            out.write(body.toString().getBytes(StandardCharsets.UTF_8));
            extra.finishWrite(out);
        } catch (IOException e) {
            extra.failWrite(out);
            throw new IOException("Cannot save Tor bridge configuration", e);
        }
    }
}
