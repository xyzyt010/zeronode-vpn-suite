package io.zeronode.vpn;

import android.content.Context;
import android.content.SharedPreferences;

import org.json.JSONArray;

import java.util.ArrayList;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Set;

/**
 * Per-app VPN selection. No app database — only the current exclusive package
 * list plus an all-apps flag. Turning Protect-all off from the main toggle
 * clears every app (None protected). It does not restore a previous Some set.
 */
final class AppSplitStore {
    private static final String PREFS = "zeronode_app_split_v2";
    private static final String KEY_ALL = "all_apps";
    private static final String KEY_PKGS = "packages_json";

    static final int MODE_ALL = 0;
    static final int MODE_SOME = 1;
    static final int MODE_NONE = 2;

    private AppSplitStore() {}

    static boolean allApps(Context ctx) {
        return prefs(ctx).getBoolean(KEY_ALL, true);
    }

    /**
     * Main / Protect-all switch.
     * On  → every app uses the VPN.
     * Off → every app toggle is cleared (None protected). Not a restore.
     */
    static void setProtectAll(Context ctx, boolean on) {
        SharedPreferences.Editor ed = prefs(ctx).edit().putBoolean(KEY_ALL, on);
        if (!on) ed.putString(KEY_PKGS, "[]");
        ed.apply();
    }

    static int mode(Context ctx) {
        if (allApps(ctx)) return MODE_ALL;
        return selectedPackages(ctx).isEmpty() ? MODE_NONE : MODE_SOME;
    }

    static String summaryLabel(Context ctx) {
        switch (mode(ctx)) {
            case MODE_ALL: return "All apps protected";
            case MODE_SOME: return "Some protected";
            default: return "None protected";
        }
    }

    static List<String> selectedPackages(Context ctx) {
        String raw = prefs(ctx).getString(KEY_PKGS, "[]");
        List<String> out = new ArrayList<String>();
        try {
            JSONArray arr = new JSONArray(raw);
            Set<String> seen = new LinkedHashSet<String>();
            for (int i = 0; i < arr.length(); i++) {
                String pkg = arr.optString(i, "").trim();
                if (pkg.length() == 0 || !seen.add(pkg)) continue;
                out.add(pkg);
            }
        } catch (Exception ignored) {
        }
        return out;
    }

    static boolean isProtected(Context ctx, String pkg) {
        if (pkg == null || pkg.length() == 0) return false;
        if (allApps(ctx)) return true;
        return selectedPackages(ctx).contains(pkg);
    }

    /**
     * Flip one app. If Protect-all was on and the user turns one off, the
     * mode becomes Some and every other listed app stays on.
     */
    static void setAppProtected(Context ctx, String pkg, boolean on, List<String> allPackages) {
        if (pkg == null || pkg.length() == 0) return;
        Set<String> set;
        if (allApps(ctx)) {
            if (on) return;
            set = new LinkedHashSet<String>();
            if (allPackages != null) {
                for (int i = 0; i < allPackages.size(); i++) {
                    String p = allPackages.get(i);
                    if (p != null && p.length() > 0 && !p.equals(pkg)) set.add(p);
                }
            }
            persist(ctx, false, set);
            return;
        }
        set = new LinkedHashSet<String>(selectedPackages(ctx));
        if (on) set.add(pkg);
        else set.remove(pkg);
        persist(ctx, false, set);
    }

    private static void persist(Context ctx, boolean all, Set<String> pkgs) {
        JSONArray arr = new JSONArray();
        if (pkgs != null) {
            for (String p : pkgs) arr.put(p);
        }
        prefs(ctx).edit()
            .putBoolean(KEY_ALL, all)
            .putString(KEY_PKGS, arr.toString())
            .apply();
    }

    private static SharedPreferences prefs(Context ctx) {
        return ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE);
    }
}
