package io.zeronode.vpn;

import android.content.Context;
import android.content.SharedPreferences;

import java.io.BufferedReader;
import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.InputStream;
import java.io.InputStreamReader;
import java.io.OutputStream;
import java.net.HttpURLConnection;
import java.net.URL;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;

/**
 * Compact offline IPv4 geolocation DB.
 *
 * <p>Format is one sorted IPv4 prefix per line, for example:
 * {@code 1.0.0.0/24,AU,AUSTRALIA,NSW,Sydney,Google,AS15169,34.93,151.21}
 * Only country and city are used on-device; full ISP/ASN/lat/lon are kept
 * when present so search results stay identical to the user. When the DB is
 * enabled, IpLookup consults this file first. Background work is limited to
 * a short download/update; there is no periodic database scan or mmap, so
 * idle/background cost is effectively zero.</p>
 */
final class OfflineIpDb {
    private static final String PREFS = "offline_ip_db";
    private static final String KEY_ENABLED = "enabled";
    private static final String KEY_VERSION = "version";
    private static final String KEY_UPDATED_AT = "updated_at";
    private static final String FILE_NAME = "ipv4-country.csv";

    // Maintained in this repository. The file may be replaced by a newer
    // release-asset version while keeping the same CSV contract; callers
    // use the default URL when no local file exists yet.
    private static final String DEFAULT_URL =
        "https://raw.githubusercontent.com/xyzyt010/zeronode-vpn-suite/main/apps/android-client/assets/ipdb/ipv4-country.csv";

    private static final class Entry {
        final int network;
        final int mask;
        final String country;
        final String code;
        final String region;
        final String city;
        final String isp;
        final String asn;
        final double lat;
        final double lon;

        Entry(int network, int mask, String country, String code, String region,
              String city, String isp, String asn, double lat, double lon) {
            this.network = network;
            this.mask = mask;
            this.country = country;
            this.code = code;
            this.region = region;
            this.city = city;
            this.isp = isp;
            this.asn = asn;
            this.lat = lat;
            this.lon = lon;
        }
    }

    private static volatile Entry[] loaded = new Entry[0];
    private static volatile long loadedAt = 0L;

    static boolean enabled(Context context) {
        return prefs(context).getBoolean(KEY_ENABLED, false);
    }

    static void setEnabled(Context context, boolean enabled) {
        prefs(context).edit().putBoolean(KEY_ENABLED, enabled).apply();
    }

    static String version(Context context) {
        return prefs(context).getString(KEY_VERSION, "");
    }

    static long updatedAt(Context context) {
        return prefs(context).getLong(KEY_UPDATED_AT, 0L);
    }

    static File file(Context context) {
        return new File(context.getFilesDir(), "ipdb/" + FILE_NAME);
    }

    static boolean hasDatabase(Context context) {
        File f = file(context);
        return f.isFile() && f.length() > 0;
    }

    static long databaseSize(Context context) {
        File f = file(context);
        return f.isFile() ? f.length() : 0L;
    }

    static String databaseSizeText(Context context) {
        long size = databaseSize(context);
        if (size <= 0L) return "Not downloaded";
        if (size < 1024L * 1024L) return (size / 1024L) + " KB";
        return String.format(java.util.Locale.US, "%.1f MB", size / (1024.0 * 1024.0));
    }

    /** One-shot background-friendly download/update. Returns true on success. */
    static boolean downloadOrUpdate(final Context context, final String url) {
        final String targetUrl = url != null && url.trim().length() > 0 ? url.trim() : DEFAULT_URL;
        HttpURLConnection conn = null;
        InputStream in = null;
        OutputStream out = null;
        try {
            File dest = file(context);
            if (dest.getParentFile() != null && !dest.getParentFile().exists()) {
                dest.getParentFile().mkdirs();
            }
            File tmp = new File(dest.getParent() + "/", dest.getName() + ".tmp");
            URL remote = new URL(targetUrl);
            conn = (HttpURLConnection) remote.openConnection();
            conn.setConnectTimeout(15000);
            conn.setReadTimeout(30000);
            conn.setInstanceFollowRedirects(true);
            conn.setRequestProperty("Accept", "text/csv");
            int code = conn.getResponseCode();
            if (code != 200) return false;
            in = conn.getInputStream();
            out = new FileOutputStream(tmp);
            byte[] buffer = new byte[8192];
            int read;
            while ((read = in.read(buffer)) != -1) {
                out.write(buffer, 0, read);
            }
            out.flush();
            out.close();
            in.close();
            out = null;
            in = null;
            if (tmp.length() <= 0 || !tmp.renameTo(dest)) return false;
            // Invalidate the in-memory table. Updated-at and version live in
            // prefs so the UI can show what is installed without scanning disk.
            prefs(context).edit()
                .putLong(KEY_UPDATED_AT, System.currentTimeMillis())
                .putString(KEY_VERSION, Long.toString(System.currentTimeMillis()))
                .apply();
            loadedAt = 0L;
            return true;
        } catch (Exception ignored) {
            return false;
        } finally {
            try { if (out != null) out.close(); } catch (Exception ignored) {}
            try { if (in != null) in.close(); } catch (Exception ignored) {}
            if (conn != null) conn.disconnect();
        }
    }

    static boolean deleteDatabase(Context context) {
        File f = file(context);
        boolean ok = !f.exists() || f.delete();
        prefs(context).edit().remove(KEY_VERSION).remove(KEY_UPDATED_AT).apply();
        loadedAt = 0L;
        return ok;
    }

    static IpLookup.Details lookup(Context context, String ip) {
        Entry hit = lookupEntry(context, ip);
        if (hit == null) return null;
        return new IpLookup.Details(
            ip, hit.country, hit.code, hit.region, hit.city,
            hit.isp, hit.asn, hit.lat, hit.lon, IpLookup.Source.API
        );
    }

    private static Entry lookupEntry(Context context, String ip) {
        int value;
        try {
            value = parseIpv4(ip);
        } catch (Exception ignored) {
            return null;
        }
        if (value == 0 && ip != null && !ip.trim().equals("0.0.0.0")) return null;
        Entry[] entries = load(context);
        if (entries.length == 0) return null;
        int lo = 0;
        int hi = entries.length - 1;
        Entry best = null;
        while (lo <= hi) {
            int mid = (lo + hi) >>> 1;
            Entry entry = entries[mid];
            int masked = value & entry.mask;
            if (masked == entry.network) {
                if (best == null || Integer.bitCount(entry.mask) > Integer.bitCount(best.mask)) {
                    best = entry;
                }
                lo = mid + 1;
            } else if ((value & 0xFFFFFFFFL) < (entry.network & 0xFFFFFFFFL)) {
                hi = mid - 1;
            } else {
                lo = mid + 1;
            }
        }
        return best;
    }

    private static Entry[] load(Context context) {
        Entry[] current = loaded;
        long at = loadedAt;
        if (current.length > 0 && at > 0) return current;
        File f = file(context);
        if (!f.isFile() || f.length() <= 0) return current;
        ArrayList<Entry> rows = new ArrayList<>();
        try (BufferedReader reader = new BufferedReader(
                new InputStreamReader(new FileInputStream(f), StandardCharsets.UTF_8))) {
            String line;
            while ((line = reader.readLine()) != null) {
                Entry entry = parseLine(line);
                if (entry != null) rows.add(entry);
            }
        } catch (Exception ignored) {
            return current;
        }
        rows.sort(new java.util.Comparator<Entry>() {
            @Override public int compare(Entry a, Entry b) {
                long av = a.network & 0xFFFFFFFFL;
                long bv = b.network & 0xFFFFFFFFL;
                if (av < bv) return -1;
                if (av > bv) return 1;
                return 0;
            }
        });
        Entry[] snapshot = rows.toArray(new Entry[0]);
        loaded = snapshot;
        loadedAt = System.currentTimeMillis();
        return snapshot;
    }

    private static Entry parseLine(String line) {
        if (line == null) return null;
        String s = line.trim();
        if (s.length() == 0 || s.startsWith("#")) return null;
        String[] parts = s.split(",", -1);
        if (parts.length < 3) return null;
        String prefix = parts[0].trim();
        if (prefix.length() == 0) return null;
        int slash = prefix.indexOf('/');
        if (slash < 0) return null;
        int bits;
        try {
            bits = Integer.parseInt(prefix.substring(slash + 1).trim());
            if (bits < 0 || bits > 32) return null;
        } catch (Exception ignored) {
            return null;
        }
        int network;
        try {
            network = parseIpv4(prefix.substring(0, slash).trim());
            if (network == 0 && !prefix.startsWith("0.0.0.0/")) return null;
        } catch (Exception ignored) {
            return null;
        }
        int mask = bits == 0 ? 0 : (0xFFFFFFFF << (32 - bits));
        network &= mask;
        String country = parts.length > 1 ? parts[1].trim() : "";
        String code = parts.length > 2 ? parts[2].trim() : "";
        String region = parts.length > 3 ? parts[3].trim() : "";
        String city = parts.length > 4 ? parts[4].trim() : "";
        String isp = parts.length > 5 ? parts[5].trim() : "";
        String asn = parts.length > 6 ? parts[6].trim() : "";
        double lat = Double.NaN;
        double lon = Double.NaN;
        if (parts.length > 7) {
            try { lat = Double.parseDouble(parts[7].trim()); } catch (Exception ignored) {}
        }
        if (parts.length > 8) {
            try { lon = Double.parseDouble(parts[8].trim()); } catch (Exception ignored) {}
        }
        return new Entry(network, mask, country, code, region, city, isp, asn, lat, lon);
    }

    private static int parseIpv4(String ip) {
        if (ip == null) throw new IllegalArgumentException("null ip");
        String[] parts = ip.trim().split("\\.", -1);
        if (parts.length != 4) throw new IllegalArgumentException("bad ip");
        int value = 0;
        for (String part : parts) {
            int octet = Integer.parseInt(part);
            if (octet < 0 || octet > 255) throw new IllegalArgumentException("bad octet");
            value = (value << 8) | octet;
        }
        return value;
    }

    private static SharedPreferences prefs(Context context) {
        return context.getSharedPreferences(PREFS, Context.MODE_PRIVATE);
    }
}
