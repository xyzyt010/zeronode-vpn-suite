package io.zeronode.vpn;

import android.content.Context;
import android.content.SharedPreferences;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;

import java.io.File;

/**
 * Launcher identity: default ZeroNode, Weather, Garden, or a user custom
 * name + image. Presets swap activity-aliases. Custom is a pinned shortcut
 * plus stored bitmap (Android cannot rewrite APK launcher resources).
 */
final class AppearanceStore {
    static final String PRESET_DEFAULT = "default";
    static final String PRESET_WEATHER = "weather";
    static final String PRESET_GARDEN = "garden";
    static final String PRESET_CUSTOM = "custom";

    private static final String PREFS = "zeronode_appearance";
    private static final String KEY_PRESET = "preset";
    private static final String KEY_CUSTOM_NAME = "custom_name";

    private AppearanceStore() {}

    static SharedPreferences prefs(Context ctx) {
        return ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE);
    }

    static String preset(Context ctx) {
        String p = prefs(ctx).getString(KEY_PRESET, PRESET_DEFAULT);
        if (p == null || p.length() == 0) return PRESET_DEFAULT;
        return p;
    }

    static String customName(Context ctx) {
        return prefs(ctx).getString(KEY_CUSTOM_NAME, "");
    }

    static File customIconFile(Context ctx) {
        return new File(ctx.getFilesDir(), "appearance/custom_icon.png");
    }

    static boolean hasCustom(Context ctx) {
        String n = customName(ctx);
        return n != null && n.trim().length() > 0 && customIconFile(ctx).isFile();
    }

    static Bitmap customIcon(Context ctx) {
        File f = customIconFile(ctx);
        if (!f.isFile()) return null;
        return BitmapFactory.decodeFile(f.getAbsolutePath());
    }

    static String displayName(Context ctx) {
        String p = preset(ctx);
        if (PRESET_WEATHER.equals(p)) return ctx.getString(R.string.alias_weather);
        if (PRESET_GARDEN.equals(p)) return ctx.getString(R.string.alias_garden);
        if (PRESET_CUSTOM.equals(p) && hasCustom(ctx)) {
            String n = customName(ctx).trim();
            if (n.length() > 0) return n;
        }
        return ctx.getString(R.string.app_name);
    }

    static void setPreset(Context ctx, String preset) {
        prefs(ctx).edit().putString(KEY_PRESET, preset).apply();
        IconChanger.apply(ctx, preset);
    }

    static void saveCustom(Context ctx, String name, Bitmap icon) throws java.io.IOException {
        if (name == null) name = "";
        name = name.trim();
        if (name.length() == 0) name = "App";
        File dest = customIconFile(ctx);
        File parent = dest.getParentFile();
        if (parent != null && !parent.exists()) parent.mkdirs();
        java.io.FileOutputStream out = new java.io.FileOutputStream(dest);
        try {
            icon.compress(Bitmap.CompressFormat.PNG, 100, out);
            out.flush();
        } finally {
            out.close();
        }
        prefs(ctx).edit()
            .putString(KEY_CUSTOM_NAME, name)
            .putString(KEY_PRESET, PRESET_CUSTOM)
            .apply();
        IconChanger.applyCustom(ctx, name, icon);
    }

    static void removeCustom(Context ctx) {
        File f = customIconFile(ctx);
        if (f.exists()) f.delete();
        prefs(ctx).edit()
            .remove(KEY_CUSTOM_NAME)
            .putString(KEY_PRESET, PRESET_DEFAULT)
            .apply();
        IconChanger.removeCustomShortcut(ctx);
        IconChanger.apply(ctx, PRESET_DEFAULT);
    }
}
