package io.zeronode.vpn;

import android.content.Context;
import android.content.SharedPreferences;

import org.json.JSONArray;
import org.json.JSONObject;

import java.io.BufferedReader;
import java.io.File;
import java.io.FileOutputStream;
import java.io.InputStream;
import java.io.InputStreamReader;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;

/**
 * Tor pluggable-transport bridges (obfs4 / snowflake). Java writes
 * {@code user-bridges.conf} into the Tor home; native {@code write_torrc}
 * includes it so Connect Tor picks up the selection.
 */
final class BridgeStore {
    static final String MODE_OFF = "off";
    static final String MODE_OBFS4 = "obfs4";
    static final String MODE_SNOWFLAKE = "snowflake";

    static final String DOCS_URL = "https://tb-manual.torproject.org/bridges/";
    static final String BRIDGES_SITE = "https://bridges.torproject.org/";
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
        prefs(ctx).edit().putString(KEY_MODE, mode).apply();
    }

    static String customLines(Context ctx) {
        String s = prefs(ctx).getString(KEY_CUSTOM, "");
        return s == null ? "" : s;
    }

    static void setCustomLines(Context ctx, String lines) {
        prefs(ctx).edit().putString(KEY_CUSTOM, lines == null ? "" : lines).apply();
    }

    static String summary(Context ctx) {
        String m = mode(ctx);
        if (MODE_OBFS4.equals(m)) return "obfs4 · looks like random TLS";
        if (MODE_SNOWFLAKE.equals(m)) return "Snowflake · looks like a video call";
        return "Direct (no bridge)";
    }

    static String maskBlurb(String mode) {
        if (MODE_OBFS4.equals(mode)) {
            return "obfs4 wraps Tor as ordinary TLS. The handshake looks like a visit to a random HTTPS site, so censors that only block the public Tor network usually miss it.";
        }
        if (MODE_SNOWFLAKE.equals(mode)) {
            return "Snowflake uses WebRTC (the same tech as video calls) and a pool of volunteer proxies. Traffic looks like a call to a CDN, not a VPN or Tor.";
        }
        return "Connecting straight to the public Tor network. This is fastest, but some networks block known Tor relays.";
    }

    static List<String> builtinBridges(Context ctx, String kind) {
        List<String> out = new ArrayList<>();
        try {
            InputStream in = ctx.getAssets().open("tor/pluggable_transports/pt_config.json");
            BufferedReader br = new BufferedReader(new InputStreamReader(in, StandardCharsets.UTF_8));
            StringBuilder sb = new StringBuilder();
            String line;
            while ((line = br.readLine()) != null) sb.append(line);
            br.close();
            JSONObject root = new JSONObject(sb.toString());
            JSONObject bridges = root.optJSONObject("bridges");
            if (bridges == null) return out;
            JSONArray arr = bridges.optJSONArray(kind);
            if (arr == null) return out;
            for (int i = 0; i < arr.length(); i++) {
                String s = arr.optString(i, "");
                if (s.length() > 0) out.add(s);
            }
        } catch (Exception ignored) {
        }
        return out;
    }

    static List<String> activeBridgeLines(Context ctx) {
        String m = mode(ctx);
        List<String> lines = new ArrayList<>();
        String custom = customLines(ctx).trim();
        if (custom.length() > 0) {
            String[] parts = custom.split("\n");
            for (String p : parts) {
                String t = p.trim();
                if (t.length() == 0 || t.startsWith("#")) continue;
                if (t.toLowerCase().startsWith("bridge ")) t = t.substring(7).trim();
                lines.add(t);
            }
        }
        if (MODE_OBFS4.equals(m) && lines.isEmpty()) {
            lines.addAll(builtinBridges(ctx, "obfs4"));
        } else if (MODE_SNOWFLAKE.equals(m) && lines.isEmpty()) {
            lines.addAll(builtinBridges(ctx, "snowflake"));
        }
        return lines;
    }

    /**
     * Write {@code tor_home/user-bridges.conf} consumed by native write_torrc.
     * Empty/off deletes the file so Tor runs without UseBridges.
     */
    static void writeTorrcExtra(Context ctx, File torHome) {
        if (torHome == null) return;
        File extra = new File(torHome, "user-bridges.conf");
        String m = mode(ctx);
        if (MODE_OFF.equals(m)) {
            if (extra.exists()) extra.delete();
            return;
        }
        List<String> lines = activeBridgeLines(ctx);
        if (lines.isEmpty()) {
            if (extra.exists()) extra.delete();
            return;
        }
        StringBuilder body = new StringBuilder();
        body.append("UseBridges 1\n");
        for (String line : lines) {
            body.append("Bridge ").append(line).append('\n');
        }
        try {
            File parent = extra.getParentFile();
            if (parent != null && !parent.exists()) parent.mkdirs();
            FileOutputStream out = new FileOutputStream(extra);
            try {
                out.write(body.toString().getBytes(StandardCharsets.UTF_8));
                out.flush();
            } finally {
                out.close();
            }
        } catch (Exception ignored) {
        }
    }
}
