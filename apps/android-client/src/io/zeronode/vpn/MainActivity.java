package io.zeronode.vpn;

import android.app.Activity;
import android.app.AlertDialog;
import android.content.BroadcastReceiver;
import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.Context;
import android.content.DialogInterface;
import android.content.Intent;
import android.content.IntentFilter;
import android.content.SharedPreferences;

import android.content.ContentValues;
import android.animation.ObjectAnimator;
import android.animation.ValueAnimator;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.Color;
import android.graphics.LinearGradient;
import android.graphics.Paint;
import android.graphics.Path;
import android.graphics.RectF;
import android.graphics.Shader;
import android.graphics.Typeface;
import android.view.animation.AccelerateDecelerateInterpolator;
import android.provider.MediaStore;
import android.graphics.drawable.ColorDrawable;
import android.graphics.drawable.Drawable;
import android.graphics.drawable.GradientDrawable;
import android.net.ConnectivityManager;
import android.net.Network;
import android.net.NetworkCapabilities;
import android.net.Uri;
import android.net.VpnService;
import android.os.Build;
import android.os.Bundle;
import android.os.Handler;
import android.os.Looper;
import android.provider.OpenableColumns;
import android.text.InputType;
import android.text.TextUtils;
import android.util.Log;
import android.view.DragAndDropPermissions;
import android.view.DragEvent;
import android.view.Gravity;
import android.view.View;
import android.view.ViewGroup;
import android.view.Window;
import android.view.WindowManager;
import android.widget.Button;
import android.widget.EditText;
import android.widget.FrameLayout;
import android.widget.HorizontalScrollView;
import android.widget.ImageView;
import android.widget.LinearLayout;
import android.widget.PopupWindow;
import android.widget.ScrollView;
import android.widget.TextView;

import java.io.BufferedReader;
import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.io.InputStreamReader;
import java.io.OutputStream;
import java.net.HttpURLConnection;
import java.net.InetAddress;
import java.net.InetSocketAddress;
import java.net.Socket;
import java.net.URL;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.Collections;
import java.util.Comparator;
import java.util.HashMap;
import java.util.HashSet;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.Set;
import java.util.concurrent.atomic.AtomicInteger;

import io.zeronode.vpn.globe.GlobeView;

/**
 * Vertical mobile shell — Java UI + Rust native core.
 * Globe matches Windows (dark sphere + green borders). Tor keeps SOCKS across
 * VpnService re-attach. Profiles: import / drag-drop / scrollable / auth popup.
 */
public final class MainActivity extends Activity {
    private static final int VPN_REQUEST_CODE = 4207;
    private static final int IMPORT_REQUEST_CODE = 4208;
    private static final int PICK_ICON_REQUEST = 4210;
    private static final int PICK_CAMERA_REQUEST = 4211;
    private static final int STATUS_INTERVAL_MS = 5_000;
    private static final int PROGRESS_INTERVAL_MS = 250;
    private static final int PROGRESS_IDLE_INTERVAL_MS = 900;
    private static final String PREFS = "zeronode_profiles";

    private static final String[] PROTOCOLS = {
        "WireGuard", "Outline", "Tor"
    };
    private static final int[] PROTOCOL_ICONS = {
        Icons.WIREGUARD, Icons.OUTLINE, Icons.TOR
    };

    private final Handler handler = new Handler(Looper.getMainLooper());

    private TextView connectionPill;
    private TextView statusBanner;
    private TextView noticeText;
    private TextView progressLabel;
    private TextView progressPercent;
    private CyberProgressBar progressBar;
    private EditText hostInput;
    private EditText profileInput;
    private LinearLayout serverListContainer;
    private View dropOverlay;
    private TextView dropOverlayTitle;
    private TextView dropOverlayHint;
    private final AtomicInteger ipRefreshGen = new AtomicInteger(0);
    private FrameLayout rootFrame;
    private LinearLayout protocolBody;
    private GlobeView globeView;
    private Button primaryConnectBtn;
    private final List<View> protocolTabs = new ArrayList<>();
    private final List<TextView> protocolTabLabels = new ArrayList<>();
    private final List<ImageView> protocolTabIcons = new ArrayList<>();
    private ImageView lockIcon;
    private View lockGlow;
    private ObjectAnimator lockPulse;
    private boolean lockShowingLocked;
    private View settingsPage;
    private View guidePage;
    private View onboardingPage;
    private Bitmap pendingCustomIcon;
    private Uri cameraImageUri;
    private TextView profileDropdownLabel;
    private TextView profileDropdownSub;
    private TextView profileDropdownIp;
    private View profileDropdownSpin;
    private View profileDropdownRow;
    private View dropdownChevron;
    private PopupWindow profileMenuWindow;
    private EditText addDialogContent;
    private View refreshIpChip;
    private TextView refreshIpLabel;
    private View refreshIpSpin;
    private View progressCard;
    private TextView appSplitSummary;
    private GreenSwitch allAppsSwitch;
    private View appProtectPage;
    private LinearLayout appProtectList;
    private GreenSwitch protectAllSwitch;
    private TextView protectAllSub;
    private final Set<String> locatingProfileIds = new HashSet<String>();
    private boolean progressHideArmed;
    private final Runnable hideProgressAfterSuccess = new Runnable() {
        @Override
        public void run() {
            progressHideArmed = false;
            if (progressCard == null) return;
            if (displayProgress >= 0.995f
                && (vpnActive || "connected".equals(activePhase))) {
                progressCard.animate().alpha(0f).setDuration(280)
                    .withEndAction(new Runnable() {
                        @Override public void run() {
                            if (progressCard != null) progressCard.setVisibility(View.GONE);
                        }
                    }).start();
            }
        }
    };

    private int protocolIndex = 0;
    private List<ServerInfo> servers = new ArrayList<>();
    private String activeServerId;
    private String activePhase = "disconnected";
    private String publicIp = "—";
    private String publicIpV6 = "";
    private String publicCountry = "";
    private String publicCountryCode = "";
    private PendingVpn pendingVpn;
    private boolean torSocksUp;
    private boolean vpnActive;
    private boolean connecting;
    private int torSocksPort;
    private float displayProgress;
    private float targetProgress;
    private String importTarget = "wg";
    private String selectedProfileId = "";
    private String currentProfileKind = ProfileStore.KIND_WG;
    private boolean stateReceiverRegistered;

    private final BroadcastReceiver vpnStateReceiver = new BroadcastReceiver() {
        @Override
        public void onReceive(Context context, Intent intent) {
            if (intent == null) return;
            String state = intent.getStringExtra(ZeroNodeVpnService.EXTRA_STATE);
            String message = intent.getStringExtra(ZeroNodeVpnService.EXTRA_MESSAGE);
            applyExternalVpnState(state, message);
        }
    };

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        applyEdgeToEdge();
        // Paint the first frame immediately — no JNI, discovery, or IP on this path
        setContentView(buildLayout());
        IconChanger.apply(this, AppearanceStore.preset(this));
        if (!OnboardingScreen.isDone(this) && onboardingPage != null) {
            onboardingPage.setVisibility(View.VISIBLE);
        }
        restoreSavedProtocol();
        restorePendingVpnIfAny();
        handleIncomingShare(getIntent());
        if (rootFrame != null) {
            rootFrame.post(new Runnable() {
                @Override
                public void run() {
                    scheduleStatusPoll();
                    scheduleProgressPoll();
                    syncFromService();
                    handler.postDelayed(new Runnable() {
                        @Override
                        public void run() {
                            NativeBridge.ensureProtectBridge();
                            runDiscovery(false);
                            refreshPublicIp(true);
                            AppCatalog.load(MainActivity.this, null);
                        }
                    }, 48);
                }
            });
        }
        new Thread(new Runnable() {
            @Override
            public void run() {
                try {
                    TorBundle.ensureExtracted(MainActivity.this);
                } catch (Exception ignored) {
                }
            }
        }, "zn-tor-extract").start();
    }

    /** Transparent status/nav bars so the globe is seamless (no top black bar). */
    private void applyEdgeToEdge() {
        Window w = getWindow();
        if (w == null) return;
        w.clearFlags(WindowManager.LayoutParams.FLAG_TRANSLUCENT_STATUS);
        w.addFlags(WindowManager.LayoutParams.FLAG_DRAWS_SYSTEM_BAR_BACKGROUNDS);
        w.setStatusBarColor(Color.TRANSPARENT);
        w.setNavigationBarColor(Color.TRANSPARENT);
        // Prefer the panel's peak refresh rate (90/120/144 Hz) — default can stick at 60
        if (Build.VERSION.SDK_INT >= 23) {
            try {
                android.view.Display display = w.getWindowManager().getDefaultDisplay();
                if (display != null) {
                    float maxHz = display.getRefreshRate();
                    if (Build.VERSION.SDK_INT >= 30) {
                        android.view.Display.Mode[] modes = display.getSupportedModes();
                        android.view.Display.Mode best = display.getMode();
                        if (modes != null) {
                            for (android.view.Display.Mode m : modes) {
                                if (m.getRefreshRate() > maxHz
                                    && m.getPhysicalWidth() >= best.getPhysicalWidth()) {
                                    maxHz = m.getRefreshRate();
                                    best = m;
                                }
                            }
                            WindowManager.LayoutParams lp = w.getAttributes();
                            lp.preferredDisplayModeId = best.getModeId();
                            lp.preferredRefreshRate = maxHz;
                            w.setAttributes(lp);
                        }
                    }
                }
            } catch (Exception ignored) {
            }
        }
        View decor = w.getDecorView();
        int flags = View.SYSTEM_UI_FLAG_LAYOUT_STABLE
            | View.SYSTEM_UI_FLAG_LAYOUT_FULLSCREEN
            | View.SYSTEM_UI_FLAG_LAYOUT_HIDE_NAVIGATION;
        decor.setSystemUiVisibility(flags);
        if (Build.VERSION.SDK_INT >= 28) {
            WindowManager.LayoutParams lp = w.getAttributes();
            lp.layoutInDisplayCutoutMode =
                WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES;
            w.setAttributes(lp);
        }
    }

    @Override
    protected void onStart() {
        super.onStart();
        if (!stateReceiverRegistered) {
            IntentFilter filter = new IntentFilter(ZeroNodeVpnService.ACTION_STATE);
            if (Build.VERSION.SDK_INT >= 33) {
                registerReceiver(vpnStateReceiver, filter, Context.RECEIVER_NOT_EXPORTED);
            } else {
                registerReceiver(vpnStateReceiver, filter);
            }
            stateReceiverRegistered = true;
        }
        syncFromService();
    }

    @Override
    protected void onStop() {
        if (stateReceiverRegistered) {
            try {
                unregisterReceiver(vpnStateReceiver);
            } catch (Exception ignored) {
            }
            stateReceiverRegistered = false;
        }
        super.onStop();
    }

    @Override
    protected void onResume() {
        super.onResume();
        syncFromService();
    }

    @Override
    public void onBackPressed() {
        if (onboardingPage != null && onboardingPage.getVisibility() == View.VISIBLE) {
            return;
        }
        if (guidePage != null && guidePage.getVisibility() == View.VISIBLE) {
            closeGuide();
            return;
        }
        if (settingsPage != null && settingsPage.getVisibility() == View.VISIBLE) {
            closeSettings();
            return;
        }
        if (appProtectPage != null && appProtectPage.getVisibility() == View.VISIBLE) {
            closeAppProtectPage();
            return;
        }
        super.onBackPressed();
    }

    @Override
    protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        setIntent(intent);
        handleIncomingShare(intent);
    }

    @Override
    protected void onDestroy() {
        handler.removeCallbacksAndMessages(null);
        super.onDestroy();
    }

    /** Keep UI honest after notification disconnect / process restore. */
    private void syncFromService() {
        boolean up = ZeroNodeVpnService.isRunning();
        String st = ZeroNodeVpnService.lastStatus();
        if (up || (st != null && st.startsWith("OK"))) {
            if (up) {
                vpnActive = true;
                connecting = false;
                activePhase = "connected";
                targetProgress = 1f;
                updateConnectionPill("connected",
                    pendingVpn != null ? pendingVpn.session : ZeroNodeVpnService.lastKind());
            }
        } else if (!connecting && !torSocksUp) {
            if (vpnActive || "connected".equals(activePhase)) {
                vpnActive = false;
                activePhase = "disconnected";
                targetProgress = 0f;
                updateConnectionPill("disconnected", null);
                setProgressUi("idle", 0f, "Idle");
            }
        }
        updatePrimaryButton();
    }

    private void applyExternalVpnState(String state, String message) {
        if (state == null) return;
        if ("disconnected".equals(state)) {
            connecting = false;
            vpnActive = false;
            torSocksUp = false;
            torSocksPort = 0;
            pendingVpn = null;
            activePhase = "disconnected";
            targetProgress = 0f;
            displayProgress = 0f;
            updateConnectionPill("disconnected", null);
            setProgressUi("idle", 0f, "Idle");
            cancelProgressHide();
            setNotice(message != null && message.length() > 0 ? message : "Disconnected.");
            updatePrimaryButton();
            applyProgressDisplay();
            refreshPublicIp(true);
        } else if ("connected".equals(state)) {
            connecting = false;
            vpnActive = true;
            activePhase = "connected";
            targetProgress = 1f;
            updateConnectionPill("connected",
                pendingVpn != null ? pendingVpn.session : message);
            setProgressUi(message != null ? message : "vpn", 1f, "active");
            updatePrimaryButton();
            setNotice(null);
            armProgressHide();
            scheduleTunnelIpRefresh();
        } else if ("error".equals(state)) {
            connecting = false;
            vpnActive = false;
            activePhase = "error";
            targetProgress = 0f;
            updateConnectionPill("error", null);
            setProgressUi("error", 0f, message);
            setNotice(message != null ? message : "VPN error");
            updatePrimaryButton();
        } else if ("connecting".equals(state)) {
            connecting = true;
            activePhase = "connecting";
            showProgressCard();
            updateConnectionPill("connecting", message);
            updatePrimaryButton();
        }
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        super.onActivityResult(requestCode, resultCode, data);
        if (requestCode == IMPORT_REQUEST_CODE && resultCode == RESULT_OK && data != null) {
            Uri uri = data.getData();
            if (uri != null) readImportedFile(uri);
            return;
        }
        if ((requestCode == PICK_ICON_REQUEST || requestCode == PICK_CAMERA_REQUEST)
            && resultCode == RESULT_OK) {
            Uri uri = data != null ? data.getData() : cameraImageUri;
            if (uri == null) uri = cameraImageUri;
            if (uri == null) {
                setNotice("No image returned.");
                return;
            }
            IconFactory.fromUri(this, uri, new IconFactory.Ready() {
                @Override public void onReady(Bitmap bmp) {
                    pendingCustomIcon = bmp;
                    setNotice("Image ready — enter a name and tap Apply custom.");
                }
                @Override public void onError(String message) {
                    setNotice(message);
                }
            });
            return;
        }
        if (requestCode == VPN_REQUEST_CODE) {
            if (resultCode == RESULT_OK) {
                if (pendingVpn == null) {
                    restorePendingVpnIfAny();
                }
                if (pendingVpn != null) {
                    startPendingVpnService();
                } else {
                    connecting = false;
                    setNotice("VPN permission granted, but no pending profile. Tap Connect again.");
                    activePhase = "error";
                    updateConnectionPill("error", null);
                    updatePrimaryButton();
                }
            } else {
                connecting = false;
                clearPendingVpnPersist();
                setNotice("VPN permission denied.");
                activePhase = "error";
                targetProgress = 0f;
                updateConnectionPill("error", null);
                updatePrimaryButton();
            }
        }
    }

    private void startPendingVpnService() {
        if (pendingVpn == null) {
            restorePendingVpnIfAny();
        }
        if (pendingVpn == null) {
            connecting = false;
            setNotice("Nothing to connect.");
            return;
        }
        persistPendingVpn(pendingVpn);
        Intent intent = ZeroNodeVpnService.startIntent(
            this,
            pendingVpn.kind,
            pendingVpn.session,
            pendingVpn.clientAddress,
            pendingVpn.dns,
            pendingVpn.profile,
            pendingVpn.host,
            pendingVpn.port,
            pendingVpn.user,
            pendingVpn.password,
            pendingVpn.method,
            pendingVpn.extra
        );
        try {
            if (Build.VERSION.SDK_INT >= 26) {
                startForegroundService(intent);
            } else {
                startService(intent);
            }
        } catch (Exception e) {
            // Fallback for some OEMs
            try {
                startService(intent);
            } catch (Exception e2) {
                connecting = false;
                setNotice("Could not start VPN service: " + e2.getMessage());
                activePhase = "error";
                updateConnectionPill("error", null);
                updatePrimaryButton();
                return;
            }
        }
        vpnActive = true;
        connecting = true;
        activePhase = "connecting";
        updateConnectionPill("connecting", pendingVpn.session);
        targetProgress = Math.max(targetProgress, 0.55f);
        setNotice("VPN starting (" + pendingVpn.kind + ")…");
        updatePrimaryButton();
        handler.postDelayed(new Runnable() {
            @Override
            public void run() {
                checkServiceResult();
            }
        }, 1800);
        // Auto-refresh exit IP after tunnel is likely ready (manual button uses same path)
        scheduleTunnelIpRefresh();
    }

    /** Multiple delayed refreshes so WireGuard exit IP settles. */
    private void scheduleTunnelIpRefresh() {
        int[] delaysMs = new int[]{1800, 4000, 8000};
        for (final int delay : delaysMs) {
            handler.postDelayed(new Runnable() {
                @Override
                public void run() {
                    if (vpnActive || ZeroNodeVpnService.isRunning() || torSocksUp) {
                        refreshPublicIp(true);
                    }
                }
            }, delay);
        }
    }

    private void checkServiceResult() {
        String st = ZeroNodeVpnService.lastStatus();
        if (st != null && st.startsWith("ERR")) {
            connecting = false;
            vpnActive = false;
            activePhase = "error";
            Map<String, String> kv = parseKV(st);
            String msg = kv.get("message") != null ? kv.get("message") : st;
            setNotice("VPN failed: " + msg);
            setProgressUi("error", 0f, msg);
            updateConnectionPill("error", null);
            updatePrimaryButton();
        } else if (ZeroNodeVpnService.isRunning() || (st != null && st.startsWith("OK"))) {
            connecting = false;
            vpnActive = true;
            activePhase = "connected";
            targetProgress = 1f;
            setProgressUi(pendingVpn != null ? pendingVpn.kind : "vpn", 1f, "active");
            updateConnectionPill("connected", pendingVpn != null ? pendingVpn.session : null);
            updatePrimaryButton();
            setNotice(null);
        } else if (connecting) {
            // Still coming up — check again
            handler.postDelayed(new Runnable() {
                @Override
                public void run() {
                    checkServiceResult();
                }
            }, 1500);
        }
    }

    // ─── Layout ───────────────────────────────────────────────────────────

    private View buildLayout() {
        rootFrame = new FrameLayout(this);
        rootFrame.setBackgroundColor(Color.BLACK);

        ScrollView scroll = new ScrollView(this);
        scroll.setFillViewport(true);
        scroll.setBackgroundColor(Color.BLACK);
        scroll.setOverScrollMode(View.OVER_SCROLL_NEVER);
        // Professional apps hide scrollbars — visible bars read as janky/desktop
        scroll.setVerticalScrollBarEnabled(false);
        scroll.setHorizontalScrollBarEnabled(false);
        scroll.setScrollbarFadingEnabled(true);
        try {
            scroll.setSmoothScrollingEnabled(true);
        } catch (Exception ignored) {
        }
        LinearLayout root = new LinearLayout(this);
        root.setOrientation(LinearLayout.VERTICAL);
        root.setPadding(0, 0, 0, dp(24) + statusBarInsetPx() / 2);
        scroll.addView(root, mm());

        root.addView(buildGlobeSection());
        root.addView(statusBanner = banner());
        root.addView(buildProtocolCard());
        root.addView(buildAppSplitCard());
        root.addView(buildNodesCard());
        root.addView(buildBottomActions());

        rootFrame.addView(scroll, new FrameLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT
        ));
        rootFrame.addView(buildDropOverlay(), new FrameLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT
        ));
        rootFrame.addView(buildAppProtectPage(), new FrameLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT
        ));
        settingsPage = SettingsScreen.build(this);
        rootFrame.addView(settingsPage, new FrameLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT
        ));
        guidePage = GuideScreen.build(this);
        rootFrame.addView(guidePage, new FrameLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT
        ));
        onboardingPage = OnboardingScreen.build(this);
        rootFrame.addView(onboardingPage, new FrameLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT
        ));

        // Full-screen drag target (overlay + content)
        installDropTarget(rootFrame);
        return rootFrame;
    }

    /**
     * Full-screen "Drop files here" layer — shown only while the user is
     * dragging .conf / ss:// keys (file dock / multi-window), like a web drop zone.
     */
    private View buildDropOverlay() {
        LinearLayout overlay = new LinearLayout(this);
        overlay.setOrientation(LinearLayout.VERTICAL);
        overlay.setGravity(Gravity.CENTER);
        overlay.setPadding(dp(28), dp(28), dp(28), dp(28));
        overlay.setVisibility(View.GONE);
        overlay.setClickable(true);
        overlay.setFocusable(false);

        GradientDrawable bg = new GradientDrawable();
        bg.setColor(0xE6080A0C);
        overlay.setBackground(bg);

        LinearLayout card = new LinearLayout(this);
        card.setOrientation(LinearLayout.VERTICAL);
        card.setGravity(Gravity.CENTER);
        card.setPadding(dp(28), dp(36), dp(28), dp(36));
        GradientDrawable cardBg = new GradientDrawable();
        cardBg.setCornerRadius(dp(20));
        cardBg.setColor(0xFF12151A);
        cardBg.setStroke(dp(2), 0xFF00FF7F);
        card.setBackground(cardBg);

        TextView icon = new TextView(this);
        icon.setText("⬇");
        icon.setTextSize(42);
        icon.setGravity(Gravity.CENTER);
        icon.setTextColor(0xFF00FF7F);
        card.addView(icon, mw());

        dropOverlayTitle = new TextView(this);
        dropOverlayTitle.setText("Drop VPN profile here");
        dropOverlayTitle.setTextColor(Color.WHITE);
        dropOverlayTitle.setTextSize(20);
        dropOverlayTitle.setTypeface(null, Typeface.BOLD);
        dropOverlayTitle.setGravity(Gravity.CENTER);
        LinearLayout.LayoutParams tlp = mw();
        tlp.topMargin = dp(12);
        card.addView(dropOverlayTitle, tlp);

        dropOverlayHint = new TextView(this);
        dropOverlayHint.setText("Accepts WireGuard .conf and Outline keys");
        dropOverlayHint.setTextColor(0xFFAAAAAA);
        dropOverlayHint.setTextSize(14);
        dropOverlayHint.setGravity(Gravity.CENTER);
        LinearLayout.LayoutParams hlp = mw();
        hlp.topMargin = dp(10);
        card.addView(dropOverlayHint, hlp);

        LinearLayout.LayoutParams clp = new LinearLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT
        );
        clp.leftMargin = dp(16);
        clp.rightMargin = dp(16);
        overlay.addView(card, clp);
        dropOverlay = overlay;
        return overlay;
    }

    private View hairline() {
        View v = new View(this);
        v.setBackgroundColor(0x22FFFFFF);
        LinearLayout.LayoutParams lp = new LinearLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT, Math.max(1, dp(1) / 2)
        );
        lp.leftMargin = dp(16);
        lp.rightMargin = dp(16);
        v.setLayoutParams(lp);
        return v;
    }

    private View buildHeader() {
        LinearLayout header = new LinearLayout(this);
        header.setOrientation(LinearLayout.HORIZONTAL);
        header.setGravity(Gravity.CENTER_VERTICAL);
        // Compact chrome under globe — no extra status-bar black strip
        header.setPadding(dp(16), dp(8), dp(16), dp(6));
        header.setBackgroundColor(Color.TRANSPARENT);

        LinearLayout titles = new LinearLayout(this);
        titles.setOrientation(LinearLayout.VERTICAL);
        TextView title = new TextView(this);
        title.setText(R.string.app_name);
        title.setTextColor(Color.WHITE);
        title.setTextSize(20);
        title.setTypeface(null, Typeface.BOLD);
        titles.addView(title, mw());
        TextView sub = new TextView(this);
        sub.setText(R.string.subtitle);
        sub.setTextColor(0xFF888888);
        sub.setTextSize(11);
        titles.addView(sub, mw());
        header.addView(titles, new LinearLayout.LayoutParams(0, vw(), 1f));

        connectionPill = new TextView(this);
        connectionPill.setTextSize(10);
        connectionPill.setTypeface(Typeface.MONOSPACE);
        connectionPill.setPadding(dp(10), dp(8), dp(10), dp(8));
        connectionPill.setGravity(Gravity.CENTER);
        updateConnectionPill("disconnected", null);
        header.addView(connectionPill, vw(), vw());
        return header;
    }

    int statusBarInsetPx() {
        int resId = getResources().getIdentifier("status_bar_height", "dimen", "android");
        if (resId > 0) {
            try {
                return getResources().getDimensionPixelSize(resId);
            } catch (Exception ignored) {
            }
        }
        return dp(24);
    }

    private TextView banner() {
        TextView t = new TextView(this);
        t.setTextColor(Color.WHITE);
        t.setTextSize(12);
        t.setPadding(dp(16), dp(8), dp(16), dp(8));
        t.setBackgroundColor(0xFF141414);
        t.setVisibility(View.GONE);
        return t;
    }

    /** Full-bleed globe with floating chrome — flag + IP live on the pin. */
    private View buildGlobeSection() {
        FrameLayout wrap = new FrameLayout(this);
        wrap.setBackgroundColor(Color.BLACK);
        int globeH = dp(428) + statusBarInsetPx();
        LinearLayout.LayoutParams wlp = new LinearLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT, globeH
        );
        wrap.setLayoutParams(wlp);

        globeView = new GlobeView(this);
        wrap.addView(globeView, new FrameLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT,
            ViewGroup.LayoutParams.MATCH_PARENT
        ));

        wrap.addView(buildLockBadge(), lockChromeParams());

        LinearLayout dock = new LinearLayout(this);
        dock.setOrientation(LinearLayout.HORIZONTAL);
        dock.setGravity(Gravity.CENTER_VERTICAL);
        dock.setPadding(dp(12), 0, dp(12), dp(12));
        dock.addView(buildRefreshIpChip(), vw(), vw());
        progressCard = buildProgressHover();
        LinearLayout.LayoutParams plp = new LinearLayout.LayoutParams(0, dp(28), 1f);
        plp.leftMargin = dp(8);
        dock.addView(progressCard, plp);
        FrameLayout.LayoutParams dlp = new FrameLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT
        );
        dlp.gravity = Gravity.BOTTOM;
        wrap.addView(dock, dlp);
        updateConnectionPill("disconnected", null);
        return wrap;
    }

    private FrameLayout.LayoutParams lockChromeParams() {
        FrameLayout.LayoutParams lp = new FrameLayout.LayoutParams(dp(52), dp(52));
        lp.gravity = Gravity.TOP | Gravity.END;
        lp.topMargin = statusBarInsetPx() + dp(4);
        lp.rightMargin = dp(8);
        return lp;
    }

    private View buildLockBadge() {
        FrameLayout badge = new FrameLayout(this);
        lockGlow = new View(this);
        GradientDrawable glow = new GradientDrawable();
        glow.setShape(GradientDrawable.OVAL);
        glow.setColor(0x6600FF7F);
        lockGlow.setBackground(glow);
        lockGlow.setAlpha(0.28f);
        badge.addView(lockGlow, new FrameLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT
        ));
        lockIcon = Icons.of(this, Icons.UNLOCK, 0xFF00FF7F);
        lockIcon.setContentDescription("Disconnected");
        FrameLayout.LayoutParams ilp = new FrameLayout.LayoutParams(dp(26), dp(26));
        ilp.gravity = Gravity.CENTER;
        badge.addView(lockIcon, ilp);
        return badge;
    }

    private View buildRefreshIpChip() {
        LinearLayout chip = new LinearLayout(this);
        chip.setOrientation(LinearLayout.HORIZONTAL);
        chip.setGravity(Gravity.CENTER);
        chip.setPadding(dp(10), dp(5), dp(10), dp(5));
        chip.setMinimumHeight(dp(26));
        GradientDrawable bg = new GradientDrawable();
        bg.setCornerRadius(dp(14));
        bg.setColor(0xCC14181E);
        bg.setStroke(dp(1), 0x3300FF7F);
        chip.setBackground(bg);
        chip.setClickable(true);
        chip.setFocusable(true);
        chip.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View v) { refreshPublicIp(true); }
        });
        refreshIpSpin = new RoundSpinnerView(this);
        refreshIpSpin.setVisibility(View.GONE);
        LinearLayout.LayoutParams slp = new LinearLayout.LayoutParams(dp(12), dp(12));
        slp.rightMargin = dp(6);
        chip.addView(refreshIpSpin, slp);
        refreshIpLabel = new TextView(this);
        refreshIpLabel.setText("Refresh IP");
        refreshIpLabel.setTextColor(0xFFE8E8E8);
        refreshIpLabel.setTextSize(10);
        refreshIpLabel.setTypeface(Typeface.create("sans-serif-medium", Typeface.NORMAL));
        refreshIpLabel.setIncludeFontPadding(false);
        refreshIpLabel.setSingleLine(true);
        chip.addView(refreshIpLabel, vw(), vw());
        refreshIpChip = chip;
        return chip;
    }

    private void setRefreshLoading(boolean on) {
        if (refreshIpSpin != null) {
            refreshIpSpin.setVisibility(on ? View.VISIBLE : View.GONE);
        }
    }

    private Button compactChip(String text, View.OnClickListener l) {
        Button b = new Button(this);
        styleButton(b, text, 0xCC14181E, 0xFFE8E8E8);
        b.setTextSize(11);
        b.setOnClickListener(l);
        GradientDrawable bg = new GradientDrawable();
        bg.setCornerRadius(dp(17));
        bg.setColor(0xCC14181E);
        bg.setStroke(dp(1), 0x3300FF7F);
        b.setBackground(bg);
        return b;
    }

    private View buildProgressHover() {
        LinearLayout hover = new LinearLayout(this);
        hover.setOrientation(LinearLayout.HORIZONTAL);
        hover.setGravity(Gravity.CENTER_VERTICAL);
        hover.setPadding(dp(10), dp(5), dp(8), dp(5));
        GradientDrawable bg = new GradientDrawable();
        bg.setCornerRadius(dp(14));
        bg.setColor(0x9910181C);
        bg.setStroke(dp(1), 0x3300FF7F);
        hover.setBackground(bg);
        hover.setVisibility(View.GONE);
        hover.setAlpha(0f);

        progressBar = new CyberProgressBar(this);
        LinearLayout.LayoutParams blp = new LinearLayout.LayoutParams(0, dp(8), 1f);
        hover.addView(progressBar, blp);

        progressPercent = new TextView(this);
        progressPercent.setText("0%");
        progressPercent.setTextColor(0xFF00FF7F);
        progressPercent.setTypeface(Typeface.MONOSPACE);
        progressPercent.setTextSize(10);
        progressPercent.setIncludeFontPadding(false);
        progressPercent.setGravity(Gravity.END | Gravity.CENTER_VERTICAL);
        progressPercent.setMinWidth(dp(32));
        LinearLayout.LayoutParams pctLp = new LinearLayout.LayoutParams(vw(), vw());
        pctLp.leftMargin = dp(8);
        hover.addView(progressPercent, pctLp);

        progressLabel = new TextView(this);
        progressLabel.setVisibility(View.GONE);
        return hover;
    }

    private View buildProtocolCard() {
        LinearLayout card = softSection();
        card.addView(sectionTitle(getString(R.string.label_protocol), 0xFF00FF7F), mw());

        LinearLayout tabs = new LinearLayout(this);
        tabs.setOrientation(LinearLayout.HORIZONTAL);
        tabs.setPadding(0, dp(8), 0, dp(2));
        tabs.setClipChildren(false);
        tabs.setClipToPadding(false);
        for (int i = 0; i < PROTOCOLS.length; i++) {
            final int idx = i;
            LinearLayout tab = new LinearLayout(this);
            tab.setOrientation(LinearLayout.HORIZONTAL);
            tab.setGravity(Gravity.CENTER);
            tab.setPadding(dp(8), dp(6), dp(8), dp(6));
            tab.setClickable(true);
            tab.setFocusable(true);
            int iconKind = PROTOCOL_ICONS[i];
            int iconColor = i == 2 ? 0xFFC084FC : 0xFF00FF7F;
            ImageView icon = Icons.of(this, iconKind, iconColor);
            LinearLayout.LayoutParams ilp = new LinearLayout.LayoutParams(dp(14), dp(14));
            ilp.rightMargin = dp(6);
            tab.addView(icon, ilp);
            TextView label = new TextView(this);
            label.setText(PROTOCOLS[i]);
            label.setTextSize(12);
            label.setGravity(Gravity.CENTER);
            label.setSingleLine(true);
            label.setIncludeFontPadding(false);
            label.setTypeface(Typeface.create("sans-serif-medium", Typeface.NORMAL));
            tab.addView(label, vw(), vw());
            tab.setOnClickListener(new View.OnClickListener() {
                @Override public void onClick(View v) { selectProtocol(idx); }
            });
            protocolTabs.add(tab);
            protocolTabLabels.add(label);
            protocolTabIcons.add(icon);
            LinearLayout.LayoutParams lp = new LinearLayout.LayoutParams(0, dp(36), 1f);
            if (i > 0) lp.leftMargin = dp(6);
            tabs.addView(tab, lp);
        }
        card.addView(tabs, mw());

        protocolBody = new LinearLayout(this);
        protocolBody.setOrientation(LinearLayout.VERTICAL);
        card.addView(protocolBody, mw());
        selectProtocol(protocolIndex);
        return card;
    }

    private void selectProtocol(int idx) {
        saveCurrentFields();
        protocolIndex = idx;
        prefs().edit().putInt("protocol_index", idx).apply();
        for (int i = 0; i < protocolTabs.size(); i++) {
            View tab = protocolTabs.get(i);
            TextView label = protocolTabLabels.get(i);
            GradientDrawable bg = new GradientDrawable();
            bg.setCornerRadius(dp(8));
            if (i == idx) {
                boolean tor = i == 2;
                bg.setColor(tor ? 0xFFA855F7 : 0xFF00FF7F);
                label.setTextColor(tor ? Color.WHITE : Color.BLACK);
                if (i < protocolTabIcons.size()) {
                    Icons.tint(protocolTabIcons.get(i), tor ? Color.WHITE : Color.BLACK);
                }
            } else {
                bg.setColor(0xFF1E2128);
                label.setTextColor(0xFFDDDDDD);
                if (i < protocolTabIcons.size()) {
                    Icons.tint(protocolTabIcons.get(i), i == 2 ? 0xFFC084FC : 0xFF00FF7F);
                }
            }
            tab.setBackground(bg);
        }
        protocolBody.removeAllViews();
        profileInput = null;
        switch (idx) {
            case 0: fillWireGuardBody(); break;
            case 1: fillOutlineBody(); break;
            case 2: fillTorBody(); break;
            default: fillWireGuardBody(); break;
        }
        updatePrimaryButton();
        if (progressBar != null) progressBar.setAccent(idx == 2);
        updateConnectionPill(activePhase, null);
    }

    private void fillWireGuardBody() {
        ensureHiddenProfileInput(prefs().getString("wg_profile", ""));
        addProfileDropdown(ProfileStore.KIND_WG);
        addPrimaryConnect("Connect");
    }

    private void fillOutlineBody() {
        ensureHiddenProfileInput(prefs().getString("outline_key", ""));
        addProfileDropdown(ProfileStore.KIND_OUTLINE);
        addPrimaryConnect("Connect");
    }

    private void ensureHiddenProfileInput(String initial) {
        profileInput = new EditText(this);
        profileInput.setVisibility(View.GONE);
        if (initial != null) profileInput.setText(initial);
        protocolBody.addView(profileInput, 0, 0);
    }

    private void addProfileDropdown(final String kind) {
        currentProfileKind = kind;
        LinearLayout row = new LinearLayout(this);
        row.setOrientation(LinearLayout.HORIZONTAL);
        row.setGravity(Gravity.CENTER_VERTICAL);
        LinearLayout.LayoutParams rowLp = mw();
        rowLp.topMargin = dp(10);
        row.setLayoutParams(rowLp);

        LinearLayout drop = new LinearLayout(this);
        drop.setOrientation(LinearLayout.HORIZONTAL);
        drop.setGravity(Gravity.CENTER_VERTICAL);
        drop.setPadding(dp(14), dp(12), dp(12), dp(12));
        GradientDrawable bg = new GradientDrawable();
        bg.setCornerRadius(dp(12));
        bg.setColor(0xFF14181E);
        bg.setStroke(dp(1), 0x2AFFFFFF);
        drop.setBackground(bg);
        drop.setClickable(true);
        drop.setFocusable(true);

        LinearLayout texts = new LinearLayout(this);
        texts.setOrientation(LinearLayout.VERTICAL);
        texts.setGravity(Gravity.CENTER_VERTICAL);

        profileDropdownLabel = new TextView(this);
        profileDropdownLabel.setTextColor(Color.WHITE);
        profileDropdownLabel.setTextSize(14);
        profileDropdownLabel.setTypeface(Typeface.create("sans-serif-medium", Typeface.NORMAL));
        profileDropdownLabel.setSingleLine(true);
        profileDropdownLabel.setEllipsize(TextUtils.TruncateAt.END);
        texts.addView(profileDropdownLabel, mw());

        LinearLayout subRow = new LinearLayout(this);
        subRow.setOrientation(LinearLayout.HORIZONTAL);
        subRow.setGravity(Gravity.CENTER_VERTICAL);
        profileDropdownSpin = new RoundSpinnerView(this);
        profileDropdownSpin.setVisibility(View.GONE);
        LinearLayout.LayoutParams spinLp = new LinearLayout.LayoutParams(dp(12), dp(12));
        spinLp.rightMargin = dp(6);
        subRow.addView(profileDropdownSpin, spinLp);
        profileDropdownSub = new TextView(this);
        profileDropdownSub.setTextColor(0xFF8A9098);
        profileDropdownSub.setTextSize(11);
        profileDropdownSub.setSingleLine(true);
        profileDropdownSub.setEllipsize(TextUtils.TruncateAt.END);
        subRow.addView(profileDropdownSub, new LinearLayout.LayoutParams(0, vw(), 1f));
        texts.addView(subRow, mw());

        profileDropdownIp = new TextView(this);
        profileDropdownIp.setTextColor(0xFF6B7178);
        profileDropdownIp.setTextSize(10);
        profileDropdownIp.setTypeface(Typeface.MONOSPACE);
        profileDropdownIp.setSingleLine(true);
        profileDropdownIp.setEllipsize(TextUtils.TruncateAt.MIDDLE);
        profileDropdownIp.setVisibility(View.GONE);
        texts.addView(profileDropdownIp, mw());

        drop.addView(texts, new LinearLayout.LayoutParams(0, vw(), 1f));

        dropdownChevron = slimChevronView();
        LinearLayout.LayoutParams chevLp = new LinearLayout.LayoutParams(dp(18), dp(18));
        chevLp.leftMargin = dp(8);
        drop.addView(dropdownChevron, chevLp);

        drop.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View v) { showProfileMenu(drop, kind); }
        });
        profileDropdownRow = drop;
        row.addView(drop, new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f));

        Button add = accentButton("+");
        add.setTextSize(20);
        add.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View v) { showAddConnectionDialog(kind); }
        });
        LinearLayout.LayoutParams addLp = new LinearLayout.LayoutParams(dp(48), dp(48));
        addLp.leftMargin = dp(8);
        row.addView(add, addLp);
        protocolBody.addView(row, rowLp);
        restoreSelectedProfile(kind);
        refreshProfileDropdownLabel(kind);
    }

    private void restoreSelectedProfile(String kind) {
        String savedId = prefs().getString("selected_" + kind, selectedProfileId);
        ProfileStore.Profile pick = null;
        if (savedId != null && savedId.length() > 0) {
            ProfileStore.Profile p = ProfileStore.get(this, savedId);
            if (p != null && kind.equals(p.kind)) pick = p;
        }
        if (pick == null) {
            java.util.List<ProfileStore.Profile> list = ProfileStore.list(this, kind);
            if (!list.isEmpty()) pick = list.get(0);
        }
        if (pick != null) {
            applySavedProfile(pick, false);
        } else {
            selectedProfileId = "";
        }
    }

    private void refreshProfileDropdownLabel(String kind) {
        if (profileDropdownLabel == null) return;
        ProfileStore.Profile p = ProfileStore.get(this, selectedProfileId);
        if (p != null && kind.equals(p.kind)) {
            profileDropdownLabel.setText(profileTitle(p));
            profileDropdownLabel.setTextColor(Color.WHITE);
            bindProfileMeta(p, kind, profileDropdownSub, profileDropdownIp, profileDropdownSpin);
        } else {
            java.util.List<ProfileStore.Profile> list = ProfileStore.list(this, kind);
            if (list.isEmpty()) {
                profileDropdownLabel.setText("Add a " + kindLabel(kind) + " connection");
                profileDropdownLabel.setTextColor(0xFF888888);
            } else {
                profileDropdownLabel.setText("Select connection");
                profileDropdownLabel.setTextColor(0xFFAAAAAA);
            }
            if (profileDropdownSub != null) profileDropdownSub.setText("");
            if (profileDropdownIp != null) {
                profileDropdownIp.setText("");
                profileDropdownIp.setVisibility(View.GONE);
            }
            if (profileDropdownSpin != null) profileDropdownSpin.setVisibility(View.GONE);
        }
    }

    private void bindProfileMeta(
        ProfileStore.Profile p,
        String kind,
        TextView sub,
        TextView ipView,
        View spin
    ) {
        if (p == null) return;
        boolean locating = locatingProfileIds.contains(p.id);
        String loc = p.locationLabel();
        if (sub != null) {
            if (locating && loc.length() == 0) {
                sub.setText("Looking up location…");
            } else if (loc.length() > 0) {
                sub.setText(loc);
            } else if (p.host != null && p.host.length() > 0) {
                sub.setText(p.host);
            } else {
                sub.setText("");
            }
        }
        if (spin != null) {
            spin.setVisibility(locating ? View.VISIBLE : View.GONE);
        }
        if (ipView != null) {
            if (ProfileStore.showsEndpointIp(kind)) {
                String ips = ProfileStore.formatEndpointIps(p.resolvedIp, p.resolvedIp6);
                if (ips.length() > 0) {
                    ipView.setText(ips);
                    ipView.setVisibility(View.VISIBLE);
                } else {
                    ipView.setText("");
                    ipView.setVisibility(View.GONE);
                }
            } else {
                ipView.setText("");
                ipView.setVisibility(View.GONE);
            }
        }
    }

    private View slimChevronView() {
        final float density = getResources().getDisplayMetrics().density;
        return new View(this) {
            private final Paint paint = new Paint(Paint.ANTI_ALIAS_FLAG);
            private final Path path = new Path();
            {
                paint.setColor(0xFF00FF7F);
                paint.setStyle(Paint.Style.STROKE);
                paint.setStrokeWidth(Math.max(1.75f, density * 1.35f));
                paint.setStrokeCap(Paint.Cap.ROUND);
                paint.setStrokeJoin(Paint.Join.ROUND);
            }
            @Override protected void onDraw(Canvas c) {
                float w = getWidth();
                float h = getHeight();
                float cx = w * 0.5f;
                float cy = h * 0.52f;
                float hw = w * 0.30f;
                float hh = h * 0.16f;
                path.reset();
                path.moveTo(cx - hw, cy - hh);
                path.lineTo(cx, cy + hh);
                path.lineTo(cx + hw, cy - hh);
                c.drawPath(path, paint);
            }
        };
    }

    private void showProfileMenu(final View anchor, final String kind) {
        if (profileMenuWindow != null && profileMenuWindow.isShowing()) {
            profileMenuWindow.dismiss();
            return;
        }

        LinearLayout menu = new LinearLayout(this);
        menu.setOrientation(LinearLayout.VERTICAL);
        menu.setPadding(dp(6), dp(6), dp(6), dp(6));
        GradientDrawable bg = new GradientDrawable();
        bg.setCornerRadius(dp(14));
        bg.setColor(0xF012151A);
        bg.setStroke(dp(1), 0x3300FF7F);
        menu.setBackground(bg);

        final LinearLayout items = new LinearLayout(this);
        items.setOrientation(LinearLayout.VERTICAL);

        ScrollView scroller = new ScrollView(this);
        scroller.setVerticalScrollBarEnabled(true);
        scroller.setOverScrollMode(View.OVER_SCROLL_IF_CONTENT_SCROLLS);
        scroller.addView(items, new ScrollView.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT,
            ViewGroup.LayoutParams.WRAP_CONTENT
        ));
        menu.addView(scroller, mw());

        View addMore = addConnectionButton(new View.OnClickListener() {
            @Override public void onClick(View v) {
                if (profileMenuWindow != null) profileMenuWindow.dismiss();
                showAddConnectionDialog(kind);
            }
        });
        LinearLayout.LayoutParams alp = mw(dp(46));
        alp.topMargin = dp(6);
        menu.addView(addMore, alp);

        int width = anchor.getWidth() > 0 ? anchor.getWidth() : dp(280);
        final PopupWindow pw = new PopupWindow(
            menu, width, ViewGroup.LayoutParams.WRAP_CONTENT, true
        );
        pw.setOutsideTouchable(true);
        pw.setFocusable(true);
        pw.setTouchable(true);
        pw.setElevation(dp(16));
        pw.setBackgroundDrawable(new ColorDrawable(Color.TRANSPARENT));
        pw.setOnDismissListener(new PopupWindow.OnDismissListener() {
            @Override public void onDismiss() {
                profileMenuWindow = null;
                if (dropdownChevron != null) {
                    dropdownChevron.animate().rotation(0f).setDuration(160).start();
                }
            }
        });

        fillProfileMenuItems(items, scroller, kind, pw);
        profileMenuWindow = pw;
        if (dropdownChevron != null) {
            dropdownChevron.animate().rotation(180f).setDuration(160).start();
        }
        pw.showAsDropDown(anchor, 0, dp(6));
    }

    private void fillProfileMenuItems(
        final LinearLayout items,
        final ScrollView scroller,
        final String kind,
        final PopupWindow pw
    ) {
        items.removeAllViews();
        java.util.List<ProfileStore.Profile> list = ProfileStore.list(this, kind);
        if (list.isEmpty()) {
            TextView empty = muted("No saved connections yet");
            empty.setPadding(dp(14), dp(12), dp(14), dp(12));
            items.addView(empty, mw());
        }
        final boolean showIp = ProfileStore.showsEndpointIp(kind);
        final int rowH = showIp ? dp(72) : dp(56);
        for (final ProfileStore.Profile p : list) {
            boolean needIp = showIp
                && (p.resolvedIp == null || p.resolvedIp.length() == 0)
                && (p.resolvedIp6 == null || p.resolvedIp6.length() == 0);
            if (!p.hasLocation() || needIp) {
                enrichProfileLocation(p.id, kind, items, scroller, pw);
            }
            LinearLayout item = new LinearLayout(this);
            item.setOrientation(LinearLayout.HORIZONTAL);
            item.setGravity(Gravity.CENTER_VERTICAL);
            item.setPadding(dp(10), dp(4), dp(4), dp(4));
            item.setMinimumHeight(rowH);
            boolean sel = p.id.equals(selectedProfileId);
            GradientDrawable ibg = new GradientDrawable();
            ibg.setCornerRadius(dp(10));
            ibg.setColor(sel ? 0xFF102018 : Color.TRANSPARENT);
            item.setBackground(ibg);

            LinearLayout names = new LinearLayout(this);
            names.setOrientation(LinearLayout.VERTICAL);
            names.setGravity(Gravity.CENTER_VERTICAL);

            TextView name = new TextView(this);
            name.setText(profileTitle(p));
            name.setTextColor(sel ? 0xFF00FF7F : Color.WHITE);
            name.setTextSize(14);
            name.setTypeface(Typeface.create("sans-serif-medium", Typeface.NORMAL));
            name.setSingleLine(true);
            name.setEllipsize(TextUtils.TruncateAt.END);
            names.addView(name, mw());

            LinearLayout subRow = new LinearLayout(this);
            subRow.setOrientation(LinearLayout.HORIZONTAL);
            subRow.setGravity(Gravity.CENTER_VERTICAL);
            View spin = new RoundSpinnerView(this);
            LinearLayout.LayoutParams spinLp = new LinearLayout.LayoutParams(dp(12), dp(12));
            spinLp.rightMargin = dp(6);
            subRow.addView(spin, spinLp);
            TextView sub = new TextView(this);
            sub.setTextColor(0xFF8A9098);
            sub.setTextSize(11);
            sub.setSingleLine(true);
            sub.setEllipsize(TextUtils.TruncateAt.END);
            subRow.addView(sub, new LinearLayout.LayoutParams(0, vw(), 1f));
            names.addView(subRow, mw());

            TextView ipLine = new TextView(this);
            ipLine.setTextColor(0xFF6B7178);
            ipLine.setTextSize(10);
            ipLine.setTypeface(Typeface.MONOSPACE);
            ipLine.setSingleLine(true);
            ipLine.setEllipsize(TextUtils.TruncateAt.MIDDLE);
            names.addView(ipLine, mw());
            bindProfileMeta(p, kind, sub, ipLine, spin);
            item.addView(names, new LinearLayout.LayoutParams(0, vw(), 1f));

            ImageView edit = Icons.of(this, Icons.EDIT, 0xFFE8EAED);
            edit.setContentDescription("Edit");
            edit.setPadding(dp(8), dp(8), dp(8), dp(8));
            edit.setClickable(true);
            edit.setOnClickListener(new View.OnClickListener() {
                @Override public void onClick(View v) {
                    if (pw != null) pw.dismiss();
                    showConnectionDialog(kind, p);
                }
            });
            LinearLayout.LayoutParams elp = new LinearLayout.LayoutParams(dp(40), dp(40));
            elp.leftMargin = dp(2);
            item.addView(edit, elp);

            Button del = compactButton("✕", new View.OnClickListener() {
                @Override public void onClick(View v) {
                    ProfileStore.delete(MainActivity.this, p.id);
                    if (p.id.equals(selectedProfileId)) {
                        selectedProfileId = "";
                        prefs().edit().remove("selected_" + kind).apply();
                        restoreSelectedProfile(kind);
                    }
                    refreshProfileDropdownLabel(kind);
                    setNotice("Deleted " + p.name);
                    // Stay open so several profiles can be removed in one pass.
                    fillProfileMenuItems(items, scroller, kind, pw);
                }
            });
            item.addView(del, dp(40), dp(36));
            item.setOnClickListener(new View.OnClickListener() {
                @Override public void onClick(View v) {
                    pw.dismiss();
                    applySavedProfile(p, true);
                    refreshProfileDropdownLabel(kind);
                }
            });
            LinearLayout.LayoutParams ilp = mw(rowH);
            ilp.topMargin = dp(2);
            items.addView(item, ilp);
        }

        int visible = Math.min(6, Math.max(1, list.size()));
        int maxH = visible * (rowH + dp(2)) + dp(4);
        ViewGroup.LayoutParams slp = scroller.getLayoutParams();
        if (slp != null) {
            slp.height = maxH;
            scroller.setLayoutParams(slp);
        }
        if (pw != null && pw.isShowing()) {
            pw.update();
        }
    }

    private void showAddConnectionDialog(final String kind) {
        showConnectionDialog(kind, null);
    }

    private void showConnectionDialog(final String kind, final ProfileStore.Profile existing) {
        final boolean editing = existing != null;
        LinearLayout layout = new LinearLayout(this);
        layout.setOrientation(LinearLayout.VERTICAL);
        layout.setPadding(dp(20), dp(8), dp(20), dp(4));

        TextView hint = muted(addHintFor(kind));
        hint.setTextSize(12);
        layout.addView(hint, mw());

        final EditText nameField = singleLine("Connection name");
        if (editing && existing.name != null) nameField.setText(existing.name);
        layout.addView(nameField, mw());

        final EditText content = scrollableProfile(addContentHint(kind), 7);
        addDialogContent = content;
        if (editing && existing.content != null) content.setText(existing.content);
        layout.addView(wrapScrollable(content, dp(160)), mw());

        LinearLayout actions = new LinearLayout(this);
        actions.setOrientation(LinearLayout.HORIZONTAL);
        actions.setPadding(0, dp(8), 0, 0);
        Button importBtn = compactButton("Import file", new View.OnClickListener() {
            @Override public void onClick(View v) {
                pickImport(importKeyFor(kind));
            }
        });
        Button pasteBtn = compactButton("Paste", new View.OnClickListener() {
            @Override public void onClick(View v) { pasteInto(content); }
        });
        actions.addView(importBtn, new LinearLayout.LayoutParams(0, dp(38), 1f));
        LinearLayout.LayoutParams plp = new LinearLayout.LayoutParams(0, dp(38), 1f);
        plp.leftMargin = dp(8);
        actions.addView(pasteBtn, plp);
        layout.addView(actions, mw());

        final EditText userField;
        final EditText passField;
        userField = null;
        passField = null;

        AlertDialog dialog = new AlertDialog.Builder(this, android.R.style.Theme_Material_Dialog_Alert)
            .setTitle((editing ? "Edit " : "Add ") + kindLabel(kind))
            .setView(layout)
            .setPositiveButton("Save", new DialogInterface.OnClickListener() {
                @Override
                public void onClick(DialogInterface d, int which) {
                    String body = textOf(content).trim();
                    if (body.isEmpty()) {
                        setNotice("Paste or import a profile first.");
                        return;
                    }
                    String user = userField != null ? textOf(userField).trim() : "";
                    String pass = passField != null ? textOf(passField) : "";
                    String id = editing ? existing.id : null;
                    ProfileStore.Profile p = ProfileStore.save(
                        MainActivity.this, id, kind,
                        nameField.getText().toString(),
                        body, user, pass, ""
                    );
                    applySavedProfile(p, true);
                    refreshProfileDropdownLabel(kind);
                    setNotice((editing ? "Updated " : "Saved ") + p.name);
                    enrichProfileLocation(p.id, kind, null, null, null);
                }
            })
            .setNegativeButton(R.string.dialog_cancel, null)
            .create();
        dialog.setOnDismissListener(new DialogInterface.OnDismissListener() {
            @Override public void onDismiss(DialogInterface d) { addDialogContent = null; }
        });
        if (dialog.getWindow() != null) {
            dialog.getWindow().setBackgroundDrawable(darkDialogBackground());
        }
        dialog.show();
        polishDialogButtons(dialog);
    }

    private String profileTitle(ProfileStore.Profile p) {
        if (p == null) return "";
        String flag = countryFlag(p.countryCode);
        if (p.countryCode != null && p.countryCode.length() == 2 && !"◎".equals(flag)) {
            return flag + "  " + p.name;
        }
        return p.name != null ? p.name : "";
    }

    private View slimPencilView() {
        return new PencilEditView(this);
    }

    /**
     * Windows-parity: resolve the profile remote/endpoint and tag it with
     * country + city (WireGuard, Outline/Shadowsocks).
     */
    private void enrichProfileLocation(
        final String profileId,
        final String kind,
        final LinearLayout items,
        final ScrollView scroller,
        final PopupWindow pw
    ) {
        if (profileId == null || profileId.length() == 0) return;
        if (!locatingProfileIds.add(profileId)) return;
        refreshProfileDropdownLabel(kind);
        new Thread(new Runnable() {
            @Override public void run() {
                try {
                    ProfileStore.Profile p = ProfileStore.get(MainActivity.this, profileId);
                    if (p == null) {
                        finishLocate(profileId, kind, items, scroller, pw, false);
                        return;
                    }
                    String host = p.host;
                    if (host == null || host.length() == 0) {
                        host = ProfileStore.extractHost(p.kind, p.content);
                    }
                    if (host == null || host.length() == 0) {
                        finishLocate(profileId, kind, items, scroller, pw, false);
                        return;
                    }
                    String v4 = "";
                    String v6 = "";
                    if (looksLikeIp(host)) {
                        if (host.contains(":")) v6 = host;
                        else v4 = host;
                    } else {
                        InetAddress[] addrs = InetAddress.getAllByName(host);
                        if (addrs != null) {
                            for (int i = 0; i < addrs.length; i++) {
                                if (addrs[i] == null) continue;
                                String a = addrs[i].getHostAddress();
                                if (a == null || a.length() == 0) continue;
                                if (a.indexOf(':') >= 0 && v6.length() == 0) v6 = a;
                                else if (a.indexOf(':') < 0 && v4.length() == 0) v4 = a;
                            }
                        }
                    }
                    String geo = v4.length() > 0 ? reverseGeoForIp(v4) : null;
                    String country = p.country;
                    String cc = p.countryCode;
                    String city = p.city;
                    double lat = p.lat;
                    double lon = p.lon;
                    if (geo != null && geo.startsWith("OK")) {
                        Map<String, String> kv = parseKV(geo);
                        country = nz(kv.get("country"));
                        cc = nz(kv.get("country_code"));
                        city = nz(kv.get("city"));
                        try { lat = Double.parseDouble(nz(kv.get("lat"))); } catch (Exception ignored) {}
                        try { lon = Double.parseDouble(nz(kv.get("lon"))); } catch (Exception ignored) {}
                        String geoResolved = nz(kv.get("ip"));
                        if (geoResolved.length() > 0 && geoResolved.indexOf(':') < 0) v4 = geoResolved;
                    }
                    if (v4.length() == 0 && v6.length() == 0 && !p.hasLocation()) {
                        finishLocate(profileId, kind, items, scroller, pw, false);
                        return;
                    }
                    ProfileStore.updateLocation(
                        MainActivity.this, profileId, country, cc, city, lat, lon, v4, v6
                    );
                    finishLocate(profileId, kind, items, scroller, pw, true);
                } catch (Exception e) {
                    Log.w("ZeroNode", "profile geo: " + e.getMessage());
                    finishLocate(profileId, kind, items, scroller, pw, false);
                }
            }
        }, "zn-profile-geo").start();
    }

    private void finishLocate(
        final String profileId,
        final String kind,
        final LinearLayout items,
        final ScrollView scroller,
        final PopupWindow pw,
        final boolean announce
    ) {
        handler.post(new Runnable() {
            @Override public void run() {
                locatingProfileIds.remove(profileId);
                if (isFinishing()) return;
                refreshProfileDropdownLabel(kind);
                if (items != null && pw != null && pw.isShowing()) {
                    fillProfileMenuItems(items, scroller, kind, pw);
                }
                if (!announce) return;
                ProfileStore.Profile now = ProfileStore.get(MainActivity.this, profileId);
                if (now != null && now.hasLocation()) {
                    String loc = now.locationLabel();
                    if (loc.length() > 0) {
                        setNotice(kindLabel(now.kind) + " · " + loc);
                    }
                }
            }
        });
    }

    private static String addHintFor(String kind) {
        if (ProfileStore.KIND_WG.equals(kind)) {
            return "Paste a WireGuard .conf or import a file.";
        }
        return "Paste an ss:// or Outline access key.";
    }

    private static String addContentHint(String kind) {
        if (ProfileStore.KIND_WG.equals(kind)) return "Paste WireGuard .conf";
        return "Paste ss:// or Outline key";
    }

    private static String importKeyFor(String kind) {
        if (ProfileStore.KIND_OUTLINE.equals(kind)) return "outline";
        return "wg";
    }

    private GradientDrawable darkDialogBackground() {
        GradientDrawable bg = new GradientDrawable();
        bg.setCornerRadius(dp(18));
        bg.setColor(0xFF12151A);
        bg.setStroke(dp(1), 0x3300FF7F);
        return bg;
    }

    private void polishDialogButtons(AlertDialog dialog) {
        Button pos = dialog.getButton(DialogInterface.BUTTON_POSITIVE);
        Button neg = dialog.getButton(DialogInterface.BUTTON_NEGATIVE);
        if (pos != null) {
            pos.setAllCaps(false);
            pos.setTextColor(0xFF00FF7F);
        }
        if (neg != null) {
            neg.setAllCaps(false);
            neg.setTextColor(0xFFAAAAAA);
        }
    }

    private void applySavedProfile(ProfileStore.Profile p, boolean announce) {
        if (p == null) return;
        selectedProfileId = p.id;
        prefs().edit().putString("selected_" + p.kind, p.id).apply();
        if (profileInput != null) profileInput.setText(p.content != null ? p.content : "");
        if (ProfileStore.KIND_WG.equals(p.kind)) {
            prefs().edit().putString("wg_profile", p.content != null ? p.content : "").apply();
        } else if (ProfileStore.KIND_OUTLINE.equals(p.kind)) {
            prefs().edit().putString("outline_key", p.content != null ? p.content : "").apply();
        }
        refreshProfileDropdownLabel(p.kind);
        boolean needIp = ProfileStore.showsEndpointIp(p.kind)
            && (p.resolvedIp == null || p.resolvedIp.length() == 0)
            && (p.resolvedIp6 == null || p.resolvedIp6.length() == 0);
        if (!p.hasLocation() || needIp) {
            enrichProfileLocation(p.id, p.kind, null, null, null);
        }
        if (announce) setNotice("Selected " + p.name);
    }

    private String activeConfigText() {
        String t = textOf(profileInput).trim();
        if (t.length() > 0) return t;
        ProfileStore.Profile p = ProfileStore.get(this, selectedProfileId);
        if (p != null && p.content != null && p.content.trim().length() > 0) {
            return p.content.trim();
        }
        return "";
    }

    private void saveNamedProfile(
        final String kind, final String content, final String user,
        final String password, final String host
    ) {
        if (content == null || content.trim().isEmpty()) {
            setNotice("Nothing to save — paste or import a profile first.");
            return;
        }
        final EditText nameField = singleLine("Profile name");
        nameField.setText(ProfileStore.defaultName(kind, content, host));
        new AlertDialog.Builder(this, android.R.style.Theme_Material_Dialog_Alert)
            .setTitle("Save profile")
            .setView(nameField)
            .setPositiveButton("Save", new DialogInterface.OnClickListener() {
                @Override
                public void onClick(DialogInterface dialog, int which) {
                    ProfileStore.Profile p = ProfileStore.save(
                        MainActivity.this,
                        selectedProfileId.length() > 0 ? selectedProfileId : null,
                        kind,
                        nameField.getText().toString(),
                        content,
                        user,
                        password,
                        host
                    );
                    selectedProfileId = p.id;
                    if (ProfileStore.KIND_WG.equals(kind)) {
                        prefs().edit().putString("wg_profile", content).apply();
                    } else if (ProfileStore.KIND_OUTLINE.equals(kind)) {
                        prefs().edit().putString("outline_key", content).apply();
                    }
                    applySavedProfile(p, false);
                    setNotice("Saved profile: " + p.name);
                }
            })
            .setNegativeButton(R.string.dialog_cancel, null)
            .show();
    }

    private static String kindLabel(String kind) {
        if (ProfileStore.KIND_WG.equals(kind)) return "WireGuard";
        if (ProfileStore.KIND_OUTLINE.equals(kind)) return "Outline";
        return kind != null ? kind : "";
    }

    private void fillTorBody() {
        LinearLayout head = new LinearLayout(this);
        head.setOrientation(LinearLayout.HORIZONTAL);
        head.setGravity(Gravity.CENTER_VERTICAL);
        View torIcon = Icons.of(this, Icons.TOR, 0xFFC084FC);
        LinearLayout.LayoutParams tilp = new LinearLayout.LayoutParams(dp(22), dp(22));
        tilp.rightMargin = dp(10);
        head.addView(torIcon, tilp);
        TextView torTitle = new TextView(this);
        torTitle.setText("Tor");
        torTitle.setTextColor(0xFFE9D5FF);
        torTitle.setTextSize(16);
        torTitle.setTypeface(Typeface.create("sans-serif-medium", Typeface.NORMAL));
        head.addView(torTitle, vw(), vw());
        LinearLayout.LayoutParams hlp = mw();
        hlp.topMargin = dp(6);
        protocolBody.addView(head, hlp);
        protocolBody.addView(muted("Routes the whole device through the Tor expert bundle."), mw());
        if (!TorBundle.isArm64Device() || !TorBundle.libTorPresent(this)) {
            protocolBody.addView(muted(getString(R.string.tor_arm64_only)), mw());
        }
        TextView torHint = muted(torSocksUp
            ? ("Tor SOCKS" + (torSocksPort > 0 ? (" :" + torSocksPort) : "")
                + (vpnActive ? " · system tunnel on" : " · starting tunnel…"))
            : "Tor idle");
        protocolBody.addView(torHint, mw());

        LinearLayout bridgeRow = new LinearLayout(this);
        bridgeRow.setOrientation(LinearLayout.HORIZONTAL);
        bridgeRow.setGravity(Gravity.CENTER_VERTICAL);
        bridgeRow.setPadding(dp(12), dp(10), dp(12), dp(10));
        GradientDrawable bbg = new GradientDrawable();
        bbg.setCornerRadius(dp(12));
        bbg.setColor(0xFF18141F);
        bbg.setStroke(dp(1), 0x44A855F7);
        bridgeRow.setBackground(bbg);
        ImageView bIcon = Icons.of(this, Icons.TOR, 0xFFC084FC);
        LinearLayout.LayoutParams bilp = new LinearLayout.LayoutParams(dp(18), dp(18));
        bilp.rightMargin = dp(10);
        bridgeRow.addView(bIcon, bilp);
        LinearLayout bTexts = new LinearLayout(this);
        bTexts.setOrientation(LinearLayout.VERTICAL);
        TextView bTitle = new TextView(this);
        bTitle.setText("Bridges");
        bTitle.setTextColor(0xFFE9D5FF);
        bTitle.setTextSize(14);
        bTitle.setTypeface(Typeface.create("sans-serif-medium", Typeface.NORMAL));
        bTexts.addView(bTitle, mw());
        TextView bSub = new TextView(this);
        bSub.setText(BridgeStore.summary(this));
        bSub.setTextColor(0xFF9AA3AD);
        bSub.setTextSize(12);
        bTexts.addView(bSub, mw());
        bridgeRow.addView(bTexts, new LinearLayout.LayoutParams(0, vw(), 1f));
        bridgeRow.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View v) { openSettings("bridges"); }
        });
        LinearLayout.LayoutParams brlp = mw();
        brlp.topMargin = dp(10);
        protocolBody.addView(bridgeRow, brlp);

        primaryConnectBtn = actionBtn("Connect Tor", new View.OnClickListener() {
            @Override public void onClick(View v) { onPrimaryConnectClick(); }
        });
        styleButton(primaryConnectBtn, "Connect Tor", 0xFFA855F7, Color.WHITE);
        stylePrimaryConnect(primaryConnectBtn);
        LinearLayout.LayoutParams clp = mw(dp(54));
        clp.topMargin = dp(12);
        protocolBody.addView(primaryConnectBtn, clp);
        updatePrimaryButton();
    }

    private void addPrimaryConnect(String label) {
        primaryConnectBtn = actionBtn(label, new View.OnClickListener() {
            @Override public void onClick(View v) { onPrimaryConnectClick(); }
        });
        stylePrimaryConnect(primaryConnectBtn);
        LinearLayout.LayoutParams clp = mw(dp(54));
        clp.topMargin = dp(12);
        protocolBody.addView(primaryConnectBtn, clp);
    }

    private void stylePrimaryConnect(Button btn) {
        if (btn == null) return;
        btn.setTextSize(17);
        btn.setTypeface(Typeface.create("sans-serif-medium", Typeface.BOLD));
        btn.setLetterSpacing(0.03f);
        btn.setPadding(dp(14), dp(12), dp(14), dp(12));
        Drawable bg = btn.getBackground();
        if (bg instanceof GradientDrawable) {
            ((GradientDrawable) bg).setCornerRadius(dp(12));
        }
    }

    private View addConnectionButton(View.OnClickListener l) {
        LinearLayout row = new LinearLayout(this);
        row.setOrientation(LinearLayout.HORIZONTAL);
        row.setGravity(Gravity.CENTER);
        row.setPadding(dp(12), dp(8), dp(12), dp(8));
        GradientDrawable bg = new GradientDrawable();
        bg.setCornerRadius(dp(12));
        bg.setColor(0xFFE8EDF2);
        row.setBackground(bg);
        ImageView plus = Icons.of(this, Icons.PLUS, 0xFF111418);
        LinearLayout.LayoutParams plp = new LinearLayout.LayoutParams(dp(18), dp(18));
        plp.rightMargin = dp(8);
        row.addView(plus, plp);
        TextView t = new TextView(this);
        t.setText("Add connection");
        t.setTextColor(0xFF111418);
        t.setTextSize(14);
        t.setTypeface(Typeface.create("sans-serif-medium", Typeface.NORMAL));
        row.addView(t, vw(), vw());
        row.setClickable(true);
        row.setOnClickListener(l);
        return row;
    }

    private View buildNodesCard() {
        LinearLayout card = softSection();
        card.addView(sectionTitle(getString(R.string.label_nodes), Color.WHITE), mw());

        LinearLayout toolbar = new LinearLayout(this);
        toolbar.setOrientation(LinearLayout.HORIZONTAL);
        toolbar.setGravity(Gravity.CENTER_VERTICAL);
        toolbar.setPadding(0, dp(6), 0, dp(6));
        hostInput = singleLine(getString(R.string.add_host_hint));
        toolbar.addView(hostInput, new LinearLayout.LayoutParams(0, dp(44), 1f));
        Button add = accentButton(getString(R.string.btn_add_host));
        add.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View v) {
                String host = hostInput.getText().toString().trim();
                if (!host.isEmpty()) {
                    hostInput.setText("");
                    runDiscoveryFor(host);
                }
            }
        });
        LinearLayout.LayoutParams alp = new LinearLayout.LayoutParams(dp(72), dp(44));
        alp.leftMargin = dp(6);
        toolbar.addView(add, alp);
        Button refresh = secondaryButton(getString(R.string.btn_discover));
        refresh.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View v) { runDiscovery(); }
        });
        LinearLayout.LayoutParams rlp = new LinearLayout.LayoutParams(dp(88), dp(44));
        rlp.leftMargin = dp(6);
        toolbar.addView(refresh, rlp);
        card.addView(toolbar, mw());

        noticeText = muted(getString(R.string.label_no_servers));
        card.addView(noticeText, mw());
        serverListContainer = new LinearLayout(this);
        serverListContainer.setOrientation(LinearLayout.VERTICAL);
        card.addView(serverListContainer, mw());
        return card;
    }

    private View buildAppSplitCard() {
        LinearLayout card = softSection();
        card.addView(sectionTitle("App protection", 0xFF00FF7F), mw());

        LinearLayout row = new LinearLayout(this);
        row.setOrientation(LinearLayout.HORIZONTAL);
        row.setGravity(Gravity.CENTER_VERTICAL);
        row.setPadding(dp(14), dp(12), dp(12), dp(12));
        GradientDrawable bg = new GradientDrawable();
        bg.setCornerRadius(dp(14));
        bg.setColor(0xFF14181E);
        bg.setStroke(dp(1), 0x2AFFFFFF);
        row.setBackground(bg);
        row.setClickable(true);
        row.setFocusable(true);
        LinearLayout.LayoutParams rowLp = mw();
        rowLp.topMargin = dp(10);
        row.setMinimumHeight(dp(58));

        LinearLayout texts = new LinearLayout(this);
        texts.setOrientation(LinearLayout.VERTICAL);
        TextView title = new TextView(this);
        title.setText("Apps");
        title.setTextColor(Color.WHITE);
        title.setTextSize(14);
        title.setTypeface(Typeface.create("sans-serif-medium", Typeface.NORMAL));
        texts.addView(title, mw());
        appSplitSummary = new TextView(this);
        appSplitSummary.setTextColor(0xFF8A9098);
        appSplitSummary.setTextSize(12);
        appSplitSummary.setSingleLine(true);
        appSplitSummary.setEllipsize(TextUtils.TruncateAt.END);
        texts.addView(appSplitSummary, mw());
        row.addView(texts, new LinearLayout.LayoutParams(0, vw(), 1f));

        allAppsSwitch = new GreenSwitch(this);
        allAppsSwitch.setOn(AppSplitStore.allApps(this), false);
        allAppsSwitch.setOnToggle(new GreenSwitch.OnToggle() {
            @Override public void onToggle(boolean on) {
                AppSplitStore.setProtectAll(MainActivity.this, on);
                updateAppSplitSummary();
                refreshAppProtectPage();
                noticeReconnectApps();
            }
        });
        row.addView(allAppsSwitch, dp(42), dp(26));
        View chev = slimChevronView();
        LinearLayout.LayoutParams chevLp = new LinearLayout.LayoutParams(dp(16), dp(16));
        chevLp.leftMargin = dp(8);
        row.addView(chev, chevLp);
        row.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View v) { openAppProtectPage(); }
        });
        card.addView(row, rowLp);
        updateAppSplitSummary();
        return card;
    }

    private void updateAppSplitSummary() {
        if (appSplitSummary != null) {
            appSplitSummary.setText(AppSplitStore.summaryLabel(this));
        }
        boolean all = AppSplitStore.allApps(this);
        if (allAppsSwitch != null) allAppsSwitch.setOn(all, false);
        if (protectAllSwitch != null) protectAllSwitch.setOn(all, false);
        if (protectAllSub != null) {
            protectAllSub.setText(AppSplitStore.summaryLabel(this));
        }
    }

    private void noticeReconnectApps() {
        if (vpnActive || connecting) {
            setNotice("Reconnect to apply which apps use the VPN.");
        }
    }

    private View buildAppProtectPage() {
        LinearLayout page = new LinearLayout(this);
        page.setOrientation(LinearLayout.VERTICAL);
        page.setVisibility(View.GONE);
        page.setClickable(true);
        page.setBackgroundColor(0xFF0B0D10);
        int topPad = statusBarInsetPx() + dp(10);
        page.setPadding(dp(16), topPad, dp(16), dp(16));

        LinearLayout header = new LinearLayout(this);
        header.setOrientation(LinearLayout.HORIZONTAL);
        header.setGravity(Gravity.CENTER_VERTICAL);
        View back = Icons.of(this, Icons.BACK, 0xFFE8EAED);
        back.setPadding(dp(8), dp(8), dp(8), dp(8));
        back.setClickable(true);
        back.setOnClickListener(new View.OnClickListener() {
            @Override public void onClick(View v) { closeAppProtectPage(); }
        });
        header.addView(back, dp(36), dp(36));
        TextView title = new TextView(this);
        title.setText("App protection");
        title.setTextColor(Color.WHITE);
        title.setTextSize(18);
        title.setTypeface(Typeface.create("sans-serif-medium", Typeface.NORMAL));
        LinearLayout.LayoutParams tlp = new LinearLayout.LayoutParams(0, vw(), 1f);
        tlp.leftMargin = dp(8);
        header.addView(title, tlp);
        page.addView(header, mw());

        LinearLayout allRow = new LinearLayout(this);
        allRow.setOrientation(LinearLayout.HORIZONTAL);
        allRow.setGravity(Gravity.CENTER_VERTICAL);
        allRow.setPadding(dp(14), dp(14), dp(14), dp(14));
        GradientDrawable allBg = new GradientDrawable();
        allBg.setCornerRadius(dp(16));
        allBg.setColor(0xFF14181E);
        allBg.setStroke(dp(1), 0x3300FF7F);
        allRow.setBackground(allBg);
        LinearLayout.LayoutParams allLp = mw();
        allLp.topMargin = dp(16);
        LinearLayout allTexts = new LinearLayout(this);
        allTexts.setOrientation(LinearLayout.VERTICAL);
        TextView allTitle = new TextView(this);
        allTitle.setText("Protect all apps");
        allTitle.setTextColor(Color.WHITE);
        allTitle.setTextSize(15);
        allTitle.setTypeface(Typeface.create("sans-serif-medium", Typeface.NORMAL));
        allTexts.addView(allTitle, mw());
        protectAllSub = new TextView(this);
        protectAllSub.setTextColor(0xFF8A9098);
        protectAllSub.setTextSize(12);
        allTexts.addView(protectAllSub, mw());
        allRow.addView(allTexts, new LinearLayout.LayoutParams(0, vw(), 1f));
        protectAllSwitch = new GreenSwitch(this);
        protectAllSwitch.setOn(AppSplitStore.allApps(this), false);
        protectAllSwitch.setOnToggle(new GreenSwitch.OnToggle() {
            @Override public void onToggle(boolean on) {
                AppSplitStore.setProtectAll(MainActivity.this, on);
                updateAppSplitSummary();
                refreshAppProtectPage();
                noticeReconnectApps();
            }
        });
        allRow.addView(protectAllSwitch, dp(42), dp(26));
        page.addView(allRow, allLp);

        appProtectList = new LinearLayout(this);
        appProtectList.setOrientation(LinearLayout.VERTICAL);
        ScrollView scroller = new ScrollView(this);
        scroller.setFillViewport(true);
        scroller.setVerticalScrollBarEnabled(false);
        scroller.addView(appProtectList, mw());
        LinearLayout.LayoutParams slp = new LinearLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT, 0, 1f
        );
        slp.topMargin = dp(12);
        page.addView(scroller, slp);
        appProtectPage = page;
        return page;
    }

    private void openAppProtectPage() {
        if (appProtectPage == null) return;
        appProtectPage.setVisibility(View.VISIBLE);
        if (appProtectList != null && appProtectList.getChildCount() == 0) {
            TextView loading = muted("Loading apps…");
            loading.setPadding(dp(8), dp(18), dp(8), dp(18));
            appProtectList.addView(loading, mw());
        }
        AppCatalog.load(this, new AppCatalog.Listener() {
            @Override public void onReady(List<AppCatalog.AppInfo> apps, boolean iconsReady) {
                if (isFinishing() || appProtectPage == null
                    || appProtectPage.getVisibility() != View.VISIBLE) {
                    return;
                }
                fillAppProtectList(apps);
            }
        });
    }

    private void closeAppProtectPage() {
        if (appProtectPage != null) appProtectPage.setVisibility(View.GONE);
        updateAppSplitSummary();
    }

    void openSettings(String section) {
        if (settingsPage == null) return;
        settingsPage.setVisibility(View.VISIBLE);
        if ("bridges".equals(section)) {
            final View target = settingsPage.findViewById(R.id.settings_bridges);
            if (target != null) {
                target.post(new Runnable() {
                    @Override public void run() {
                        target.requestRectangleOnScreen(new android.graphics.Rect(
                            0, 0, target.getWidth(), target.getHeight()), true);
                    }
                });
            }
        }
    }

    void closeSettings() {
        if (settingsPage != null) settingsPage.setVisibility(View.GONE);
        refreshChromeTitle();
    }

    void rebuildSettings() {
        if (rootFrame == null || settingsPage == null) return;
        boolean vis = settingsPage.getVisibility() == View.VISIBLE;
        int idx = rootFrame.indexOfChild(settingsPage);
        rootFrame.removeView(settingsPage);
        settingsPage = SettingsScreen.build(this);
        if (idx >= 0) {
            rootFrame.addView(settingsPage, idx, new FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT));
        } else {
            rootFrame.addView(settingsPage, new FrameLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT));
        }
        if (vis) settingsPage.setVisibility(View.VISIBLE);
    }

    void openGuide() {
        if (guidePage != null) guidePage.setVisibility(View.VISIBLE);
    }

    void closeGuide() {
        if (guidePage != null) guidePage.setVisibility(View.GONE);
    }

    void closeOnboarding() {
        if (onboardingPage != null) onboardingPage.setVisibility(View.GONE);
    }

    void refreshChromeTitle() {
        // Top branding chrome was removed; launcher name still lives in Settings.
    }

    void pickCustomIcon(boolean camera) {
        try {
            if (camera) {
                ContentValues values = new ContentValues();
                values.put(MediaStore.Images.Media.DISPLAY_NAME,
                    "zeronode_icon_" + System.currentTimeMillis() + ".jpg");
                values.put(MediaStore.Images.Media.MIME_TYPE, "image/jpeg");
                cameraImageUri = getContentResolver().insert(
                    MediaStore.Images.Media.EXTERNAL_CONTENT_URI, values);
                Intent take = new Intent(MediaStore.ACTION_IMAGE_CAPTURE);
                if (cameraImageUri != null) {
                    take.putExtra(MediaStore.EXTRA_OUTPUT, cameraImageUri);
                }
                startActivityForResult(take, PICK_CAMERA_REQUEST);
            } else {
                Intent pick = new Intent(Intent.ACTION_GET_CONTENT);
                pick.addCategory(Intent.CATEGORY_OPENABLE);
                pick.setType("image/*");
                pick.putExtra(Intent.EXTRA_MIME_TYPES, new String[]{
                    "image/png", "image/jpeg", "image/webp", "image/svg+xml", "image/*"
                });
                startActivityForResult(Intent.createChooser(pick, "Choose image"), PICK_ICON_REQUEST);
            }
        } catch (Exception e) {
            setNotice("Could not open picker: " + e.getMessage());
        }
    }

    void applyPendingCustom(String name) {
        Bitmap bmp = pendingCustomIcon;
        if (bmp == null) bmp = AppearanceStore.customIcon(this);
        if (bmp == null) {
            setNotice("Pick an image first.");
            return;
        }
        try {
            AppearanceStore.saveCustom(this, name, bmp);
            refreshChromeTitle();
            setNotice("Custom look applied. Accept the home-screen shortcut if Android asks.");
            rebuildSettings();
        } catch (Exception e) {
            setNotice("Could not save icon: " + e.getMessage());
        }
    }

    void openExternalUrl(String url) {
        try {
            startActivity(new Intent(Intent.ACTION_VIEW, Uri.parse(url)));
        } catch (Exception e) {
            setNotice("No browser available.");
        }
    }

    private void refreshAppProtectPage() {
        if (appProtectPage == null || appProtectPage.getVisibility() != View.VISIBLE) return;
        List<AppCatalog.AppInfo> apps = AppCatalog.snapshot();
        if (!apps.isEmpty()) fillAppProtectList(apps);
    }

    private void fillAppProtectList(List<AppCatalog.AppInfo> apps) {
        if (appProtectList == null) return;
        appProtectList.removeAllViews();
        if (apps == null || apps.isEmpty()) {
            TextView empty = muted("No launchable apps found");
            empty.setPadding(dp(8), dp(16), dp(8), dp(16));
            appProtectList.addView(empty, mw());
            return;
        }
        List<AppCatalog.AppInfo> browsers = new ArrayList<AppCatalog.AppInfo>();
        List<AppCatalog.AppInfo> others = new ArrayList<AppCatalog.AppInfo>();
        for (int i = 0; i < apps.size(); i++) {
            AppCatalog.AppInfo a = apps.get(i);
            if (a.browser) browsers.add(a);
            else others.add(a);
        }
        addAppSection("Browsers", Icons.BROWSER, browsers);
        addAppSection("Other apps", Icons.APPS, others);
    }

    private void addAppSection(String title, int iconKind, List<AppCatalog.AppInfo> apps) {
        LinearLayout head = new LinearLayout(this);
        head.setOrientation(LinearLayout.HORIZONTAL);
        head.setGravity(Gravity.CENTER_VERTICAL);
        head.setPadding(dp(4), dp(16), dp(4), dp(6));
        View ic = Icons.of(this, iconKind, 0xFF00FF7F);
        LinearLayout.LayoutParams ilp = new LinearLayout.LayoutParams(dp(14), dp(14));
        ilp.rightMargin = dp(8);
        head.addView(ic, ilp);
        TextView t = new TextView(this);
        t.setText(title);
        t.setTextColor(0xFF8A9098);
        t.setTextSize(12);
        t.setTypeface(Typeface.create("sans-serif-medium", Typeface.NORMAL));
        t.setAllCaps(true);
        t.setLetterSpacing(0.06f);
        head.addView(t, vw(), vw());
        appProtectList.addView(head, mw());
        if (apps.isEmpty()) {
            TextView empty = muted("None found");
            empty.setPadding(dp(8), dp(6), dp(8), dp(10));
            appProtectList.addView(empty, mw());
            return;
        }
        for (int i = 0; i < apps.size(); i++) {
            appProtectList.addView(makeAppProtectRow(apps.get(i)), mw());
        }
    }

    private View makeAppProtectRow(final AppCatalog.AppInfo app) {
        LinearLayout row = new LinearLayout(this);
        row.setOrientation(LinearLayout.HORIZONTAL);
        row.setGravity(Gravity.CENTER_VERTICAL);
        row.setPadding(dp(10), dp(8), dp(10), dp(8));
        row.setMinimumHeight(dp(52));
        ImageView icon = new ImageView(this);
        if (app.icon != null) icon.setImageDrawable(app.icon);
        else icon.setBackgroundColor(0xFF2A2E36);
        LinearLayout.LayoutParams ilp = new LinearLayout.LayoutParams(dp(34), dp(34));
        ilp.rightMargin = dp(12);
        row.addView(icon, ilp);
        TextView label = new TextView(this);
        label.setText(app.label);
        label.setTextColor(Color.WHITE);
        label.setTextSize(14);
        label.setSingleLine(true);
        label.setEllipsize(TextUtils.TruncateAt.END);
        row.addView(label, new LinearLayout.LayoutParams(0, vw(), 1f));
        final GreenSwitch sw = new GreenSwitch(this);
        sw.setOn(AppSplitStore.isProtected(this, app.pkg), false);
        sw.setOnToggle(new GreenSwitch.OnToggle() {
            @Override public void onToggle(boolean on) {
                AppSplitStore.setAppProtected(
                    MainActivity.this, app.pkg, on, AppCatalog.allPackages()
                );
                updateAppSplitSummary();
                refreshAppProtectPage();
                noticeReconnectApps();
            }
        });
        row.addView(sw, dp(42), dp(26));
        return row;
    }

    private View buildBottomActions() {
        LinearLayout bar = new LinearLayout(this);
        bar.setOrientation(LinearLayout.HORIZONTAL);
        bar.setGravity(Gravity.CENTER);
        bar.setPadding(dp(16), dp(18), dp(16), dp(10));

        LinearLayout pair = new LinearLayout(this);
        pair.setOrientation(LinearLayout.HORIZONTAL);
        pair.setGravity(Gravity.CENTER);
        pair.addView(footerIconButton(R.drawable.ic_settings, "Settings", new View.OnClickListener() {
            @Override public void onClick(View v) {
                v.animate().rotationBy(60f).setDuration(220).start();
                openSettings(null);
            }
        }));
        View spacer = new View(this);
        pair.addView(spacer, new LinearLayout.LayoutParams(dp(36), 1));
        pair.addView(footerIconButton(R.drawable.ic_guide, "Guide", new View.OnClickListener() {
            @Override public void onClick(View v) { openGuide(); }
        }));
        bar.addView(pair, vw(), vw());
        return bar;
    }

    private View footerIconButton(int drawable, String label, View.OnClickListener l) {
        LinearLayout col = new LinearLayout(this);
        col.setOrientation(LinearLayout.VERTICAL);
        col.setGravity(Gravity.CENTER_HORIZONTAL);
        col.setPadding(dp(16), dp(8), dp(16), dp(8));
        col.setClickable(true);
        col.setFocusable(true);
        ImageView icon = Icons.vector(this, drawable, 0xFFE3E3E3);
        col.addView(icon, dp(26), dp(26));
        TextView t = new TextView(this);
        t.setText(label);
        t.setTextColor(0xFFB8BDC4);
        t.setTextSize(10);
        t.setGravity(Gravity.CENTER);
        t.setPadding(0, dp(4), 0, 0);
        t.setIncludeFontPadding(false);
        col.addView(t, vw(), vw());
        col.setOnClickListener(l);
        return col;
    }

    // ─── Connect / disconnect ─────────────────────────────────────────────

    private void onPrimaryConnectClick() {
        if (isSessionActive()) {
            disconnectAll();
            return;
        }
        showProgressCard();
        switch (protocolIndex) {
            case 0: connectWireGuard(); break;
            case 1: connectOutline(); break;
            case 2: connectTorFull(); break;
            default: connectWireGuard(); break;
        }
    }

    private boolean isSessionActive() {
        return connecting || vpnActive || torSocksUp
            || "connected".equals(activePhase)
            || "connecting".equals(activePhase);
    }

    private void updatePrimaryButton() {
        if (primaryConnectBtn == null) return;
        boolean active = isSessionActive();
        String label;
        int bg;
        int fg;
        if (protocolIndex == 2) {
            label = active ? "Disconnect Tor" : "Connect Tor";
            bg = active ? 0xFF2A2D35 : 0xFFA855F7;
            fg = Color.WHITE;
        } else {
            label = active ? "Disconnect" : "Connect";
            bg = active ? 0xFF2A2D35 : 0xFF00FF7F;
            fg = active ? Color.WHITE : Color.BLACK;
        }
        styleButton(primaryConnectBtn, label, bg, fg);
        stylePrimaryConnect(primaryConnectBtn);
    }

    private void connectWireGuard() {
        String conf = activeConfigText();
        if (conf.isEmpty()) {
            setNotice("Add a WireGuard connection first.");
            showAddConnectionDialog(ProfileStore.KIND_WG);
            return;
        }
        prefs().edit().putString("wg_profile", conf).apply();
        // Auto-save as named profile (Windows-like)
        ProfileStore.Profile auto = ProfileStore.save(
            this, selectedProfileId.length() > 0 ? selectedProfileId : null,
            ProfileStore.KIND_WG, null, conf, "", "", ""
        );
        selectedProfileId = auto.id;
        String parsed = NativeBridge.parseWireGuard(conf);
        Map<String, String> kv = parseKV(parsed);
        if (!"OK".equals(kv.get("status"))) {
            setNotice(kv.get("message") != null ? kv.get("message") : "Invalid WireGuard config");
            return;
        }
        String clientIp = "10.8.0.2";
        String addr = kv.get("address");
        if (addr != null && addr.length() > 0) {
            // Prefer first IPv4 address from Address= line
            for (String part : addr.split(",")) {
                String first = part.trim();
                int slash = first.indexOf('/');
                if (slash > 0) first = first.substring(0, slash);
                if (first.indexOf(':') < 0 && first.indexOf('.') > 0) {
                    clientIp = first;
                    break;
                }
            }
        }
        File f = writeTempProfile("wg.conf", conf);
        pendingVpn = PendingVpn.wireguard(f.getAbsolutePath(), kv.get("endpoint"), clientIp);
        persistPendingVpn(pendingVpn);
        // DNS from conf when present
        String dns = dnsFromConfText(conf);
        if (dns != null && dns.length() > 0) pendingVpn.dns = dns;
        connecting = true;
        setProgressUi("wireguard", 0.15f, "requesting VPN permission");
        requestVpnPermission();
    }

    private static String dnsFromConfText(String conf) {
        if (conf == null) return null;
        for (String line : conf.split("\n")) {
            String t = line.trim();
            if (t.isEmpty() || t.startsWith("#")) continue;
            int eq = t.indexOf('=');
            if (eq <= 0) continue;
            if (!"DNS".equalsIgnoreCase(t.substring(0, eq).trim())) continue;
            String val = t.substring(eq + 1).trim();
            if (val.isEmpty()) continue;
            int comma = val.indexOf(',');
            return comma > 0 ? val.substring(0, comma).trim() : val;
        }
        return null;
    }


    private void connectOutline() {
        String key = activeConfigText();
        if (key.isEmpty()) {
            setNotice("Add a Shadowsocks / Outline connection first.");
            showAddConnectionDialog(ProfileStore.KIND_OUTLINE);
            return;
        }
        prefs().edit().putString("outline_key", key).apply();
        ProfileStore.Profile auto = ProfileStore.save(
            this, selectedProfileId.length() > 0 ? selectedProfileId : null,
            ProfileStore.KIND_OUTLINE, null, key, "", "", ""
        );
        selectedProfileId = auto.id;
        String parsed = NativeBridge.parseOutline(key);
        Map<String, String> kv = parseKV(parsed);
        if (!"OK".equals(kv.get("status"))) {
            setNotice(kv.get("message") != null ? kv.get("message") : "Invalid Outline key");
            return;
        }
        pendingVpn = PendingVpn.outline(
            kv.get("host"), kv.get("port"), kv.get("password"), kv.get("method"), key
        );
        persistPendingVpn(pendingVpn);
        connecting = true;
        setProgressUi("outline", 0.15f, "requesting VPN permission");
        requestVpnPermission();
    }

    private void connectTorFull() {
        if (!TorBundle.isArm64Device()) {
            setNotice(getString(R.string.tor_arm64_only));
            return;
        }
        connecting = true;
        setNotice("Starting Tor…");
        activePhase = "connecting";
        updateConnectionPill("connecting", "Tor");
        setProgressUi("tor", 0.08f, "preparing expert bundle");
        updatePrimaryButton();

        new Thread(new Runnable() {
            @Override
            public void run() {
                try {
                    File home = TorBundle.ensureExtracted(MainActivity.this);
                    BridgeStore.writeTorrcExtra(MainActivity.this, home);
                    postProgress("tor", 0.18f, BridgeStore.enabled(MainActivity.this)
                        ? ("launching libTor.so · " + BridgeStore.summary(MainActivity.this))
                        : "launching libTor.so");
                    final String result = NativeBridge.startTorSocks(
                        home.getAbsolutePath(),
                        TorBundle.nativeLibDir(MainActivity.this)
                    );
                    final Map<String, String> kv = parseKV(result);
                    if (!"OK".equals(kv.get("status"))) {
                        handler.post(new Runnable() {
                            @Override public void run() {
                                connecting = false;
                                activePhase = "error";
                                updateConnectionPill("error", null);
                                setProgressUi("tor", 0f, nz(kv.get("message")));
                                setNotice(kv.get("message") != null ? kv.get("message") : result);
                                updatePrimaryButton();
                            }
                        });
                        return;
                    }
                    int port = 0;
                    try {
                        if (kv.get("socks_port") != null) {
                            port = Integer.parseInt(kv.get("socks_port"));
                        }
                    } catch (NumberFormatException ignored) {
                    }
                    torSocksUp = true;
                    torSocksPort = port;
                    postProgress("tor", 0.55f, "SOCKS ready :" + port + " · building circuits");
                    // Wait for bootstrap progress without resetting UI fraction
                    for (int i = 0; i < 20; i++) {
                        String boot = NativeBridge.torBootstrap();
                        if (boot != null && boot.startsWith("OK")) {
                            postProgress("tor", 0.68f, "circuits ready");
                            break;
                        }
                        postProgress("tor", 0.55f + (i * 0.006f),
                            boot != null ? boot : "bootstrapping…");
                        Thread.sleep(500);
                    }
                    postProgress("tor", 0.72f, "attaching system tunnel");
                    handler.post(new Runnable() {
                        @Override
                        public void run() {
                            pendingVpn = PendingVpn.tor();
                            setNotice("Tor SOCKS ready — starting device tunnel…");
                            requestVpnPermission();
                        }
                    });
                } catch (Exception e) {
                    final String msg = e.getMessage();
                    handler.post(new Runnable() {
                        @Override public void run() {
                            connecting = false;
                            activePhase = "error";
                            updateConnectionPill("error", null);
                            setProgressUi("tor", 0f, msg != null ? msg : "Tor failed");
                            setNotice("Tor failed: " + msg);
                            updatePrimaryButton();
                        }
                    });
                }
            }
        }, "zn-tor-full").start();
    }

    private void postProgress(final String stage, final float frac, final String detail) {
        handler.post(new Runnable() {
            @Override public void run() {
                setProgressUi(stage, frac, detail);
            }
        });
    }

    private void disconnectAll() {
        connecting = false;
        vpnActive = false;
        torSocksUp = false;
        torSocksPort = 0;
        pendingVpn = null;
        clearPendingVpnPersist();
        activeServerId = null;
        activePhase = "disconnected";
        targetProgress = 0f;
        displayProgress = 0f;
        cancelProgressHide();
        setNotice("Disconnecting…");
        setProgressUi("idle", 0f, "disconnecting");
        updateConnectionPill("disconnected", null);
        updatePrimaryButton();
        applyProgressDisplay();

        // Stop VpnService first (kills TUN + notification), then native engines.
        try {
            Intent stop = ZeroNodeVpnService.stopIntent(this);
            if (Build.VERSION.SDK_INT >= 26) {
                try {
                    startForegroundService(stop);
                } catch (Exception e) {
                    startService(stop);
                }
            } else {
                startService(stop);
            }
        } catch (Exception e) {
            try {
                stopService(ZeroNodeVpnService.stopIntent(this));
            } catch (Exception ignored) {
            }
        }

        new Thread(new Runnable() {
            @Override
            public void run() {
                try {
                    NativeBridge.disconnect();
                } catch (Exception ignored) {
                }
                try {
                    NativeBridge.stopEverything();
                } catch (Exception ignored) {
                }
                handler.post(new Runnable() {
                    @Override
                    public void run() {
                        setNotice("Disconnected.");
                        setProgressUi("idle", 0f, "Idle");
                        updatePrimaryButton();
                        refreshPublicIp(true);
                        renderServerList();
                    }
                });
            }
        }, "zn-disconnect").start();
    }

    // ─── Generic auth callback (server password etc.) ─────────────────────

    private interface AuthCallback {
        void onAuth(String user, String pass);
    }

    private void showAuthPopup(final AuthCallback cb) {
        LinearLayout layout = new LinearLayout(this);
        layout.setOrientation(LinearLayout.VERTICAL);
        layout.setPadding(dp(22), dp(12), dp(22), dp(4));
        final EditText user = singleLine("Username");
        final EditText pass = singleLine("Password");
        layout.addView(user, mw());
        layout.addView(passwordRow(pass), mw());
        new AlertDialog.Builder(this, android.R.style.Theme_Material_Dialog_Alert)
            .setTitle("Authenticate")
            .setMessage("This connection needs a username and password.")
            .setView(layout)
            .setPositiveButton(R.string.btn_connect, new DialogInterface.OnClickListener() {
                @Override
                public void onClick(DialogInterface dialog, int which) {
                    cb.onAuth(user.getText().toString(), pass.getText().toString());
                }
            })
            .setNegativeButton(R.string.dialog_cancel, null)
            .show();
    }

    // ─── Import / drag-drop ───────────────────────────────────────────────

    private void pickImport(String target) {
        importTarget = target;
        Intent intent = new Intent(Intent.ACTION_OPEN_DOCUMENT);
        intent.addCategory(Intent.CATEGORY_OPENABLE);
        intent.setType("*/*");
        intent.putExtra(Intent.EXTRA_MIME_TYPES, new String[]{
            "text/plain", "application/octet-stream", "*/*"
        });
        try {
            startActivityForResult(intent, IMPORT_REQUEST_CODE);
        } catch (Exception e) {
            setNotice("File picker unavailable: " + e.getMessage());
        }
    }

    private void handleIncomingShare(Intent intent) {
        if (intent == null) return;
        String action = intent.getAction();
        if (Intent.ACTION_SEND.equals(action) && intent.getParcelableExtra(Intent.EXTRA_STREAM) != null) {
            Uri uri = intent.getParcelableExtra(Intent.EXTRA_STREAM);
            if (uri != null) {
                guessImportTarget(uri);
                readImportedFile(uri);
            }
        } else if (Intent.ACTION_VIEW.equals(action) && intent.getData() != null) {
            Uri uri = intent.getData();
            guessImportTarget(uri);
            readImportedFile(uri);
        }
    }

    private void guessImportTarget(Uri uri) {
        String path = uri != null ? uri.getLastPathSegment() : null;
        if (path == null && uri != null) path = uri.toString();
        if (path == null) path = "";
        String lower = path.toLowerCase(Locale.US);
        if (lower.endsWith(".conf") || lower.contains("wireguard") || lower.contains("wg-")) {
            importTarget = "wg";
        } else if (lower.contains("outline") || lower.contains("ss://") || lower.endsWith(".json")) {
            importTarget = "outline";
        } else {
            importTarget = protocolIndex == 1 ? "outline" : "wg";
        }
    }

    /** Content-aware protocol switch + target after reading file body. */
    private void applySmartImport(String text, String fileName) {
        String detected = ProfileStore.detectKind(text, fileName);
        if (detected == null) {
            // fall back to current importTarget
            if ("outline".equals(importTarget)) detected = ProfileStore.KIND_OUTLINE;
            else detected = ProfileStore.KIND_WG;
        }
        int targetIdx = 0;
        String target = "wg";
        if (ProfileStore.KIND_OUTLINE.equals(detected)) {
            targetIdx = 1;
            target = "outline";
        } else {
            targetIdx = 0;
            target = "wg";
        }
        importTarget = target;
        if (protocolIndex != targetIdx) {
            selectProtocol(targetIdx);
        }
        if (addDialogContent != null) {
            addDialogContent.setText(text);
            setNotice("Imported into the add-connection window.");
            return;
        }
        if (profileInput != null) {
            profileInput.setText(text);
        }
        ProfileStore.Profile saved = ProfileStore.save(
            this, null, detected, null, text, "", "", ""
        );
        applySavedProfile(saved, false);
        setNotice("Imported as " + kindLabel(detected) + " · saved: " + saved.name);
    }

    /**
     * File-dock / multi-window drag-and-drop (OEM sidebars + cross-app DnD).
     * Copies dropped WireGuard / Outline files into private storage then imports.
     * Shows a full-screen drop zone while the drag is over the app.
     */
    private void installDropTarget(View root) {
        root.setOnDragListener(new View.OnDragListener() {
            @Override
            public boolean onDrag(View v, DragEvent event) {
                switch (event.getAction()) {
                    case DragEvent.ACTION_DRAG_STARTED: {
                        android.content.ClipDescription desc = event.getClipDescription();
                        boolean accept = desc == null
                            || desc.hasMimeType(android.content.ClipDescription.MIMETYPE_TEXT_URILIST)
                            || desc.hasMimeType(android.content.ClipDescription.MIMETYPE_TEXT_PLAIN)
                            || desc.hasMimeType("*/*")
                            || desc.getMimeTypeCount() > 0;
                        if (accept) {
                            // Show overlay as soon as a drag begins over the app
                            showDropOverlay(true);
                        }
                        return accept;
                    }
                    case DragEvent.ACTION_DRAG_ENTERED:
                        showDropOverlay(true);
                        return true;
                    case DragEvent.ACTION_DRAG_LOCATION:
                        return true;
                    case DragEvent.ACTION_DRAG_EXITED:
                        // Keep overlay while drag session active; hide only on END
                        return true;
                    case DragEvent.ACTION_DROP: {
                        showDropOverlay(false);
                        return handleFileDockDrop(event);
                    }
                    case DragEvent.ACTION_DRAG_ENDED:
                        showDropOverlay(false);
                        return true;
                    default:
                        return false;
                }
            }
        });
    }

    private void showDropOverlay(boolean on) {
        if (dropOverlay == null) return;
        dropOverlay.setVisibility(on ? View.VISIBLE : View.GONE);
        if (on && dropOverlayTitle != null) {
            dropOverlayTitle.setText("Drop VPN profile here");
            dropOverlayHint.setText("Accepts WireGuard .conf and Outline keys");
        }
    }

    private boolean handleFileDockDrop(DragEvent event) {
        // 1) Temporary URI permission for system File Dock / SuperHub / multi-window
        DragAndDropPermissions dropPermissions = null;
        try {
            dropPermissions = requestDragAndDropPermissions(event);
        } catch (Exception ignored) {
        }

        final DragAndDropPermissions perms = dropPermissions;
        try {
            ClipData clip = event.getClipData();
            if (clip == null || clip.getItemCount() == 0) {
                setNotice("Drop ignored — no file data.");
                return false;
            }

            final List<Uri> uris = new ArrayList<>();
            for (int i = 0; i < clip.getItemCount(); i++) {
                ClipData.Item item = clip.getItemAt(i);
                if (item == null) continue;
                Uri uri = item.getUri();
                if (uri != null) {
                    uris.add(uri);
                    continue;
                }
                // Some docks put path/text only
                CharSequence text = item.coerceToText(this);
                if (text != null && text.length() > 20) {
                    String body = text.toString();
                    String detected = ProfileStore.detectKind(body, "dropped.txt");
                    if (ProfileStore.KIND_WG.equals(detected)
                        || ProfileStore.KIND_OUTLINE.equals(detected)) {
                        applySmartImport(body, ProfileStore.KIND_OUTLINE.equals(detected)
                            ? "dropped.json" : "dropped.conf");
                        return true;
                    }
                }
            }

            if (uris.isEmpty()) {
                setNotice("Drop ignored — only WireGuard .conf or Outline keys are accepted.");
                return false;
            }

            // Process on background thread; release permissions when done
            new Thread(new Runnable() {
                @Override
                public void run() {
                    try {
                        int accepted = 0;
                        for (Uri uri : uris) {
                            String name = resolveDisplayName(uri);
                            if (!isAcceptedProfileFile(name, uri)) {
                                final String rejected = name != null ? name : uri.toString();
                                handler.post(new Runnable() {
                                    @Override public void run() {
                                        setNotice("Ignored (not a WireGuard/Outline profile): " + rejected);
                                    }
                                });
                                continue;
                            }
                            // Private sandbox copy — never store content:// long-term
                            File local = copyUriToPrivateDock(uri, name);
                            if (local == null) continue;
                            final String text = readFileUtf8(local);
                            final String fileName = local.getName();
                            if (text == null || text.trim().isEmpty()) continue;
                            accepted++;
                            handler.post(new Runnable() {
                                @Override public void run() {
                                    applySmartImport(text, fileName);
                                }
                            });
                        }
                        if (accepted == 0) {
                            handler.post(new Runnable() {
                                @Override public void run() {
                                    setNotice("No WireGuard or Outline profiles in drop.");
                                }
                            });
                        }
                    } finally {
                        if (perms != null) {
                            try {
                                perms.release();
                            } catch (Exception ignored) {
                            }
                        }
                    }
                }
            }, "zn-dock-drop").start();
            return true;
        } catch (Exception e) {
            if (perms != null) {
                try {
                    perms.release();
                } catch (Exception ignored) {
                }
            }
            setNotice("Drop failed: " + e.getMessage());
            return false;
        }
    }

    /** WireGuard .conf and Outline/ss keys for file-dock drops. */
    private boolean isAcceptedProfileFile(String displayName, Uri uri) {
        String name = displayName != null ? displayName : "";
        String lower = name.toLowerCase(Locale.US);
        if (lower.endsWith(".conf") || lower.endsWith(".json") || lower.endsWith(".txt")) return true;
        if (uri != null) {
            String path = uri.getLastPathSegment();
            if (path != null) {
                String pl = path.toLowerCase(Locale.US);
                if (pl.endsWith(".conf") || pl.endsWith(".json") || pl.contains(".conf")) {
                    return true;
                }
            }
        }
        return false;
    }

    private String resolveDisplayName(Uri uri) {
        if (uri == null) return null;
        String name = null;
        try {
            android.database.Cursor c = getContentResolver().query(uri, null, null, null, null);
            if (c != null) {
                try {
                    int idx = c.getColumnIndex(OpenableColumns.DISPLAY_NAME);
                    if (c.moveToFirst() && idx >= 0) {
                        name = c.getString(idx);
                    }
                } finally {
                    c.close();
                }
            }
        } catch (Exception ignored) {
        }
        if (name == null || name.length() == 0) {
            name = uri.getLastPathSegment();
        }
        return name;
    }

    /**
     * Stream content:// into app-private storage so access survives after
     * drag permissions are released.
     */
    private File copyUriToPrivateDock(Uri uri, String displayName) {
        try {
            File dir = new File(getFilesDir(), "private_dock_files");
            if (!dir.exists()) dir.mkdirs();
            String safe = displayName != null ? displayName : ("dropped_" + System.currentTimeMillis());
            // Strip path separators from display name
            safe = safe.replace('/', '_').replace('\\', '_');
            if (!safe.toLowerCase(Locale.US).endsWith(".conf")
                && !safe.toLowerCase(Locale.US).endsWith(".json")
                && !safe.toLowerCase(Locale.US).endsWith(".txt")) {
                String path = uri.getLastPathSegment();
                if (path != null && path.toLowerCase(Locale.US).contains(".json")) {
                    safe = safe + ".json";
                } else {
                    safe = safe + ".conf";
                }
            }
            File local = new File(dir, safe);
            InputStream in = getContentResolver().openInputStream(uri);
            if (in == null) return null;
            try {
                FileOutputStream out = new FileOutputStream(local);
                try {
                    byte[] buf = new byte[8192];
                    int n;
                    int total = 0;
                    while ((n = in.read(buf)) >= 0) {
                        out.write(buf, 0, n);
                        total += n;
                        if (total > 2 * 1024 * 1024) {
                            throw new IOException("file too large (>2MB)");
                        }
                    }
                } finally {
                    out.close();
                }
            } finally {
                in.close();
            }
            return local;
        } catch (Exception e) {
            Log.w("ZeroNode", "dock copy failed: " + e.getMessage());
            return null;
        }
    }

    private static String readFileUtf8(File f) {
        try {
            InputStream in = new java.io.FileInputStream(f);
            try {
                ByteArrayOutputStream bos = new ByteArrayOutputStream();
                byte[] buf = new byte[8192];
                int n;
                while ((n = in.read(buf)) >= 0) bos.write(buf, 0, n);
                return new String(bos.toByteArray(), StandardCharsets.UTF_8);
            } finally {
                in.close();
            }
        } catch (Exception e) {
            return null;
        }
    }

    private void readImportedFile(final Uri uri) {
        setNotice("Reading file…");
        new Thread(new Runnable() {
            @Override
            public void run() {
                try {
                    String name = resolveDisplayName(uri);
                    // Picker / share: prefer .conf / Outline keys, still sniff content
                    File local = copyUriToPrivateDock(uri,
                        name != null ? name : "import_" + System.currentTimeMillis());
                    final String text;
                    final String nameHint;
                    if (local != null) {
                        text = readFileUtf8(local);
                        nameHint = local.getName();
                    } else {
                        InputStream in = getContentResolver().openInputStream(uri);
                        if (in == null) throw new Exception("could not open file");
                        ByteArrayOutputStream bos = new ByteArrayOutputStream();
                        byte[] buf = new byte[8192];
                        int n;
                        int total = 0;
                        while ((n = in.read(buf)) >= 0) {
                            bos.write(buf, 0, n);
                            total += n;
                            if (total > 2 * 1024 * 1024) {
                                throw new Exception("file too large (>2MB)");
                            }
                        }
                        in.close();
                        text = new String(bos.toByteArray(), StandardCharsets.UTF_8);
                        nameHint = name != null ? name : uri.getLastPathSegment();
                    }
                    if (text == null || text.trim().isEmpty()) {
                        throw new Exception("empty file");
                    }
                    handler.post(new Runnable() {
                        @Override
                        public void run() {
                            applySmartImport(text, nameHint != null ? nameHint : "import");
                        }
                    });
                } catch (Exception e) {
                    final String msg = e.getMessage();
                    handler.post(new Runnable() {
                        @Override public void run() {
                            setNotice("Import failed: " + msg);
                        }
                    });
                }
            }
        }, "zn-import").start();
    }

    // ─── Discovery ────────────────────────────────────────────────────────

    private void runDiscovery() {
        runDiscovery(true);
    }

    private void runDiscovery(final boolean announce) {
        if (announce) setNotice("Discovering servers…");
        new Thread(new Runnable() {
            @Override
            public void run() {
                final String result = NativeBridge.discover("");
                final List<ServerInfo> parsed = parseServers(result);
                handler.post(new Runnable() {
                    @Override
                    public void run() {
                        servers = parsed;
                        renderServerList();
                        if (announce) {
                            setNotice(servers.isEmpty() ? "No servers found. Add a host." : null);
                        }
                    }
                });
            }
        }, "zn-discover").start();
    }

    private void runDiscoveryFor(final String host) {
        setNotice("Discovering " + host + "…");
        new Thread(new Runnable() {
            @Override
            public void run() {
                final String result = NativeBridge.discover(host);
                final List<ServerInfo> parsed = parseServers(result);
                handler.post(new Runnable() {
                    @Override
                    public void run() {
                        for (ServerInfo s : parsed) {
                            boolean exists = false;
                            for (int i = 0; i < servers.size(); i++) {
                                if (servers.get(i).id.equals(s.id)) {
                                    servers.set(i, s);
                                    exists = true;
                                    break;
                                }
                            }
                            if (!exists) servers.add(s);
                        }
                        renderServerList();
                        setNotice(null);
                    }
                });
            }
        }, "zn-discover-host").start();
    }

    private void runConnectServer(final ServerInfo server, final String password) {
        setNotice("Connecting to " + server.name + "…");
        connecting = true;
        updateConnectionPill("connecting", server.name);
        updatePrimaryButton();
        new Thread(new Runnable() {
            @Override
            public void run() {
                final String result = NativeBridge.connect(server.endpoint, password);
                final Map<String, String> values = parseKV(result);
                handler.post(new Runnable() {
                    @Override
                    public void run() {
                        if ("OK".equals(values.get("status"))) {
                            activeServerId = values.get("server_id");
                            if (activeServerId == null) activeServerId = server.id;
                            activePhase = "connected";
                            updateConnectionPill("connected", values.get("server"));
                            if (globeView != null) {
                                globeView.panToCountry(server.countryCode, server.name);
                            }
                            pendingVpn = PendingVpn.zeronode(
                                values.get("profile"),
                                values.get("client_ip"),
                                values.get("server")
                            );
                            setNotice("Lease OK — starting VPN…");
                            renderServerList();
                            requestVpnPermission();
                        } else {
                            connecting = false;
                            activePhase = "error";
                            updateConnectionPill("error", null);
                            setNotice(values.get("message") != null
                                ? values.get("message") : "Connection failed");
                            updatePrimaryButton();
                        }
                    }
                });
            }
        }, "zn-connect").start();
    }

    private void renderServerList() {
        serverListContainer.removeAllViews();
        if (servers.isEmpty()) {
            noticeText.setText(R.string.label_no_servers);
            return;
        }
        int online = 0;
        for (ServerInfo s : servers) if (s.online) online++;
        noticeText.setText(String.format(Locale.US, "%d online / %d offline",
            online, servers.size() - online));
        for (ServerInfo server : servers) {
            serverListContainer.addView(buildServerCard(server));
        }
    }

    private View buildServerCard(final ServerInfo server) {
        boolean isActive = server.id.equals(activeServerId) && !"disconnected".equals(activePhase);
        LinearLayout card = new LinearLayout(this);
        card.setOrientation(LinearLayout.VERTICAL);
        card.setPadding(dp(12), dp(10), dp(12), dp(10));
        GradientDrawable bg = new GradientDrawable();
        bg.setCornerRadius(dp(10));
        bg.setColor(isActive ? 0xFF101C16 : 0xFF0A0A0A);
        bg.setStroke(dp(1), isActive ? 0xFF00FF7F : 0xFF1A1A1A);
        card.setBackground(bg);

        LinearLayout top = new LinearLayout(this);
        top.setOrientation(LinearLayout.HORIZONTAL);
        top.setGravity(Gravity.CENTER_VERTICAL);
        TextView flag = new TextView(this);
        flag.setText(countryFlag(server.countryCode));
        flag.setTextSize(20);
        flag.setPadding(0, 0, dp(8), 0);
        top.addView(flag, vw(), vw());
        LinearLayout names = new LinearLayout(this);
        names.setOrientation(LinearLayout.VERTICAL);
        TextView name = new TextView(this);
        name.setText(server.name);
        name.setTextColor(server.online ? Color.WHITE : 0xFF444444);
        name.setTypeface(null, Typeface.BOLD);
        name.setTextSize(14);
        names.addView(name, mw());
        names.addView(muted(server.countryName), mw());
        top.addView(names, new LinearLayout.LayoutParams(0, vw(), 1f));
        card.addView(top, mw());

        LinearLayout bottom = new LinearLayout(this);
        bottom.setOrientation(LinearLayout.HORIZONTAL);
        bottom.setGravity(Gravity.CENTER_VERTICAL);
        bottom.setPadding(0, dp(6), 0, 0);
        TextView ep = muted(server.hasPassword ? maskEndpoint(server.endpoint) : server.endpoint);
        ep.setTypeface(Typeface.MONOSPACE);
        bottom.addView(ep, new LinearLayout.LayoutParams(0, vw(), 1f));
        if (isActive) {
            Button b = accentButton("Connected");
            b.setEnabled(false);
            b.setAlpha(0.7f);
            bottom.addView(b, dp(110), dp(40));
        } else if (server.online) {
            Button b = accentButton(getString(R.string.btn_connect));
            b.setOnClickListener(new View.OnClickListener() {
                @Override
                public void onClick(View v) {
                    if (server.hasPassword) showPasswordDialog(server);
                    else runConnectServer(server, null);
                }
            });
            bottom.addView(b, dp(110), dp(40));
        } else {
            Button b = secondaryButton("Offline");
            b.setEnabled(false);
            b.setAlpha(0.4f);
            bottom.addView(b, dp(110), dp(40));
        }
        card.addView(bottom, mw());
        LinearLayout.LayoutParams lp = mw();
        lp.topMargin = dp(6);
        card.setLayoutParams(lp);
        return card;
    }

    private void showPasswordDialog(final ServerInfo server) {
        LinearLayout layout = new LinearLayout(this);
        layout.setOrientation(LinearLayout.VERTICAL);
        layout.setPadding(dp(22), dp(12), dp(22), dp(4));
        final EditText passwordField = singleLine(getString(R.string.dialog_password_hint));
        layout.addView(passwordRow(passwordField), mw());
        new AlertDialog.Builder(this, android.R.style.Theme_Material_Dialog_Alert)
            .setTitle("Authenticate to " + server.name)
            .setView(layout)
            .setPositiveButton(R.string.btn_connect, new DialogInterface.OnClickListener() {
                @Override
                public void onClick(DialogInterface dialog, int which) {
                    String password = passwordField.getText().toString();
                    runConnectServer(server, password.isEmpty() ? null : password);
                }
            })
            .setNegativeButton(R.string.dialog_cancel, null)
            .show();
    }

    // ─── IP / progress / VPN permission ───────────────────────────────────

    /**
     * Real public-IP refresh (IPv4 and/or IPv6).
     * <ul>
     *   <li>Outline / Tor → SOCKS exit (package excluded from VPN)</li>
     *   <li>WireGuard → app rides the TUN (no disallow); plain HTTP = exit IP</li>
     *   <li>Generic VPN Network bind as fallback</li>
     *   <li>Offline → UI stays open, shows "No internet"</li>
     * </ul>
     */
    private void refreshPublicIp(final boolean forcePan) {
        final int gen = ipRefreshGen.incrementAndGet();
        final boolean tunnelUp = vpnActive || ZeroNodeVpnService.isRunning() || torSocksUp;
        final String kind = ZeroNodeVpnService.lastKind();
        setRefreshLoading(true);

        new Thread(new Runnable() {
            @Override
            public void run() {
                if (gen != ipRefreshGen.get()) return;

                String v4 = null;
                String v6 = "";
                int attempts = tunnelUp ? 5 : 2;
                for (int i = 0; i < attempts; i++) {
                    if (gen != ipRefreshGen.get()) return;
                    IpLookupResult r = lookupPublicIps(tunnelUp);
                    if (r.v4Ok || r.v6Ok) {
                        v4 = r.v4;
                        v6 = r.v6 != null ? r.v6 : "";
                        break;
                    }
                    try {
                        Thread.sleep(tunnelUp ? 700 : 250);
                    } catch (InterruptedException ignored) {
                    }
                }

                if (gen != ipRefreshGen.get()) return;
                final String finalV4 = v4;
                final String finalV6 = v6;
                final boolean viaTunnel = tunnelUp;
                handler.post(new Runnable() {
                    @Override
                    public void run() {
                        if (gen != ipRefreshGen.get()) return;
                        applyIpLookupResult(finalV4, finalV6, viaTunnel, forcePan);
                    }
                });
            }
        }, "zn-ip").start();
    }

    private void applyIpLookupResult(
        String v4kv, String ipv6, boolean viaTunnel, boolean forcePan
    ) {
        setRefreshLoading(false);
        Map<String, String> kv = parseKV(v4kv);
        boolean v4ok = "OK".equals(kv.get("status")) && kv.get("ip") != null
            && kv.get("ip").length() > 0;
        boolean v6ok = ipv6 != null && ipv6.length() > 0 && ipv6.contains(":");

        if (!v4ok && !v6ok) {
            publicIp = "No internet";
            publicIpV6 = "";
            publicCountry = "";
            publicCountryCode = "";
            if (viaTunnel) {
                setNotice("Could not read exit IP yet — tap ↻ IP again.");
            } else {
                setNotice(null);
            }
            return;
        }

        if (v4ok) {
            publicIp = kv.get("ip");
            publicCountry = kv.get("country") != null ? kv.get("country") : "";
            publicCountryCode = kv.get("country_code") != null ? kv.get("country_code") : "";
            String city = kv.get("city") != null ? kv.get("city") : "";
            String flag = countryFlag(publicCountryCode);
            String locLabel = city.length() > 0
                ? (city + (publicCountry.length() > 0 ? ", " + publicCountry : ""))
                : publicCountry;
            float lat = Float.NaN, lon = Float.NaN;
            try {
                if (kv.get("lat") != null && kv.get("lat").length() > 0) {
                    lat = Float.parseFloat(kv.get("lat"));
                }
                if (kv.get("lon") != null && kv.get("lon").length() > 0) {
                    lon = Float.parseFloat(kv.get("lon"));
                }
            } catch (NumberFormatException ignored) {
            }
            boolean hasCoords = !Float.isNaN(lat) && !Float.isNaN(lon)
                && !(lat == 0f && lon == 0f)
                && lat >= -90f && lat <= 90f && lon >= -180f && lon <= 180f;
            if (globeView != null) {
                if (forcePan) {
                    if (hasCoords) {
                        globeView.panToExit(lat, lon, flag, publicIp, locLabel);
                    } else if (publicCountryCode.length() == 2) {
                        globeView.setExitBadge(flag, publicIp, locLabel);
                        globeView.panToCountry(publicCountryCode, locLabel.length() > 0
                            ? locLabel : publicIp);
                    } else {
                        globeView.setExitBadge(flag, publicIp, locLabel);
                    }
                } else {
                    globeView.setExitBadge(flag, publicIp, locLabel);
                }
            }
        } else if (v6ok) {
            publicIp = ipv6;
            if (globeView != null) globeView.setExitBadge("", ipv6, "IPv6");
        }

        publicIpV6 = v6ok ? ipv6 : "";
        setNotice(null);
    }

    /** Combined IPv4 + IPv6 public address lookup. */
    private static final class IpLookupResult {
        String v4; // OK\nip=... or ERR
        String v6; // bare IPv6 or empty
        boolean v4Ok;
        boolean v6Ok;
    }

    private IpLookupResult lookupPublicIps(boolean preferTunnel) {
        IpLookupResult r = new IpLookupResult();
        r.v4 = "ERR\nmessage=lookup failed";
        r.v6 = "";

        final String kind = ZeroNodeVpnService.lastKind();
        final boolean isWg = preferTunnel && kind != null
            && ("wireguard".equalsIgnoreCase(kind) || "zeronode".equalsIgnoreCase(kind));
        // WireGuard no longer uses addDisallowedApplication — app sockets ride
        // the TUN, so plain HTTP already sees the exit IP (Windows parity).
        // Outline/Tor still need SOCKS.

        // 1) SOCKS path for Outline / Tor (app excluded from those VPNs)
        int socks = resolveTunnelSocksPort(preferTunnel);
        if (socks > 0) {
            String viaSocks = fetchIpViaSocks(socks);
            if (viaSocks != null && viaSocks.startsWith("OK")) {
                r.v4 = viaSocks;
                r.v4Ok = true;
            }
            String v6s = fetchIpV6ViaSocks(socks);
            if (v6s != null && v6s.contains(":")) {
                r.v6 = v6s;
                r.v6Ok = true;
            }
            if (r.v4Ok || r.v6Ok) return r;
        }

        ConnectivityManager cm = (ConnectivityManager) getSystemService(CONNECTIVITY_SERVICE);
        Network net = null;
        if (preferTunnel) {
            for (int i = 0; i < 10 && net == null; i++) {
                net = findVpnNetwork(cm);
                if (net != null) break;
                try {
                    Thread.sleep(200);
                } catch (InterruptedException ignored) {
                }
            }
        } else if (cm != null && Build.VERSION.SDK_INT >= 23) {
            net = cm.getActiveNetwork();
        }

        // 2a) WireGuard: process already routes via TUN — plain fetch first.
        // Also try VPN Network bind as a belt-and-suspenders path.
        if (isWg) {
            // Small settle so handshake + routes are fully in the kernel table
            try {
                Thread.sleep(150);
            } catch (InterruptedException ignored) {
            }
            String v4plain = httpFetchIpDetails(null, true);
            if (v4plain != null && v4plain.startsWith("OK")) {
                r.v4 = v4plain;
                r.v4Ok = true;
            }
            if (!r.v4Ok && net != null) {
                String viaBind = httpFetchIpViaBoundSocket(net);
                if (viaBind != null && viaBind.startsWith("OK")) {
                    r.v4 = viaBind;
                    r.v4Ok = true;
                }
            }
            if (!r.v4Ok && net != null) {
                String v4n = httpFetchIpDetails(net, true);
                if (v4n != null && v4n.startsWith("OK")) {
                    r.v4 = v4n;
                    r.v4Ok = true;
                }
            }
            // Last resort: bind entire process to VPN network for the lookup
            if (!r.v4Ok && net != null && cm != null && Build.VERSION.SDK_INT >= 23) {
                boolean rebound = false;
                try {
                    rebound = cm.bindProcessToNetwork(net);
                    if (rebound) {
                        String v4 = httpFetchIpDetails(null, true);
                        if (v4 != null && v4.startsWith("OK")) {
                            r.v4 = v4;
                            r.v4Ok = true;
                        }
                    }
                } finally {
                    if (rebound) {
                        try {
                            cm.bindProcessToNetwork(null);
                        } catch (Exception ignored) {
                        }
                    }
                }
            }
        } else {
            // 2b) Generic VPN: app is disallowed → must bind to VPN Network
            if (preferTunnel && net == null) {
                r.v4 = "ERR\nmessage=VPN network not visible yet";
                return r;
            }
            if (preferTunnel && net != null) {
                // bindProcessToNetwork FIRST — most reliable on OEMs with disallow
                if (cm != null && Build.VERSION.SDK_INT >= 23) {
                    boolean rebound = false;
                    try {
                        rebound = cm.bindProcessToNetwork(net);
                        if (rebound) {
                            String v4 = httpFetchIpDetails(null, true);
                            if (v4 != null && v4.startsWith("OK")) {
                                r.v4 = v4;
                                r.v4Ok = true;
                            }
                        }
                    } finally {
                        if (rebound) {
                            try {
                                cm.bindProcessToNetwork(null);
                            } catch (Exception ignored) {
                            }
                        }
                    }
                }
                if (!r.v4Ok) {
                    String viaBind = httpFetchIpViaBoundSocket(net);
                    if (viaBind != null && viaBind.startsWith("OK")) {
                        r.v4 = viaBind;
                        r.v4Ok = true;
                    }
                }
            }
            if (!r.v4Ok) {
                String v4 = httpFetchIpDetails(net, preferTunnel);
                if (v4 != null && v4.startsWith("OK")) {
                    r.v4 = v4;
                    r.v4Ok = true;
                }
            }
        }

        String v6 = httpFetchPlainIp(isWg ? null : net, true);
        if (v6 != null && v6.contains(":")) {
            r.v6 = v6;
            r.v6Ok = true;
        }
        if (!r.v4Ok && !r.v6Ok) {
            String any = httpFetchPlainIp(isWg ? null : net, false);
            if (any != null) {
                if (any.contains(":")) {
                    r.v6 = any;
                    r.v6Ok = true;
                } else if (any.indexOf('.') > 0) {
                    r.v4 = "OK\nip=" + any
                        + "\ncountry=\ncountry_code=\ncity=\nlat=0\nlon=0\nisp=";
                    r.v4Ok = true;
                }
            }
        }

        // Windows parity: once we have a confirmed exit IP, reverse-geolocate
        // THAT address (clearnet is fine). Never trust a GeoIP "query" that
        // may have leaked around the tunnel.
        if (r.v4Ok) {
            String ip = kvField(r.v4, "ip");
            if (!looksLikeIp(ip)) {
                Map<String, String> kv = parseKV(r.v4);
                ip = kv.get("ip");
            }
            if (looksLikeIp(ip) && !ip.contains(":")) {
                String geo = reverseGeoForIp(ip);
                if (geo != null && geo.startsWith("OK")) {
                    r.v4 = overwriteIpField(geo, ip);
                }
            }
        }
        return r;
    }

    /**
     * Force traffic onto the VPN {@link Network} via {@link Network#bindSocket}.
     * Works even when the package is addDisallowedApplication — required for
     * real exit-IP on WireGuard.
     */
    private String httpFetchIpViaBoundSocket(Network vpn) {
        if (vpn == null || Build.VERSION.SDK_INT < 21) return null;
        // Prefer full GeoIP (city lat/lon) so the globe can pan precisely.
        String geo = httpFetchViaBoundSocketRaw(vpn, "ip-api.com", 80,
            "/json/?fields=status,message,query,country,countryCode,city,lat,lon,isp");
        if (geo != null && geo.startsWith("{")) {
            String parsed = parseIpApiBody(geo);
            if (parsed != null && parsed.startsWith("OK")) return parsed;
        }
        String[] hosts = new String[]{"api.ipify.org", "icanhazip.com", "ifconfig.me"};
        for (String host : hosts) {
            String path = host.contains("ifconfig") ? "/ip" : "/";
            if (host.contains("ipify")) path = "/?format=text";
            String body = httpFetchViaBoundSocketRaw(vpn, host, 80, path);
            if (body == null) continue;
            String ip = body.trim().split("\\s+")[0];
            if (looksLikeIp(ip) && !ip.contains(":")) {
                return "OK\nip=" + ip
                    + "\ncountry=\ncountry_code=\ncity=\nlat=0\nlon=0\nisp=";
            }
        }
        return null;
    }

    private String httpFetchViaBoundSocketRaw(Network vpn, String host, int port, String path) {
        Socket sock = null;
        try {
            java.net.InetAddress[] addrs;
            try {
                addrs = vpn.getAllByName(host);
            } catch (Exception e) {
                addrs = java.net.InetAddress.getAllByName(host);
            }
            if (addrs == null || addrs.length == 0) return null;
            // Prefer IPv4 for exit checks
            java.net.InetAddress target = addrs[0];
            for (java.net.InetAddress a : addrs) {
                if (a instanceof java.net.Inet4Address) {
                    target = a;
                    break;
                }
            }
            sock = new Socket();
            vpn.bindSocket(sock);
            sock.connect(new InetSocketAddress(target, port), 9000);
            sock.setSoTimeout(9000);
            String req = "GET " + path + " HTTP/1.0\r\nHost: " + host
                + "\r\nUser-Agent: ZeroNodeVPN/1.0\r\nConnection: close\r\n\r\n";
            OutputStream out = sock.getOutputStream();
            out.write(req.getBytes(StandardCharsets.UTF_8));
            out.flush();
            BufferedReader br = new BufferedReader(
                new InputStreamReader(sock.getInputStream(), StandardCharsets.UTF_8)
            );
            StringBuilder sb = new StringBuilder();
            String line;
            boolean body = false;
            while ((line = br.readLine()) != null) {
                if (!body) {
                    if (line.length() == 0) body = true;
                    continue;
                }
                if (sb.length() > 0) sb.append('\n');
                sb.append(line);
            }
            br.close();
            return sb.toString().trim();
        } catch (Exception e) {
            Log.w("ZeroNode", "bound-socket " + host + ": " + e.getMessage());
            return null;
        } finally {
            if (sock != null) {
                try {
                    sock.close();
                } catch (Exception ignored) {
                }
            }
        }
    }

    private int resolveTunnelSocksPort(boolean preferTunnel) {
        if (!preferTunnel && !torSocksUp) return 0;
        if (torSocksUp && torSocksPort > 0) return torSocksPort;
        int outline = NativeBridge.outlineSocksPort();
        return outline > 0 ? outline : 0;
    }

    private String fetchIpViaSocks(int socksPort) {
        String bust = String.valueOf(System.currentTimeMillis());
        String path = "/json/?fields=status,message,query,country,countryCode,city,lat,lon,isp&_="
            + bust;
        try {
            String body = httpGetViaSocks("ip-api.com", 80, path, socksPort);
            if (body != null) {
                String parsed = parseIpApiBody(body);
                if (parsed != null && parsed.startsWith("OK")) return parsed;
            }
        } catch (Exception ignored) {
        }
        try {
            String body = httpGetViaSocks("api.ipify.org", 80, "/?format=text", socksPort);
            if (body != null) {
                String ip = body.trim().split("\\s+")[0];
                if (looksLikeIp(ip)) {
                    return "OK\nip=" + ip + "\ncountry=\ncountry_code=\ncity=\nlat=0\nlon=0\nisp=";
                }
            }
        } catch (Exception ignored) {
        }
        return null;
    }

    private String fetchIpV6ViaSocks(int socksPort) {
        // Most SOCKS exits are IPv4-only; try anyway
        try {
            String body = httpGetViaSocks("api64.ipify.org", 80, "/?format=text", socksPort);
            if (body != null) {
                String ip = body.trim().split("\\s+")[0];
                if (ip.contains(":")) return ip;
            }
        } catch (Exception ignored) {
        }
        return null;
    }

    private String httpFetchIpDetails(Network net, boolean ignoredPreferTunnel) {
        String bust = String.valueOf(System.currentTimeMillis());
        String[] urls = new String[]{
            "http://ip-api.com/json/?fields=status,message,query,country,countryCode,city,lat,lon,isp&_=" + bust,
            "http://ip-api.com/json/?fields=status,message,query,country,countryCode,city,lat,lon,isp",
            "http://api.ipify.org?format=json",
            "http://icanhazip.com",
            "http://ifconfig.me/ip",
            "http://api.ipify.org?format=text"
        };
        Exception last = null;
        for (String urlStr : urls) {
            try {
                String body = httpGetBody(urlStr, net, 8000);
                if (body == null || body.isEmpty()) continue;
                if (body.startsWith("{")) {
                    if (body.contains("\"query\"")) {
                        String parsed = parseIpApiBody(body);
                        if (parsed != null && parsed.startsWith("OK")) return parsed;
                    }
                    String ip = jsonStr(body, "ip");
                    if (looksLikeIp(ip)) {
                        return "OK\nip=" + ip
                            + "\ncountry=\ncountry_code=\ncity=\nlat=0\nlon=0\nisp=";
                    }
                } else {
                    String ip = body.split("\\s+")[0].trim();
                    if (looksLikeIp(ip) && !ip.contains(":")) {
                        return "OK\nip=" + ip
                            + "\ncountry=\ncountry_code=\ncity=\nlat=0\nlon=0\nisp=";
                    }
                }
            } catch (Exception e) {
                last = e;
            }
        }
        if (last != null) return "ERR\nmessage=" + last.getMessage();
        return "ERR\nmessage=IP lookup failed";
    }

    private String httpFetchPlainIp(Network net, boolean ipv6Only) {
        String[] urls = ipv6Only
            ? new String[]{
                "https://api6.ipify.org?format=text",
                "http://api64.ipify.org?format=text"
            }
            : new String[]{
                "http://api.ipify.org?format=text",
                "http://icanhazip.com",
                "https://api64.ipify.org?format=text",
                "https://api6.ipify.org?format=text"
            };
        for (String urlStr : urls) {
            try {
                String body = httpGetBody(urlStr, net, 6000);
                if (body == null) continue;
                String ip = body.trim().split("\\s+")[0];
                if (!looksLikeIp(ip)) continue;
                if (ipv6Only && !ip.contains(":")) continue;
                return ip;
            } catch (Exception ignored) {
            }
        }
        return null;
    }

    private String httpGetBody(String urlStr, Network net, int timeoutMs) throws Exception {
        URL url = new URL(urlStr);
        HttpURLConnection conn;
        if (net != null && Build.VERSION.SDK_INT >= 21) {
            conn = (HttpURLConnection) net.openConnection(url);
        } else {
            conn = (HttpURLConnection) url.openConnection();
        }
        conn.setConnectTimeout(timeoutMs);
        conn.setReadTimeout(timeoutMs);
        conn.setUseCaches(false);
        conn.setInstanceFollowRedirects(true);
        conn.setRequestProperty("Cache-Control", "no-cache, no-store");
        conn.setRequestProperty("Pragma", "no-cache");
        conn.setRequestProperty("User-Agent", "ZeroNodeVPN/1.0");
        int code = conn.getResponseCode();
        InputStream in = code >= 400 ? conn.getErrorStream() : conn.getInputStream();
        if (in == null) {
            conn.disconnect();
            return null;
        }
        BufferedReader br = new BufferedReader(new InputStreamReader(in, StandardCharsets.UTF_8));
        StringBuilder sb = new StringBuilder();
        String line;
        while ((line = br.readLine()) != null) {
            if (sb.length() > 0) sb.append('\n');
            sb.append(line);
        }
        br.close();
        conn.disconnect();
        return sb.toString().trim();
    }

    private static boolean looksLikeIp(String ip) {
        if (ip == null || ip.length() < 3) return false;
        if (ip.indexOf('<') >= 0 || ip.indexOf(' ') >= 0) return false;
        return ip.indexOf('.') > 0 || ip.indexOf(':') > 0;
    }

    private static Network findVpnNetwork(ConnectivityManager cm) {
        if (cm == null) return null;
        try {
            if (Build.VERSION.SDK_INT >= 23) {
                Network active = cm.getActiveNetwork();
                if (active != null) {
                    NetworkCapabilities caps = cm.getNetworkCapabilities(active);
                    if (caps != null && caps.hasTransport(NetworkCapabilities.TRANSPORT_VPN)) {
                        return active;
                    }
                }
            }
            Network best = null;
            for (Network n : cm.getAllNetworks()) {
                NetworkCapabilities caps = cm.getNetworkCapabilities(n);
                if (caps == null) continue;
                if (!caps.hasTransport(NetworkCapabilities.TRANSPORT_VPN)) continue;
                if (caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)) {
                    return n;
                }
                if (best == null) best = n;
            }
            return best;
        } catch (Exception ignored) {
        }
        return null;
    }

    private static String httpGetViaSocks(String host, int port, String path, int socksPort)
        throws Exception {
        Socket sock = new Socket();
        sock.connect(new InetSocketAddress("127.0.0.1", socksPort), 8000);
        sock.setSoTimeout(12000);
        OutputStream out = sock.getOutputStream();
        InputStream in = sock.getInputStream();
        out.write(new byte[]{0x05, 0x01, 0x00});
        out.flush();
        byte[] resp = new byte[2];
        readFully(in, resp);
        if (resp[0] != 0x05 || resp[1] != 0x00) {
            sock.close();
            throw new Exception("SOCKS auth rejected");
        }
        byte[] hostBytes = host.getBytes(StandardCharsets.UTF_8);
        ByteArrayOutputStream req = new ByteArrayOutputStream();
        req.write(0x05);
        req.write(0x01);
        req.write(0x00);
        req.write(0x03);
        req.write(hostBytes.length);
        req.write(hostBytes);
        req.write((port >> 8) & 0xFF);
        req.write(port & 0xFF);
        out.write(req.toByteArray());
        out.flush();
        byte[] hdr = new byte[4];
        readFully(in, hdr);
        if (hdr[1] != 0x00) {
            sock.close();
            throw new Exception("SOCKS connect failed");
        }
        if (hdr[3] == 0x01) readFully(in, new byte[6]);
        else if (hdr[3] == 0x03) {
            int l = in.read();
            readFully(in, new byte[l + 2]);
        } else if (hdr[3] == 0x04) readFully(in, new byte[18]);
        String http = "GET " + path + " HTTP/1.0\r\nHost: " + host
            + "\r\nUser-Agent: ZeroNodeVPN/1.0\r\nConnection: close\r\n\r\n";
        out.write(http.getBytes(StandardCharsets.UTF_8));
        out.flush();
        ByteArrayOutputStream bos = new ByteArrayOutputStream();
        byte[] buf = new byte[4096];
        int n;
        while ((n = in.read(buf)) >= 0) bos.write(buf, 0, n);
        sock.close();
        String full = bos.toString("UTF-8");
        int bodyAt = full.indexOf("\r\n\r\n");
        return bodyAt >= 0 ? full.substring(bodyAt + 4) : full;
    }

    private static void readFully(InputStream in, byte[] buf) throws Exception {
        int off = 0;
        while (off < buf.length) {
            int n = in.read(buf, off, buf.length - off);
            if (n < 0) throw new Exception("EOF");
            off += n;
        }
    }

    private static String parseIpApiBody(String body) {
        try {
            String ip = jsonStr(body, "query");
            if (ip == null || ip.isEmpty()) ip = jsonStr(body, "ip");
            String status = jsonStr(body, "status");
            if ("fail".equals(status) || "false".equals(status)) {
                return "ERR\nmessage=" + nz(jsonStr(body, "message"));
            }
            String cc = jsonStr(body, "countryCode");
            if (cc.isEmpty()) cc = jsonStr(body, "country_code");
            String lat = jsonNum(body, "lat");
            if (lat.isEmpty()) lat = jsonNum(body, "latitude");
            String lon = jsonNum(body, "lon");
            if (lon.isEmpty()) lon = jsonNum(body, "longitude");
            String country = jsonStr(body, "country");
            if (country.isEmpty()) country = jsonStr(body, "country_name");
            String isp = jsonStr(body, "isp");
            if (isp.isEmpty()) isp = jsonStr(body, "org");
            if (ip == null || ip.isEmpty()) {
                return "ERR\nmessage=" + nz(jsonStr(body, "message"));
            }
            return "OK\nip=" + nz(ip)
                + "\ncountry=" + nz(country)
                + "\ncountry_code=" + nz(cc)
                + "\ncity=" + nz(jsonStr(body, "city"))
                + "\nlat=" + nz(lat)
                + "\nlon=" + nz(lon)
                + "\nisp=" + nz(isp);
        } catch (Exception e) {
            return "ERR\nmessage=parse: " + e.getMessage();
        }
    }

    /**
     * Reverse-geolocate a confirmed public IP. Request may ride clearnet —
     * we already know the address, so we only need city/lat/lon/flag.
     */
    private String reverseGeoForIp(String ip) {
        if (!looksLikeIp(ip) || ip.contains(":")) return null;
        String bust = String.valueOf(System.currentTimeMillis());
        String[] urls = new String[]{
            "http://ip-api.com/json/" + ip
                + "?fields=status,message,query,country,countryCode,city,lat,lon,isp&_=" + bust,
            "http://ipwho.is/" + ip + "?_=" + bust,
            "http://ip-api.com/json/" + ip
        };
        for (String url : urls) {
            try {
                String body = httpGetBody(url, null, 7000);
                if (body == null || body.isEmpty() || !body.startsWith("{")) continue;
                String parsed = parseIpApiBody(body);
                if (parsed != null && parsed.startsWith("OK")) {
                    return overwriteIpField(parsed, ip);
                }
            } catch (Exception ignored) {
            }
        }
        try {
            String rust = NativeBridge.fetchPublicIp();
            if (rust != null && rust.startsWith("OK")) {
                Map<String, String> kv = parseKV(rust);
                if (ip.equals(kv.get("ip"))) return rust;
            }
        } catch (Exception ignored) {
        }
        return null;
    }

    private static String overwriteIpField(String kvBlock, String ip) {
        if (kvBlock == null) return "OK\nip=" + ip;
        String[] lines = kvBlock.split("\n");
        StringBuilder sb = new StringBuilder();
        boolean wrote = false;
        for (String line : lines) {
            if (line.startsWith("ip=")) {
                sb.append("ip=").append(ip).append('\n');
                wrote = true;
            } else {
                sb.append(line).append('\n');
            }
        }
        if (!wrote) {
            if (sb.length() == 0) sb.append("OK\n");
            sb.append("ip=").append(ip).append('\n');
        }
        return sb.toString().trim();
    }

    private static String kvField(String block, String key) {
        if (block == null) return "";
        String prefix = key + "=";
        for (String line : block.split("\n")) {
            if (line.startsWith(prefix)) return line.substring(prefix.length());
        }
        return "";
    }

    private static String jsonStr(String json, String key) {
        String pat = "\"" + key + "\"";
        int i = json.indexOf(pat);
        if (i < 0) return "";
        int colon = json.indexOf(':', i + pat.length());
        if (colon < 0) return "";
        int start = colon + 1;
        while (start < json.length() && Character.isWhitespace(json.charAt(start))) start++;
        if (start >= json.length()) return "";
        if (json.charAt(start) == '"') {
            int q2 = json.indexOf('"', start + 1);
            return q2 > start ? json.substring(start + 1, q2) : "";
        }
        int end = start;
        while (end < json.length()) {
            char c = json.charAt(end);
            if (c == ',' || c == '}' || Character.isWhitespace(c)) break;
            end++;
        }
        return json.substring(start, end);
    }

    private static String jsonNum(String json, String key) {
        return jsonStr(json, key);
    }

    private void requestVpnPermission() {
        if (pendingVpn == null) {
            connecting = false;
            setNotice("Nothing to connect.");
            return;
        }
        persistPendingVpn(pendingVpn);
        // Must run on UI thread
        if (Looper.myLooper() != Looper.getMainLooper()) {
            handler.post(new Runnable() {
                @Override public void run() { requestVpnPermission(); }
            });
            return;
        }
        try {
            Intent prepare = VpnService.prepare(MainActivity.this);
            if (prepare != null) {
                connecting = true;
                activePhase = "connecting";
                setNotice("Grant VPN permission in the system dialog…");
                setProgressUi(pendingVpn.kind, Math.max(targetProgress, 0.2f),
                    "waiting for VPN permission");
                updateConnectionPill("connecting", pendingVpn.session);
                updatePrimaryButton();
                startActivityForResult(prepare, VPN_REQUEST_CODE);
            } else {
                // Already granted — start service immediately (do not stay on "permission")
                setProgressUi(pendingVpn.kind, Math.max(targetProgress, 0.35f),
                    "starting VPN service");
                setNotice("Starting " + pendingVpn.kind + "…");
                startPendingVpnService();
            }
        } catch (Exception e) {
            connecting = false;
            clearPendingVpnPersist();
            setNotice("VPN permission error: " + e.getMessage());
            activePhase = "error";
            updateConnectionPill("error", null);
            updatePrimaryButton();
        }
    }

    private void persistPendingVpn(PendingVpn p) {
        if (p == null) return;
        prefs().edit()
            .putString("pending_kind", p.kind)
            .putString("pending_session", p.session)
            .putString("pending_profile", p.profile)
            .putString("pending_host", p.host)
            .putString("pending_port", p.port)
            .putString("pending_user", p.user)
            .putString("pending_password", p.password)
            .putString("pending_method", p.method)
            .putString("pending_extra", p.extra)
            .putString("pending_client", p.clientAddress)
            .putString("pending_dns", p.dns)
            .putBoolean("pending_valid", true)
            .apply();
    }

    private void clearPendingVpnPersist() {
        prefs().edit().putBoolean("pending_valid", false).apply();
    }

    private void restorePendingVpnIfAny() {
        if (pendingVpn != null) return;
        SharedPreferences p = prefs();
        if (!p.getBoolean("pending_valid", false)) return;
        PendingVpn v = new PendingVpn();
        v.kind = p.getString("pending_kind", "");
        v.session = p.getString("pending_session", "ZeroNode");
        v.profile = p.getString("pending_profile", "");
        v.host = p.getString("pending_host", "");
        v.port = p.getString("pending_port", "");
        v.user = p.getString("pending_user", "");
        v.password = p.getString("pending_password", "");
        v.method = p.getString("pending_method", "");
        v.extra = p.getString("pending_extra", "");
        v.clientAddress = p.getString("pending_client", "10.7.0.2");
        v.dns = p.getString("pending_dns", "1.1.1.1");
        if (v.kind != null && v.kind.length() > 0) {
            pendingVpn = v;
        }
    }

    private void scheduleStatusPoll() {
        handler.postDelayed(new Runnable() {
            @Override
            public void run() {
                pollStatus();
                scheduleStatusPoll();
            }
        }, STATUS_INTERVAL_MS);
    }

    private int progressAnimTick;

    private void scheduleProgressPoll() {
        handler.postDelayed(new Runnable() {
            @Override
            public void run() {
                boolean animating = Math.abs(displayProgress - targetProgress) > 0.0015f;
                if (connecting || animating) {
                    if (progressAnimTick == 0 || progressAnimTick % 15 == 0) pollProgress();
                    progressAnimTick++;
                } else {
                    progressAnimTick = 0;
                }
                if (animating) {
                    displayProgress += (targetProgress - displayProgress) * 0.14f;
                    if (Math.abs(displayProgress - targetProgress) < 0.003f) {
                        displayProgress = targetProgress;
                    }
                    applyProgressDisplay();
                }
                long next = (connecting || animating)
                    ? 16
                    : PROGRESS_IDLE_INTERVAL_MS;
                handler.postDelayed(this, next);
            }
        }, PROGRESS_INTERVAL_MS);
    }

    private void pollStatus() {
        new Thread(new Runnable() {
            @Override
            public void run() {
                final String result = NativeBridge.getStatus();
                final Map<String, String> values = parseKV(result);
                final String tor = NativeBridge.torBootstrap();
                final boolean serviceUp = ZeroNodeVpnService.isRunning();
                final String svcStatus = ZeroNodeVpnService.lastStatus();
                handler.post(new Runnable() {
                    @Override
                    public void run() {
                        if (svcStatus != null && svcStatus.startsWith("ERR") && connecting) {
                            connecting = false;
                            vpnActive = false;
                            activePhase = "error";
                            Map<String, String> ek = parseKV(svcStatus);
                            setNotice("VPN failed: " + nz(ek.get("message")));
                            setProgressUi("error", 0f, nz(ek.get("message")));
                            updateConnectionPill("error", null);
                            updatePrimaryButton();
                            return;
                        }
                        // Trust live service only — never sticky OK after stop
                        boolean wasActive = vpnActive;
                        vpnActive = serviceUp;
                        if (vpnActive && connecting) {
                            connecting = false;
                            activePhase = "connected";
                            targetProgress = Math.max(targetProgress, 1f);
                            updateConnectionPill("connected",
                                pendingVpn != null ? pendingVpn.session : values.get("server_name"));
                            // Auto-refresh exit IP when tunnel becomes active
                            if (!wasActive) {
                                scheduleTunnelIpRefresh();
                            }
                        }
                        String phase = values.get("phase");
                        if (vpnActive && (phase == null || "disconnected".equals(phase))) {
                            phase = "connected";
                        }
                        if (vpnActive && phase != null && !"disconnected".equals(phase)) {
                            activePhase = phase;
                            String serverName = values.get("server_name");
                            if (serverName == null && pendingVpn != null) {
                                serverName = pendingVpn.session;
                            }
                            if (values.get("server_id") != null) {
                                activeServerId = values.get("server_id");
                            }
                            updateConnectionPill(phase, serverName);
                        } else if (!serviceUp && !torSocksUp && !connecting) {
                            if (vpnActive || "connected".equals(activePhase)
                                || "connecting".equals(activePhase)) {
                                // External disconnect (notification / revoke)
                                vpnActive = false;
                                activePhase = "disconnected";
                                targetProgress = 0f;
                                updateConnectionPill("disconnected", null);
                                setProgressUi("idle", 0f, "Idle");
                            } else {
                                activePhase = "disconnected";
                                updateConnectionPill("disconnected", null);
                            }
                        }
                        if (tor != null && tor.startsWith("OK")) {
                            torSocksUp = true;
                            int idx = tor.lastIndexOf(':');
                            if (idx > 0) {
                                try {
                                    String p = tor.substring(idx + 1).replaceAll("[^0-9]", "");
                                    if (p.length() > 0) torSocksPort = Integer.parseInt(p);
                                } catch (Exception ignored) {
                                }
                            }
                        } else if (tor != null && tor.startsWith("STOPPED") && !connecting) {
                            // Only clear if we are not mid Tor start
                            if (torSocksUp && !vpnActive) {
                                torSocksUp = false;
                                torSocksPort = 0;
                            }
                        }
                        updatePrimaryButton();
                    }
                });
            }
        }, "zn-status").start();
    }

    private void pollProgress() {
        new Thread(new Runnable() {
            @Override
            public void run() {
                final String result = NativeBridge.getProgress();
                final Map<String, String> kv = parseKV(result);
                handler.post(new Runnable() {
                    @Override
                    public void run() {
                        float frac = -1f;
                        try {
                            if (kv.get("fraction") != null) {
                                frac = Float.parseFloat(kv.get("fraction"));
                            }
                        } catch (NumberFormatException ignored) {
                        }
                        String stage = kv.get("stage");
                        String detail = kv.get("detail");

                        // Never drop progress to 0 while connecting (old bug: 62% → 0%)
                        if (connecting || vpnActive || torSocksUp) {
                            if (frac > targetProgress) {
                                targetProgress = frac;
                            }
                            if (vpnActive && targetProgress < 1f && frac >= 0.65f) {
                                targetProgress = 1f;
                                if (detail == null || detail.contains("SOCKS")) {
                                    detail = "system tunnel active";
                                }
                            }
                            if (stage != null && stage.length() > 0) {
                                String label = stage;
                                if (detail != null && detail.length() > 0) {
                                    label = stage + " · " + detail;
                                }
                                if (progressLabel != null) progressLabel.setText(label);
                            }
                        } else {
                            if (frac >= 0f) targetProgress = frac;
                            if (stage == null || stage.isEmpty()) {
                                if (progressLabel != null) progressLabel.setText("Idle");
                                targetProgress = 0f;
                            } else if (progressLabel != null) {
                                progressLabel.setText(stage
                                    + (detail != null && detail.length() > 0 ? " · " + detail : ""));
                            }
                        }
                        applyProgressDisplay();
                    }
                });
            }
        }, "zn-progress").start();
    }

    private void setProgressUi(String stage, float fraction, String detail) {
        // Monotonic while connecting
        float f = Math.max(0f, Math.min(1f, fraction));
        if (connecting || vpnActive || torSocksUp) {
            targetProgress = Math.max(targetProgress, f);
            if (f < 0.995f) showProgressCard();
        } else {
            targetProgress = f;
        }
        if (progressLabel != null) {
            if (detail != null) progressLabel.setText(stage + " · " + detail);
            else progressLabel.setText(stage);
        }
        applyProgressDisplay();
    }

    private void applyProgressDisplay() {
        if (progressBar != null) {
            progressBar.setAccent(isTorSession());
            progressBar.setFraction(displayProgress);
        }
        if (progressPercent != null) {
            int accent = isTorSession() ? 0xFFC084FC : 0xFF00FF7F;
            progressPercent.setTextColor(accent);
            progressPercent.setText(String.format(Locale.US, "%d%%",
                Math.round(displayProgress * 100f)));
        }
        if (displayProgress >= 0.995f
            && (vpnActive || torSocksUp || "connected".equals(activePhase))) {
            armProgressHide();
        }
    }

    private boolean isTorSession() {
        return protocolIndex == 2
            || (pendingVpn != null && "tor".equals(pendingVpn.kind));
    }

    private void showProgressCard() {
        handler.removeCallbacks(hideProgressAfterSuccess);
        progressHideArmed = false;
        if (progressCard == null) return;
        if (progressBar != null) progressBar.setAccent(isTorSession());
        Drawable hoverBg = progressCard.getBackground();
        if (hoverBg instanceof GradientDrawable) {
            ((GradientDrawable) hoverBg).setStroke(dp(1),
                isTorSession() ? 0x66A855F7 : 0x3300FF7F);
        }
        if (progressCard.getVisibility() != View.VISIBLE) {
            progressCard.setAlpha(0f);
            progressCard.setVisibility(View.VISIBLE);
        }
        progressCard.animate().alpha(1f).setDuration(180).start();
    }

    private void cancelProgressHide() {
        handler.removeCallbacks(hideProgressAfterSuccess);
        progressHideArmed = false;
    }

    private void armProgressHide() {
        if (progressHideArmed) return;
        if (progressCard == null || progressCard.getVisibility() != View.VISIBLE) return;
        progressHideArmed = true;
        handler.postDelayed(hideProgressAfterSuccess, 3000);
    }

    // ─── UI helpers ───────────────────────────────────────────────────────

    private void updateConnectionPill(String phase, String detail) {
        if (phase == null) phase = "disconnected";
        boolean locked = "connected".equals(phase);
        boolean connecting = "connecting".equals(phase);
        boolean error = "error".equals(phase);
        boolean tor = isTorSession();
        int color = error ? 0xFFFF3B3B
            : (tor ? 0xFFA855F7 : 0xFF00FF7F);
        if (!locked && !connecting && !error) {
            color = tor ? 0xFFC084FC : 0xFF00FF7F;
        }
        if (lockIcon != null) {
            boolean swap = lockShowingLocked != locked;
            lockShowingLocked = locked;
            if (swap) {
                lockIcon.animate().cancel();
                final int res = locked ? R.drawable.ic_lock : R.drawable.ic_unlock;
                final int tint = color;
                lockIcon.animate().scaleX(0.82f).scaleY(0.82f).setDuration(90)
                    .withEndAction(new Runnable() {
                        @Override public void run() {
                            lockIcon.setImageResource(res);
                            Icons.tint(lockIcon, tint);
                            lockIcon.animate().scaleX(1f).scaleY(1f).setDuration(160).start();
                        }
                    }).start();
            } else {
                lockIcon.setImageResource(locked ? R.drawable.ic_lock : R.drawable.ic_unlock);
                Icons.tint(lockIcon, color);
            }
            lockIcon.setContentDescription(locked ? "Connected" : (connecting ? "Connecting" : "Disconnected"));
        }
        if (lockGlow != null) {
            GradientDrawable glow = new GradientDrawable();
            glow.setShape(GradientDrawable.OVAL);
            int glowColor = (color & 0x00FFFFFF) | 0x66000000;
            glow.setColor(glowColor);
            lockGlow.setBackground(glow);
            startLockPulse(connecting || locked || error, connecting ? 480 : 1100);
        }
        if (connectionPill != null) {
            connectionPill.setVisibility(View.GONE);
        }
    }

    private void startLockPulse(boolean pulse, int durationMs) {
        if (lockGlow == null) return;
        if (lockPulse != null) {
            lockPulse.cancel();
            lockPulse = null;
        }
        if (!pulse) {
            lockGlow.setAlpha(0.22f);
            return;
        }
        lockPulse = ObjectAnimator.ofFloat(lockGlow, "alpha", 0.28f, 0.95f);
        lockPulse.setDuration(durationMs);
        lockPulse.setRepeatMode(ValueAnimator.REVERSE);
        lockPulse.setRepeatCount(ValueAnimator.INFINITE);
        lockPulse.setInterpolator(new AccelerateDecelerateInterpolator());
        lockPulse.start();
    }

    void setNotice(String text) {
        if (text == null || text.isEmpty()) {
            statusBanner.setVisibility(View.GONE);
        } else {
            statusBanner.setText(text);
            statusBanner.setVisibility(View.VISIBLE);
        }
    }

    private SharedPreferences prefs() {
        return getSharedPreferences(PREFS, MODE_PRIVATE);
    }

    private void restoreSavedProtocol() {
        migrateLegacyProfilesOnce();
        int idx = prefs().getInt("protocol_index", 0);
        // Old tabs: 0 WG, 1 OpenVPN, 2 Outline, 3 Tor
        if (idx == 1) idx = 0;
        else if (idx == 2) idx = 1;
        else if (idx >= 3) idx = 2;
        if (idx < 0 || idx >= PROTOCOLS.length) idx = 0;
        if (idx != protocolIndex) selectProtocol(idx);
    }

    /** One-shot: lift old single-box prefs into the profile database. */
    private void migrateLegacyProfilesOnce() {
        SharedPreferences p = prefs();
        if (p.getBoolean("profile_db_migrated_v1", false)) return;
        SharedPreferences.Editor ed = p.edit();
        String wg = p.getString("wg_profile", "");
        if (wg != null && wg.trim().length() > 20) {
            ProfileStore.save(this, null, ProfileStore.KIND_WG, "Imported WireGuard", wg, "", "", "");
        }
        String ol = p.getString("outline_key", "");
        if (ol != null && ol.trim().length() > 8) {
            ProfileStore.save(this, null, ProfileStore.KIND_OUTLINE, "Imported Outline", ol, "", "", "");
        }
        // PPTP intentionally not migrated — removed from Android client
        ed.putBoolean("profile_db_migrated_v1", true).apply();
    }

    private void saveCurrentFields() {
        SharedPreferences.Editor ed = prefs().edit();
        if (protocolIndex == 0 && profileInput != null) {
            ed.putString("wg_profile", textOf(profileInput));
        } else if (protocolIndex == 1 && profileInput != null) {
            ed.putString("outline_key", textOf(profileInput));
        }
        ed.apply();
    }

    private File writeTempProfile(String name, String content) {
        File dir = new File(getFilesDir(), "profiles");
        //noinspection ResultOfMethodCallIgnored
        dir.mkdirs();
        File f = new File(dir, name);
        try {
            FileOutputStream out = new FileOutputStream(f);
            out.write(content.getBytes(StandardCharsets.UTF_8));
            out.close();
        } catch (Exception e) {
            setNotice("Could not write profile: " + e.getMessage());
        }
        return f;
    }

    private void pasteInto(EditText field) {
        if (field == null) return;
        ClipboardManager cm = (ClipboardManager) getSystemService(CLIPBOARD_SERVICE);
        if (cm == null || !cm.hasPrimaryClip()) return;
        ClipData clip = cm.getPrimaryClip();
        if (clip == null || clip.getItemCount() == 0) return;
        CharSequence t = clip.getItemAt(0).coerceToText(this);
        if (t != null) field.setText(t);
    }

    private static String textOf(EditText e) {
        return e == null || e.getText() == null ? "" : e.getText().toString();
    }

    private List<ServerInfo> parseServers(String result) {
        List<ServerInfo> list = new ArrayList<>();
        Map<String, String> kv = parseKV(result);
        if (!"OK".equals(kv.get("status"))) return list;
        int count;
        try {
            count = Integer.parseInt(kv.get("count") != null ? kv.get("count") : "0");
        } catch (NumberFormatException e) {
            return list;
        }
        for (int i = 0; i < count; i++) {
            String prefix = "server." + i + ".";
            ServerInfo s = new ServerInfo();
            s.id = nz(kv.get(prefix + "id"));
            s.name = nz(kv.get(prefix + "name"));
            s.countryCode = nz(kv.get(prefix + "country_code"));
            s.countryName = nz(kv.get(prefix + "country_name"));
            s.endpoint = nz(kv.get(prefix + "endpoint"));
            s.wireguardEndpoint = nz(kv.get(prefix + "wireguard_endpoint"));
            s.hasPassword = "true".equals(kv.get(prefix + "has_password"));
            s.online = "true".equals(kv.get(prefix + "online"));
            list.add(s);
        }
        return list;
    }

    private Map<String, String> parseKV(String result) {
        Map<String, String> values = new HashMap<>();
        if (result == null || result.isEmpty()) {
            values.put("status", "ERR");
            values.put("message", "Empty response.");
            return values;
        }
        String[] lines = result.split("\n");
        values.put("status", lines[0]);
        for (int i = 1; i < lines.length; i++) {
            int eq = lines[i].indexOf('=');
            if (eq > 0) {
                values.put(lines[i].substring(0, eq), lines[i].substring(eq + 1));
            }
        }
        return values;
    }

    private static String nz(String s) {
        return s == null ? "" : s;
    }

    private String countryFlag(String code) {
        if (code == null || code.length() != 2) return "◎";
        int first = Character.codePointAt(code.toUpperCase(Locale.US), 0) - 'A' + 0x1F1E6;
        int second = Character.codePointAt(code.toUpperCase(Locale.US), 1) - 'A' + 0x1F1E6;
        return new String(Character.toChars(first)) + new String(Character.toChars(second));
    }

    private String maskEndpoint(String endpoint) {
        int colon = endpoint.lastIndexOf(':');
        String host = colon > 0 ? endpoint.substring(0, colon) : endpoint;
        String[] parts = host.split("\\.");
        if (parts.length == 4) return parts[0] + "." + parts[1] + ".•••.•••";
        return "•••.•••.•••.•••";
    }

    private LinearLayout softSection() {
        LinearLayout card = new LinearLayout(this);
        card.setOrientation(LinearLayout.VERTICAL);
        card.setPadding(dp(16), dp(12), dp(16), dp(12));
        // No heavy border — seamless sections with padding only
        return card;
    }

    private TextView sectionTitle(String text, int color) {
        TextView t = new TextView(this);
        t.setText(text);
        t.setTextColor(color);
        t.setTextSize(13);
        t.setTypeface(null, Typeface.BOLD);
        return t;
    }

    private TextView muted(String text) {
        TextView t = new TextView(this);
        t.setText(text);
        t.setTextColor(0xFF888888);
        t.setTextSize(11);
        t.setPadding(0, dp(2), 0, dp(2));
        return t;
    }

    private EditText singleLine(String hint) {
        EditText e = new EditText(this);
        e.setHint(hint);
        e.setSingleLine(true);
        e.setTextColor(Color.WHITE);
        e.setHintTextColor(0xFF666666);
        e.setTextSize(13);
        e.setPadding(dp(12), dp(10), dp(12), dp(10));
        GradientDrawable bg = new GradientDrawable();
        bg.setCornerRadius(dp(8));
        bg.setColor(0xFF1A1D24);
        e.setBackground(bg);
        LinearLayout.LayoutParams lp = mw();
        lp.topMargin = dp(6);
        e.setLayoutParams(lp);
        return e;
    }

    /**
     * Password field + drawn eye toggle (open = hidden, slashed = visible).
     */
    private View passwordRow(final EditText field) {
        field.setInputType(InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_PASSWORD);
        field.setSingleLine(true);
        field.setPadding(dp(12), dp(10), dp(4), dp(10));
        field.setBackgroundColor(Color.TRANSPARENT);

        LinearLayout row = new LinearLayout(this);
        row.setOrientation(LinearLayout.HORIZONTAL);
        row.setGravity(Gravity.CENTER_VERTICAL);
        GradientDrawable bg = new GradientDrawable();
        bg.setCornerRadius(dp(8));
        bg.setColor(0xFF1A1D24);
        row.setBackground(bg);
        row.setPadding(0, 0, dp(4), 0);
        LinearLayout.LayoutParams rlp = mw();
        rlp.topMargin = dp(6);
        row.setLayoutParams(rlp);

        row.addView(field, new LinearLayout.LayoutParams(0, vw(), 1f));

        final EyeToggleView eye = new EyeToggleView(this);
        eye.setContentDescription("Show password");
        final boolean[] shown = {false};
        eye.setOnClickListener(new View.OnClickListener() {
            @Override
            public void onClick(View v) {
                shown[0] = !shown[0];
                int start = field.getSelectionStart();
                int end = field.getSelectionEnd();
                if (shown[0]) {
                    field.setInputType(InputType.TYPE_CLASS_TEXT
                        | InputType.TYPE_TEXT_VARIATION_VISIBLE_PASSWORD);
                    eye.setRevealed(true);
                    eye.setContentDescription("Hide password");
                } else {
                    field.setInputType(InputType.TYPE_CLASS_TEXT
                        | InputType.TYPE_TEXT_VARIATION_PASSWORD);
                    eye.setRevealed(false);
                    eye.setContentDescription("Show password");
                }
                try {
                    field.setSelection(Math.max(0, start), Math.max(0, end));
                } catch (Exception ignored) {
                }
            }
        });
        row.addView(eye, dp(44), dp(40));
        return row;
    }

    /** Vector eye icon — open (password hidden) or slashed (password visible). */
    private static final class EyeToggleView extends View {
        private final Paint stroke = new Paint(Paint.ANTI_ALIAS_FLAG);
        private final Paint fill = new Paint(Paint.ANTI_ALIAS_FLAG);
        private final Path lid = new Path();
        private boolean revealed;

        EyeToggleView(Context ctx) {
            super(ctx);
            setClickable(true);
            setFocusable(true);
            GradientDrawable bg = new GradientDrawable();
            bg.setCornerRadius(dp(ctx, 6));
            bg.setColor(0xFF252A33);
            setBackground(bg);
            stroke.setStyle(Paint.Style.STROKE);
            stroke.setStrokeCap(Paint.Cap.ROUND);
            stroke.setStrokeJoin(Paint.Join.ROUND);
            fill.setStyle(Paint.Style.FILL);
        }

        void setRevealed(boolean on) {
            revealed = on;
            invalidate();
        }

        @Override
        protected void onDraw(Canvas c) {
            float w = getWidth();
            float h = getHeight();
            float d = getResources().getDisplayMetrics().density;
            float cx = w * 0.5f;
            float cy = h * 0.5f;
            float ew = Math.min(w, h) * 0.28f;
            float eh = ew * 0.58f;
            int color = revealed ? 0xFF00FF7F : 0xFFC8CDD3;
            stroke.setColor(color);
            fill.setColor(color);
            stroke.setStrokeWidth(Math.max(1.6f, d * 1.35f));

            lid.reset();
            lid.moveTo(cx - ew, cy);
            lid.quadTo(cx, cy - eh, cx + ew, cy);
            lid.quadTo(cx, cy + eh, cx - ew, cy);
            lid.close();
            c.drawPath(lid, stroke);
            c.drawCircle(cx, cy, eh * 0.38f, fill);

            if (revealed) {
                stroke.setStrokeWidth(Math.max(1.8f, d * 1.5f));
                c.drawLine(cx - ew * 0.92f, cy + eh * 0.95f,
                    cx + ew * 0.92f, cy - eh * 0.95f, stroke);
            }
        }

        private static int dp(Context ctx, int v) {
            return Math.round(v * ctx.getResources().getDisplayMetrics().density);
        }
    }

    private Button compactButton(String text, View.OnClickListener l) {
        Button b = secondaryButton(text);
        b.setTextSize(11);
        b.setOnClickListener(l);
        b.setPadding(dp(8), dp(4), dp(8), dp(4));
        return b;
    }

    private EditText scrollableProfile(String hint, int minLines) {
        EditText e = singleLine(hint);
        e.setSingleLine(false);
        e.setMinLines(minLines);
        e.setMaxLines(minLines);
        e.setGravity(Gravity.TOP | Gravity.START);
        e.setTypeface(Typeface.MONOSPACE);
        e.setTextSize(11);
        e.setVerticalScrollBarEnabled(false);
        e.setHorizontalScrollBarEnabled(false);
        e.setOverScrollMode(View.OVER_SCROLL_NEVER);
        // Nested scroll: don't expand forever
        e.setOnTouchListener(new View.OnTouchListener() {
            @Override
            public boolean onTouch(View v, android.view.MotionEvent event) {
                v.getParent().requestDisallowInterceptTouchEvent(true);
                if ((event.getAction() & android.view.MotionEvent.ACTION_MASK)
                    == android.view.MotionEvent.ACTION_UP) {
                    v.getParent().requestDisallowInterceptTouchEvent(false);
                }
                return false;
            }
        });
        return e;
    }

    private View wrapScrollable(EditText e, int maxHeight) {
        ScrollView sv = new ScrollView(this);
        sv.setFillViewport(true);
        sv.setVerticalScrollBarEnabled(false);
        sv.setHorizontalScrollBarEnabled(false);
        sv.setOverScrollMode(View.OVER_SCROLL_NEVER);
        LinearLayout.LayoutParams outer = new LinearLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT, maxHeight
        );
        outer.topMargin = dp(6);
        sv.setLayoutParams(outer);
        GradientDrawable bg = new GradientDrawable();
        bg.setCornerRadius(dp(8));
        bg.setColor(0xFF1A1D24);
        bg.setStroke(dp(1), 0x22FFFFFF);
        sv.setBackground(bg);
        e.setBackgroundColor(Color.TRANSPARENT);
        e.setLayoutParams(new ViewGroup.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT,
            ViewGroup.LayoutParams.WRAP_CONTENT
        ));
        sv.addView(e);
        return sv;
    }

    private LinearLayout threeButtons(Button a, Button b, Button c) {
        LinearLayout row = new LinearLayout(this);
        row.setOrientation(LinearLayout.HORIZONTAL);
        row.setPadding(0, dp(8), 0, 0);
        // Compact secondary actions (Import / Paste / Save)
        if (a != null) {
            a.setTextSize(11);
            row.addView(a, new LinearLayout.LayoutParams(0, dp(38), 1f));
        }
        if (b != null) {
            b.setTextSize(11);
            LinearLayout.LayoutParams lp = new LinearLayout.LayoutParams(0, dp(38), 1f);
            lp.leftMargin = dp(6);
            row.addView(b, lp);
        }
        if (c != null) {
            c.setTextSize(11);
            LinearLayout.LayoutParams lp = new LinearLayout.LayoutParams(0, dp(38), 1f);
            lp.leftMargin = dp(6);
            row.addView(c, lp);
        }
        return row;
    }

    private Button actionBtn(String text, View.OnClickListener l) {
        Button b = accentButton(text);
        b.setOnClickListener(l);
        return b;
    }

    private Button accentButton(String text) {
        Button btn = new Button(this);
        styleButton(btn, text, 0xFF00FF7F, Color.BLACK);
        return btn;
    }

    private Button secondaryButton(String text) {
        Button btn = new Button(this);
        styleButton(btn, text, 0xFFE8EDF2, Color.BLACK);
        return btn;
    }

    private Button dangerButton(String text) {
        Button btn = new Button(this);
        styleButton(btn, text, 0xFF22252C, Color.WHITE);
        return btn;
    }

    private void styleButton(Button btn, String text, int bgColor, int fg) {
        btn.setText(text);
        btn.setTextColor(fg);
        btn.setTextSize(12);
        btn.setAllCaps(false);
        GradientDrawable bg = new GradientDrawable();
        bg.setCornerRadius(dp(8));
        bg.setColor(bgColor);
        btn.setBackground(bg);
        btn.setPadding(dp(8), dp(6), dp(8), dp(6));
        btn.setMinimumHeight(0);
        btn.setMinHeight(0);
        btn.setMinWidth(0);
        btn.setMinimumWidth(0);
        btn.setStateListAnimator(null);
        btn.setOnTouchListener(new View.OnTouchListener() {
            @Override
            public boolean onTouch(View v, android.view.MotionEvent event) {
                int action = event.getActionMasked();
                if (action == android.view.MotionEvent.ACTION_DOWN) {
                    v.animate().scaleX(0.97f).scaleY(0.97f).setDuration(70).start();
                } else if (action == android.view.MotionEvent.ACTION_UP
                    || action == android.view.MotionEvent.ACTION_CANCEL) {
                    v.animate().scaleX(1f).scaleY(1f).setDuration(110).start();
                }
                return false;
            }
        });
    }

    int dp(int value) {
        return (int) (value * getResources().getDisplayMetrics().density);
    }

    private static int vw() {
        return ViewGroup.LayoutParams.WRAP_CONTENT;
    }

    private static LinearLayout.LayoutParams mw() {
        return new LinearLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT,
            ViewGroup.LayoutParams.WRAP_CONTENT
        );
    }

    private static LinearLayout.LayoutParams mw(int height) {
        return new LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, height);
    }

    private static LinearLayout.LayoutParams mm() {
        return new LinearLayout.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT,
            ViewGroup.LayoutParams.MATCH_PARENT
        );
    }

    static final class PencilEditView extends View {
        private final Paint fill = new Paint(Paint.ANTI_ALIAS_FLAG);
        private final Paint stroke = new Paint(Paint.ANTI_ALIAS_FLAG);
        private final Path path = new Path();
        private final RectF rect = new RectF();

        PencilEditView(Context ctx) {
            super(ctx);
            setClickable(true);
            setFocusable(true);
            setPadding(dp(ctx, 6), dp(ctx, 6), dp(ctx, 6), dp(ctx, 6));
            fill.setStyle(Paint.Style.FILL);
            stroke.setStyle(Paint.Style.STROKE);
            stroke.setStrokeCap(Paint.Cap.ROUND);
            stroke.setStrokeJoin(Paint.Join.ROUND);
        }

        @Override
        protected void onDraw(Canvas c) {
            float w = getWidth();
            float h = getHeight();
            float s = Math.min(w, h);
            float cx = w * 0.5f;
            float cy = h * 0.5f;
            c.save();
            c.rotate(-45f, cx, cy);
            float hw = s * 0.09f;
            // eraser
            fill.setColor(0xFF7DFFB8);
            rect.set(cx - hw, cy - s * 0.34f, cx + hw, cy - s * 0.22f);
            c.drawRoundRect(rect, hw * 0.55f, hw * 0.55f, fill);
            // ferrule
            fill.setColor(0xFFD8DEE4);
            rect.set(cx - hw, cy - s * 0.22f, cx + hw, cy - s * 0.16f);
            c.drawRect(rect, fill);
            stroke.setColor(0xFF9AA3AD);
            stroke.setStrokeWidth(Math.max(1f, s * 0.018f));
            c.drawLine(cx - hw, cy - s * 0.205f, cx + hw, cy - s * 0.205f, stroke);
            c.drawLine(cx - hw, cy - s * 0.175f, cx + hw, cy - s * 0.175f, stroke);
            // barrel
            fill.setColor(0xFF00E66E);
            path.reset();
            path.moveTo(cx - hw, cy - s * 0.16f);
            path.lineTo(cx + hw, cy - s * 0.16f);
            path.lineTo(cx + hw * 0.92f, cy + s * 0.10f);
            path.lineTo(cx - hw * 0.92f, cy + s * 0.10f);
            path.close();
            c.drawPath(path, fill);
            fill.setColor(0xFF00FF7F);
            path.reset();
            path.moveTo(cx - hw * 0.18f, cy - s * 0.16f);
            path.lineTo(cx + hw, cy - s * 0.16f);
            path.lineTo(cx + hw * 0.92f, cy + s * 0.10f);
            path.lineTo(cx + hw * 0.10f, cy + s * 0.10f);
            path.close();
            c.drawPath(path, fill);
            // wood
            fill.setColor(0xFFE6C28A);
            path.reset();
            path.moveTo(cx - hw * 0.92f, cy + s * 0.10f);
            path.lineTo(cx + hw * 0.92f, cy + s * 0.10f);
            path.lineTo(cx, cy + s * 0.28f);
            path.close();
            c.drawPath(path, fill);
            // graphite
            fill.setColor(0xFF1A1D22);
            path.reset();
            path.moveTo(cx - hw * 0.28f, cy + s * 0.22f);
            path.lineTo(cx + hw * 0.28f, cy + s * 0.22f);
            path.lineTo(cx, cy + s * 0.32f);
            path.close();
            c.drawPath(path, fill);
            c.restore();
        }

        private static int dp(Context ctx, int v) {
            return (int) (v * ctx.getResources().getDisplayMetrics().density);
        }
    }

    static final class RoundSpinnerView extends View {
        private final Paint paint = new Paint(Paint.ANTI_ALIAS_FLAG);
        private final Paint trackPaint = new Paint(Paint.ANTI_ALIAS_FLAG);
        private float angle;
        private boolean running;
        private final Runnable tick = new Runnable() {
            @Override public void run() {
                if (!running || getVisibility() != VISIBLE) return;
                angle = (angle + 10f) % 360f;
                invalidate();
                postDelayed(this, 16);
            }
        };

        RoundSpinnerView(Context ctx) {
            super(ctx);
            paint.setStyle(Paint.Style.STROKE);
            paint.setStrokeCap(Paint.Cap.ROUND);
            paint.setColor(0xCC66FFAA);
            trackPaint.setStyle(Paint.Style.STROKE);
            trackPaint.setStrokeCap(Paint.Cap.ROUND);
            trackPaint.setColor(0x66A8B4A8);
        }

        @Override
        protected void onAttachedToWindow() {
            super.onAttachedToWindow();
            start();
        }

        @Override
        protected void onDetachedFromWindow() {
            running = false;
            removeCallbacks(tick);
            super.onDetachedFromWindow();
        }

        @Override
        public void setVisibility(int visibility) {
            super.setVisibility(visibility);
            if (visibility == VISIBLE) start();
            else {
                running = false;
                removeCallbacks(tick);
            }
        }

        private void start() {
            if (running || getVisibility() != VISIBLE) return;
            running = true;
            post(tick);
        }

        @Override
        protected void onDraw(Canvas c) {
            float s = Math.min(getWidth(), getHeight());
            if (s <= 0f) return;
            float w = Math.max(1.6f, s * 0.16f);
            paint.setStrokeWidth(w);
            trackPaint.setStrokeWidth(w);
            float p = w;
            c.drawArc(p, p, getWidth() - p, getHeight() - p, 0f, 360f, false, trackPaint);
            c.drawArc(p, p, getWidth() - p, getHeight() - p, angle, 92f, false, paint);
        }
    }

    static final class GreenSwitch extends View {
        interface OnToggle {
            void onToggle(boolean on);
        }

        private final Paint paint = new Paint(Paint.ANTI_ALIAS_FLAG);
        private boolean on;
        private OnToggle listener;

        GreenSwitch(Context ctx) {
            super(ctx);
            setClickable(true);
            setFocusable(true);
            setOnClickListener(new OnClickListener() {
                @Override public void onClick(View v) {
                    if (!isEnabled()) return;
                    setOn(!on, true);
                    if (listener != null) listener.onToggle(on);
                }
            });
        }

        void setOn(boolean value, boolean animate) {
            on = value;
            invalidate();
        }

        void setOnToggle(OnToggle l) {
            listener = l;
        }

        @Override
        protected void onDraw(Canvas c) {
            float w = getWidth();
            float h = getHeight();
            float r = h * 0.5f;
            paint.setColor(on ? 0xFF00C853 : 0xFF2A2F38);
            c.drawRoundRect(0, 0, w, h, r, r, paint);
            float thumb = h - 6f;
            float x = on ? (w - thumb - 3f) : 3f;
            paint.setColor(on ? 0xFF04140C : 0xFFE8EAED);
            c.drawCircle(x + thumb * 0.5f, h * 0.5f, thumb * 0.5f, paint);
        }
    }

    static final class CyberProgressBar extends View {
        private final Paint trackPaint = new Paint(Paint.ANTI_ALIAS_FLAG);
        private final Paint fillPaint = new Paint(Paint.ANTI_ALIAS_FLAG);
        private final Paint edgePaint = new Paint(Paint.ANTI_ALIAS_FLAG);
        private final RectF rect = new RectF();
        private float fraction;
        private boolean tor;

        CyberProgressBar(android.content.Context context) {
            super(context);
            trackPaint.setColor(0x6612181C);
            edgePaint.setStyle(Paint.Style.STROKE);
            edgePaint.setStrokeWidth(1.2f);
            edgePaint.setColor(0x4400FF7F);
        }

        void setAccent(boolean torAccent) {
            if (tor == torAccent) return;
            tor = torAccent;
            edgePaint.setColor(tor ? 0x66A855F7 : 0x4400FF7F);
            invalidate();
        }

        void setFraction(float f) {
            fraction = Math.max(0f, Math.min(1f, f));
            invalidate();
        }

        @Override
        protected void onDraw(Canvas canvas) {
            super.onDraw(canvas);
            float w = getWidth();
            float h = getHeight();
            if (w <= 0f || h <= 0f) return;
            float r = h * 0.5f;
            rect.set(1, 1, w - 1, h - 1);
            canvas.drawRoundRect(rect, r, r, trackPaint);
            if (fraction > 0.001f) {
                float fillW = Math.max(h, (w - 2) * fraction);
                rect.set(1, 1, fillW, h - 1);
                int[] colors = tor
                    ? new int[]{0xCC6B21A8, 0xFFA855F7, 0xFFD8B4FE}
                    : new int[]{0xCC00AA55, 0xFF00FF7F, 0xFF66FFAA};
                fillPaint.setShader(new LinearGradient(
                    0, 0, fillW, 0,
                    colors,
                    new float[]{0f, 0.55f, 1f},
                    Shader.TileMode.CLAMP
                ));
                canvas.drawRoundRect(rect, r, r, fillPaint);
                fillPaint.setShader(null);
            }
            rect.set(1, 1, w - 1, h - 1);
            canvas.drawRoundRect(rect, r, r, edgePaint);
        }
    }

    static final class ServerInfo {
        String id = "";
        String name = "";
        String countryCode = "";
        String countryName = "";
        String endpoint = "";
        String wireguardEndpoint = "";
        boolean hasPassword;
        boolean online;
    }

    static final class PendingVpn {
        String kind;
        String session;
        String clientAddress = "10.7.0.2";
        String dns = "1.1.1.1";
        String profile = "";
        String host = "";
        String port = "";
        String user = "";
        String password = "";
        String method = "";
        String extra = "";

        static PendingVpn wireguard(String path, String endpoint, String clientIp) {
            PendingVpn p = new PendingVpn();
            p.kind = "wireguard";
            p.session = "WireGuard";
            p.profile = path;
            p.host = endpoint != null ? endpoint : "";
            p.clientAddress = clientIp != null && clientIp.length() > 0 ? clientIp : "10.8.0.2";
            return p;
        }

        static PendingVpn outline(String host, String port, String password, String method, String key) {
            PendingVpn p = new PendingVpn();
            p.kind = "outline";
            p.session = "Outline";
            p.host = host != null ? host : "";
            p.port = port != null ? port : "";
            p.password = password != null ? password : "";
            p.method = method != null ? method : "";
            p.profile = key != null ? key : "";
            p.clientAddress = "10.9.0.2";
            return p;
        }

        static PendingVpn tor() {
            PendingVpn p = new PendingVpn();
            p.kind = "tor";
            p.session = "Tor";
            p.clientAddress = "10.10.0.2";
            return p;
        }

        static PendingVpn zeronode(String profile, String clientIp, String name) {
            PendingVpn p = new PendingVpn();
            p.kind = "zeronode";
            p.session = name != null ? name : "ZeroNode";
            p.profile = profile != null ? profile : "";
            if (clientIp != null && clientIp.length() > 0) p.clientAddress = clientIp;
            return p;
        }
    }
}
