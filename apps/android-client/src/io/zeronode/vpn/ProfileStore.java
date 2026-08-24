package io.zeronode.vpn;

import android.content.Context;
import android.content.SharedPreferences;

import org.json.JSONArray;
import org.json.JSONObject;

import java.util.ArrayList;
import java.util.Collections;
import java.util.Comparator;
import java.util.List;
import java.util.Locale;
import java.util.UUID;

/**
 * Named profile database (SharedPreferences JSON) — Windows-like selectable list
 * for WireGuard / Outline credentials.
 */
final class ProfileStore {
    private static final String PREFS = "zeronode_profile_db_v1";
    private static final String KEY = "profiles_json";

    static final String KIND_WG = "wireguard";
    static final String KIND_OUTLINE = "outline";

    static final class Profile {
        String id;
        String kind;
        String name;
        String content;
        String user;
        String password;
        String host;
        String country = "";
        String countryCode = "";
        String city = "";
        double lat;
        double lon;
        String resolvedIp = "";
        String resolvedIp6 = "";
        long updatedAt;

        Profile() {
            id = UUID.randomUUID().toString();
            updatedAt = System.currentTimeMillis();
        }

        boolean hasLocation() {
            return (countryCode != null && countryCode.length() == 2)
                || (country != null && country.trim().length() > 0);
        }

        String locationLabel() {
            String c = city == null ? "" : city.trim();
            String n = country == null ? "" : country.trim();
            if (c.length() > 0 && n.length() > 0) return c + ", " + n;
            if (n.length() > 0) return n;
            if (c.length() > 0) return c;
            if (countryCode != null && countryCode.length() == 2) return countryCode.toUpperCase(Locale.US);
            return "";
        }
    }

    private ProfileStore() {}

    static List<Profile> list(Context ctx, String kind) {
        List<Profile> all = loadAll(ctx);
        List<Profile> out = new ArrayList<>();
        for (Profile p : all) {
            if (kind == null || kind.equalsIgnoreCase(p.kind)) out.add(p);
        }
        Collections.sort(out, new Comparator<Profile>() {
            @Override
            public int compare(Profile a, Profile b) {
                return Long.compare(b.updatedAt, a.updatedAt);
            }
        });
        return out;
    }

    static Profile get(Context ctx, String id) {
        if (id == null) return null;
        for (Profile p : loadAll(ctx)) {
            if (id.equals(p.id)) return p;
        }
        return null;
    }

    static Profile save(
        Context ctx,
        String idOrNull,
        String kind,
        String name,
        String content,
        String user,
        String password,
        String host
    ) {
        List<Profile> all = loadAll(ctx);
        Profile target = null;
        if (idOrNull != null) {
            for (Profile p : all) {
                if (idOrNull.equals(p.id)) {
                    target = p;
                    break;
                }
            }
        }
        if (target == null) {
            // Dedup by same kind+content
            String c = content == null ? "" : content.trim();
            for (Profile p : all) {
                if (kind.equalsIgnoreCase(p.kind)
                    && c.length() > 0
                    && c.equals(p.content != null ? p.content.trim() : "")) {
                    target = p;
                    break;
                }
            }
        }
        if (target == null) {
            target = new Profile();
            all.add(target);
        }
        String prevContent = target.content != null ? target.content.trim() : "";
        String newContent = content == null ? "" : content.trim();
        boolean contentChanged = !prevContent.equals(newContent);
        target.kind = kind;
        target.name = (name == null || name.trim().isEmpty())
            ? defaultName(kind, content, host)
            : name.trim();
        target.content = content == null ? "" : content;
        target.user = user == null ? "" : user;
        target.password = password == null ? "" : password;
        String resolvedHost = host == null || host.trim().isEmpty()
            ? extractHost(kind, target.content)
            : host.trim();
        target.host = resolvedHost;
        if (contentChanged) {
            target.country = "";
            target.countryCode = "";
            target.city = "";
            target.lat = 0;
            target.lon = 0;
            target.resolvedIp = "";
            target.resolvedIp6 = "";
        }
        target.updatedAt = System.currentTimeMillis();
        persist(ctx, all);
        return target;
    }

    static void updateLocation(
        Context ctx,
        String id,
        String country,
        String countryCode,
        String city,
        double lat,
        double lon,
        String resolvedIp,
        String resolvedIp6
    ) {
        if (id == null) return;
        List<Profile> all = loadAll(ctx);
        for (Profile p : all) {
            if (!id.equals(p.id)) continue;
            p.country = country == null ? "" : country.trim();
            p.countryCode = countryCode == null ? "" : countryCode.trim().toUpperCase(Locale.US);
            p.city = city == null ? "" : city.trim();
            p.lat = lat;
            p.lon = lon;
            p.resolvedIp = resolvedIp == null ? "" : resolvedIp.trim();
            p.resolvedIp6 = resolvedIp6 == null ? "" : resolvedIp6.trim();
            if (isPlaceholderName(p.name, p.kind, p.content, p.host) && p.hasLocation()) {
                p.name = taggedName(p);
            }
            p.updatedAt = System.currentTimeMillis();
            persist(ctx, all);
            return;
        }
    }

    static boolean showsEndpointIp(String kind) {
        return KIND_WG.equals(kind);
    }

    static String formatEndpointIps(String v4, String v6) {
        String a = v4 == null ? "" : v4.trim();
        String b = v6 == null ? "" : v6.trim();
        if (a.length() > 0 && b.length() > 0) return a + "  ·  " + b;
        if (a.length() > 0) return a;
        return b;
    }

    static boolean isPlaceholderName(String name, String kind, String content, String host) {
        if (name == null || name.trim().isEmpty()) return true;
        String n = name.trim();
        String def = defaultName(kind, content, host);
        if (n.equalsIgnoreCase(def)) return true;
        if (n.equalsIgnoreCase("WireGuard profile")
            || n.equalsIgnoreCase("Outline key")
            || n.equalsIgnoreCase("Profile")) {
            return true;
        }
        if (host != null && host.length() > 0 && (n.equals(host) || n.equals("WG " + host))) {
            return true;
        }
        return n.startsWith("WG ") || n.startsWith("Outline ");
    }

    static String taggedName(Profile p) {
        String loc = p.locationLabel();
        if (loc.length() == 0) {
            return p.name != null && p.name.length() > 0 ? p.name : defaultName(p.kind, p.content, p.host);
        }
        return loc;
    }

    static String extractHost(String kind, String content) {
        if (content == null) return "";
        String c = content.trim();
        if (c.length() == 0) return "";
        if (KIND_WG.equals(kind)) return hostFromWireGuard(c);
        if (KIND_OUTLINE.equals(kind)) return hostFromOutline(c);
        String wg = hostFromWireGuard(c);
        if (wg.length() > 0) return wg;
        return hostFromOutline(c);
    }

    private static String hostFromWireGuard(String content) {
        for (String line : content.split("\n")) {
            String t = line.trim();
            if (t.toLowerCase(Locale.US).startsWith("endpoint")) {
                int eq = t.indexOf('=');
                if (eq < 0) continue;
                return stripPort(t.substring(eq + 1).trim());
            }
        }
        return "";
    }

    private static String hostFromOutline(String content) {
        String t = content.trim();
        if (t.startsWith("ss://") || t.startsWith("ssconf://")) {
            String rest = t.substring(t.indexOf("://") + 3);
            int hash = rest.indexOf('#');
            if (hash >= 0) rest = rest.substring(0, hash);
            int at = rest.lastIndexOf('@');
            String hostPort = at >= 0 ? rest.substring(at + 1) : rest;
            // ss://BASE64 or ss://BASE64@host:port
            if (at < 0 && !hostPort.contains(":")) {
                try {
                    String pad = hostPort;
                    int rem = pad.length() % 4;
                    if (rem > 0) {
                        StringBuilder sb = new StringBuilder(pad);
                        for (int i = 0; i < 4 - rem; i++) sb.append('=');
                        pad = sb.toString();
                    }
                    byte[] dec = android.util.Base64.decode(pad, android.util.Base64.URL_SAFE);
                    String decoded = new String(dec, java.nio.charset.StandardCharsets.UTF_8);
                    int dat = decoded.lastIndexOf('@');
                    if (dat >= 0) return stripPort(decoded.substring(dat + 1).trim());
                    if (decoded.contains(":")) {
                        // method:pass@host:port already handled; method:pass:host:port
                    }
                } catch (Exception ignored) {
                }
            }
            return stripPort(hostPort);
        }
        if (t.startsWith("{")) {
            String server = jsonLoose(t, "server");
            if (server.length() == 0) server = jsonLoose(t, "host");
            if (server.length() == 0) server = jsonLoose(t, "serverHost");
            return stripPort(server);
        }
        return "";
    }

    private static String jsonLoose(String json, String key) {
        String needle = "\"" + key + "\"";
        int i = json.indexOf(needle);
        if (i < 0) return "";
        int colon = json.indexOf(':', i + needle.length());
        if (colon < 0) return "";
        int q1 = json.indexOf('"', colon + 1);
        if (q1 < 0) return "";
        int q2 = json.indexOf('"', q1 + 1);
        if (q2 < 0) return "";
        return json.substring(q1 + 1, q2).trim();
    }

    static String stripPort(String hostPort) {
        if (hostPort == null) return "";
        String s = hostPort.trim();
        if (s.startsWith("[")) {
            int end = s.indexOf(']');
            if (end > 0) return s.substring(1, end);
        }
        // host:port — don't split IPv6
        int colon = s.lastIndexOf(':');
        if (colon > 0 && s.indexOf(':') == colon) {
            return s.substring(0, colon);
        }
        return s;
    }

    static void delete(Context ctx, String id) {
        if (id == null) return;
        List<Profile> all = loadAll(ctx);
        for (int i = all.size() - 1; i >= 0; i--) {
            if (id.equals(all.get(i).id)) all.remove(i);
        }
        persist(ctx, all);
    }

    static String detectKind(String text, String fileNameHint) {
        String lowerName = fileNameHint == null ? "" : fileNameHint.toLowerCase(Locale.US);
        String t = text == null ? "" : text.trim();
        String lower = t.toLowerCase(Locale.US);

        if (lowerName.endsWith(".conf") || lowerName.contains("wireguard") || lowerName.contains("wg")) {
            // Still sniff content
        }
        if (lower.startsWith("ss://") || lower.startsWith("ssconf://")
            || lower.contains("\"method\"") && lower.contains("\"password\"")
            || lower.contains("outline")) {
            return KIND_OUTLINE;
        }
        if (lower.contains("[interface]") && lower.contains("[peer]")) {
            return KIND_WG;
        }
        if (lower.contains("privatekey") && lower.contains("endpoint") && lower.contains("publickey")) {
            return KIND_WG;
        }
        if (lowerName.endsWith(".conf")) return KIND_WG;
        if (t.startsWith("{") && lower.contains("server") && lower.contains("password")) {
            return KIND_OUTLINE;
        }
        return null;
    }

    static String defaultName(String kind, String content, String host) {
        if (host != null && host.trim().length() > 0) {
            return host.trim();
        }
        if (content != null) {
            String c = content.trim();
            if (c.startsWith("ss://")) {
                int hash = c.lastIndexOf('#');
                if (hash > 0 && hash < c.length() - 1) {
                    try {
                        return java.net.URLDecoder.decode(c.substring(hash + 1), "UTF-8");
                    } catch (Exception ignored) {
                        return c.substring(hash + 1);
                    }
                }
                return "Outline " + shortId(c);
            }
            // WireGuard endpoint
            for (String line : c.split("\n")) {
                String t = line.trim();
                if (t.toLowerCase(Locale.US).startsWith("endpoint")) {
                    int eq = t.indexOf('=');
                    if (eq > 0) return "WG " + t.substring(eq + 1).trim();
                }

            }
        }
        String k = kind == null ? "Profile" : kind;
        if (KIND_WG.equals(k)) return "WireGuard profile";
        if (KIND_OUTLINE.equals(k)) return "Outline key";
        return "Profile";
    }

    private static String shortId(String s) {
        int h = s.hashCode();
        return Integer.toHexString(h & 0xFFFF);
    }

    private static List<Profile> loadAll(Context ctx) {
        SharedPreferences prefs = ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE);
        String raw = prefs.getString(KEY, "[]");
        List<Profile> list = new ArrayList<>();
        try {
            JSONArray arr = new JSONArray(raw);
            for (int i = 0; i < arr.length(); i++) {
                JSONObject o = arr.optJSONObject(i);
                if (o == null) continue;
                Profile p = new Profile();
                p.id = o.optString("id", UUID.randomUUID().toString());
                p.kind = o.optString("kind", "");
                if ("openvpn".equalsIgnoreCase(p.kind) || "ovpn".equalsIgnoreCase(p.kind)) {
                    continue;
                }
                p.name = o.optString("name", "Profile");
                p.content = o.optString("content", "");
                p.user = o.optString("user", "");
                p.password = o.optString("password", "");
                p.host = o.optString("host", "");
                p.country = o.optString("country", "");
                p.countryCode = o.optString("countryCode", "");
                p.city = o.optString("city", "");
                p.lat = o.optDouble("lat", 0);
                p.lon = o.optDouble("lon", 0);
                p.resolvedIp = o.optString("resolvedIp", "");
                p.resolvedIp6 = o.optString("resolvedIp6", "");
                p.updatedAt = o.optLong("updatedAt", System.currentTimeMillis());
                list.add(p);
            }
        } catch (Exception ignored) {
        }
        return list;
    }

    private static void persist(Context ctx, List<Profile> all) {
        JSONArray arr = new JSONArray();
        try {
            for (Profile p : all) {
                JSONObject o = new JSONObject();
                o.put("id", p.id);
                o.put("kind", p.kind);
                o.put("name", p.name);
                o.put("content", p.content);
                o.put("user", p.user);
                o.put("password", p.password);
                o.put("host", p.host);
                o.put("country", p.country == null ? "" : p.country);
                o.put("countryCode", p.countryCode == null ? "" : p.countryCode);
                o.put("city", p.city == null ? "" : p.city);
                o.put("lat", p.lat);
                o.put("lon", p.lon);
                o.put("resolvedIp", p.resolvedIp == null ? "" : p.resolvedIp);
                o.put("resolvedIp6", p.resolvedIp6 == null ? "" : p.resolvedIp6);
                o.put("updatedAt", p.updatedAt);
                arr.put(o);
            }
        } catch (Exception ignored) {
        }
        ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
            .edit()
            .putString(KEY, arr.toString())
            .apply();
    }
}
