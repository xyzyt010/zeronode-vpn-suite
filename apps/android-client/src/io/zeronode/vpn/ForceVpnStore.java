package io.zeronode.vpn;

import android.content.Context;

final class ForceVpnStore {
    private static final String PREFS = "zeronode_force_vpn";
    private static final String KEY_ENABLED = "enabled";

    private ForceVpnStore() {}

    static synchronized boolean enabled(Context ctx) {
        return ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getBoolean(KEY_ENABLED, true);
    }

    static synchronized void setEnabled(Context ctx, boolean on) {
        ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putBoolean(KEY_ENABLED, on).apply();
    }
}
