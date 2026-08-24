package io.zeronode.vpn;

import android.content.Context;
import android.content.res.AssetManager;

import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;

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

    /**
     * Extract assets if missing or incomplete. Safe to call repeatedly.
     */
    static synchronized File ensureExtracted(Context context) throws IOException {
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
        copyAsset(am, "tor/data/geoip6", new File(data, "geoip6"));
        copyAsset(am, "tor/data/torrc-defaults", new File(data, "torrc-defaults"));
        copyAsset(am, "tor/pluggable_transports/lyrebird", new File(pts, "lyrebird"));
        copyAsset(am, "tor/pluggable_transports/conjure-client", new File(pts, "conjure-client"));
        copyAsset(am, "tor/pluggable_transports/pt_config.json", new File(pts, "pt_config.json"));

        setExecutable(new File(pts, "lyrebird"));
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
