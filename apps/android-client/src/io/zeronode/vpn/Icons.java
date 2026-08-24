package io.zeronode.vpn;

import android.content.Context;
import android.graphics.Color;
import android.graphics.PorterDuff;
import android.widget.ImageView;

/** Vector icons for chrome, protocol tabs, and overlay pages. */
final class Icons {
    static final int TOR = 0;
    static final int WIREGUARD = 1;
    static final int OUTLINE = 2;
    static final int LOCK = 3;
    static final int UNLOCK = 4;
    static final int BROWSER = 5;
    static final int APPS = 6;
    static final int BACK = 7;
    static final int EDIT = 8;
    static final int SETTINGS = 9;
    static final int PLUS = 10;
    static final int GUIDE = 11;

    private Icons() {}

    static int drawableOf(int kind) {
        switch (kind) {
            case TOR: return R.drawable.ic_proto_tor;
            case WIREGUARD: return R.drawable.ic_proto_wireguard;
            case OUTLINE: return R.drawable.ic_proto_outline;
            case LOCK: return R.drawable.ic_lock;
            case UNLOCK: return R.drawable.ic_unlock;
            case BROWSER: return R.drawable.ic_browser;
            case APPS: return R.drawable.ic_apps;
            case BACK: return R.drawable.ic_back;
            case EDIT: return R.drawable.ic_edit;
            case SETTINGS: return R.drawable.ic_settings;
            case PLUS: return R.drawable.ic_plus;
            case GUIDE: return R.drawable.ic_guide;
            default: return R.drawable.ic_lock;
        }
    }

    static ImageView of(Context ctx, int kind, int color) {
        return vector(ctx, drawableOf(kind), color);
    }

    static ImageView vector(Context ctx, int resId, int color) {
        ImageView v = new ImageView(ctx);
        v.setImageResource(resId);
        v.setScaleType(ImageView.ScaleType.FIT_CENTER);
        v.setAdjustViewBounds(true);
        if (color != 0) {
            v.setColorFilter(color, PorterDuff.Mode.SRC_IN);
        }
        return v;
    }

    static void tint(ImageView v, int color) {
        if (v == null) return;
        if (color == 0) v.clearColorFilter();
        else v.setColorFilter(color, PorterDuff.Mode.SRC_IN);
    }

    static ImageView chromeButton(Context ctx, int resId, int color) {
        ImageView v = vector(ctx, resId, color);
        v.setClickable(true);
        v.setFocusable(true);
        v.setBackgroundColor(Color.TRANSPARENT);
        v.setPadding(dp(ctx, 8), dp(ctx, 8), dp(ctx, 8), dp(ctx, 8));
        return v;
    }

    private static int dp(Context ctx, int v) {
        return Math.round(v * ctx.getResources().getDisplayMetrics().density);
    }
}
