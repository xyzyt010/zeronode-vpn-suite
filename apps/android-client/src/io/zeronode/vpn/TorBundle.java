package io.zeronode.vpn;

import android.content.Context;
import android.content.res.AssetManager;
import android.os.Build;
import android.system.ErrnoException;
import android.system.Os;

import org.json.JSONObject;

import java.io.BufferedReader;
import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.InputStreamReader;
import java.io.OutputStream;
import java.nio.charset.StandardCharsets;

/**
 * Extracts the Tor expert bundle from APK assets into the app files dir and
 * prepares pluggable-transport binaries for execution.
 *
 * Expected assets:
 *   assets/tor/data/geoip
 *   assets/tor/data/geoip6
 *   assets/tor/data/torrc-defaults
 *   assets/tor/pluggable_transports/lyrebird
 *   assets/tor/pluggable_transports/conjure-client
 *   assets/tor/pluggable_transports/pt_config.json
 *
 * libTor.so is shipped as jniLibs/arm64-v8a/libTor.so and resolved via
 * {@link android.content.pm.ApplicationInfo#nativeLibraryDir}.
 */
final class TorBundle {
    private TorBundle() {}

    // Mirror of the on-demand Tor bundle URL. Change this single constant to
    // point at a small static file host serving a zip of libTor.so + geoip +
    // lyrebird + pt_config.json. The base APK ships WITHOUT libTor.so /
    // geoip (removed from build-android-vpnservice.ps1) so the resident set
    // for WireGuard/Outline stays <150 MB. On first Tor tap we
    // download-once into filesDir/tor/; subsequent taps reuse the cache.
    // Play Feature Delivery can replace downloadTorBundleUrl() 1:1.
    private static String downloadTorBundleUrl() {
        return "https://github.com/xyzyt010/zeronode-vpn-suite/releases/download/tor-bundle-arm64-v1/tor-bundle-arm64.zip";
    }

    private static void downloadTorBundleIfNeeded(Context ctx) throws IOException {
        File root = home(ctx);
        File data = new File(root, "data");
        File dlMarker = new File(root, ".tor-downloaded");
        File libTor = new File(nativeLibDir(ctx), "libTor.so");
        File localLib = new File(root, "libTor.so");
        if (dlMarker.exists()) return;
        // Fast path: previously bundled build had libTor in jniLibs.
        if (libTor.isFile()) {
            if (data.exists() && new File(data, "geoip").isFile()) {
                try { new File(dlMarker.getAbsolutePath()).createNewFile(); } catch (Exception ignored) {}
                return;
            }
        }
        if (localLib.isFile() && new File(data, "geoip").isFile()) {
            try { new File(dlMarker.getAbsolutePath()).createNewFile(); } catch (Exception ignored) {}
            return;
        }
        // On-demand: fetch tor-bundle zip once (first Tor Connect only).
        // Base APK (0.4.23+) ships WITHOUT libTor.so/geoip so WireGuard/
        // Outline resident set stays <150 MB; Tor users pay one download.
        String urlStr = downloadTorBundleUrl();
        if (urlStr == null || urlStr.isEmpty()) {
            throw new IOException("Tor bundle not bundled and no URL configured");
        }
        if (!root.exists()) root.mkdirs();
        if (!data.exists()) data.mkdirs();
        java.net.URL url = new java.net.URL(urlStr);
        java.net.HttpURLConnection conn = (java.net.HttpURLConnection) url.openConnection();
        conn.setConnectTimeout(15000);
        conn.setReadTimeout(30000);
        conn.setInstanceFollowRedirects(true);
        conn.connect();
        if (conn.getResponseCode() != 200) {
            throw new IOException("Tor bundle download failed: HTTP " + conn.getResponseCode());
        }
        File zipFile = new File(root, "tor-bundle.zip");
        try (InputStream in = conn.getInputStream();
             OutputStream out = new FileOutputStream(zipFile)) {
            byte[] buf = new byte[8192];
            int n;
            while ((n = in.read(buf)) > 0) out.write(buf, 0, n);
            out.flush();
        } finally { conn.disconnect(); }
        // Unzip: libTor.so -> root/, geoip -> data/
        try (java.util.zip.ZipInputStream zis =
                 new java.util.zip.ZipInputStream(new FileInputStream(zipFile))) {
            java.util.zip.ZipEntry entry;
            while ((entry = zis.getNextEntry()) != null) {
                String name = entry.getName().replace('\\', '/');
                String tail = name.contains("/")
                    ? name.substring(name.lastIndexOf('/') + 1) : name;
                File dest = tail.equals("geoip") ? new File(data, "geoip")
                    : new File(root, tail);
                if (!dest.getParentFile().exists()) dest.getParentFile().mkdirs();
                try (OutputStream out = new FileOutputStream(dest)) {
                    byte[] buf = new byte[8192];
                    int n;
                    while ((n = zis.read(buf)) > 0) out.write(buf, 0, n);
                }
            }
        } finally {
            zipFile.delete();
            try { new File(dlMarker.getAbsolutePath()).createNewFile(); } catch (Exception ignored) {}
        }
        setExecutable(localLib);
    }

    static File home(Context context) {
        return new File(context.getFilesDir(), "tor");
    }

    static String nativeLibDir(Context context) {
        return context.getApplicationInfo().nativeLibraryDir;
    }

    static boolean isArm64Device() {
        for (String abi : android.os.Build.SUPPORTED_ABIS) {
            if ("arm64-v8a".equals(abi)) return true;
        }
        return false;
    }

    static boolean libTorPresent(Context context) {
        File f = new File(nativeLibDir(context), "libTor.so");
        return f.isFile();
    }

    static JSONObject transportConfig(Context context) throws IOException {
        try (BufferedReader reader = new BufferedReader(new InputStreamReader(
            context.getAssets().open("tor/pluggable_transports/pt_config.json"), StandardCharsets.UTF_8))) {
            StringBuilder text = new StringBuilder();
            String line;
            while ((line = reader.readLine()) != null) text.append(line);
            return new JSONObject(text.toString());
        } catch (Exception e) {
            throw new IOException("Cannot read bundled transport configuration", e);
        }
    }

    static void requireBridgeTransport(Context context, File root) throws IOException {
        File transport = new File(root, "pluggable_transports/lyrebird");
        File packaged = new File(nativeLibDir(context), "liblyrebird.so");
        if (Build.VERSION.SDK_INT >= 29 && context.getApplicationInfo().targetSdkVersion >= 29) {
            if (!packaged.isFile() || !packaged.canExecute()
                || !transport.getCanonicalFile().equals(packaged.getCanonicalFile())) {
                throw new IOException("Bridge transport unavailable: package Lyrebird as jniLibs/arm64-v8a/liblyrebird.so");
            }
        } else if (!transport.isFile() || !transport.canExecute()) {
            throw new IOException("Bundled Lyrebird transport is missing or not executable");
        }
    }

    private static void prepareLyrebird(Context context, File pts) throws IOException {
        File packaged = new File(nativeLibDir(context), "liblyrebird.so");
        File dest = new File(pts, "lyrebird");
        if (packaged.isFile()) {
            if (dest.getCanonicalFile().equals(packaged.getCanonicalFile())) return;
            try {
                try {
                    Os.remove(dest.getAbsolutePath());
                } catch (ErrnoException e) {
                    if (e.errno != android.system.OsConstants.ENOENT) throw e;
                }
                Os.symlink(packaged.getAbsolutePath(), dest.getAbsolutePath());
            } catch (ErrnoException e) {
                throw new IOException("Cannot prepare packaged Lyrebird", e);
            }
        } else if (Build.VERSION.SDK_INT < 29 || context.getApplicationInfo().targetSdkVersion < 29) {
            copyAsset(context.getAssets(), "tor/pluggable_transports/lyrebird", dest);
            setExecutable(dest);
        }
    }

    /**
     * Extract assets if missing or incomplete. Safe to call repeatedly.
     * On-demand: triggers downloadTorBundleIfNeeded first (first Tor tap
     * only) so the base APK ships without libTor.so/geoip (see build
     * script). Bundled assets remain as fallback for the geoip that the
     * zip may not contain in a pre-release stage.
     */
    static synchronized File ensureExtracted(Context context) throws IOException {
        downloadTorBundleIfNeeded(context);
        File root = home(context);
        File data = new File(root, "data");
        File pts = new File(root, "pluggable_transports");
        if (!data.exists() && !data.mkdirs()) {
            throw new IOException("cannot create " + data);
        }
        if (!pts.exists() && !pts.mkdirs()) {
            throw new IOException("cannot create " + pts);
        }

        AssetManager am = context.getAssets();
        copyAsset(am, "tor/data/geoip", new File(data, "geoip"));
        // geoip6 is intentionally NOT extracted: Tor runs IPv4-only on
        // Android (matches ForceVpn/ForceVpnStore IPv6 block). Shipping it
        // cost 15 MB disk + mmap pages for a file Tor never needed.
        if (new File(data, "geoip6").exists()) {
            try { new File(data, "geoip6").delete(); } catch (Exception ignored) {}
        }
        copyAsset(am, "tor/data/torrc-defaults", new File(data, "torrc-defaults"));
        prepareLyrebird(context, pts);
        copyAsset(am, "tor/pluggable_transports/conjure-client", new File(pts, "conjure-client"));
        copyAsset(am, "tor/pluggable_transports/pt_config.json", new File(pts, "pt_config.json"));

        setExecutable(new File(pts, "conjure-client"));
        return root;
    }

    private static void copyAsset(AssetManager am, String assetPath, File dest) throws IOException {
        if (dest.exists() && dest.length() > 0) {
            return;
        }
        File parent = dest.getParentFile();
        if (parent != null && !parent.exists()) {
            //noinspection ResultOfMethodCallIgnored
            parent.mkdirs();
        }
        InputStream in = null;
        OutputStream out = null;
        try {
            in = am.open(assetPath);
            out = new FileOutputStream(dest);
            byte[] buf = new byte[8192];
            int n;
            while ((n = in.read(buf)) > 0) {
                out.write(buf, 0, n);
            }
            out.flush();
        } catch (IOException e) {
            // Optional assets (e.g. conjure) may be missing — ignore soft failures for optional names.
            if (assetPath.endsWith("conjure-client") || assetPath.endsWith("README.CONJURE.md")) {
                return;
            }
            throw new IOException("failed to extract " + assetPath + ": " + e.getMessage(), e);
        } finally {
            if (in != null) try { in.close(); } catch (IOException ignored) {}
            if (out != null) try { out.close(); } catch (IOException ignored) {}
        }
    }

    private static void setExecutable(File f) {
        if (f.exists()) {
            //noinspection ResultOfMethodCallIgnored
            f.setExecutable(true, false);
            //noinspection ResultOfMethodCallIgnored
            f.setReadable(true, false);
        }
    }
}
