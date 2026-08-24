package io.zeronode.vpn;

import android.app.Activity;
import android.content.Intent;
import android.graphics.Bitmap;
import android.graphics.Color;
import android.graphics.Typeface;
import android.graphics.drawable.GradientDrawable;
import android.net.Uri;
import android.os.Build;
import android.provider.MediaStore;
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
        TextView hint = muted(a, "Change the name and icon on the home screen. Weather and Garden are real launcher aliases. Custom uses your image as a home-screen shortcut.");
        card.addView(hint, mw());

        LinearLayout row = new LinearLayout(a);
        row.setOrientation(LinearLayout.HORIZONTAL);
        row.setPadding(0, a.dp(10), 0, a.dp(4));
        row.addView(presetTile(a, AppearanceStore.PRESET_DEFAULT, "ZeroNode", 0),
            new LinearLayout.LayoutParams(0, vw(), 1f));
        LinearLayout.LayoutParams g = new LinearLayout.LayoutParams(0, vw(), 1f);
        g.leftMargin = a.dp(8);
        row.addView(presetTile(a, AppearanceStore.PRESET_WEATHER, "Weather", R.drawable.ic_alias_weather), g);
        LinearLayout.LayoutParams g2 = new LinearLayout.LayoutParams(0, vw(), 1f);
        g2.leftMargin = a.dp(8);
        row.addView(presetTile(a, AppearanceStore.PRESET_GARDEN, "Garden", R.drawable.ic_alias_garden), g2);
        card.addView(row, mw());

        TextView customHead = sectionTitle(a, "Custom look", Color.WHITE);
        LinearLayout.LayoutParams ch = mw();
        ch.topMargin = a.dp(12);
        card.addView(customHead, ch);
        card.addView(muted(a, "Pick any PNG, JPEG, WebP, or SVG. It is cropped square and sized as an app icon. Give it a name, then Apply."), mw());

        final EditText name = new EditText(a);
        name.setHint("Custom name");
        name.setHintTextColor(0xFF6B7178);
        name.setTextColor(Color.WHITE);
        name.setTextSize(14);
        name.setSingleLine(true);
        name.setInputType(InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_FLAG_CAP_WORDS);
        name.setBackground(inputBg(a));
        name.setPadding(a.dp(12), a.dp(10), a.dp(12), a.dp(10));
        if (AppearanceStore.hasCustom(a)) name.setText(AppearanceStore.customName(a));
        LinearLayout.LayoutParams nlp = mw();
        nlp.topMargin = a.dp(8);
        card.addView(name, nlp);

        LinearLayout actions = new LinearLayout(a);
        actions.setOrientation(LinearLayout.HORIZONTAL);
        actions.setPadding(0, a.dp(8), 0, 0);
        actions.addView(smallBtn(a, "Gallery", new View.OnClickListener() {
            @Override public void onClick(View v) { a.pickCustomIcon(false); }
        }), new LinearLayout.LayoutParams(0, a.dp(42), 1f));
        LinearLayout.LayoutParams camLp = new LinearLayout.LayoutParams(0, a.dp(42), 1f);
        camLp.leftMargin = a.dp(8);
        actions.addView(smallBtn(a, "Camera", new View.OnClickListener() {
            @Override public void onClick(View v) { a.pickCustomIcon(true); }
        }), camLp);
        card.addView(actions, mw());

        LinearLayout applyRow = new LinearLayout(a);
        applyRow.setOrientation(LinearLayout.HORIZONTAL);
        applyRow.setPadding(0, a.dp(8), 0, 0);
        applyRow.addView(smallBtn(a, "Apply custom", new View.OnClickListener() {
            @Override public void onClick(View v) {
                a.applyPendingCustom(name.getText().toString());
            }
        }), new LinearLayout.LayoutParams(0, a.dp(42), 1f));
        if (AppearanceStore.hasCustom(a)) {
            LinearLayout.LayoutParams rlp = new LinearLayout.LayoutParams(0, a.dp(42), 1f);
            rlp.leftMargin = a.dp(8);
            applyRow.addView(smallBtn(a, "Remove custom", new View.OnClickListener() {
                @Override public void onClick(View v) {
                    AppearanceStore.removeCustom(a);
                    a.refreshChromeTitle();
                    a.setNotice("Custom look removed. ZeroNode icon restored.");
                    a.rebuildSettings();
                }
            }), rlp);
        }
        card.addView(applyRow, mw());
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
        if (drawable != 0) {
            icon.setImageResource(drawable);
        } else {
            icon.setImageResource(R.drawable.ic_alias_default);
            Icons.tint(icon, 0xFF00FF7F);
        }
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
                a.rebuildSettings();
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
        card.addView(muted(a, "If Tor is blocked on your network, turn a bridge on. obfs4 mimics HTTPS. Snowflake mimics a video call."), mw());

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
        sw.setOnToggle(new MainActivity.GreenSwitch.OnToggle() {
            @Override public void onToggle(boolean on) {
                if (on && BridgeStore.MODE_OFF.equals(BridgeStore.mode(a))) {
                    BridgeStore.setMode(a, BridgeStore.MODE_OBFS4);
                } else if (!on) {
                    BridgeStore.setMode(a, BridgeStore.MODE_OFF);
                }
                a.rebuildSettings();
            }
        });
        toggle.addView(sw, a.dp(42), a.dp(26));
        card.addView(toggle, tlp);

        LinearLayout modes = new LinearLayout(a);
        modes.setOrientation(LinearLayout.HORIZONTAL);
        LinearLayout.LayoutParams mlp = mw();
        mlp.topMargin = a.dp(8);
        modes.addView(modeChip(a, BridgeStore.MODE_OFF, "Direct"),
            new LinearLayout.LayoutParams(0, a.dp(38), 1f));
        LinearLayout.LayoutParams m1 = new LinearLayout.LayoutParams(0, a.dp(38), 1f);
        m1.leftMargin = a.dp(6);
        modes.addView(modeChip(a, BridgeStore.MODE_OBFS4, "obfs4"), m1);
        LinearLayout.LayoutParams m2 = new LinearLayout.LayoutParams(0, a.dp(38), 1f);
        m2.leftMargin = a.dp(6);
        modes.addView(modeChip(a, BridgeStore.MODE_SNOWFLAKE, "Snowflake"), m2);
        card.addView(modes, mlp);

        TextView blurb = muted(a, BridgeStore.maskBlurb(BridgeStore.mode(a)));
        blurb.setTextColor(0xFFC4C8CE);
        LinearLayout.LayoutParams blp = mw();
        blp.topMargin = a.dp(8);
        card.addView(blurb, blp);

        java.util.List<String> lines = BridgeStore.activeBridgeLines(a);
        if (!lines.isEmpty() && BridgeStore.enabled(a)) {
            TextView listHead = new TextView(a);
            listHead.setText("Active bridges");
            listHead.setTextColor(0xFFE8EAED);
            listHead.setTextSize(13);
            listHead.setPadding(0, a.dp(10), 0, a.dp(4));
            card.addView(listHead, mw());
            int show = Math.min(3, lines.size());
            for (int i = 0; i < show; i++) {
                TextView line = new TextView(a);
                line.setText(shortBridge(lines.get(i)));
                line.setTextColor(0xFF9AA3AD);
                line.setTextSize(11);
                line.setTypeface(Typeface.MONOSPACE);
                line.setPadding(0, a.dp(2), 0, a.dp(2));
                card.addView(line, mw());
            }
            if (lines.size() > show) {
                card.addView(muted(a, "+" + (lines.size() - show) + " more from the Tor bundle"), mw());
            }
        }

        final EditText custom = new EditText(a);
        custom.setHint("Paste extra Bridge lines (optional)");
        custom.setHintTextColor(0xFF6B7178);
        custom.setTextColor(0xFFDDDDDD);
        custom.setTextSize(12);
        custom.setTypeface(Typeface.MONOSPACE);
        custom.setMinLines(3);
        custom.setGravity(Gravity.TOP | Gravity.START);
        custom.setBackground(inputBg(a));
        custom.setPadding(a.dp(10), a.dp(8), a.dp(10), a.dp(8));
        custom.setText(BridgeStore.customLines(a));
        LinearLayout.LayoutParams clp = mw();
        clp.topMargin = a.dp(10);
        card.addView(custom, clp);
        LinearLayout.LayoutParams saveLp = mw(a.dp(42));
        saveLp.topMargin = a.dp(8);
        card.addView(smallBtn(a, "Save custom bridges", new View.OnClickListener() {
            @Override public void onClick(View v) {
                BridgeStore.setCustomLines(a, custom.getText().toString());
                a.setNotice("Bridge lines saved. Reconnect Tor to apply.");
            }
        }), saveLp);

        card.addView(linkRow(a, "Short guide", "obfs4 = fake HTTPS · Snowflake = fake video call. Request extra bridges from Tor if these are blocked."), mw());
        card.addView(linkBtn(a, "Tor bridges manual", BridgeStore.DOCS_URL), mw());
        card.addView(linkBtn(a, "Get bridges (bridges.torproject.org)", BridgeStore.BRIDGES_SITE), mw());
        card.addView(linkBtn(a, "Telegram · @GetBridgesBot", BridgeStore.TELEGRAM_BOT), mw());
        return card;
    }

    private static View modeChip(final MainActivity a, final String mode, String label) {
        TextView t = new TextView(a);
        t.setText(label);
        t.setGravity(Gravity.CENTER);
        t.setTextSize(13);
        t.setTypeface(Typeface.create("sans-serif-medium", Typeface.NORMAL));
        GradientDrawable bg = new GradientDrawable();
        bg.setCornerRadius(a.dp(10));
        boolean on = mode.equals(BridgeStore.mode(a));
        if (on) {
            bg.setColor(BridgeStore.MODE_OFF.equals(mode) ? 0xFF00FF7F : 0xFFA855F7);
            t.setTextColor(BridgeStore.MODE_OFF.equals(mode) ? Color.BLACK : Color.WHITE);
        } else {
            bg.setColor(0xFF1E2128);
            t.setTextColor(0xFFDDDDDD);
        }
        t.setBackground(bg);
        t.setClickable(true);
        t.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View v) {
                BridgeStore.setMode(a, mode);
                a.rebuildSettings();
            }
        });
        return t;
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

    private static String shortBridge(String line) {
        if (line == null) return "";
        String s = line;
        int cert = s.indexOf(" cert=");
        if (cert > 0) s = s.substring(0, cert);
        if (s.length() > 72) s = s.substring(0, 72) + "…";
        return s;
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
