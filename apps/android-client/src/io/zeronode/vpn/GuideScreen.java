package io.zeronode.vpn;

import android.graphics.Color;
import android.graphics.Typeface;
import android.graphics.drawable.GradientDrawable;
import android.view.Gravity;
import android.view.View;
import android.view.ViewGroup;
import android.widget.ImageView;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.TextView;

/** In-app guide with expandable sections and FAQs. */
final class GuideScreen {
    private GuideScreen() {}

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
            @Override public void onClick(View v) { a.closeGuide(); }
        });
        header.addView(back, a.dp(36), a.dp(36));
        ImageView ic = Icons.of(a, Icons.GUIDE, 0xFF00FF7F);
        LinearLayout.LayoutParams ilp = new LinearLayout.LayoutParams(a.dp(20), a.dp(20));
        ilp.leftMargin = a.dp(4);
        ilp.rightMargin = a.dp(8);
        header.addView(ic, ilp);
        TextView title = new TextView(a);
        title.setText("Guide");
        title.setTextColor(Color.WHITE);
        title.setTextSize(18);
        title.setTypeface(Typeface.create("sans-serif-medium", Typeface.NORMAL));
        header.addView(title, vw(), vw());
        page.addView(header, mw());

        ScrollView scroller = new ScrollView(a);
        scroller.setFillViewport(true);
        scroller.setVerticalScrollBarEnabled(false);
        LinearLayout body = new LinearLayout(a);
        body.setOrientation(LinearLayout.VERTICAL);
        body.setPadding(0, a.dp(8), 0, a.dp(24));

        addBlock(a, body, "What is ZeroNode?",
            "ZeroNode is a multi-protocol VPN for Android. Pick WireGuard, Outline (Shadowsocks), or Tor, add a connection, then tap Connect. The globe shows your exit country and IP after you refresh.");
        addBlock(a, body, "WireGuard",
            "Fast, modern VPN. Import a .conf file or paste the profile, give it a name, and Connect. Handshake uses your keys. The grey IP under a saved connection is the server address after DNS lookup — not your public IP.");
        addBlock(a, body, "Outline / Shadowsocks",
            "Paste an ss:// or Outline access key. Traffic is encrypted and looks like ordinary HTTPS to many networks. Use this when WireGuard is blocked but a Shadowsocks key still works.");
        addBlock(a, body, "Tor",
            "Routes the whole device through the official Tor expert bundle (arm64). Circuits take longer than a VPN. Your exit IP is a Tor relay, not a country you pick. If Tor is blocked, open Settings → Tor bridges.");
        addBlock(a, body, "Tor bridges (obfs4 & Snowflake)",
            "Bridges hide the fact that you are using Tor.\n\n"
                + "• Direct — connect to the public Tor network. Fastest, but easy to block.\n"
                + "• obfs4 — traffic looks like a random HTTPS site (TLS). Best first try on censored Wi‑Fi.\n"
                + "• Snowflake — traffic looks like a WebRTC video call via volunteers and a CDN.\n\n"
                + "Turn a bridge on in Settings or from the Tor tab, then reconnect Tor. Request extra lines from bridges.torproject.org or the Telegram bot @GetBridgesBot if the built-in set is blocked.\n\n"
                + "Full manual: https://tb-manual.torproject.org/bridges/");
        addBlock(a, body, "App protection",
            "Choose which apps use the VPN tunnel.\n\n"
                + "• Protect all apps — every app goes through the tunnel.\n"
                + "• Turn one app off — status becomes Some protected. That list is the current exclusive set.\n"
                + "• Protect-all off — every app is unprotected (None). It does not restore a previous Some list.\n\n"
                + "Browsers are grouped first, then other apps, both A–Z. Changes while connected need a reconnect.");
        addBlock(a, body, "App look (Weather / Garden / custom)",
            "Settings → App look can disguise ZeroNode on the home screen.\n\n"
                + "• Weather / Garden — real launcher icon and name (activity aliases).\n"
                + "• Custom — pick PNG, JPEG, WebP, or SVG from gallery or camera, crop to a square app icon, set a name, and pin a home-screen shortcut.\n"
                + "• Remove custom — deletes that look and restores ZeroNode.\n\n"
                + "The default ZeroNode icon stays as-is until you ship a branded mark.");
        addBlock(a, body, "Globe & Refresh IP",
            "Drag to orbit, pinch to zoom. The pin shows flag, IPv4, and city after Refresh IP. Location tags stay with a saved WireGuard connection.");
        addBlock(a, body, "FAQ",
            "Q: Connect is grey / fails?\nA: Add a profile first. For WireGuard the .conf must include Interface and Peer. Grant the VPN permission when Android asks.\n\n"
                + "Q: Tor is slow or stuck?\nA: First bootstrap can take a minute. Try obfs4, then Snowflake. Airplane mode off, then on, then reconnect.\n\n"
                + "Q: Per-app VPN did nothing?\nA: Disconnect, change the list, Connect again. The allowed-app set is locked when the tunnel is established.\n\n"
                + "Q: Icon did not change?\nA: Android sometimes keeps the old icon until you pull down the app drawer or reboot. Custom look is a shortcut — drag it to the home screen if asked.\n\n"
                + "Q: Is OpenVPN included?\nA: Not on Android. Use WireGuard, Outline, or Tor.");

        scroller.addView(body, mw());
        LinearLayout.LayoutParams slp = new LinearLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT, 0, 1f);
        slp.topMargin = a.dp(8);
        page.addView(scroller, slp);
        return page;
    }

    private static void addBlock(final MainActivity a, LinearLayout body, String title, String text) {
        final LinearLayout card = new LinearLayout(a);
        card.setOrientation(LinearLayout.VERTICAL);
        card.setPadding(a.dp(14), a.dp(12), a.dp(14), a.dp(12));
        GradientDrawable bg = new GradientDrawable();
        bg.setCornerRadius(a.dp(14));
        bg.setColor(0xFF12151A);
        bg.setStroke(a.dp(1), 0x22FFFFFF);
        card.setBackground(bg);

        final TextView head = new TextView(a);
        head.setText(title);
        head.setTextColor(Color.WHITE);
        head.setTextSize(15);
        head.setTypeface(Typeface.create("sans-serif-medium", Typeface.NORMAL));
        card.addView(head, mw());

        final TextView bodyText = new TextView(a);
        bodyText.setText(text);
        bodyText.setTextColor(0xFFB4B8BE);
        bodyText.setTextSize(13);
        bodyText.setLineSpacing(a.dp(2), 1.12f);
        bodyText.setPadding(0, a.dp(8), 0, 0);
        bodyText.setVisibility(View.GONE);
        card.addView(bodyText, mw());

        card.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View v) {
                boolean open = bodyText.getVisibility() != View.VISIBLE;
                bodyText.setVisibility(open ? View.VISIBLE : View.GONE);
            }
        });
        LinearLayout.LayoutParams lp = mw();
        lp.topMargin = a.dp(8);
        body.addView(card, lp);
    }

    private static int vw() { return ViewGroup.LayoutParams.WRAP_CONTENT; }

    private static LinearLayout.LayoutParams mw() {
        return new LinearLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
    }
}
