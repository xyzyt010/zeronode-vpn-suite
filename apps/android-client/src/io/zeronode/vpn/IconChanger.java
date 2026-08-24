package io.zeronode.vpn;

import android.app.PendingIntent;
import android.content.ComponentName;
import android.content.Context;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.content.pm.ShortcutInfo;
import android.content.pm.ShortcutManager;
import android.graphics.Bitmap;
import android.graphics.drawable.Icon;
import android.os.Build;

import java.util.Collections;

/** Enable one launcher activity-alias at a time. Custom uses a pinned shortcut. */
final class IconChanger {
    static final String ALIAS_DEFAULT = "io.zeronode.vpn.LauncherDefault";
    static final String ALIAS_WEATHER = "io.zeronode.vpn.LauncherWeather";
    static final String ALIAS_GARDEN = "io.zeronode.vpn.LauncherGarden";
    static final String SHORTCUT_CUSTOM = "zn-custom-look";

    private IconChanger() {}

    static void apply(Context ctx, String preset) {
        PackageManager pm = ctx.getPackageManager();
        boolean weather = AppearanceStore.PRESET_WEATHER.equals(preset);
        boolean garden = AppearanceStore.PRESET_GARDEN.equals(preset);
        boolean custom = AppearanceStore.PRESET_CUSTOM.equals(preset);
        // Keep default enabled for custom so the app stays in the drawer
        // if the user declines the home-screen shortcut.
        setEnabled(pm, ctx, ALIAS_DEFAULT, !weather && !garden);
        setEnabled(pm, ctx, ALIAS_WEATHER, weather);
        setEnabled(pm, ctx, ALIAS_GARDEN, garden);
        if (!custom) removeCustomShortcut(ctx);
    }

    static void applyCustom(Context ctx, String name, Bitmap icon) {
        apply(ctx, AppearanceStore.PRESET_CUSTOM);
        if (Build.VERSION.SDK_INT < 26) return;
        try {
            ShortcutManager sm = ctx.getSystemService(ShortcutManager.class);
            if (sm == null) return;
            Intent launch = new Intent(ctx, MainActivity.class);
            launch.setAction(Intent.ACTION_MAIN);
            launch.addCategory(Intent.CATEGORY_LAUNCHER);
            launch.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK | Intent.FLAG_ACTIVITY_CLEAR_TOP);
            ShortcutInfo info = new ShortcutInfo.Builder(ctx, SHORTCUT_CUSTOM)
                .setShortLabel(name)
                .setLongLabel(name)
                .setIcon(Icon.createWithBitmap(icon))
                .setIntent(launch)
                .build();
            sm.setDynamicShortcuts(Collections.singletonList(info));
            if (sm.isRequestPinShortcutSupported()) {
                PendingIntent ok = PendingIntent.getBroadcast(
                    ctx, 0, new Intent(),
                    PendingIntent.FLAG_UPDATE_CURRENT | PendingIntent.FLAG_IMMUTABLE
                );
                sm.requestPinShortcut(info, ok.getIntentSender());
            }
        } catch (Exception ignored) {
        }
    }

    static void removeCustomShortcut(Context ctx) {
        if (Build.VERSION.SDK_INT < 25) return;
        try {
            ShortcutManager sm = ctx.getSystemService(ShortcutManager.class);
            if (sm == null) return;
            sm.removeDynamicShortcuts(Collections.singletonList(SHORTCUT_CUSTOM));
            if (Build.VERSION.SDK_INT >= 25) {
                try {
                    sm.disableShortcuts(
                        Collections.singletonList(SHORTCUT_CUSTOM),
                        "Custom look removed"
                    );
                } catch (Exception ignored) {
                }
            }
        } catch (Exception ignored) {
        }
    }

    private static void setEnabled(PackageManager pm, Context ctx, String alias, boolean on) {
        try {
            pm.setComponentEnabledSetting(
                new ComponentName(ctx, alias),
                on
                    ? PackageManager.COMPONENT_ENABLED_STATE_ENABLED
                    : PackageManager.COMPONENT_ENABLED_STATE_DISABLED,
                PackageManager.DONT_KILL_APP
            );
        } catch (Exception ignored) {
        }
    }
}
