package io.zeronode.vpn;

import android.content.Context;
import android.util.AtomicFile;

import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.nio.charset.StandardCharsets;

final class TorExitStore {
    private static final String PREFS = "zeronode_tor_exit";
    private static final String KEY = "country";

    /**
     * Exit countries offered in the UI. {@code ""} means any exit in the
     * world; the rest are ISO 3166-1 alpha-2 codes understood by Tor's
     * {@code ExitNodes} option.
     */
    private static final String[] COUNTRIES = {
        "", "us", "de", "nl", "fr", "gb", "se", "ch", "es", "ir", "cn", "kr"
    };

    private static final String[] LABELS = {
        "Worldwide", "USA", "Germany", "Netherlands", "France", "United Kingdom",
        "Sweden", "Switzerland", "Spain", "Iran", "China", "South Korea"
    };

    private TorExitStore() {}

    /** Offered exit countries, in menu order. */
    static String[] countries() {
        return COUNTRIES.clone();
    }

    static String allowlisted(String country) {
        if (country == null) return "";
        for (String code : COUNTRIES) {
            if (code.equals(country)) return code;
        }
        return "";
    }

    static synchronized String country(Context context) {
        return allowlisted(context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getString(KEY, ""));
    }

    static synchronized boolean setCountry(Context context, String country) {
        return context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit()
            .putString(KEY, allowlisted(country)).commit();
    }

    static String label(String country) {
        String code = allowlisted(country);
        for (int i = 0; i < COUNTRIES.length; i++) {
            if (COUNTRIES[i].equals(code)) return LABELS[i];
        }
        return "Worldwide";
    }

    /**
     * Tor needs at least one exit relay carrying the Exit flag in the chosen
     * country. Iran and China have none in practice, so restricting to them
     * stalls circuit building — the UI warns instead of failing silently.
     */
    static boolean hasPublicRelays(String country) {
        String code = allowlisted(country);
        return !"ir".equals(code) && !"cn".equals(code);
    }

    static synchronized void writeTorrcExtra(Context context, File torHome) throws IOException {
        if (torHome == null) throw new IOException("Tor home is missing");
        if (!torHome.isDirectory() && !torHome.mkdirs()) throw new IOException("Cannot create Tor home");
        String country = country(context);
        String body = country.isEmpty() ? "" : "ExitNodes {" + country + "}\nStrictNodes 1\n";
        AtomicFile extra = new AtomicFile(new File(torHome, "user-exit.conf"));
        FileOutputStream out = null;
        try {
            out = extra.startWrite();
            out.write(body.getBytes(StandardCharsets.UTF_8));
            extra.finishWrite(out);
        } catch (IOException e) {
            extra.failWrite(out);
            throw new IOException("Cannot save Tor exit country configuration", e);
        }
    }
}
