package io.zeronode.vpn;

import android.graphics.Color;
import android.graphics.Typeface;
import android.graphics.drawable.GradientDrawable;
import android.view.Gravity;
import android.view.View;
import android.view.ViewGroup;
import android.widget.ImageView;
import android.widget.LinearLayout;
import android.widget.TextView;

/** First-launch walkthrough. */
final class OnboardingScreen {
    private static final String PREFS = "zeronode_onboarding";
    private static final String KEY_DONE = "done";

    private OnboardingScreen() {}

    static boolean isDone(MainActivity a) {
        return a.getSharedPreferences(PREFS, 0).getBoolean(KEY_DONE, false);
    }

    static void markDone(MainActivity a) {
        a.getSharedPreferences(PREFS, 0).edit().putBoolean(KEY_DONE, true).apply();
    }

    static View build(final MainActivity a) {
        final LinearLayout page = new LinearLayout(a);
        page.setOrientation(LinearLayout.VERTICAL);
        page.setVisibility(View.GONE);
        page.setClickable(true);
        page.setBackgroundColor(0xF014161A);
        int topPad = a.statusBarInsetPx() + a.dp(24);
        page.setPadding(a.dp(24), topPad, a.dp(24), a.dp(28));
        page.setGravity(Gravity.CENTER_HORIZONTAL);

        final int[] icons = {
            R.drawable.ic_lock,
            R.drawable.ic_proto_wireguard,
            R.drawable.ic_proto_tor,
            R.drawable.ic_settings
        };
        final String[] titles = {
            "Welcome to ZeroNode",
            "Pick a protocol",
            "Connect & check your exit",
            "Settings, disguise & bridges"
        };
        final String[] bodies = {
            "A multi-protocol VPN. WireGuard, Outline, and Tor on one screen. This short guide shows how to use it.",
            "Use the three tabs. WireGuard needs a .conf. Outline needs an ss:// key. Tor uses the built-in expert bundle — no profile file.",
            "Tap Connect (the large green button). When it is up, tap Refresh IP on the globe. The pin shows flag and IPv4. Drag the globe; pinch to zoom.",
            "The gear opens Settings: app look (Weather, Garden, or your own icon), Tor bridges (obfs4 / Snowflake), and the full Guide. App protection picks which apps use the tunnel."
        };
        final int[][] colors = {
            {0xFF00FF7F, Color.BLACK},
            {0xFF00FF7F, Color.BLACK},
            {0xFFA855F7, Color.WHITE},
            {0xFFE8EDF2, Color.BLACK}
        };

        final ImageView icon = Icons.vector(a, icons[0], 0xFF00FF7F);
        LinearLayout.LayoutParams ilp = new LinearLayout.LayoutParams(a.dp(64), a.dp(64));
        ilp.topMargin = a.dp(28);
        page.addView(icon, ilp);

        final TextView title = new TextView(a);
        title.setText(titles[0]);
        title.setTextColor(Color.WHITE);
        title.setTextSize(22);
        title.setTypeface(Typeface.create("sans-serif-medium", Typeface.BOLD));
        title.setGravity(Gravity.CENTER);
        LinearLayout.LayoutParams tlp = mw();
        tlp.topMargin = a.dp(20);
        page.addView(title, tlp);

        final TextView body = new TextView(a);
        body.setText(bodies[0]);
        body.setTextColor(0xFFB4B8BE);
        body.setTextSize(15);
        body.setGravity(Gravity.CENTER);
        body.setLineSpacing(a.dp(3), 1.15f);
        LinearLayout.LayoutParams blp = mw();
        blp.topMargin = a.dp(12);
        page.addView(body, blp);

        final LinearLayout dots = new LinearLayout(a);
        dots.setOrientation(LinearLayout.HORIZONTAL);
        dots.setGravity(Gravity.CENTER);
        LinearLayout.LayoutParams dlp = mw();
        dlp.topMargin = a.dp(28);
        page.addView(dots, dlp);

        View spacer = new View(a);
        page.addView(spacer, new LinearLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT, 0, 1f));

        final TextView next = new TextView(a);
        next.setGravity(Gravity.CENTER);
        next.setTextSize(16);
        next.setTypeface(Typeface.create("sans-serif-medium", Typeface.NORMAL));
        GradientDrawable nbg = new GradientDrawable();
        nbg.setCornerRadius(a.dp(14));
        nbg.setColor(0xFF00FF7F);
        next.setBackground(nbg);
        next.setTextColor(Color.BLACK);
        next.setText("Next");
        page.addView(next, mw(a.dp(52)));

        final int[] step = {0};
        final Runnable paint = new Runnable() {
            @Override public void run() {
                int i = step[0];
                icon.setImageResource(icons[i]);
                Icons.tint(icon, i == 2 ? 0xFFC084FC : 0xFF00FF7F);
                title.setText(titles[i]);
                body.setText(bodies[i]);
                next.setText(i == titles.length - 1 ? "Get started" : "Next");
                dots.removeAllViews();
                for (int d = 0; d < titles.length; d++) {
                    View dot = new View(a);
                    GradientDrawable dg = new GradientDrawable();
                    dg.setShape(GradientDrawable.OVAL);
                    dg.setColor(d == i ? 0xFF00FF7F : 0xFF2A2F38);
                    dot.setBackground(dg);
                    LinearLayout.LayoutParams lp = new LinearLayout.LayoutParams(a.dp(8), a.dp(8));
                    if (d > 0) lp.leftMargin = a.dp(6);
                    dots.addView(dot, lp);
                }
            }
        };
        paint.run();
        next.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View v) {
                if (step[0] >= titles.length - 1) {
                    markDone(a);
                    a.closeOnboarding();
                    return;
                }
                step[0]++;
                paint.run();
            }
        });
        return page;
    }

    private static LinearLayout.LayoutParams mw() {
        return new LinearLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
    }

    private static LinearLayout.LayoutParams mw(int h) {
        return new LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, h);
    }
}
