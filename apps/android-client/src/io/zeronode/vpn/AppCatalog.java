package io.zeronode.vpn;

import android.content.Context;
import android.content.Intent;
import android.content.pm.PackageManager;
import android.content.pm.ResolveInfo;
import android.graphics.drawable.Drawable;
import android.net.Uri;
import android.os.Build;
import android.os.Handler;
import android.os.Looper;

import java.util.ArrayList;
import java.util.Collections;
import java.util.Comparator;
import java.util.HashSet;
import java.util.List;
import java.util.Locale;
import java.util.Set;
import java.util.concurrent.atomic.AtomicBoolean;

/**
 * Fast in-memory launcher catalog. Labels first, icons in the background.
 * No disk database — PackageManager is the source of truth.
 */
final class AppCatalog {
    static final class AppInfo {
        String pkg = "";
        String label = "";
        boolean browser;
        Drawable icon;
    }

    interface Listener {
        void onReady(List<AppInfo> apps, boolean iconsReady);
    }

    private static final Object LOCK = new Object();
    private static List<AppInfo> cached;
    private static boolean iconsReady;
    private static final AtomicBoolean loading = new AtomicBoolean(false);
    private static final Handler MAIN = new Handler(Looper.getMainLooper());

    private AppCatalog() {}

    static List<AppInfo> snapshot() {
        synchronized (LOCK) {
            return cached == null ? Collections.<AppInfo>emptyList() : new ArrayList<AppInfo>(cached);
        }
    }

    static boolean hasCache() {
        synchronized (LOCK) {
            return cached != null && !cached.isEmpty();
        }
    }

    static List<String> allPackages() {
        List<AppInfo> apps = snapshot();
        List<String> out = new ArrayList<String>(apps.size());
        for (int i = 0; i < apps.size(); i++) out.add(apps.get(i).pkg);
        return out;
    }

    static void load(final Context ctx, final Listener listener) {
        List<AppInfo> ready;
        boolean icons;
        synchronized (LOCK) {
            ready = cached == null ? null : new ArrayList<AppInfo>(cached);
            icons = iconsReady;
        }
        if (ready != null && listener != null) {
            listener.onReady(ready, icons);
            if (icons) return;
        }
        if (!loading.compareAndSet(false, true) && ready != null) return;
        final Context app = ctx.getApplicationContext();
        new Thread(new Runnable() {
            @Override public void run() {
                try {
                    List<AppInfo> rows = queryLaunchers(app);
                    markBrowsers(app, rows);
                    Collections.sort(rows, new Comparator<AppInfo>() {
                        @Override
                        public int compare(AppInfo a, AppInfo b) {
                            if (a.browser != b.browser) return a.browser ? -1 : 1;
                            return a.label.compareToIgnoreCase(b.label);
                        }
                    });
                    synchronized (LOCK) {
                        cached = rows;
                        iconsReady = false;
                    }
                    post(listener, rows, false);
                    PackageManager pm = app.getPackageManager();
                    for (int i = 0; i < rows.size(); i++) {
                        AppInfo info = rows.get(i);
                        try {
                            info.icon = pm.getApplicationIcon(info.pkg);
                        } catch (Exception ignored) {
                        }
                    }
                    synchronized (LOCK) {
                        iconsReady = true;
                    }
                    post(listener, rows, true);
                } catch (Exception e) {
                    android.util.Log.w("ZeroNode", "app catalog: " + e.getMessage());
                    post(listener, snapshot(), iconsReady);
                } finally {
                    loading.set(false);
                }
            }
        }, "zn-app-catalog").start();
    }

    private static void post(final Listener listener, final List<AppInfo> rows, final boolean icons) {
        if (listener == null) return;
        final List<AppInfo> copy = new ArrayList<AppInfo>(rows);
        MAIN.post(new Runnable() {
            @Override public void run() {
                listener.onReady(copy, icons);
            }
        });
    }

    private static List<AppInfo> queryLaunchers(Context ctx) {
        PackageManager pm = ctx.getPackageManager();
        Intent launch = new Intent(Intent.ACTION_MAIN, null);
        launch.addCategory(Intent.CATEGORY_LAUNCHER);
        List<ResolveInfo> infos;
        if (Build.VERSION.SDK_INT >= 33) {
            infos = pm.queryIntentActivities(launch, PackageManager.ResolveInfoFlags.of(0));
        } else {
            infos = pm.queryIntentActivities(launch, 0);
        }
        List<AppInfo> rows = new ArrayList<AppInfo>(infos.size());
        Set<String> seen = new HashSet<String>();
        String self = ctx.getPackageName();
        for (int i = 0; i < infos.size(); i++) {
            ResolveInfo ri = infos.get(i);
            if (ri.activityInfo == null) continue;
            String pkg = ri.activityInfo.packageName;
            if (pkg == null || pkg.equals(self) || !seen.add(pkg)) continue;
            AppInfo row = new AppInfo();
            row.pkg = pkg;
            CharSequence label = ri.loadLabel(pm);
            row.label = label != null ? label.toString() : pkg;
            rows.add(row);
        }
        return rows;
    }

    private static void markBrowsers(Context ctx, List<AppInfo> rows) {
        Set<String> browsers = new HashSet<String>();
        try {
            PackageManager pm = ctx.getPackageManager();
            Intent view = new Intent(Intent.ACTION_VIEW, Uri.parse("https://zeronode.example"));
            List<ResolveInfo> infos;
            if (Build.VERSION.SDK_INT >= 33) {
                infos = pm.queryIntentActivities(view, PackageManager.ResolveInfoFlags.of(0));
            } else {
                infos = pm.queryIntentActivities(view, 0);
            }
            for (int i = 0; i < infos.size(); i++) {
                ResolveInfo ri = infos.get(i);
                if (ri.activityInfo != null && ri.activityInfo.packageName != null) {
                    browsers.add(ri.activityInfo.packageName);
                }
            }
        } catch (Exception ignored) {
        }
        for (int i = 0; i < rows.size(); i++) {
            AppInfo row = rows.get(i);
            if (browsers.contains(row.pkg) || isKnownBrowser(row.pkg)) {
                row.browser = true;
            }
        }
    }

    private static boolean isKnownBrowser(String pkg) {
        if (pkg == null) return false;
        String p = pkg.toLowerCase(Locale.US);
        return p.contains("chrome")
            || p.contains("firefox")
            || p.contains("browser")
            || p.contains("opera")
            || p.contains("brave")
            || p.contains("vivaldi")
            || p.contains("duckduckgo")
            || p.contains("samsung.android.app.sbrowser")
            || p.contains("sec.android.app.sbrowser")
            || p.contains("microsoft.emmx")
            || p.contains("torbrowser")
            || p.contains("bromite")
            || p.contains("kiwi")
            || p.contains("ecosia")
            || p.equals("com.android.browser");
    }
}
