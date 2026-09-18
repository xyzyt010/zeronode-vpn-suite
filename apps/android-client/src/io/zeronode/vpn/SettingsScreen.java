package io.zeronode.vpn;

import android.graphics.Color;
import android.graphics.Typeface;
import android.graphics.drawable.GradientDrawable;
import android.os.Build;
import android.text.InputType;
import android.view.Gravity;
import android.view.View;
import android.view.ViewGroup;
import android.widget.EditText;
import android.widget.ImageView;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.TextView;

/** Full-screen Settings: appearance + Tor bridges. */
final class SettingsScreen {
    private SettingsScreen() {}

    static View build(final MainActivity a) {
        LinearLayout page = new LinearLayout(a);
        page.setOrientation(LinearLayout.VERTICAL);
        page.setVisibility(View.GONE);
        page.setClickable(true);
        page.setBackgroundColor(0xFF0B0D10);
        int topPad = a.statusBarInsetPx() + a.dp(10);
        page.setPadding(a.dp(16), topPad, a.dp(16), a.dp(16));

        LinearLayout header = new LinearLayout(a);
        header.setOrientation(LinearLayout.HORIZONTAL);
        header.setGravity(Gravity.CENTER_VERTICAL);
        ImageView back = Icons.of(a, Icons.BACK, 0xFFE8EAED);
        back.setPadding(a.dp(8), a.dp(8), a.dp(8), a.dp(8));
        back.setClickable(true);
        back.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View v) { a.closeSettings(); }
        });
        header.addView(back, a.dp(36), a.dp(36));
        TextView title = new TextView(a);
        title.setText("Settings");
        title.setTextColor(Color.WHITE);
        title.setTextSize(18);
        title.setTypeface(Typeface.create("sans-serif-medium", Typeface.NORMAL));
        LinearLayout.LayoutParams tlp = new LinearLayout.LayoutParams(0, vw(), 1f);
        tlp.leftMargin = a.dp(8);
        header.addView(title, tlp);
        page.addView(header, mw());

        ScrollView scroller = new ScrollView(a);
        scroller.setFillViewport(true);
        scroller.setVerticalScrollBarEnabled(false);
        LinearLayout body = new LinearLayout(a);
        body.setOrientation(LinearLayout.VERTICAL);
        body.setFocusableInTouchMode(true);
        body.setDescendantFocusability(ViewGroup.FOCUS_BEFORE_DESCENDANTS);
        body.addView(appearanceCard(a), mw());
        LinearLayout.LayoutParams blp = mw();
        blp.topMargin = a.dp(14);
        body.addView(bridgesCard(a), blp);
        LinearLayout.LayoutParams glp = mw();
        glp.topMargin = a.dp(14);
        body.addView(guideCard(a), glp);
        scroller.addView(body, mw());
        LinearLayout.LayoutParams slp = new LinearLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT, 0, 1f
        );
        slp.topMargin = a.dp(12);
        page.addView(scroller, slp);
        return page;
    }

    private static View appearanceCard(final MainActivity a) {
        LinearLayout card = section(a);
        card.addView(sectionTitle(a, "App look", 0xFF00FF7F), mw());
        TextView hint = muted(a, "Choose ZeroNode, Weather, or Garden as your launcher name and icon.");
        card.addView(hint, mw());

        LinearLayout row = new LinearLayout(a);
        row.setOrientation(LinearLayout.HORIZONTAL);
        row.setPadding(0, a.dp(10), 0, a.dp(4));
        row.addView(presetTile(a, AppearanceStore.PRESET_DEFAULT, "ZeroNode", R.drawable.ic_alias_zeronode_adaptive),
            new LinearLayout.LayoutParams(0, vw(), 1f));
        LinearLayout.LayoutParams g = new LinearLayout.LayoutParams(0, vw(), 1f);
        g.leftMargin = a.dp(8);
        row.addView(presetTile(a, AppearanceStore.PRESET_WEATHER, "Weather", R.drawable.ic_alias_weather_adaptive), g);
        LinearLayout.LayoutParams g2 = new LinearLayout.LayoutParams(0, vw(), 1f);
        g2.leftMargin = a.dp(8);
        row.addView(presetTile(a, AppearanceStore.PRESET_GARDEN, "Garden", R.drawable.ic_alias_garden), g2);
        card.addView(row, mw());

        return card;
    }

    private static View presetTile(final MainActivity a, final String preset, String label, int drawable) {
        LinearLayout tile = new LinearLayout(a);
        tile.setOrientation(LinearLayout.VERTICAL);
        tile.setGravity(Gravity.CENTER_HORIZONTAL);
        tile.setPadding(a.dp(8), a.dp(12), a.dp(8), a.dp(12));
        GradientDrawable bg = new GradientDrawable();
        bg.setCornerRadius(a.dp(14));
        boolean on = preset.equals(AppearanceStore.preset(a));
        bg.setColor(on ? 0xFF102018 : 0xFF14181E);
        bg.setStroke(a.dp(1), on ? 0xFF00FF7F : 0x2AFFFFFF);
        tile.setBackground(bg);
        ImageView icon = new ImageView(a);
        icon.setScaleType(ImageView.ScaleType.CENTER_CROP);
        icon.setImageResource(drawable);
        GradientDrawable clip = new GradientDrawable();
        clip.setCornerRadius(a.dp(14));
        clip.setColor(0xFF1A1D24);
        icon.setBackground(clip);
        if (Build.VERSION.SDK_INT >= 21) icon.setClipToOutline(true);
        tile.addView(icon, a.dp(56), a.dp(56));
        TextView t = new TextView(a);
        t.setText(label);
        t.setTextColor(on ? 0xFF00FF7F : 0xFFE8EAED);
        t.setTextSize(12);
        t.setGravity(Gravity.CENTER);
        t.setPadding(0, a.dp(8), 0, 0);
        tile.addView(t, mw());
        tile.setClickable(true);
        tile.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View v) {
                AppearanceStore.setPreset(a, preset);
                a.refreshChromeTitle();
                a.setNotice("Launcher look: " + label
                    + (AppearanceStore.PRESET_DEFAULT.equals(preset)
                    ? "" : " — may take a moment on the home screen."));
                LinearLayout row = (LinearLayout) v.getParent();
                LinearLayout card = (LinearLayout) row.getParent();
                ViewGroup parent = (ViewGroup) card.getParent();
                int index = parent.indexOfChild(card);
                ViewGroup.LayoutParams params = card.getLayoutParams();
                parent.removeView(card);
                parent.addView(appearanceCard(a), index, params);
            }
        });
        return tile;
    }

    private static View bridgesCard(final MainActivity a) {
        LinearLayout card = section(a);
        card.setId(R.id.settings_bridges);
        LinearLayout head = new LinearLayout(a);
        head.setOrientation(LinearLayout.HORIZONTAL);
        head.setGravity(Gravity.CENTER_VERTICAL);
        ImageView tor = Icons.of(a, Icons.TOR, 0xFFC084FC);
        LinearLayout.LayoutParams ilp = new LinearLayout.LayoutParams(a.dp(20), a.dp(20));
        ilp.rightMargin = a.dp(8);
        head.addView(tor, ilp);
        head.addView(sectionTitle(a, "Tor bridges", 0xFFC084FC), vw(), vw());
        card.addView(head, mw());
        card.addView(muted(a, "If Tor is blocked, turn on bridges. Snowflake just works; obfs4 uses the bundled bridges unless you paste your own. Reconnect Tor after changing bridges."), mw());

        LinearLayout toggle = rowBox(a);
        LinearLayout.LayoutParams tlp = mw();
        tlp.topMargin = a.dp(10);
        LinearLayout texts = new LinearLayout(a);
        texts.setOrientation(LinearLayout.VERTICAL);
        TextView onTitle = new TextView(a);
        onTitle.setText("Use a bridge");
        onTitle.setTextColor(Color.WHITE);
        onTitle.setTextSize(15);
        onTitle.setTypeface(Typeface.create("sans-serif-medium", Typeface.NORMAL));
        texts.addView(onTitle, mw());
        final TextView sub = new TextView(a);
        sub.setText(BridgeStore.summary(a));
        sub.setTextColor(0xFF8A9098);
        sub.setTextSize(12);
        texts.addView(sub, mw());
        toggle.addView(texts, new LinearLayout.LayoutParams(0, vw(), 1f));
        final MainActivity.GreenSwitch sw = new MainActivity.GreenSwitch(a);
        sw.setOn(BridgeStore.enabled(a), false);
        toggle.addView(sw, a.dp(42), a.dp(26));
        card.addView(toggle, tlp);

        final TextView directLine = muted(a, "Direct — connects straight to public Tor relays, no bridge. Some networks block them.");
        LinearLayout.LayoutParams dlp = mw();
        dlp.topMargin = a.dp(8);
        card.addView(directLine, dlp);

        final LinearLayout modes = new LinearLayout(a);
        modes.setOrientation(LinearLayout.HORIZONTAL);
        LinearLayout.LayoutParams mlp = mw();
        mlp.topMargin = a.dp(8);
        modes.addView(modeChip(a, BridgeStore.MODE_SNOWFLAKE, "Snowflake"),
            new LinearLayout.LayoutParams(0, a.dp(38), 1f));
        LinearLayout.LayoutParams m1 = new LinearLayout.LayoutParams(0, a.dp(38), 1f);
        m1.leftMargin = a.dp(6);
        modes.addView(modeChip(a, BridgeStore.MODE_OBFS4, "obfs4"), m1);
        card.addView(modes, mlp);

        final TextView blurb = muted(a, BridgeStore.maskBlurb(BridgeStore.mode(a)));
        blurb.setMinLines(2);
        blurb.setTextColor(0xFFC4C8CE);
        LinearLayout.LayoutParams blp = mw();
        blp.topMargin = a.dp(8);
        card.addView(blurb, blp);

        final EditText custom = new EditText(a);
        custom.setHint("Optional obfs4 overrides, one complete bridge per line");
        custom.setInputType(InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_FLAG_MULTI_LINE
            | InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS);
        custom.setHintTextColor(0xFF6B7178);
        custom.setTextColor(0xFFDDDDDD);
        custom.setTextSize(12);
        custom.setTypeface(Typeface.MONOSPACE);
        custom.setMinLines(3);
        custom.setGravity(Gravity.TOP | Gravity.START);
        custom.setBackground(inputBg(a));
        custom.setPadding(a.dp(10), a.dp(8), a.dp(10), a.dp(8));
        custom.setText(BridgeStore.customLines(a));

        final LinearLayout obfs4Box = new LinearLayout(a);
        obfs4Box.setOrientation(LinearLayout.VERTICAL);
        LinearLayout.LayoutParams clp = mw();
        clp.topMargin = a.dp(10);
        obfs4Box.addView(custom, clp);

        final Runnable refresh = new Runnable() {
            @Override public void run() {
                boolean on = BridgeStore.enabled(a);
                boolean obfs4 = BridgeStore.MODE_OBFS4.equals(BridgeStore.mode(a));
                sub.setText(BridgeStore.summary(a));
                sw.setOn(on, false);
                directLine.setVisibility(on ? View.GONE : View.VISIBLE);
                modes.setVisibility(on ? View.VISIBLE : View.GONE);
                blurb.setVisibility(on ? View.VISIBLE : View.GONE);
                if (on) blurb.setText(BridgeStore.maskBlurb(BridgeStore.mode(a)));
                obfs4Box.setVisibility(on && obfs4 ? View.VISIBLE : View.GONE);
                for (int i = 0; i < modes.getChildCount(); i++) {
                    TextView chip = (TextView) modes.getChildAt(i);
                    styleModeChip(a, chip, (String) chip.getTag());
                }
            }
        };
        sw.setOnToggle(new MainActivity.GreenSwitch.OnToggle() {
            @Override public void onToggle(boolean on) {
                BridgeStore.setMode(a, on ? BridgeStore.MODE_SNOWFLAKE : BridgeStore.MODE_OFF);
                refresh.run();
                a.setNotice("Bridge selection saved. Reconnect Tor to apply.");
            }
        });
        for (int i = 0; i < modes.getChildCount(); i++) {
            final TextView chip = (TextView) modes.getChildAt(i);
            chip.setOnClickListener(new View.OnClickListener() {
                @Override public void onClick(View v) {
                    BridgeStore.setMode(a, (String) chip.getTag());
                    refresh.run();
                    a.setNotice("Bridge selection saved. Reconnect Tor to apply.");
                }
            });
        }

        LinearLayout.LayoutParams saveLp = mw(a.dp(42));
        saveLp.topMargin = a.dp(8);
        obfs4Box.addView(smallBtn(a, "Save custom bridges", new View.OnClickListener() {
            @Override public void onClick(View v) {
                try {
                    BridgeStore.setCustomLines(a, custom.getText().toString());
                    custom.setError(null);
                    refresh.run();
                    a.setNotice("obfs4 bridges saved. Reconnect Tor to apply.");
                } catch (IllegalArgumentException e) {
                    custom.setError(e.getMessage());
                }
            }
        }), saveLp);

        obfs4Box.addView(linkRow(a, "Get obfs4 bridges", "Open the official Tor bot, send /start then /obfs4, and paste the complete lines above. Leave blank to use the bundled obfs4 bridges."), mw());
        obfs4Box.addView(linkBtn(a, "Telegram · @GetBridgesBot", BridgeStore.TELEGRAM_BOT), mw());
        card.addView(obfs4Box, mw());
        refresh.run();
        return card;
    }

    private static View modeChip(final MainActivity a, final String mode, String label) {
        TextView t = new TextView(a);
        t.setText(label);
        t.setGravity(Gravity.CENTER);
        t.setTextSize(13);
        t.setTypeface(Typeface.create("sans-serif-medium", Typeface.NORMAL));
        t.setTag(mode);
        t.setClickable(true);
        styleModeChip(a, t, mode);
        return t;
    }

    private static void styleModeChip(MainActivity a, TextView t, String mode) {
        GradientDrawable bg = new GradientDrawable();
        bg.setCornerRadius(a.dp(10));
        boolean on = mode.equals(BridgeStore.mode(a));
        t.setSelected(on);
        if (on) {
            bg.setColor(BridgeStore.MODE_OFF.equals(mode) ? 0xFF00FF7F : 0xFFA855F7);
            t.setTextColor(BridgeStore.MODE_OFF.equals(mode) ? Color.BLACK : Color.WHITE);
        } else {
            bg.setColor(0xFF1E2128);
            t.setTextColor(0xFFDDDDDD);
        }
        t.setBackground(bg);
    }

    private static View guideCard(final MainActivity a) {
        LinearLayout card = section(a);
        LinearLayout row = new LinearLayout(a);
        row.setOrientation(LinearLayout.HORIZONTAL);
        row.setGravity(Gravity.CENTER_VERTICAL);
        ImageView ic = Icons.of(a, Icons.GUIDE, 0xFF00FF7F);
        LinearLayout.LayoutParams ilp = new LinearLayout.LayoutParams(a.dp(20), a.dp(20));
        ilp.rightMargin = a.dp(10);
        row.addView(ic, ilp);
        LinearLayout texts = new LinearLayout(a);
        texts.setOrientation(LinearLayout.VERTICAL);
        TextView t = new TextView(a);
        t.setText("User guide");
        t.setTextColor(Color.WHITE);
        t.setTextSize(15);
        t.setTypeface(Typeface.create("sans-serif-medium", Typeface.NORMAL));
        texts.addView(t, mw());
        texts.addView(muted(a, "Protocols, bridges, app protection, disguise, FAQs."), mw());
        row.addView(texts, new LinearLayout.LayoutParams(0, vw(), 1f));
        card.addView(row, mw());
        card.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View v) { a.openGuide(); }
        });
        return card;
    }

    private static View linkRow(MainActivity a, String title, String body) {
        LinearLayout col = new LinearLayout(a);
        col.setOrientation(LinearLayout.VERTICAL);
        col.setPadding(0, a.dp(10), 0, 0);
        TextView t = new TextView(a);
        t.setText(title);
        t.setTextColor(Color.WHITE);
        t.setTextSize(13);
        col.addView(t, mw());
        col.addView(muted(a, body), mw());
        return col;
    }

    private static View linkBtn(final MainActivity a, String label, final String url) {
        TextView t = new TextView(a);
        t.setText(label);
        t.setTextColor(0xFF7DD3FC);
        t.setTextSize(13);
        t.setPadding(0, a.dp(8), 0, a.dp(4));
        t.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View v) { a.openExternalUrl(url); }
        });
        return t;
    }

    private static LinearLayout section(MainActivity a) {
        LinearLayout card = new LinearLayout(a);
        card.setOrientation(LinearLayout.VERTICAL);
        card.setPadding(a.dp(14), a.dp(14), a.dp(14), a.dp(14));
        GradientDrawable bg = new GradientDrawable();
        bg.setCornerRadius(a.dp(16));
        bg.setColor(0xFF12151A);
        bg.setStroke(a.dp(1), 0x22FFFFFF);
        card.setBackground(bg);
        return card;
    }

    private static LinearLayout rowBox(MainActivity a) {
        LinearLayout row = new LinearLayout(a);
        row.setOrientation(LinearLayout.HORIZONTAL);
        row.setGravity(Gravity.CENTER_VERTICAL);
        row.setPadding(a.dp(12), a.dp(12), a.dp(12), a.dp(12));
        GradientDrawable bg = new GradientDrawable();
        bg.setCornerRadius(a.dp(14));
        bg.setColor(0xFF14181E);
        bg.setStroke(a.dp(1), 0x2AFFFFFF);
        row.setBackground(bg);
        return row;
    }

    private static TextView sectionTitle(MainActivity a, String s, int color) {
        TextView t = new TextView(a);
        t.setText(s);
        t.setTextColor(color);
        t.setTextSize(14);
        t.setTypeface(Typeface.create("sans-serif-medium", Typeface.NORMAL));
        t.setLetterSpacing(0.04f);
        return t;
    }

    private static TextView muted(MainActivity a, String s) {
        TextView t = new TextView(a);
        t.setText(s);
        t.setTextColor(0xFF8A9098);
        t.setTextSize(12);
        t.setPadding(0, a.dp(4), 0, 0);
        t.setLineSpacing(0, 1.15f);
        return t;
    }

    private static TextView smallBtn(MainActivity a, String label, View.OnClickListener l) {
        TextView t = new TextView(a);
        t.setText(label);
        t.setGravity(Gravity.CENTER);
        t.setTextColor(Color.BLACK);
        t.setTextSize(13);
        t.setTypeface(Typeface.create("sans-serif-medium", Typeface.NORMAL));
        GradientDrawable bg = new GradientDrawable();
        bg.setCornerRadius(a.dp(10));
        bg.setColor(0xFFE8EDF2);
        t.setBackground(bg);
        t.setClickable(true);
        t.setOnClickListener(l);
        return t;
    }

    private static GradientDrawable inputBg(MainActivity a) {
        GradientDrawable bg = new GradientDrawable();
        bg.setCornerRadius(a.dp(10));
        bg.setColor(0xFF1A1D24);
        bg.setStroke(a.dp(1), 0x22FFFFFF);
        return bg;
    }

    private static int vw() { return ViewGroup.LayoutParams.WRAP_CONTENT; }

    private static LinearLayout.LayoutParams mw() {
        return new LinearLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
    }

    private static LinearLayout.LayoutParams mw(int h) {
        return new LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, h);
    }
}
