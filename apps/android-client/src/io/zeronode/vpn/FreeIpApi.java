package io.zeronode.vpn;

import java.util.HashMap;
import java.util.Locale;
import java.util.Map;

/**
 * FreeIPAPI (https://freeipapi.com) client for the free endpoint
 * https://free.freeipapi.com/api/v1/json/{ip} — omitted IP means the caller's
 * address. Free tier allows 10 requests per 10 seconds, up to 60 per minute;
 * cache + backoff keep this client under both. Transport (clearnet TLS,
 * VPN-bound socket, SOCKS tunnel) is injected by the caller so exit-privacy
 * stays under MainActivity control.
 */
final class FreeIpApi {
    interface Fetcher {
        String fetch(String url) throws Exception;
    }

    interface Clock {
        long now();
    }

    static final String HOST = "free.freeipapi.com";

    private static final long CACHE_MS = 60_000L;
    private static final long BACKOFF_MS = 12_000L;

    private final Fetcher fetcher;
    private final Clock clock;
    private final Map<String, String> cache = new HashMap<>();
    private final Map<String, Long> cacheAt = new HashMap<>();
    private final Map<String, Long> backoffUntil = new HashMap<>();

    FreeIpApi(Fetcher fetcher, Clock clock) {
        this.fetcher = fetcher;
        this.clock = clock;
    }

    /**
     * Look up {@code rawIp} (empty = caller's IP as seen by the endpoint) via
     * the injected transport. Returns the standard OK/ERR KV block on success,
     * a cached block within the cache window, or null on failure/backoff.
     */
    synchronized String lookup(String rawIp, String routeKey, Fetcher perCallFetcher) {
        String ip = normalizeIpLiteral(rawIp);
        String key = ip.isEmpty() ? "caller:" + routeKey : ip.toLowerCase(Locale.US);
        long now = clock.now();
        Long until = backoffUntil.get(key);
        if (until != null && until.longValue() > now) return cache.get(key);
        Long at = cacheAt.get(key);
        if (at != null && now - at.longValue() <= CACHE_MS) return cache.get(key);
        String url = "https://" + HOST + "/api/v1/json/"
            + (ip.isEmpty() ? "" : urlEncodePath(ip));
        try {
            String body = (perCallFetcher != null ? perCallFetcher : fetcher).fetch(url);
            String kv = parseBody(body, ip);
            cache.put(key, kv);
            cacheAt.put(key, now);
            backoffUntil.remove(key);
            return kv;
        } catch (Exception e) {
            backoffUntil.put(key, now + BACKOFF_MS);
            return null;
        }
    }

    /**
     * Map a FreeIPAPI JSON response onto the app's KV block. Only documented
     * fields are read: ipAddress, countryName, countryCode, cityName,
     * latitude, longitude, asnOrganization. The response IP must match the
     * requested one (when an IP was requested).
     */
    static String parseBody(String body, String expectedIp) throws Exception {
        if (body == null || !body.startsWith("{")) throw new Exception("unexpected response");
        String respIp = normalizeIpLiteral(jsonStr(body, "ipAddress"));
        if (respIp.isEmpty() || !looksLikeIp(respIp)) throw new Exception("missing ipAddress");
        String expected = normalizeIpLiteral(expectedIp);
        if (!expected.isEmpty() && !respIp.equalsIgnoreCase(expected)) {
            throw new Exception("response IP mismatch");
        }
        return "OK\nip=" + respIp
            + "\ncountry=" + nz(jsonStr(body, "countryName"))
            + "\ncountry_code=" + nz(jsonStr(body, "countryCode"))
            + "\ncity=" + nz(jsonStr(body, "cityName"))
            + "\nlat=" + nz(jsonStr(body, "latitude"))
            + "\nlon=" + nz(jsonStr(body, "longitude"))
            + "\nisp=" + nz(jsonStr(body, "asnOrganization"));
    }

    /** Strip brackets and IPv6 zone id, trim whitespace. */
    static String normalizeIpLiteral(String ip) {
        if (ip == null) return "";
        String s = ip.trim();
        if (s.startsWith("[") && s.endsWith("]") && s.length() > 2) {
            s = s.substring(1, s.length() - 1);
        }
        int zone = s.indexOf('%');
        if (zone > 0) s = s.substring(0, zone);
        return s.trim();
    }

    /** Percent-encode an IP literal for safe placement in a URL path. */
    static String urlEncodePath(String ip) {
        StringBuilder sb = new StringBuilder();
        for (int i = 0; i < ip.length(); i++) {
            char c = ip.charAt(i);
            boolean safe = (c >= '0' && c <= '9') || (c >= 'a' && c <= 'z')
                || (c >= 'A' && c <= 'Z') || c == '.' || c == ':' || c == '-';
            if (safe) {
                sb.append(c);
            } else {
                sb.append('%').append(String.format(Locale.US, "%02X", (int) c));
            }
        }
        return sb.toString();
    }

    private static boolean looksLikeIp(String ip) {
        if (ip == null || ip.length() < 3) return false;
        if (ip.indexOf('<') >= 0 || ip.indexOf(' ') >= 0) return false;
        return ip.indexOf('.') > 0 || ip.indexOf(':') > 0;
    }

    private static String jsonStr(String json, String key) {
        String pat = "\"" + key + "\"";
        int i = json.indexOf(pat);
        if (i < 0) return "";
        int colon = json.indexOf(':', i + pat.length());
        if (colon < 0) return "";
        int start = colon + 1;
        while (start < json.length() && Character.isWhitespace(json.charAt(start))) start++;
        if (start >= json.length()) return "";
        if (json.charAt(start) == '"') {
            int q2 = json.indexOf('"', start + 1);
            return q2 > start ? json.substring(start + 1, q2) : "";
        }
        int end = start;
        while (end < json.length()) {
            char c = json.charAt(end);
            if (c == ',' || c == '}' || c == ']' || Character.isWhitespace(c)) break;
            end++;
        }
        return json.substring(start, end);
    }

    private static String nz(String s) {
        return s == null ? "" : s;
    }
}
