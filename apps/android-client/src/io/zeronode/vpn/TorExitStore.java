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

    private TorExitStore() {}

    static String allowlisted(String country) {
        return "us".equals(country) || "de".equals(country) || "nl".equals(country) ? country : "";
    }

    static synchronized String country(Context context) {
        return allowlisted(context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getString(KEY, ""));
    }

    static synchronized boolean setCountry(Context context, String country) {
        return context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit()
            .putString(KEY, allowlisted(country)).commit();
    }

    static String label(String country) {
        switch (allowlisted(country)) {
            case "us": return "USA";
            case "de": return "Germany";
            case "nl": return "Netherlands";
            default: return "Worldwide";
        }
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
