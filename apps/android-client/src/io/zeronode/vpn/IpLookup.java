package io.zeronode.vpn;

import android.content.Context;
import android.content.SharedPreferences;
import java.util.HashMap;
import java.util.Locale;
import java.util.Map;

public final class IpLookup {
    public enum Source {
        API,
        OFFLINE
    }

    public static final class Details {
        public final String ip;
        public final String country;
        public final String countryCode;
        public final String region;
        public final String city;
        public final String isp;
        public final String asn;
        public final double lat;
        public final double lon;
        public final Source source;

        public Details(String ip, String country, String countryCode, String region, String city, String isp, String asn, double lat, double lon, Source source) {
            this.ip = ip == null ? "" : ip;
            this.country = country == null ? "" : country;
            String cc = countryCode == null ? "" : countryCode.trim();
            if (cc.length() == 2) {
                cc = cc.toUpperCase(Locale.US);
            }
            this.countryCode = cc;
            this.region = region == null ? "" : region;
            this.city = city == null ? "" : city;
            this.isp = isp == null ? "" : isp;
            this.asn = asn == null ? "" : asn;
            this.lat = lat;
            this.lon = lon;
            this.source = source == null ? Source.API : source;
        }
    }

    public interface OfflineProvider {
        Details lookup(String ip);
    }

    public interface Route {
        String fetch(String url) throws Exception;
    }

    private static final String PREFS = "zeronode_profiles";
    private static final String KEY_USE_OFFLINE = "ip_use_offline_db";
    private static volatile OfflineProvider offlineProvider = null;

    private IpLookup() {
    }

    public static void registerOfflineProvider(OfflineProvider provider) {
        offlineProvider = provider;
    }

    public static OfflineProvider getOfflineProvider() {
        return offlineProvider;
    }

    public static void setUseOfflineDb(Context ctx, boolean use) {
        if (ctx == null) {
            return;
        }
        try {
            SharedPreferences prefs = ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE);
            prefs.edit().putBoolean(KEY_USE_OFFLINE, use).apply();
        } catch (Exception e) {
            return;
        }
    }

    public static boolean useOfflineDb(Context ctx) {
        if (ctx == null) {
            return false;
        }
        try {
            SharedPreferences prefs = ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE);
            return prefs.getBoolean(KEY_USE_OFFLINE, false);
        } catch (Exception e) {
            return false;
        }
    }

    private static final FreeIpApi sharedApi = new FreeIpApi(
        new FreeIpApi.Fetcher() {
            @Override
            public String fetch(String url) throws Exception {
                throw new java.io.IOException("no direct transport");
            }
        },
        new FreeIpApi.Clock() {
            @Override
            public long now() {
                return System.currentTimeMillis();
            }
        }
    );

    public static Details lookupIpDetails(Context ctx, String ip, Route route) {
        String clean = FreeIpApi.normalizeIpLiteral(ip);
        boolean wantOffline = false;
        OfflineProvider provider = offlineProvider;
        try {
            wantOffline = useOfflineDb(ctx);
        } catch (Exception e) {
            wantOffline = false;
        }
        if (wantOffline && provider != null && clean.length() > 0) {
            try {
                Details cached = provider.lookup(clean);
                if (cached != null) {
                    return cached;
                }
            } catch (Exception e) {
            }
        }
        if (route == null) {
            return null;
        }
        if (clean.length() > 0 && !looksLikeIp(clean)) {
            return null;
        }
        final Route transport = route;
        FreeIpApi.Fetcher fetcher = new FreeIpApi.Fetcher() {
            @Override
            public String fetch(String url) throws Exception {
                return transport.fetch(url);
            }
        };
        String block;
        try {
            block = sharedApi.lookup(clean, "seam", fetcher);
        } catch (Exception e) {
            return null;
        }
        if (block == null || !block.startsWith("OK")) {
            return null;
        }
        Map<String, String> kv = parseKv(block);
        String outIp = kv.get("ip");
        if (outIp == null || outIp.length() == 0) {
            outIp = clean;
        }
        if (wantOffline && provider != null) {
            try {
                Details hit = provider.lookup(outIp);
                if (hit != null) {
                    return hit;
                }
            } catch (Exception e) {
            }
        }
        String country = nz(kv.get("country"));
        String countryCode = nz(kv.get("country_code"));
        if (countryCode.length() == 2) {
            countryCode = countryCode.toUpperCase(Locale.US);
        }
        String region = nz(kv.get("region"));
        if (region.length() == 0) {
            region = nz(kv.get("region_name"));
        }
        String city = nz(kv.get("city"));
        String isp = nz(kv.get("isp"));
        String asn = nz(kv.get("asn"));
        double lat = parseDouble(kv.get("lat"));
        double lon = parseDouble(kv.get("lon"));
        return new Details(outIp, country, countryCode, region, city, isp, asn, lat, lon, Source.API);
    }

    static Details detailsFromKvBlock(String block, String fallbackIp, Source source) {
        if (block == null) {
            return null;
        }
        Map<String, String> kv = parseKv(block);
        String status = kv.get("status");
        if (status != null && status.length() > 0 && !status.equals("OK")) {
            return null;
        }
        String outIp = kv.get("ip");
        if (outIp == null || outIp.length() == 0) {
            outIp = fallbackIp == null ? "" : fallbackIp;
        }
        if (outIp.length() == 0) {
            return null;
        }
        String country = nz(kv.get("country"));
        String countryCode = nz(kv.get("country_code"));
        if (countryCode.length() == 2) {
            countryCode = countryCode.toUpperCase(Locale.US);
        }
        String region = nz(kv.get("region"));
        if (region.length() == 0) {
            region = nz(kv.get("region_name"));
        }
        String city = nz(kv.get("city"));
        String isp = nz(kv.get("isp"));
        String asn = nz(kv.get("asn"));
        double lat = parseDouble(kv.get("lat"));
        double lon = parseDouble(kv.get("lon"));
        Source src = source == null ? Source.API : source;
        return new Details(outIp, country, countryCode, region, city, isp, asn, lat, lon, src);
    }

    private static Map<String, String> parseKv(String block) {
        Map<String, String> values = new HashMap<String, String>();
        if (block == null || block.length() == 0) {
            return values;
        }
        String[] lines = block.split("\n");
        if (lines.length > 0) {
            values.put("status", lines[0]);
        }
        for (int i = 1; i < lines.length; i++) {
            String line = lines[i];
            int eq = line.indexOf('=');
            if (eq > 0) {
                values.put(line.substring(0, eq), line.substring(eq + 1));
            }
        }
        return values;
    }

    private static String nz(String value) {
        return value == null ? "" : value;
    }

    private static double parseDouble(String value) {
        if (value == null || value.length() == 0) {
            return Double.NaN;
        }
        try {
            return Double.parseDouble(value.trim());
        } catch (Exception e) {
            return Double.NaN;
        }
    }

    private static boolean looksLikeIp(String ip) {
        if (ip == null || ip.length() < 3) {
            return false;
        }
        if (ip.indexOf('<') >= 0 || ip.indexOf(' ') >= 0) {
            return false;
        }
        return ip.indexOf('.') > 0 || ip.indexOf(':') > 0;
    }
}
