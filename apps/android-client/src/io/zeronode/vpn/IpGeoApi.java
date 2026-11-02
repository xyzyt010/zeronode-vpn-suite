package io.zeronode.vpn;

import java.util.HashMap;
import java.util.Locale;
import java.util.Map;

/**
 * Online GeoIP lookup for one specific IP address.
 *
 * <p>Endpoint parity with the desktop clients — the same three services the
 * Linux and Windows builds already use in {@code apps/client/src/ovpn.rs} and
 * {@code apps/client/src/tor_geo.rs}:
 *
 * <ol>
 *   <li>{@code https://ipwho.is/{ip}} — HTTPS, IPv4 + IPv6, rich geo</li>
 *   <li>{@code http://ip-api.com/json/{ip}?fields=...} — desktop endpoint</li>
 *   <li>{@code https://ipapi.co/{ip}/json/} — HTTPS fallback</li>
 * </ol>
 *
 * <p>No on-device address database is consulted: every answer comes from the
 * live API, so an exit address and an arbitrary pasted address are described
 * exactly the same way on Android, Linux and Windows.
 *
 * <p>Results are returned in the app's standard {@code OK / key=value} block
 * format so every existing consumer ({@link IpLookup#detailsFromKvBlock},
 * {@code MainActivity#detailsFromKv}, {@code overwriteIpField}) keeps working
 * unchanged.
 */
final class IpGeoApi {

    /** Cache window — geo answers change slowly, and the free tiers rate-limit. */
    private static final long CACHE_MS = 5 * 60 * 1000L;

    private static final Map<String, String> CACHE = new HashMap<>();
    private static final Map<String, Long> CACHE_AT = new HashMap<>();

    private IpGeoApi() {
    }

    /**
     * Reverse-geolocate {@code rawIp} through the live API.
     *
     * @param rawIp  IPv4 or IPv6 literal (brackets / zone id tolerated)
     * @param route  transport used to reach the API (clearnet, tunnel, SOCKS)
     * @return the {@code OK} KV block, or {@code null} when every endpoint fails
     */
    static synchronized String lookup(String rawIp, IpLookup.Route route) {
        String ip = FreeIpApi.normalizeIpLiteral(rawIp);
        if (ip.length() == 0 || route == null || !looksLikeIp(ip)) {
            return null;
        }
        String key = ip.toLowerCase(Locale.US);
        Long at = CACHE_AT.get(key);
        if (at != null && System.currentTimeMillis() - at.longValue() <= CACHE_MS) {
            String hit = CACHE.get(key);
            if (hit != null) {
                return hit;
            }
        }
        String block = fromIpWhoIs(ip, route);
        if (block == null) {
            block = fromIpApiCom(ip, route);
        }
        if (block == null) {
            block = fromIpApiCo(ip, route);
        }
        if (block != null) {
            CACHE.put(key, block);
            CACHE_AT.put(key, Long.valueOf(System.currentTimeMillis()));
        }
        return block;
    }

    // ─── Endpoints ───────────────────────────────────────────────────────

    /** ipwho.is — HTTPS, both address families. */
    private static String fromIpWhoIs(String ip, IpLookup.Route route) {
        String body = fetch(route, "https://ipwho.is/" + FreeIpApi.urlEncodePath(ip));
        if (body == null) {
            return null;
        }
        if ("false".equalsIgnoreCase(jsonPath(body, "success"))) {
            return null;
        }
        String outIp = jsonPath(body, "ip");
        if (ipDefinitelyDiffers(ip, outIp)) {
            return null;
        }
        String country = jsonPath(body, "country");
        String code = jsonPath(body, "country_code");
        String region = jsonPath(body, "region");
        String city = jsonPath(body, "city");
        String isp = jsonPath(body, "connection.isp");
        String org = jsonPath(body, "connection.org");
        String asnNum = jsonPath(body, "connection.asn");
        String lat = jsonPath(body, "latitude");
        String lon = jsonPath(body, "longitude");
        if (country.length() == 0 && city.length() == 0 && isp.length() == 0) {
            return null;
        }
        if (isp.length() == 0) {
            isp = org;
        }
        return kv(ip, country, code, region, city, lat, lon, isp, asName(asnNum, org));
    }

    /** ip-api.com — the endpoint the desktop clients query first. */
    private static String fromIpApiCom(String ip, IpLookup.Route route) {
        String body = fetch(route, "http://ip-api.com/json/" + FreeIpApi.urlEncodePath(ip)
            + "?fields=status,message,query,country,countryCode,region,regionName,city,zip,lat,lon,timezone,isp,org,as");
        if (body == null) {
            return null;
        }
        if (!"success".equalsIgnoreCase(jsonPath(body, "status"))) {
            return null;
        }
        String outIp = jsonPath(body, "query");
        if (ipDefinitelyDiffers(ip, outIp)) {
            return null;
        }
        String country = jsonPath(body, "country");
        String code = jsonPath(body, "countryCode");
        String region = jsonPath(body, "regionName");
        if (region.length() == 0) {
            region = jsonPath(body, "region");
        }
        String city = jsonPath(body, "city");
        String isp = jsonPath(body, "isp");
        if (isp.length() == 0) {
            isp = jsonPath(body, "org");
        }
        String asn = jsonPath(body, "as");
        return kv(ip, country, code, region, city,
            jsonPath(body, "lat"), jsonPath(body, "lon"), isp, asn);
    }

    /** ipapi.co — HTTPS fallback for networks that block the other two. */
    private static String fromIpApiCo(String ip, IpLookup.Route route) {
        String body = fetch(route, "https://ipapi.co/" + FreeIpApi.urlEncodePath(ip) + "/json/");
        if (body == null) {
            return null;
        }
        if (jsonPath(body, "error").length() > 0) {
            return null;
        }
        String outIp = jsonPath(body, "ip");
        if (ipDefinitelyDiffers(ip, outIp)) {
            return null;
        }
        String country = jsonPath(body, "country");
        String code = jsonPath(body, "country_code");
        String region = jsonPath(body, "region");
        String city = jsonPath(body, "city");
        String org = jsonPath(body, "org");
        if (country.length() == 0 && city.length() == 0 && org.length() == 0) {
            return null;
        }
        return kv(ip, country, code, region, city,
            jsonPath(body, "latitude"), jsonPath(body, "longitude"), org, org);
    }

    private static String fetch(IpLookup.Route route, String url) {
        try {
            return route.fetch(url);
        } catch (Exception e) {
            return null;
        }
    }

    // ─── Helpers ─────────────────────────────────────────────────────────

    /** Build the app's standard {@code OK / key=value} block. */
    private static String kv(String ip, String country, String countryCode, String region,
                             String city, String lat, String lon, String isp, String asn) {
        return "OK\nip=" + nz(ip)
            + "\ncountry=" + nz(country)
            + "\ncountry_code=" + nz(countryCode)
            + "\nregion=" + nz(region)
            + "\nregion_name=" + nz(region)
            + "\ncity=" + nz(city)
            + "\nlat=" + nz(lat)
            + "\nlon=" + nz(lon)
            + "\nisp=" + nz(isp)
            + "\nasn=" + nz(asn);
    }

    /** "15169" + "Google LLC" → "AS15169 Google LLC". */
    private static String asName(String asnNumber, String org) {
        String n = nz(asnNumber);
        String o = nz(org);
        if (n.length() == 0) {
            return o;
        }
        if (n.startsWith("AS")) {
            return o.length() > 0 ? n + " " + o : n;
        }
        return o.length() > 0 ? "AS" + n + " " + o : "AS" + n;
    }

    /**
     * True only when the API answered about a clearly different address than
     * the one we asked for. When the two forms cannot be compared (an unusual
     * literal), the answer is trusted rather than discarded.
     */
    static boolean ipDefinitelyDiffers(String requested, String returned) {
        if (requested == null || requested.length() == 0) {
            return false;
        }
        if (returned == null || returned.length() == 0) {
            return false;
        }
        String a = requested.trim();
        String b = returned.trim();
        if (a.equalsIgnoreCase(b)) {
            return false;
        }
        if (a.indexOf(':') >= 0 && b.indexOf(':') >= 0) {
            String ea = expandIpv6(a);
            String eb = expandIpv6(b);
            if (ea != null && eb != null) {
                return !ea.equals(eb);
            }
        }
        return false;
    }

    /** Expand an IPv6 literal to eight lowercase hex groups, or null. */
    static String expandIpv6(String ip) {
        if (ip == null) {
            return null;
        }
        String s = ip.trim().toLowerCase(Locale.US);
        if (s.startsWith("[") && s.endsWith("]")) {
            s = s.substring(1, s.length() - 1);
        }
        int pct = s.indexOf('%');
        if (pct > 0) {
            s = s.substring(0, pct);
        }
        if (s.indexOf(':') < 0) {
            return null;
        }
        String head;
        String tail = "";
        int dbl = s.indexOf("::");
        if (dbl >= 0) {
            head = s.substring(0, dbl);
            tail = s.substring(dbl + 2);
        } else {
            head = s;
        }
        String[] hp = head.length() > 0 ? head.split(":", -1) : new String[0];
        String[] tp = tail.length() > 0 ? tail.split(":", -1) : new String[0];
        if (hp.length + tp.length > 8) {
            return null;
        }
        String[] groups = new String[8];
        for (int i = 0; i < 8; i++) {
            groups[i] = "0";
        }
        for (int i = 0; i < hp.length; i++) {
            groups[i] = normGroup(hp[i]);
            if (groups[i] == null) {
                return null;
            }
        }
        for (int i = 0; i < tp.length; i++) {
            groups[8 - tp.length + i] = normGroup(tp[i]);
            if (groups[8 - tp.length + i] == null) {
                return null;
            }
        }
        StringBuilder sb = new StringBuilder();
        for (int i = 0; i < 8; i++) {
            if (i > 0) {
                sb.append(':');
            }
            sb.append(groups[i]);
        }
        return sb.toString();
    }

    /** One hex group, or null when the text is not a valid group. */
    private static String normGroup(String group) {
        if (group == null || group.length() == 0 || group.length() > 4) {
            return null;
        }
        for (int i = 0; i < group.length(); i++) {
            char c = group.charAt(i);
            boolean hex = (c >= '0' && c <= '9') || (c >= 'a' && c <= 'f');
            if (!hex) {
                return null;
            }
        }
        String v = group;
        while (v.length() < 4) {
            v = "0" + v;
        }
        return v;
    }

    private static String nz(String s) {
        return s == null ? "" : s;
    }

    static boolean looksLikeIp(String ip) {
        if (ip == null || ip.length() < 3) {
            return false;
        }
        if (ip.indexOf('<') >= 0 || ip.indexOf(' ') >= 0) {
            return false;
        }
        return ip.indexOf('.') > 0 || ip.indexOf(':') > 0;
    }

    // ─── Minimal JSON reader ─────────────────────────────────────────────

    /**
     * Read a scalar by dotted path, e.g. {@code connection.isp} or
     * {@code timezone.id}. Returns {@code ""} when the path is absent.
     */
    static String jsonPath(String json, String path) {
        if (json == null || json.length() == 0 || path == null || path.length() == 0) {
            return "";
        }
        String scope = json;
        String[] parts = path.split("\\.");
        for (int i = 0; i < parts.length; i++) {
            String key = parts[i];
            int at = scope.indexOf('"' + key + '"');
            if (at < 0) {
                return "";
            }
            int colon = scope.indexOf(':', at + key.length() + 2);
            if (colon < 0) {
                return "";
            }
            int start = colon + 1;
            while (start < scope.length() && Character.isWhitespace(scope.charAt(start))) {
                start++;
            }
            if (start >= scope.length()) {
                return "";
            }
            char c = scope.charAt(start);
            boolean last = (i == parts.length - 1);
            if (c == '{' || c == '[') {
                if (last) {
                    return "";
                }
                int end = matchBlock(scope, start);
                if (end < 0) {
                    return "";
                }
                scope = scope.substring(start, end + 1);
                continue;
            }
            if (!last) {
                return "";
            }
            return readScalar(scope, start);
        }
        return "";
    }

    /** Index of the {@code }} / {@code ]} closing the block that opens at {@code start}. */
    private static int matchBlock(String s, int start) {
        char open = s.charAt(start);
        char close = open == '{' ? '}' : ']';
        int depth = 0;
        boolean inString = false;
        boolean escaped = false;
        for (int i = start; i < s.length(); i++) {
            char c = s.charAt(i);
            if (inString) {
                if (escaped) {
                    escaped = false;
                } else if (c == '\\') {
                    escaped = true;
                } else if (c == '"') {
                    inString = false;
                }
                continue;
            }
            if (c == '"') {
                inString = true;
            } else if (c == open) {
                depth++;
            } else if (c == close) {
                depth--;
                if (depth == 0) {
                    return i;
                }
            }
        }
        return -1;
    }

    /** Read a JSON string or bare number starting at {@code start}. */
    private static String readScalar(String s, int start) {
        if (start >= s.length()) {
            return "";
        }
        if (s.charAt(start) != '"') {
            int end = start;
            while (end < s.length()) {
                char c = s.charAt(end);
                if (c == ',' || c == '}' || c == ']' || Character.isWhitespace(c)) {
                    break;
                }
                end++;
            }
            return s.substring(start, end);
        }
        StringBuilder sb = new StringBuilder();
        for (int i = start + 1; i < s.length(); i++) {
            char c = s.charAt(i);
            if (c == '\\' && i + 1 < s.length()) {
                char esc = s.charAt(++i);
                if (esc == 'u' && i + 4 < s.length()) {
                    try {
                        sb.append((char) Integer.parseInt(s.substring(i + 1, i + 5), 16));
                        i += 4;
                    } catch (Exception ignored) {
                    }
                } else {
                    sb.append(esc);
                }
                continue;
            }
            if (c == '"') {
                break;
            }
            sb.append(c);
        }
        return sb.toString();
    }
}
