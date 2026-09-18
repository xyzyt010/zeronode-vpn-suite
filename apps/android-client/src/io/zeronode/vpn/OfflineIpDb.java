package io.zeronode.vpn;

import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.FileInputStream;
import java.io.IOException;
import java.io.InputStream;
import java.nio.charset.StandardCharsets;
import java.util.HashMap;

/**
 * Offline on-device IP lookup backed by sapics/ip-location-db MMDB files.
 *
 * <p>Dependency-free: only {@code java.io}, {@code java.lang} and
 * {@code java.util} are used — no {@code android.*} imports anywhere in this
 * file, so the core reader compiles and runs on a plain JVM
 * ({@code javac OfflineIpDb.java && java ...}). All Android asset access is
 * isolated behind {@link AssetOpener}, which the app implements with a single
 * {@code getAssets().open(...)} call (see the MainActivity patch spec in the
 * release report; {@link #openAssets} / {@link #installLazy} never throw).
 *
 * <p>Data files (fetched via {@code tools/fetch-ipdb.ps1} into
 * {@code vendor/ipdb/}, packaged as {@code assets/ipdb/*.mmdb} by the
 * {@code -OfflineIpDb} build flavor):
 * <ul>
 *   <li>dbip-city-ipv4.mmdb / dbip-city-ipv6.mmdb — DB-IP City Lite
 *       (CC BY 4.0, attribution required where results are displayed).
 *       Flat record: {city, country_code, latitude, longitude, postcode,
 *       state1, state2, timezone}; {@code state1} is surfaced as region.</li>
 *   <li>origin-asn-ipv4.mmdb / origin-asn-ipv6.mmdb — origin-asn
 *       (ODC PDDL 1.0, no attribution required).
 *       Flat record: {autonomous_system_number,
 *       autonomous_system_organization}.</li>
 * </ul>
 *
 * <p>Reader properties (mobile, 100&nbsp;MB+ tries):
 * <ul>
 *   <li>Zero-copy buffer reads directly over the caller's {@code byte[]} —
 *       no slicing, no {@code ByteBuffer} allocation per lookup.</li>
 *   <li>No per-lookup allocations beyond the returned {@link Result}
 *       (unknown record fields are skipped by offset arithmetic; known keys
 *       are matched against raw bytes).</li>
 *   <li>Thread-safe concurrent lookups: all database state is final and
 *       never mutated after construction; lookups use only locals.</li>
 *   <li>IPv4 is resolved in the 32-bit trees; IPv6 in the 128-bit trees.
 *       IPv4-mapped IPv6 ({@code ::ffff:a.b.c.d}), 6to4
 *       ({@code 2002:V4ADDR::/48}) and Teredo ({@code 2001::/32}, client
 *       address decoded per RFC 4380) are reduced to their embedded IPv4
 *       address and resolved offline.</li>
 * </ul>
 *
 * <p>JVM accuracy gate (before every DB refresh): resolve the fixed probe set
 * (public IPv4 + IPv6) with {@link #lookup} and compare country/city/ASN
 * against live FreeIPAPI ({@code https://free.freeipapi.com/api/v1/json/{ip}}).
 * Country and ASN must agree; city-level mismatches on a minority of
 * anycast IPs are acceptable (DB-IP Lite and FreeIPAPI share lineage, so
 * agreement is normally near-total).
 */
public final class OfflineIpDb {

    /** Asset paths inside the APK; must match fetch-ipdb.ps1 + build script. */
    public static final String ASSET_CITY_V4 = "ipdb/dbip-city-ipv4.mmdb";
    public static final String ASSET_CITY_V6 = "ipdb/dbip-city-ipv6.mmdb";
    public static final String ASSET_ASN_V4 = "ipdb/origin-asn-ipv4.mmdb";
    public static final String ASSET_ASN_V6 = "ipdb/origin-asn-ipv6.mmdb";

    /** Isolates Android asset access to one caller-supplied method. */
    public interface AssetOpener {
        InputStream open(String assetPath) throws IOException;
    }

    /** Lookup result. A fresh instance is allocated per successful lookup. */
    public static final class Result {
        public String countryCode = "";
        public String country = "";
        public String region = "";
        public String city = "";
        /** Organization name (parity with online {@code isp=asnOrganization}). */
        public String isp = "";
        /** Autonomous system number as decimal text ("" when unknown). */
        public String asn = "";
        public double lat = Double.NaN;
        public double lon = Double.NaN;
    }

    private final Mmdb cityV4;
    private final Mmdb cityV6;
    private final Mmdb asnV4;
    private final Mmdb asnV6;

    private OfflineIpDb(Mmdb cityV4, Mmdb cityV6, Mmdb asnV4, Mmdb asnV6) {
        this.cityV4 = cityV4;
        this.cityV6 = cityV6;
        this.asnV4 = asnV4;
        this.asnV6 = asnV6;
    }

    /** True when at least one database was loaded. */
    public boolean isAvailable() {
        return cityV4 != null || cityV6 != null || asnV4 != null || asnV6 != null;
    }

    /**
     * Resolve an IP literal. Returns null when the literal is invalid or when
     * neither the city nor the ASN database covers it (caller falls back to
     * the online provider). Never throws for corrupt data — returns null.
     */
    public Result lookup(String ipLiteral) {
        try {
            return lookupInner(ipLiteral);
        } catch (RuntimeException e) {
            return null;
        }
    }

    /**
     * Same as {@link #lookup} but rendered as the app's standard
     * {@code OK\nk=v} block (keys: ip, country, country_code, region, city,
     * lat, lon, isp, asn), parseable by the IpLookup hook. Null when offline
     * has no answer.
     */
    public String lookupKv(String ipLiteral) {
        Result r = lookup(ipLiteral);
        if (r == null) {
            return null;
        }
        String ip = normalize(ipLiteral);
        StringBuilder sb = new StringBuilder(160);
        sb.append("OK\nip=").append(ip)
            .append("\ncountry=").append(r.country)
            .append("\ncountry_code=").append(r.countryCode)
            .append("\nregion=").append(r.region)
            .append("\ncity=").append(r.city)
            .append("\nlat=").append(fmtCoord(r.lat))
            .append("\nlon=").append(fmtCoord(r.lon))
            .append("\nisp=").append(r.isp)
            .append("\nasn=").append(r.asn);
        return sb.toString();
    }

    // ─── factories ──────────────────────────────────────────────────────

    /** Open from in-memory images (null entries = family unavailable). */
    public static OfflineIpDb open(byte[] cityV4, byte[] cityV6,
                                   byte[] asnV4, byte[] asnV6) {
        return new OfflineIpDb(Mmdb.openOrNull(cityV4),
            Mmdb.openOrNull(cityV6), Mmdb.openOrNull(asnV4),
            Mmdb.openOrNull(asnV6));
    }

    /**
     * Open from streams (e.g. {@code assets.open(...)}) — the single
     * stream-loading method; null streams mean "family unavailable".
     * Streams are fully consumed and closed.
     */
    public static OfflineIpDb openStreams(InputStream cityV4, InputStream cityV6,
                                          InputStream asnV4, InputStream asnV6)
            throws IOException {
        return new OfflineIpDb(Mmdb.openOrNull(readAll(cityV4)),
            Mmdb.openOrNull(readAll(cityV6)), Mmdb.openOrNull(readAll(asnV4)),
            Mmdb.openOrNull(readAll(asnV6)));
    }

    /**
     * Open via an {@link AssetOpener}. Never throws: a missing/unreadable
     * asset only disables that family (non-DB flavors ship no assets and get
     * an unavailable instance).
     */
    public static OfflineIpDb openAssets(AssetOpener opener) {
        byte[] c4 = null;
        byte[] c6 = null;
        byte[] a4 = null;
        byte[] a6 = null;
        if (opener != null) {
            c4 = tryAsset(opener, ASSET_CITY_V4);
            c6 = tryAsset(opener, ASSET_CITY_V6);
            a4 = tryAsset(opener, ASSET_ASN_V4);
            a6 = tryAsset(opener, ASSET_ASN_V6);
        }
        return open(c4, c6, a4, a6);
    }

    /**
     * Open from a directory holding the four {@code *.mmdb} files by their
     * plain file names (used by JVM tests against {@code vendor/ipdb/}).
     */
    public static OfflineIpDb openFiles(File dir) throws IOException {
        return openStreams(streamOrNull(dir, "dbip-city-ipv4.mmdb"),
            streamOrNull(dir, "dbip-city-ipv6.mmdb"),
            streamOrNull(dir, "origin-asn-ipv4.mmdb"),
            streamOrNull(dir, "origin-asn-ipv6.mmdb"));
    }

    // ─── process-wide install (IpLookup hook adapter target) ────────────

    private static volatile OfflineIpDb sInstalled;
    private static volatile AssetOpener sLazyOpener;
    private static final Object S_LOCK = new Object();

    /** Install a ready instance (null clears). */
    public static void install(OfflineIpDb db) {
        synchronized (S_LOCK) {
            sInstalled = db;
        }
    }

    /**
     * Install a lazy opener: databases load on the first
     * {@link #lookupInstalled} call (which always runs on a worker thread),
     * never on the UI thread. Safe to call from MainActivity.onCreate.
     */
    public static void installLazy(AssetOpener opener) {
        synchronized (S_LOCK) {
            sLazyOpener = opener;
            if (opener == null) {
                sInstalled = null;
            }
        }
    }

    public static OfflineIpDb installed() {
        return sInstalled;
    }

    /**
     * Lookup through the installed instance, loading lazily on first use.
     * Returns null when nothing is installed/available/covered. Never throws.
     */
    public static Result lookupInstalled(String ipLiteral) {
        OfflineIpDb db = sInstalled;
        if (db == null) {
            synchronized (S_LOCK) {
                db = sInstalled;
                if (db == null) {
                    AssetOpener opener = sLazyOpener;
                    if (opener == null) {
                        return null;
                    }
                    db = openAssets(opener);
                    sInstalled = db;
                }
            }
        }
        if (!db.isAvailable()) {
            return null;
        }
        try {
            return db.lookup(ipLiteral);
        } catch (RuntimeException e) {
            return null;
        }
    }

    // ─── lookup core ────────────────────────────────────────────────────

    private Result lookupInner(String ipLiteral) {
        if (ipLiteral == null) {
            return null;
        }
        String s = normalize(ipLiteral);
        if (s.length() == 0) {
            return null;
        }
        byte[] v4 = null;
        byte[] v6 = null;
        if (s.indexOf(':') >= 0) {
            v6 = parseIPv6(s);
            if (v6 == null) {
                return null;
            }
            // IPv4-mapped, 6to4 and Teredo all embed a usable IPv4 address.
            byte[] embedded = embeddedIPv4(v6);
            if (embedded != null) {
                v4 = embedded;
                v6 = null;
            }
        } else {
            v4 = parseIPv4(s);
            if (v4 == null) {
                return null;
            }
        }
        Mmdb cityDb = (v4 != null) ? cityV4 : cityV6;
        Mmdb asnDb = (v4 != null) ? asnV4 : asnV6;
        byte[] addr = (v4 != null) ? v4 : v6;
        int bits = (v4 != null) ? 32 : 128;
        int wantVer = (v4 != null) ? 4 : 6;
        int cityOff = -1;
        int asnOff = -1;
        // Guard against misnamed files: only walk a tree whose ip_version
        // matches the address family (a 32-bit walk in a 128-bit tree would
        // otherwise land on a wrong record).
        if (cityDb != null && cityDb.ipVersion == wantVer) {
            cityOff = cityDb.find(addr, bits);
        }
        if (asnDb != null && asnDb.ipVersion == wantVer) {
            asnOff = asnDb.find(addr, bits);
        }
        if (cityOff < 0 && asnOff < 0) {
            return null;
        }
        Result out = new Result();
        if (cityOff >= 0) {
            cityDb.extractCity(cityOff, out);
        }
        if (asnOff >= 0) {
            asnDb.extractAsn(asnOff, out);
        }
        if (out.countryCode.length() == 2) {
            out.country = countryName(out.countryCode);
        }
        return out;
    }

    /** Strip brackets / zone id / whitespace (same contract as FreeIpApi). */
    static String normalize(String ip) {
        if (ip == null) {
            return "";
        }
        String s = ip.trim();
        if (s.length() > 2 && s.charAt(0) == '[' && s.charAt(s.length() - 1) == ']') {
            s = s.substring(1, s.length() - 1).trim();
        }
        int zone = s.indexOf('%');
        if (zone > 0) {
            s = s.substring(0, zone).trim();
        }
        return s;
    }

    private static String fmtCoord(double v) {
        return Double.isNaN(v) ? "" : Double.toString(v);
    }

    private static byte[] tryAsset(AssetOpener opener, String path) {
        InputStream in = null;
        try {
            in = opener.open(path);
            return readAll(in);
        } catch (Exception e) {
            return null;
        } finally {
            if (in != null) {
                try {
                    in.close();
                } catch (Exception ignored) {
                }
            }
        }
    }

    private static InputStream streamOrNull(File dir, String name) {
        if (dir == null) {
            return null;
        }
        File f = new File(dir, name);
        if (!f.isFile()) {
            return null;
        }
        try {
            return new FileInputStream(f);
        } catch (IOException e) {
            return null;
        }
    }

    private static byte[] readAll(InputStream in) throws IOException {
        if (in == null) {
            return null;
        }
        ByteArrayOutputStream bos = new ByteArrayOutputStream(1 << 20);
        byte[] buf = new byte[65536];
        int n;
        long total = 0;
        while ((n = in.read(buf)) >= 0) {
            total += n;
            if (total > 536870912L) {
                throw new IOException("ipdb file too large");
            }
            bos.write(buf, 0, n);
        }
        try {
            in.close();
        } catch (IOException ignored) {
        }
        byte[] out = bos.toByteArray();
        return out.length == 0 ? null : out;
    }

    // ─── IP literal parsing (plain-Java, no DNS) ────────────────────────

    static byte[] parseIPv4(String s) {
        int len = s.length();
        if (len < 7 || len > 15) {
            return null;
        }
        byte[] out = new byte[4];
        int part = 0;
        int value = -1;
        int parts = 0;
        for (int i = 0; i <= len; i++) {
            char c = i < len ? s.charAt(i) : '.';
            if (c == '.') {
                if (value < 0 || value > 255 || part == 0) {
                    return null;
                }
                if (parts >= 4) {
                    return null;
                }
                out[parts++] = (byte) value;
                value = -1;
                part = 0;
            } else if (c >= '0' && c <= '9') {
                if (value < 0) {
                    value = 0;
                }
                value = value * 10 + (c - '0');
                if (value > 255) {
                    return null;
                }
                part++;
                if (part > 3) {
                    return null;
                }
            } else {
                return null;
            }
        }
        return parts == 4 ? out : null;
    }

    static byte[] parseIPv6(String s) {
        int len = s.length();
        if (len < 2 || len > 45) {
            return null;
        }
        // Split around a single "::" compression.
        int dc = s.indexOf("::");
        if (dc >= 0 && s.indexOf("::", dc + 2) >= 0) {
            return null;
        }
        String head = dc >= 0 ? s.substring(0, dc) : s;
        String tail = dc >= 0 ? s.substring(dc + 2) : "";
        // Reject a single leading/trailing ':' that is not part of '::'.
        if (head.length() > 0 && (head.charAt(0) == ':' || head.charAt(head.length() - 1) == ':')) {
            return null;
        }
        if (tail.length() > 0 && (tail.charAt(0) == ':' || tail.charAt(tail.length() - 1) == ':')) {
            return null;
        }
        int[] groups = new int[8];
        int count = 0;
        // Parse head groups.
        if (head.length() > 0) {
            int start = 0;
            while (true) {
                int colon = head.indexOf(':', start);
                String g = colon < 0 ? head.substring(start) : head.substring(start, colon);
                int[] parsed = new int[2];
                int used = parseV6Group(g, parsed);
                if (used < 0) {
                    return null;
                }
                for (int k = 0; k < used; k++) {
                    if (count >= 8) {
                        return null;
                    }
                    groups[count++] = parsed[k];
                }
                if (colon < 0) {
                    break;
                }
                start = colon + 1;
            }
        }
        int tailCount = 0;
        int[] tailGroups = new int[8];
        if (tail.length() > 0) {
            int start = 0;
            while (true) {
                int colon = tail.indexOf(':', start);
                String g = colon < 0 ? tail.substring(start) : tail.substring(start, colon);
                int[] parsed = new int[2];
                int used = parseV6Group(g, parsed);
                if (used < 0) {
                    return null;
                }
                for (int k = 0; k < used; k++) {
                    if (tailCount >= 8) {
                        return null;
                    }
                    tailGroups[tailCount++] = parsed[k];
                }
                if (colon < 0) {
                    break;
                }
                start = colon + 1;
            }
        }
        if (dc < 0) {
            if (count != 8) {
                return null;
            }
        } else {
            if (count + tailCount > 7) {
                return null;
            }
            int zeros = 8 - count - tailCount;
            int[] full = new int[8];
            int p = 0;
            for (int i = 0; i < count; i++) {
                full[p++] = groups[i];
            }
            for (int i = 0; i < zeros; i++) {
                full[p++] = 0;
            }
            for (int i = 0; i < tailCount; i++) {
                full[p++] = tailGroups[i];
            }
            groups = full;
        }
        byte[] out = new byte[16];
        for (int i = 0; i < 8; i++) {
            out[i * 2] = (byte) ((groups[i] >> 8) & 0xFF);
            out[i * 2 + 1] = (byte) (groups[i] & 0xFF);
        }
        return out;
    }

    /**
     * Parse one hextet, or an embedded dotted-quad (uses two slots).
     * Returns slot count (1 or 2), or -1 on failure.
     */
    private static int parseV6Group(String g, int[] slots) {
        if (g.length() == 0) {
            return -1;
        }
        if (g.indexOf('.') >= 0) {
            byte[] v4 = parseIPv4(g);
            if (v4 == null) {
                return -1;
            }
            slots[0] = ((v4[0] & 0xFF) << 8) | (v4[1] & 0xFF);
            slots[1] = ((v4[2] & 0xFF) << 8) | (v4[3] & 0xFF);
            return 2;
        }
        if (g.length() > 4) {
            return -1;
        }
        int v = 0;
        for (int i = 0; i < g.length(); i++) {
            char c = g.charAt(i);
            int d;
            if (c >= '0' && c <= '9') {
                d = c - '0';
            } else if (c >= 'a' && c <= 'f') {
                d = c - 'a' + 10;
            } else if (c >= 'A' && c <= 'F') {
                d = c - 'A' + 10;
            } else {
                return -1;
            }
            v = (v << 4) | d;
        }
        slots[0] = v;
        return 1;
    }

    /**
     * Reduce special IPv6 addresses to their embedded IPv4 address:
     * IPv4-mapped ({@code ::ffff:0:0/96}), 6to4 ({@code 2002::/16}) and
     * Teredo ({@code 2001::/32}, client bits decoded per RFC 4380).
     * Returns null for genuine IPv6 addresses.
     */
    private static byte[] embeddedIPv4(byte[] v6) {
        boolean first80Zero = true;
        for (int i = 0; i < 10; i++) {
            if (v6[i] != 0) {
                first80Zero = false;
                break;
            }
        }
        if (first80Zero && (v6[10] & 0xFF) == 0xFF && (v6[11] & 0xFF) == 0xFF) {
            return new byte[]{v6[12], v6[13], v6[14], v6[15]};
        }
        // 6to4: 2002:V4ADDR::/48 — embedded address follows the prefix.
        if ((v6[0] & 0xFF) == 0x20 && (v6[1] & 0xFF) == 0x02) {
            return new byte[]{v6[2], v6[3], v6[4], v6[5]};
        }
        // Teredo: 2001:0000::/32 — client IPv4 is the last 32 bits XOR FF.
        if ((v6[0] & 0xFF) == 0x20 && (v6[1] & 0xFF) == 0x01
                && v6[2] == 0 && v6[3] == 0) {
            return new byte[]{(byte) (v6[12] ^ 0xFF), (byte) (v6[13] ^ 0xFF),
                (byte) (v6[14] ^ 0xFF), (byte) (v6[15] ^ 0xFF)};
        }
        return null;
    }

    // ─── minimal MMDB reader (MaxMind DB v2, records 24/28/32-bit) ──────

    private static final int T_PTR = 1;
    private static final int T_STR = 2;
    private static final int T_DBL = 3;
    private static final int T_BYTES = 4;
    private static final int T_U16 = 5;
    private static final int T_U32 = 6;
    private static final int T_MAP = 7;
    private static final int T_I32 = 8;
    private static final int T_U64 = 9;
    private static final int T_U128 = 10;
    private static final int T_ARR = 11;
    private static final int T_BOOL = 14;
    private static final int T_FLOAT = 15;

    private static final int MAX_DEPTH = 32;

    /**
     * Header packing: {@code (type << 40) | (sizeOrCount << 16) | headerLen}.
     * Payload sizes fit 24 bits (max ~16.8M); map/array counts likewise.
     */
    private static long readHeader(byte[] b, int off) {
        int ctrl = b[off] & 0xFF;
        int type = (ctrl >>> 5) & 7;
        int size = ctrl & 31;
        int hlen = 1;
        if (type == 0) {
            type = 7 + (b[off + 1] & 0xFF);
            hlen = 2;
        }
        if (type != T_PTR) {
            long ext = readExtSize(b, off + hlen, size);
            size = (int) (ext >>> 32);
            hlen += (int) ext;
        }
        return (((long) type) << 40) | (((long) size) << 16) | hlen;
    }

    /** Returns {@code (size << 32) | extraBytes} for a 5-bit size nibble. */
    private static long readExtSize(byte[] b, int off, int size) {
        if (size < 29) {
            return (((long) size) << 32);
        }
        if (size == 29) {
            return (((long) (29 + (b[off] & 0xFF))) << 32) | 1L;
        }
        if (size == 30) {
            int v = ((b[off] & 0xFF) << 8) | (b[off + 1] & 0xFF);
            return (((long) (285 + v)) << 32) | 2L;
        }
        int v = ((b[off] & 0xFF) << 16) | ((b[off + 1] & 0xFF) << 8) | (b[off + 2] & 0xFF);
        return (((long) (65821 + v)) << 32) | 3L;
    }

    /**
     * Pointer target relative to the section base, packed as
     * {@code (target << 32) | headerLen}.
     */
    private static long readPointer(byte[] b, int off) {
        int ctrl = b[off] & 0xFF;
        int size = ctrl & 31;
        int kind = (size >>> 3) & 3;
        int val = size & 7;
        if (kind == 0) {
            val = (val << 8) | (b[off + 1] & 0xFF);
            return (((long) val) << 32) | 2L;
        }
        if (kind == 1) {
            val = (val << 16) | ((b[off + 1] & 0xFF) << 8) | (b[off + 2] & 0xFF);
            return (((long) (val + 2048)) << 32) | 3L;
        }
        if (kind == 2) {
            val = (val << 24) | ((b[off + 1] & 0xFF) << 16)
                | ((b[off + 2] & 0xFF) << 8) | (b[off + 3] & 0xFF);
            return (((long) (val + 526336)) << 32) | 4L;
        }
        long full = (((long) (b[off + 1] & 0xFF)) << 24)
            | (((long) (b[off + 2] & 0xFF)) << 16)
            | (((long) (b[off + 3] & 0xFF)) << 8)
            | ((long) (b[off + 4] & 0xFF));
        return (full << 32) | 5L;
    }

    /** End offset of the value at {@code off} (pointers skipped by header). */
    private static int skipValue(byte[] b, int off, int depth) {
        if (depth > MAX_DEPTH) {
            throw new IllegalArgumentException("mmdb: nesting too deep");
        }
        long h = readHeader(b, off);
        int type = (int) (h >>> 40);
        int size = (int) ((h >>> 16) & 0xFFFFFFL);
        int hlen = (int) (h & 0xFFFFL);
        if (type == T_PTR) {
            return off + (int) readPointer(b, off);
        }
        int p = off + hlen;
        if (type == T_MAP) {
            for (int i = 0; i < size; i++) {
                p = skipValue(b, p, depth + 1);
                p = skipValue(b, p, depth + 1);
            }
            return p;
        }
        if (type == T_ARR) {
            for (int i = 0; i < size; i++) {
                p = skipValue(b, p, depth + 1);
            }
            return p;
        }
        if (type == T_DBL) {
            return p + 8;
        }
        if (type == T_FLOAT) {
            return p + 4;
        }
        if (type == T_BOOL) {
            return p;
        }
        return p + size;
    }

    private static void checkBounds(byte[] b, int off, int len) {
        if (off < 0 || len < 0 || off + len > b.length) {
            throw new IllegalArgumentException("mmdb: out of bounds");
        }
    }

    private static String decodeString(byte[] b, int off, int len) {
        checkBounds(b, off, len);
        return new String(b, off, len, StandardCharsets.UTF_8);
    }

    /** Raw-byte key match (ASCII keys, no allocation). */
    private static boolean keyIs(byte[] b, int off, int len, String key) {
        if (len != key.length()) {
            return false;
        }
        checkBounds(b, off, len);
        for (int i = 0; i < len; i++) {
            if ((b[off + i] & 0xFF) != (key.charAt(i) & 0xFF)) {
                return false;
            }
        }
        return true;
    }

    /**
     * Key match where the key may be inline ({@code koff/klen} valid) or a
     * data-section pointer ({@code pkey} already resolved, possibly null).
     */
    private static boolean isKey(byte[] b, int koff, int klen, String pkey,
                                 String want) {
        if (pkey != null) {
            return pkey.equals(want);
        }
        return keyIs(b, koff, klen, want);
    }

    /**
     * Resolve a key/value pointer to the UTF-8 string it targets.
     * Returns null when the target is not a string.
     */
    private static String pointedString(byte[] b, int ptrOff, int base) {
        long tp = readPointer(b, ptrOff);
        int toff = base + (int) (tp >>> 32);
        long th = readHeader(b, toff);
        if ((int) (th >>> 40) != T_STR) {
            return null;
        }
        int tsize = (int) ((th >>> 16) & 0xFFFFFFL);
        return decodeString(b, toff + (int) (th & 0xFFFFL), tsize);
    }

    /**
     * Resolve the value at {@code voff} (following one data-section pointer;
     * the spec forbids pointer-to-pointer chains). Packs
     * {@code (type << 56) | (size << 32) | payloadOff}, or {@code -1} when
     * unresolvable. No allocation.
     */
    private static long resolveValue(byte[] b, int voff, int base) {
        long vh = readHeader(b, voff);
        int vtype = (int) (vh >>> 40);
        int vsize = (int) ((vh >>> 16) & 0xFFFFFFL);
        int vpay = voff + (int) (vh & 0xFFFFL);
        if (vtype == T_PTR) {
            long tp = readPointer(b, voff);
            int toff = base + (int) (tp >>> 32);
            long th = readHeader(b, toff);
            int ttype = (int) (th >>> 40);
            if (ttype == T_PTR) {
                return -1L;
            }
            vtype = ttype;
            vsize = (int) ((th >>> 16) & 0xFFFFFFL);
            vpay = toff + (int) (th & 0xFFFFL);
        }
        return (((long) vtype) << 56) | (((long) vsize) << 32)
            | (((long) vpay) & 0xFFFFFFFFL);
    }

    private static double decodeDouble(byte[] b, int off, int type, int size) {
        checkBounds(b, off, type == T_DBL ? 8 : (type == T_FLOAT ? 4 : size));
        if (type == T_DBL && size == 8) {
            long v = 0;
            for (int i = 0; i < 8; i++) {
                v = (v << 8) | (b[off + i] & 0xFF);
            }
            return Double.longBitsToDouble(v);
        }
        if (type == T_FLOAT && size == 4) {
            int v = ((b[off] & 0xFF) << 24) | ((b[off + 1] & 0xFF) << 16)
                | ((b[off + 2] & 0xFF) << 8) | (b[off + 3] & 0xFF);
            return (double) Float.intBitsToFloat(v);
        }
        if (isUint(type)) {
            return (double) decodeUnsigned(b, off, size);
        }
        if (type == T_I32) {
            long v = 0;
            for (int i = 0; i < size; i++) {
                v = (v << 8) | (b[off + i] & 0xFF);
            }
            if (size == 4 && (v & 0x80000000L) != 0) {
                v -= 1L << 32;
            }
            return (double) v;
        }
        return Double.NaN;
    }

    private static boolean isUint(int type) {
        return type == T_U16 || type == T_U32 || type == T_U64 || type == T_U128;
    }

    private static long decodeUnsigned(byte[] b, int off, int size) {
        checkBounds(b, off, size);
        long v = 0;
        for (int i = 0; i < size; i++) {
            v = (v << 8) | (b[off + i] & 0xFF);
        }
        return v;
    }

    /** One opened MMDB image. Immutable and thread-safe. */
    private static final class Mmdb {
        final byte[] buf;
        final int nodeCount;
        final int recordSize;
        final int ipVersion;
        final int treeBytes;
        final int dataStart;

        private Mmdb(byte[] buf, int nodeCount, int recordSize,
                     int ipVersion, int treeBytes, int dataStart) {
            this.buf = buf;
            this.nodeCount = nodeCount;
            this.recordSize = recordSize;
            this.ipVersion = ipVersion;
            this.treeBytes = treeBytes;
            this.dataStart = dataStart;
        }

        static Mmdb openOrNull(byte[] buf) {
            if (buf == null || buf.length < 32) {
                return null;
            }
            try {
                return open(buf);
            } catch (RuntimeException e) {
                return null;
            }
        }

        private static int lastIndexOf(byte[] buf, byte[] marker) {
            outer:
            for (int i = buf.length - marker.length; i >= 0; i--) {
                for (int j = 0; j < marker.length; j++) {
                    if (buf[i + j] != marker[j]) {
                        continue outer;
                    }
                }
                return i;
            }
            return -1;
        }

        static Mmdb open(byte[] buf) {
            byte[] marker = new byte[]{(byte) 0xAB, (byte) 0xCD, (byte) 0xEF,
                'M', 'a', 'x', 'M', 'i', 'n', 'd', '.', 'c', 'o', 'm'};
            int at = lastIndexOf(buf, marker);
            if (at < 0) {
                throw new IllegalArgumentException("mmdb: marker missing");
            }
            int metaStart = at + marker.length;
            long h = readHeader(buf, metaStart);
            if ((int) (h >>> 40) != T_MAP) {
                throw new IllegalArgumentException("mmdb: bad metadata");
            }
            int count = (int) ((h >>> 16) & 0xFFFFFFL);
            int off = metaStart + (int) (h & 0xFFFFL);
            int nodeCount = -1;
            int recordSize = -1;
            int ipVersion = -1;
            int major = -1;
            for (int i = 0; i < count; i++) {
                long kh = readHeader(buf, off);
                int ktype = (int) (kh >>> 40);
                int klen = (int) ((kh >>> 16) & 0xFFFFFFL);
                int khlen = (int) (kh & 0xFFFFL);
                int koff = off + khlen;
                off = koff + klen;
                boolean want = ktype == T_STR && (keyIs(buf, koff, klen, "node_count")
                    || keyIs(buf, koff, klen, "record_size")
                    || keyIs(buf, koff, klen, "ip_version")
                    || keyIs(buf, koff, klen, "binary_format_major_version"));
                long vh = readHeader(buf, off);
                int vtype = (int) (vh >>> 40);
                int vsize = (int) ((vh >>> 16) & 0xFFFFFFL);
                int vhlen = (int) (vh & 0xFFFFL);
                if (want && (isUint(vtype) || vtype == T_I32)) {
                    long v = 0;
                    for (int k = 0; k < vsize; k++) {
                        v = (v << 8) | (buf[off + vhlen + k] & 0xFF);
                    }
                    if (keyIs(buf, koff, klen, "node_count")) {
                        nodeCount = (int) v;
                    } else if (keyIs(buf, koff, klen, "record_size")) {
                        recordSize = (int) v;
                    } else if (keyIs(buf, koff, klen, "ip_version")) {
                        ipVersion = (int) v;
                    } else {
                        major = (int) v;
                    }
                }
                off = skipValue(buf, off, 0);
            }
            if (major != 2) {
                throw new IllegalArgumentException("mmdb: unsupported version");
            }
            if (nodeCount <= 0 || (recordSize != 24 && recordSize != 28 && recordSize != 32)) {
                throw new IllegalArgumentException("mmdb: bad metadata values");
            }
            if (ipVersion != 4 && ipVersion != 6) {
                throw new IllegalArgumentException("mmdb: bad ip_version");
            }
            int treeBytes = ((recordSize * 2) / 8) * nodeCount;
            int dataStart = treeBytes + 16;
            if (dataStart >= buf.length || at <= dataStart) {
                throw new IllegalArgumentException("mmdb: bad section layout");
            }
            for (int i = 0; i < 16; i++) {
                if (buf[treeBytes + i] != 0) {
                    throw new IllegalArgumentException("mmdb: separator missing");
                }
            }
            return new Mmdb(buf, nodeCount, recordSize, ipVersion,
                treeBytes, dataStart);
        }

        private int readRecord(int node, int bit) {
            byte[] b = buf;
            if (recordSize == 24) {
                int b0 = node * 6 + bit * 3;
                return ((b[b0] & 0xFF) << 16) | ((b[b0 + 1] & 0xFF) << 8)
                    | (b[b0 + 2] & 0xFF);
            }
            if (recordSize == 28) {
                int b0 = node * 7;
                int mid = b[b0 + 3] & 0xFF;
                if (bit == 0) {
                    return ((mid >>> 4) << 24) | ((b[b0] & 0xFF) << 16)
                        | ((b[b0 + 1] & 0xFF) << 8) | (b[b0 + 2] & 0xFF);
                }
                return ((mid & 15) << 24) | ((b[b0 + 4] & 0xFF) << 16)
                    | ((b[b0 + 5] & 0xFF) << 8) | (b[b0 + 6] & 0xFF);
            }
            int b0 = node * 8 + bit * 4;
            return ((b[b0] & 0xFF) << 24) | ((b[b0 + 1] & 0xFF) << 16)
                | ((b[b0 + 2] & 0xFF) << 8) | (b[b0 + 3] & 0xFF);
        }

        /**
         * Walk the trie. Returns the absolute data-record offset, or -1 for
         * "not in database".
         */
        int find(byte[] addr, int bits) {
            int node = 0;
            for (int i = 0; i < bits; i++) {
                if (node < 0 || node >= nodeCount) {
                    return -1;
                }
                int bit = (addr[i >> 3] >> (7 - (i & 7))) & 1;
                int rec = readRecord(node, bit);
                // rec is an unsigned 24/28/32-bit value held in a signed int.
                long u = rec & 0xFFFFFFFFL;
                long nc = nodeCount & 0xFFFFFFFFL;
                if (u == nc) {
                    return -1;
                }
                if (u > nc) {
                    long fileOff = (u - nc) + ((long) treeBytes);
                    return fileOff > Integer.MAX_VALUE ? -1 : (int) fileOff;
                }
                node = rec;
            }
            return -1;
        }

        void extractCity(int absOff, Result out) {
            scanRecord(buf, absOff, dataStart, out, true, 0);
        }

        void extractAsn(int absOff, Result out) {
            scanRecord(buf, absOff, dataStart, out, false, 0);
        }
    }

    /**
     * Scan one record map, filling the fields this database provides.
     * Unknown fields (postcode, state2, timezone, ...) are skipped without
     * allocation, following the data-section pointer base for dedup targets.
     */
    private static void scanRecord(byte[] b, int off, int base, Result out,
                                   boolean cityMode, int depth) {
        if (depth > MAX_DEPTH) {
            return;
        }
        long h = readHeader(b, off);
        int type = (int) (h >>> 40);
        if (type == T_PTR) {
            long p = readPointer(b, off);
            scanRecord(b, base + (int) (p >>> 32), base, out, cityMode, depth + 1);
            return;
        }
        if (type != T_MAP) {
            return;
        }
        int count = (int) ((h >>> 16) & 0xFFFFFFL);
        int p = off + (int) (h & 0xFFFFL);
        for (int i = 0; i < count; i++) {
            // Keys may be inline strings or dedup pointers (sapics files
            // deduplicate both keys and values aggressively).
            int keyOff = p;
            long kh = readHeader(b, keyOff);
            int ktype = (int) (kh >>> 40);
            int klen = (int) ((kh >>> 16) & 0xFFFFFFL);
            int khlen = (int) (kh & 0xFFFFL);
            String pkey = null;
            int koff;
            if (ktype == T_PTR) {
                p = keyOff + (int) readPointer(b, keyOff);
                pkey = pointedString(b, keyOff, base);
                koff = -1;
            } else {
                koff = keyOff + khlen;
                p = koff + klen;
            }
            int voff = p;
            p = skipValue(b, voff, depth + 1);
            long rv = resolveValue(b, voff, base);
            if (rv == -1L) {
                continue;
            }
            int vtype = (int) (rv >>> 56);
            int vsize = (int) ((rv >>> 32) & 0xFFFFFFL);
            int vpay = (int) rv;
            if (ktype == T_STR || pkey != null) {
                if (cityMode) {
                    if (isKey(b, koff, klen, pkey, "country_code") && vtype == T_STR) {
                        out.countryCode = decodeString(b, vpay, vsize);
                    } else if (isKey(b, koff, klen, pkey, "city") && vtype == T_STR) {
                        out.city = decodeString(b, vpay, vsize);
                    } else if (isKey(b, koff, klen, pkey, "state1") && vtype == T_STR) {
                        out.region = decodeString(b, vpay, vsize);
                    } else if (isKey(b, koff, klen, pkey, "latitude")
                            && (vtype == T_DBL || vtype == T_FLOAT
                            || isUint(vtype) || vtype == T_I32)) {
                        out.lat = decodeDouble(b, vpay, vtype, vsize);
                    } else if (isKey(b, koff, klen, pkey, "longitude")
                            && (vtype == T_DBL || vtype == T_FLOAT
                            || isUint(vtype) || vtype == T_I32)) {
                        out.lon = decodeDouble(b, vpay, vtype, vsize);
                    } else if ((vtype == T_MAP || vtype == T_ARR)
                            && fallbackNeeded(out, true)) {
                        scanNested(b, voff, base, koff, klen, pkey,
                            out, true, depth + 1);
                    }
                } else {
                    if (isKey(b, koff, klen, pkey, "autonomous_system_number")
                            && (isUint(vtype) || vtype == T_I32)) {
                        long asn = decodeUnsigned(b, vpay, vsize);
                        if (vtype == T_I32 && vsize == 4 && (asn & 0x80000000L) != 0) {
                            asn -= 1L << 32;
                        }
                        if (asn > 0) {
                            out.asn = Long.toString(asn);
                        }
                    } else if (isKey(b, koff, klen, pkey,
                            "autonomous_system_organization") && vtype == T_STR) {
                        out.isp = decodeString(b, vpay, vsize);
                    } else if ((vtype == T_MAP || vtype == T_ARR)
                            && fallbackNeeded(out, false)) {
                        scanNested(b, voff, base, koff, klen, pkey,
                            out, false, depth + 1);
                    }
                }
            }
        }
    }

    private static boolean fallbackNeeded(Result out, boolean cityMode) {
        if (cityMode) {
            return out.countryCode.length() == 0 || out.city.length() == 0
                || out.region.length() == 0
                || Double.isNaN(out.lat) || Double.isNaN(out.lon);
        }
        return out.isp.length() == 0 && out.asn.length() == 0;
    }

    /**
     * One-level tolerant fallback for MaxMind-style nested records
     * ({@code country:{iso_code}}, {@code city:{names:{en}}},
     * {@code subdivisions:[{iso_code}]}, {@code location:{latitude}}}).
     * Never used by the pinned sapics flat-layout files; kept so a future
     * database swap degrades gracefully instead of going blank.
     */
    private static void scanNested(byte[] b, int voff, int base,
                                   int koff, int klen, String pkey, Result out,
                                   boolean cityMode, int depth) {
        if (depth > MAX_DEPTH) {
            return;
        }
        long rv = resolveValue(b, voff, base);
        if (rv == -1L) {
            return;
        }
        int vtype = (int) (rv >>> 56);
        int vcount = (int) ((rv >>> 32) & 0xFFFFFFL);
        int start = (int) rv;
        if (vtype == T_ARR) {
            // subdivisions: first element's iso_code becomes the region.
            int e = start;
            for (int i = 0; i < vcount; i++) {
                long eh = readHeader(b, e);
                int etype = (int) (eh >>> 40);
                if (i == 0 && etype == T_MAP && cityMode && out.region.length() == 0) {
                    String iso = findMapString(b, e, base, "iso_code", depth + 1);
                    if (iso != null) {
                        out.region = iso;
                    }
                }
                e = skipValue(b, e, depth + 1);
            }
            return;
        }
        if (vtype != T_MAP) {
            return;
        }
        boolean isCountry = isKey(b, koff, klen, pkey, "country");
        boolean isCity = isKey(b, koff, klen, pkey, "city");
        boolean isLocation = isKey(b, koff, klen, pkey, "location");
        boolean isNames = isKey(b, koff, klen, pkey, "names");
        int e = start;
        for (int i = 0; i < vcount; i++) {
            int fkeyOff = e;
            long kh = readHeader(b, fkeyOff);
            int ktype = (int) (kh >>> 40);
            int klen2 = (int) ((kh >>> 16) & 0xFFFFFFL);
            int khlen = (int) (kh & 0xFFFFL);
            String pkey2 = null;
            int koff2;
            if (ktype == T_PTR) {
                e = fkeyOff + (int) readPointer(b, fkeyOff);
                pkey2 = pointedString(b, fkeyOff, base);
                koff2 = -1;
            } else {
                koff2 = fkeyOff + khlen;
                e = koff2 + klen2;
            }
            int valOff = e;
            e = skipValue(b, valOff, depth + 1);
            long fr = resolveValue(b, valOff, base);
            if (fr == -1L) {
                continue;
            }
            int ftype = (int) (fr >>> 56);
            int fsize = (int) ((fr >>> 32) & 0xFFFFFFL);
            int fpay = (int) fr;
            if ((ktype == T_STR || pkey2 != null) && cityMode) {
                if (isCountry && isKey(b, koff2, klen2, pkey2, "iso_code") && ftype == T_STR
                        && out.countryCode.length() == 0) {
                    out.countryCode = decodeString(b, fpay, fsize);
                } else if ((isCity || isNames) && isKey(b, koff2, klen2, pkey2, "en")
                        && ftype == T_STR && out.city.length() == 0) {
                    out.city = decodeString(b, fpay, fsize);
                } else if (isKey(b, koff2, klen2, pkey2, "names") && ftype == T_MAP
                        && out.city.length() == 0) {
                    String en = findMapString(b, valOff, base, "en", depth + 1);
                    if (en != null) {
                        out.city = en;
                    }
                } else if (isLocation && isKey(b, koff2, klen2, pkey2, "latitude")
                        && Double.isNaN(out.lat)
                        && (ftype == T_DBL || ftype == T_FLOAT
                        || isUint(ftype) || ftype == T_I32)) {
                    out.lat = decodeDouble(b, fpay, ftype, fsize);
                } else if (isLocation && isKey(b, koff2, klen2, pkey2, "longitude")
                        && Double.isNaN(out.lon)
                        && (ftype == T_DBL || ftype == T_FLOAT
                        || isUint(ftype) || ftype == T_I32)) {
                    out.lon = decodeDouble(b, fpay, ftype, fsize);
                }
            }
        }
    }

    /** Find a direct string field inside the map at {@code mapOff}. */
    private static String findMapString(byte[] b, int mapOff, int base,
                                        String want, int depth) {
        if (depth > MAX_DEPTH) {
            return null;
        }
        long h = readHeader(b, mapOff);
        int type = (int) (h >>> 40);
        if (type == T_PTR) {
            long p = readPointer(b, mapOff);
            return findMapString(b, base + (int) (p >>> 32), base, want, depth + 1);
        }
        if (type != T_MAP) {
            return null;
        }
        int count = (int) ((h >>> 16) & 0xFFFFFFL);
        int p = mapOff + (int) (h & 0xFFFFL);
        for (int i = 0; i < count; i++) {
            int keyOff = p;
            long kh = readHeader(b, keyOff);
            int ktype = (int) (kh >>> 40);
            int klen = (int) ((kh >>> 16) & 0xFFFFFFL);
            int khlen = (int) (kh & 0xFFFFL);
            String pkey = null;
            int koff;
            if (ktype == T_PTR) {
                p = keyOff + (int) readPointer(b, keyOff);
                pkey = pointedString(b, keyOff, base);
                koff = -1;
            } else {
                koff = keyOff + khlen;
                p = koff + klen;
            }
            int valOff = p;
            p = skipValue(b, valOff, depth + 1);
            long rv = resolveValue(b, valOff, base);
            if (rv == -1L) {
                continue;
            }
            if ((ktype == T_STR || pkey != null)
                    && isKey(b, koff, klen, pkey, want)
                    && (int) (rv >>> 56) == T_STR) {
                return decodeString(b, (int) rv,
                    (int) ((rv >>> 32) & 0xFFFFFFL));
            }
        }
        return null;
    }

    // ─── ISO 3166-1 country names (code -> English short name) ──────────

    private static final HashMap<String, String> COUNTRY = buildCountryMap();

    private static String countryName(String cc) {
        String name = COUNTRY.get(cc);
        return name == null ? cc : name;
    }

    private static HashMap<String, String> buildCountryMap() {
        String[] pairs = {
            "AF|Afghanistan", "AX|Aland Islands", "AL|Albania", "DZ|Algeria",
            "AS|American Samoa", "AD|Andorra", "AO|Angola", "AI|Anguilla",
            "AQ|Antarctica", "AG|Antigua and Barbuda", "AR|Argentina",
            "AM|Armenia", "AW|Aruba", "AU|Australia", "AT|Austria",
            "AZ|Azerbaijan", "BS|Bahamas", "BH|Bahrain", "BD|Bangladesh",
            "BB|Barbados", "BY|Belarus", "BE|Belgium", "BZ|Belize",
            "BJ|Benin", "BM|Bermuda", "BT|Bhutan", "BO|Bolivia",
            "BQ|Bonaire, Sint Eustatius and Saba", "BA|Bosnia and Herzegovina",
            "BW|Botswana", "BV|Bouvet Island", "BR|Brazil",
            "IO|British Indian Ocean Territory", "BN|Brunei",
            "BG|Bulgaria", "BF|Burkina Faso", "BI|Burundi", "CV|Cabo Verde",
            "KH|Cambodia", "CM|Cameroon", "CA|Canada", "KY|Cayman Islands",
            "CF|Central African Republic", "TD|Chad", "CL|Chile", "CN|China",
            "CX|Christmas Island", "CC|Cocos Islands", "CO|Colombia",
            "KM|Comoros", "CG|Congo", "CD|Congo (Democratic Republic)",
            "CK|Cook Islands", "CR|Costa Rica", "CI|Cote d'Ivoire",
            "HR|Croatia", "CU|Cuba", "CW|Curacao", "CY|Cyprus",
            "CZ|Czechia", "DK|Denmark", "DJ|Djibouti", "DM|Dominica",
            "DO|Dominican Republic", "EC|Ecuador", "EG|Egypt",
            "SV|El Salvador", "GQ|Equatorial Guinea", "ER|Eritrea",
            "EE|Estonia", "SZ|Eswatini", "ET|Ethiopia",
            "FK|Falkland Islands", "FO|Faroe Islands", "FJ|Fiji",
            "FI|Finland", "FR|France", "GF|French Guiana",
            "PF|French Polynesia", "TF|French Southern Territories",
            "GA|Gabon", "GM|Gambia", "GE|Georgia", "DE|Germany",
            "GH|Ghana", "GI|Gibraltar", "GR|Greece", "GL|Greenland",
            "GD|Grenada", "GP|Guadeloupe", "GU|Guam", "GT|Guatemala",
            "GG|Guernsey", "GN|Guinea", "GW|Guinea-Bissau", "GY|Guyana",
            "HT|Haiti", "HM|Heard Island and McDonald Islands",
            "VA|Holy See", "HN|Honduras", "HK|Hong Kong", "HU|Hungary",
            "IS|Iceland", "IN|India", "ID|Indonesia", "IR|Iran", "IQ|Iraq",
            "IE|Ireland", "IM|Isle of Man", "IL|Israel", "IT|Italy",
            "JM|Jamaica", "JP|Japan", "JE|Jersey", "JO|Jordan",
            "KZ|Kazakhstan", "KE|Kenya", "KI|Kiribati",
            "KP|North Korea", "KR|South Korea", "KW|Kuwait",
            "KG|Kyrgyzstan", "LA|Laos", "LV|Latvia", "LB|Lebanon",
            "LS|Lesotho", "LR|Liberia", "LY|Libya", "LI|Liechtenstein",
            "LT|Lithuania", "LU|Luxembourg", "MO|Macao",
            "MG|Madagascar", "MW|Malawi", "MY|Malaysia", "MV|Maldives",
            "ML|Mali", "MT|Malta", "MH|Marshall Islands", "MQ|Martinique",
            "MR|Mauritania", "MU|Mauritius", "YT|Mayotte", "MX|Mexico",
            "FM|Micronesia", "MD|Moldova", "MC|Monaco", "MN|Mongolia",
            "ME|Montenegro", "MS|Montserrat", "MA|Morocco", "MZ|Mozambique",
            "MM|Myanmar", "NA|Namibia", "NR|Nauru", "NP|Nepal",
            "NL|Netherlands", "NC|New Caledonia", "NZ|New Zealand",
            "NI|Nicaragua", "NE|Niger", "NG|Nigeria", "NU|Niue",
            "NF|Norfolk Island", "MK|North Macedonia", "MP|Northern Mariana Islands",
            "NO|Norway", "OM|Oman", "PK|Pakistan", "PW|Palau",
            "PS|Palestine", "PA|Panama", "PG|Papua New Guinea",
            "PY|Paraguay", "PE|Peru", "PH|Philippines", "PN|Pitcairn",
            "PL|Poland", "PT|Portugal", "PR|Puerto Rico", "QA|Qatar",
            "RE|Reunion", "RO|Romania", "RU|Russia", "RW|Rwanda",
            "BL|Saint Barthelemy", "SH|Saint Helena",
            "KN|Saint Kitts and Nevis", "LC|Saint Lucia",
            "MF|Saint Martin (French part)", "PM|Saint Pierre and Miquelon",
            "VC|Saint Vincent and the Grenadines", "WS|Samoa",
            "SM|San Marino", "ST|Sao Tome and Principe", "SA|Saudi Arabia",
            "SN|Senegal", "RS|Serbia", "SC|Seychelles", "SL|Sierra Leone",
            "SG|Singapore", "SX|Sint Maarten (Dutch part)", "SK|Slovakia",
            "SI|Slovenia", "SB|Solomon Islands", "SO|Somalia",
            "ZA|South Africa", "GS|South Georgia and the South Sandwich Islands",
            "SS|South Sudan", "ES|Spain", "LK|Sri Lanka", "SD|Sudan",
            "SR|Suriname", "SJ|Svalbard and Jan Mayen", "SE|Sweden",
            "CH|Switzerland", "SY|Syria", "TW|Taiwan", "TJ|Tajikistan",
            "TZ|Tanzania", "TH|Thailand", "TL|Timor-Leste", "TG|Togo",
            "TK|Tokelau", "TO|Tonga", "TT|Trinidad and Tobago", "TN|Tunisia",
            "TR|Turkiye", "TM|Turkmenistan", "TC|Turks and Caicos Islands",
            "TV|Tuvalu", "UG|Uganda", "UA|Ukraine", "AE|United Arab Emirates",
            "GB|United Kingdom", "US|United States",
            "UM|United States Minor Outlying Islands", "UY|Uruguay",
            "UZ|Uzbekistan", "VU|Vanuatu", "VE|Venezuela", "VN|Vietnam",
            "VG|Virgin Islands (British)", "VI|Virgin Islands (U.S.)",
            "WF|Wallis and Futuna", "EH|Western Sahara", "YE|Yemen",
            "ZM|Zambia", "ZW|Zimbabwe",
        };
        HashMap<String, String> map = new HashMap<String, String>(pairs.length * 2);
        for (int i = 0; i < pairs.length; i++) {
            String kv = pairs[i];
            int bar = kv.indexOf('|');
            map.put(kv.substring(0, bar), kv.substring(bar + 1));
        }
        return map;
    }
}
