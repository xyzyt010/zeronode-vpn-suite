package io.zeronode.vpn.globe;

import android.content.Context;
import android.graphics.Bitmap;
import android.graphics.Canvas;
import android.graphics.Color;
import android.graphics.Paint;
import android.graphics.RadialGradient;
import android.graphics.Shader;
import android.graphics.Typeface;
import android.os.Build;
import android.os.Handler;
import android.os.Looper;
import android.os.SystemClock;
import android.util.AttributeSet;
import android.view.MotionEvent;
import android.view.ScaleGestureDetector;
import android.view.View;

import org.json.JSONArray;
import org.json.JSONObject;

import java.io.BufferedReader;
import java.io.InputStream;
import java.io.InputStreamReader;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.Iterator;
import java.util.List;
import java.util.Locale;
import java.util.Map;

/**
 * Windows-parity globe (apps/client/src/globe):
 * - Same lat/lng → unit-sphere math as lat_lng_to_vec3 in mod.rs
 * - Full countries_50m.geojson borders (no dummy/low-res substitute)
 * - Dark sphere + neon green borders, drag orbit + inertia, pinch zoom,
 *   pan-to-location with auto zoom/tilt (COUNTRY_FOCUS_ZOOM = 2.55)
 */
public final class GlobeView extends View {
    private static final int BODY = 0xFF121416;
    private static final int BORDER = 0x8C00FF7F; // green * ~0.55
    private static final int RIM = 0x4000FF7F;
    private static final int HALO = 0x1200FF7F;
    private static final int BEACON = 0xFF00FF7F;

    /**
     * Base orbit gain at zoom=1. Drag is divided by zoom so a pinch-in
     * (close-up) does not whip the globe around, and pinch-out stays usable.
     */
    private static final float ORBIT_DRAG = 0.0034f;
    private static final float MIN_ZOOM = 0.70f;
    private static final float MAX_ZOOM = 7.2f;
    /** Matches Windows COUNTRY_FOCUS_ZOOM for city-level focus. */
    private static final float COUNTRY_FOCUS_ZOOM = 2.55f;
    private static final float CITY_FOCUS_ZOOM = 2.85f;
    private static final float GLOBE_CENTER_Y_OFFSET = 0.02f;

    private final Paint bodyPaint = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint rimPaint = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint haloPaint = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint borderPaint = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint pinPaint = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint pinGlowPaint = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint labelPaint = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint atmPaint = new Paint(Paint.ANTI_ALIAS_FLAG);

    private float rotY;
    private float rotX;
    private float zoom = 1f;

    private float velY;
    private float velX;
    private boolean dragging;
    private float lastX;
    private float lastY;
    private long lastMoveMs;

    private boolean panActive;
    private long panStartMs;
    private float panStartY, panStartX, panStartZoom;
    private float panTargetY, panTargetX, panTargetZoom = COUNTRY_FOCUS_ZOOM;

    private float pinLat = Float.NaN;
    private float pinLon = Float.NaN;
    private String pinLabel = "";
    /** Flag emoji shown near the pin (Windows-style exit badge). */
    private String pinFlag = "";
    /** IPv4 shown under the pin label. */
    private String pinIp = "";
    private String pinCity = "";

    private final Map<String, float[]> centroids = new HashMap<>();

    /**
     * Closed country rings on the unit sphere. Each ring is xyz triplets
     * (every vertex — no stride). Drawn as a continuous Path so coasts stay
     * smooth instead of squiggly/dashed.
     */
    private float[][] rings = new float[0][];
    private volatile boolean bordersReady;
    private final android.graphics.Path borderPath = new android.graphics.Path();

    private final Handler mainHandler = new Handler(Looper.getMainLooper());
    private long animTimeMs;
    private boolean tickerPosted;
    private ScaleGestureDetector scaleDetector;
    private boolean scaling;
    private float focusX;
    private float focusY;

    private RadialGradient cachedAtm;
    private float cachedAtmR = -1f;
    private float cachedAtmCx, cachedAtmCy;

    /**
     * Cached static globe (body + borders) so idle/pin-pulse frames do NOT
     * re-project 50k–100k segments every vsync — that was the root of the
     * "15 Hz flimsy" feel on 60–144 Hz panels.
     */
    private Bitmap staticCache;
    private Canvas staticCacheCanvas;
    private boolean staticCacheValid;
    private float cacheRotY = Float.NaN, cacheRotX = Float.NaN, cacheZoom = Float.NaN;
    private int cacheW, cacheH;
    /**
     * Pin overlay is a cheap blit + a few circles/glyphs. Drive it at the
     * panel refresh rate (60–144 Hz) so the glow feels like the Windows globe.
     */
    private long lastPinPulseMs;
    private long pinPulseMinMs = 33;
    private long lastCamMoveMs;

    public GlobeView(Context context) {
        super(context);
        init(context);
    }

    public GlobeView(Context context, AttributeSet attrs) {
        super(context, attrs);
        init(context);
    }

    private void init(Context context) {
        setBackgroundColor(Color.TRANSPARENT);
        // Software cache of borders is faster than HW layer re-recording 100k lines.
        // Keep default layer; we blit a pre-rasterized bitmap when camera is still.
        setWillNotDraw(false);

        bodyPaint.setColor(BODY);
        bodyPaint.setStyle(Paint.Style.FILL);

        rimPaint.setStyle(Paint.Style.STROKE);
        rimPaint.setStrokeWidth(1.2f);
        rimPaint.setColor(RIM);

        haloPaint.setStyle(Paint.Style.STROKE);
        haloPaint.setStrokeWidth(6f);
        haloPaint.setColor(HALO);

        borderPaint.setStyle(Paint.Style.STROKE);
        // Hairline coasts: BUTT caps + path joins. ROUND caps on short
        // segments read as pearls / squiggles. Width stays readable at 1.5px.
        borderPaint.setStrokeWidth(1.5f);
        borderPaint.setColor(BORDER);
        borderPaint.setStrokeCap(Paint.Cap.BUTT);
        borderPaint.setStrokeJoin(Paint.Join.ROUND);
        borderPaint.setAntiAlias(true);
        borderPaint.setDither(true);

        pinPaint.setColor(BEACON);
        pinPaint.setStyle(Paint.Style.FILL);
        pinPaint.setAntiAlias(true);
        pinGlowPaint.setStyle(Paint.Style.STROKE);
        pinGlowPaint.setStrokeWidth(1.6f);
        pinGlowPaint.setColor(BEACON);
        pinGlowPaint.setAntiAlias(true);

        int textFlags = Paint.ANTI_ALIAS_FLAG
            | Paint.SUBPIXEL_TEXT_FLAG
            | Paint.LINEAR_TEXT_FLAG
            | Paint.FILTER_BITMAP_FLAG;
        labelPaint.setFlags(textFlags);
        labelPaint.setColor(0xFFF2F4F6);
        labelPaint.setTextSize(13f * density());
        labelPaint.setTextAlign(Paint.Align.LEFT);
        labelPaint.setTypeface(Typeface.create("sans-serif-medium", Typeface.NORMAL));
        if (Build.VERSION.SDK_INT >= 21) {
            try {
                labelPaint.setElegantTextHeight(true);
            } catch (Exception ignored) {
            }
        }

        setClickable(true);
        setFocusable(true);
        // Hardware layer: idle frames are a bitmap blit + pin, not 80k lines.
        setLayerType(LAYER_TYPE_HARDWARE, null);
        syncPinPulseToDisplay();

        scaleDetector = new ScaleGestureDetector(context,
            new ScaleGestureDetector.SimpleOnScaleGestureListener() {
                @Override
                public boolean onScaleBegin(ScaleGestureDetector d) {
                    scaling = true;
                    panActive = false;
                    velX = 0;
                    velY = 0;
                    focusX = d.getFocusX();
                    focusY = d.getFocusY();
                    return true;
                }

                @Override
                public boolean onScale(ScaleGestureDetector d) {
                    float factor = d.getScaleFactor();
                    // Slightly dampen for control
                    factor = 1f + (factor - 1f) * 1.15f;
                    zoom = clamp(zoom * factor, MIN_ZOOM, MAX_ZOOM);
                    focusX = d.getFocusX();
                    focusY = d.getFocusY();
                    requestAnim();
                    return true;
                }

                @Override
                public void onScaleEnd(ScaleGestureDetector d) {
                    scaling = false;
                }
            });

        loadCentroidsAsync(context);
        loadBordersAsync(context);
        animTimeMs = SystemClock.uptimeMillis();
        requestAnim();
    }

    private void loadCentroidsAsync(final Context context) {
        new Thread(new Runnable() {
            @Override
            public void run() {
                final Map<String, float[]> loaded = new HashMap<>();
                try {
                    InputStream in = context.getAssets().open("globe/country_centroids.json");
                    BufferedReader br = new BufferedReader(new InputStreamReader(in));
                    StringBuilder sb = new StringBuilder();
                    String line;
                    while ((line = br.readLine()) != null) sb.append(line);
                    br.close();
                    JSONObject root = new JSONObject(sb.toString());
                    Iterator<String> keys = root.keys();
                    while (keys.hasNext()) {
                        String code = keys.next();
                        Object val = root.get(code);
                        if (!(val instanceof JSONObject)) continue;
                        JSONObject o = (JSONObject) val;
                        float lat = (float) o.optDouble("lat", o.optDouble("latitude", 0));
                        float lon = (float) o.optDouble("lon", o.optDouble("lng", o.optDouble("longitude", 0)));
                        loaded.put(code.toUpperCase(Locale.US), new float[]{lat, lon});
                    }
                } catch (Exception ignored) {
                    loaded.put("US", new float[]{39f, -98f});
                    loaded.put("GB", new float[]{54f, -2f});
                    loaded.put("DE", new float[]{51f, 10f});
                    loaded.put("IN", new float[]{22f, 79f});
                    loaded.put("JP", new float[]{36f, 138f});
                    loaded.put("NL", new float[]{52.3f, 5.3f});
                }
                mainHandler.post(new Runnable() {
                    @Override
                    public void run() {
                        centroids.clear();
                        centroids.putAll(loaded);
                    }
                });
            }
        }, "zn-globe-centroids").start();
    }

    /**
     * Load full Windows geojson. Keep every outer-ring vertex (same as
     * borders.rs). No stride, no dual-LOD — those made coasts squiggly.
     */
    private void loadBordersAsync(final Context context) {
        new Thread(new Runnable() {
            @Override
            public void run() {
                try {
                    InputStream in = context.getAssets().open("globe/countries_50m.geojson");
                    BufferedReader br = new BufferedReader(new InputStreamReader(in), 128 * 1024);
                    StringBuilder sb = new StringBuilder(3 * 1024 * 1024);
                    char[] buf = new char[128 * 1024];
                    int n;
                    while ((n = br.read(buf)) >= 0) sb.append(buf, 0, n);
                    br.close();

                    JSONObject root = new JSONObject(sb.toString());
                    JSONArray features = root.optJSONArray("features");
                    if (features == null) return;

                    List<float[]> loaded = new ArrayList<>(4000);
                    for (int i = 0; i < features.length(); i++) {
                        JSONObject feat = features.optJSONObject(i);
                        if (feat == null) continue;
                        JSONObject geom = feat.optJSONObject("geometry");
                        if (geom == null) continue;
                        String type = geom.optString("type", "");
                        JSONArray coords = geom.optJSONArray("coordinates");
                        if (coords == null) continue;
                        if ("Polygon".equals(type)) {
                            float[] ring = ringToSphere(coords.optJSONArray(0));
                            if (ring != null) loaded.add(ring);
                        } else if ("MultiPolygon".equals(type)) {
                            for (int p = 0; p < coords.length(); p++) {
                                JSONArray poly = coords.optJSONArray(p);
                                if (poly == null) continue;
                                float[] ring = ringToSphere(poly.optJSONArray(0));
                                if (ring != null) loaded.add(ring);
                            }
                        }
                    }

                    final float[][] ready = loaded.toArray(new float[0][]);
                    mainHandler.post(new Runnable() {
                        @Override
                        public void run() {
                            rings = ready;
                            bordersReady = true;
                            staticCacheValid = false;
                            invalidate();
                        }
                    });
                } catch (Exception e) {
                    android.util.Log.e("GlobeView", "border load failed", e);
                }
            }
        }, "zn-globe-borders").start();
    }

    /** Every vertex of an outer ring → unit-sphere xyz. Closes the loop. */
    private static float[] ringToSphere(JSONArray ring) {
        if (ring == null || ring.length() < 3) return null;
        int len = ring.length();
        float[] pts = new float[len * 3 + 3];
        int n = 0;
        float firstX = 0, firstY = 0, firstZ = 0;
        float prevX = 0, prevY = 0, prevZ = 0;
        for (int i = 0; i < len; i++) {
            JSONArray pt = ring.optJSONArray(i);
            if (pt == null || pt.length() < 2) continue;
            float lon = (float) pt.optDouble(0, 0);
            float lat = (float) pt.optDouble(1, 0);
            float[] v = latLngToVec3Windows(lat, lon, 1.002f);
            if (n > 0) {
                float dx = v[0] - prevX, dy = v[1] - prevY, dz = v[2] - prevZ;
                if (dx * dx + dy * dy + dz * dz < 1e-14f) continue;
            }
            pts[n * 3] = v[0];
            pts[n * 3 + 1] = v[1];
            pts[n * 3 + 2] = v[2];
            if (n == 0) {
                firstX = v[0];
                firstY = v[1];
                firstZ = v[2];
            }
            prevX = v[0];
            prevY = v[1];
            prevZ = v[2];
            n++;
        }
        if (n < 3) return null;
        float dx = firstX - prevX, dy = firstY - prevY, dz = firstZ - prevZ;
        if (dx * dx + dy * dy + dz * dz > 1e-14f) {
            pts[n * 3] = firstX;
            pts[n * 3 + 1] = firstY;
            pts[n * 3 + 2] = firstZ;
            n++;
        }
        if (n * 3 == pts.length) return pts;
        float[] tight = new float[n * 3];
        System.arraycopy(pts, 0, tight, 0, tight.length);
        return tight;
    }

    /**
     * Exact port of apps/client/src/globe/mod.rs lat_lng_to_vec3.
     * This is the coordinate system the Windows globe uses — do not replace
     * with the classic ECEF formula or geography will not match.
     */
    private static float[] latLngToVec3Windows(float latDeg, float lonDeg, float radius) {
        double phi = Math.toRadians(90.0 - latDeg);
        double theta = Math.toRadians(lonDeg + 180.0);
        double r = radius;
        float x = (float) (-r * Math.sin(phi) * Math.cos(theta));
        float y = (float) (r * Math.cos(phi));
        float z = (float) (r * Math.sin(phi) * Math.sin(theta));
        return new float[]{x, y, z};
    }

    /**
     * Pan to exact GeoIP coordinates (city-level, Windows parity).
     * Prefer this over {@link #panToCountry} whenever lat/lon are available.
     */
    public void panTo(float lat, float lon, String label) {
        panToPrecise(lat, lon, label, "", "", CITY_FOCUS_ZOOM);
    }

    /**
     * Full exit badge: city-precise pin + flag emoji + IPv4 on the globe.
     */
    public void panToExit(
        float lat, float lon, String flagEmoji, String ipv4, String cityCountry
    ) {
        pinFlag = flagEmoji == null ? "" : flagEmoji;
        pinIp = ipv4 == null ? "" : ipv4;
        pinCity = cityCountry == null ? "" : cityCountry;
        String label = pinCity;
        if (label.length() == 0 && pinIp.length() > 0) label = pinIp;
        panToPrecise(lat, lon, label, pinFlag, pinIp, CITY_FOCUS_ZOOM);
    }

    public void panTo(float lat, float lon, String label, float focusZoom) {
        panToPrecise(lat, lon, label, pinFlag, pinIp, focusZoom);
    }

    private void panToPrecise(
        float lat, float lon, String label, String flag, String ip, float focusZoom
    ) {
        if (Float.isNaN(lat) || Float.isNaN(lon)) return;
        if (lat < -90f || lat > 90f || lon < -180f || lon > 180f) return;
        pinLat = lat;
        pinLon = lon;
        pinLabel = label == null ? "" : label;
        if (flag != null) pinFlag = flag;
        if (ip != null) pinIp = ip;
        float[] target = rotationsToCenter(lat, lon);
        panStartY = rotY;
        panStartX = rotX;
        panStartZoom = zoom;
        panTargetY = target[0];
        float dy = panTargetY - panStartY;
        while (dy > Math.PI) {
            panTargetY -= (float) (2 * Math.PI);
            dy = panTargetY - panStartY;
        }
        while (dy < -Math.PI) {
            panTargetY += (float) (2 * Math.PI);
            dy = panTargetY - panStartY;
        }
        panTargetX = clamp(target[1], -1.35f, 1.35f);
        panTargetZoom = clamp(focusZoom, MIN_ZOOM, MAX_ZOOM);
        panStartMs = SystemClock.uptimeMillis();
        panActive = true;
        velX = 0;
        velY = 0;
        requestAnim();
    }

    public void panToCountry(String iso2, String label) {
        if (iso2 != null && iso2.length() == 2) {
            float[] c = centroids.get(iso2.toUpperCase(Locale.US));
            if (c != null) {
                panToPrecise(c[0], c[1], label != null ? label : iso2,
                    pinFlag, pinIp, COUNTRY_FOCUS_ZOOM);
                return;
            }
        }
        panToPrecise(20f, 0f, label != null ? label : "", pinFlag, pinIp, COUNTRY_FOCUS_ZOOM);
    }

    /** Update floating exit badge without re-panning (IP card refresh). */
    public void setExitBadge(String flagEmoji, String ipv4, String cityCountry) {
        pinFlag = flagEmoji == null ? "" : flagEmoji;
        pinIp = ipv4 == null ? "" : ipv4;
        pinCity = cityCountry == null ? "" : cityCountry;
        if (pinCity.length() > 0) pinLabel = pinCity;
        else if (pinIp.length() > 0) pinLabel = pinIp;
        invalidate();
    }

    public void forcePanRefresh() {
        if (!Float.isNaN(pinLat)) {
            panToPrecise(pinLat, pinLon, pinLabel, pinFlag, pinIp, CITY_FOCUS_ZOOM);
        }
    }

    /** Windows rotations_to_center. */
    private static float[] rotationsToCenter(float lat, float lon) {
        float[] p = latLngToVec3Windows(lat, lon, 1f);
        float rotY = (float) Math.atan2(-p[0], p[2]);
        float cy = (float) Math.cos(rotY);
        float sy = (float) Math.sin(rotY);
        float py2 = p[1];
        float pz2 = -sy * p[0] + cy * p[2];
        float rotX = (float) Math.atan2(py2, Math.max(1e-6, pz2));
        return new float[]{rotY, clamp(rotX, -1.35f, 1.35f)};
    }

    private void requestAnim() {
        staticCacheValid = false; // camera moving — rebuild cache next settle
        if (!tickerPosted) {
            tickerPosted = true;
            postOnAnimation(ticker);
        }
        invalidate();
    }

    private final Runnable ticker = new Runnable() {
        @Override
        public void run() {
            tickerPosted = false;
            long now = SystemClock.uptimeMillis();
            float dt = Math.max(1f / 240f, Math.min(1f / 20f, (now - animTimeMs) / 1000f));
            animTimeMs = now;
            boolean cameraMoving = false;

            if (panActive) {
                float elapsed = (now - panStartMs) / 1000f;
                float duration = 0.68f;
                float t = clamp(elapsed / duration, 0f, 1f);
                float ease = t < 0.5f
                    ? 4f * t * t * t
                    : 1f - (float) Math.pow(-2f * t + 2f, 3) / 2f;
                float zoomT = clamp((t - 0.06f) / 0.94f, 0f, 1f);
                float zoomEase = zoomT < 0.5f
                    ? 4f * zoomT * zoomT * zoomT
                    : 1f - (float) Math.pow(-2f * zoomT + 2f, 3) / 2f;
                rotY = panStartY + (panTargetY - panStartY) * ease;
                rotX = panStartX + (panTargetX - panStartX) * ease;
                zoom = panStartZoom + (panTargetZoom - panStartZoom) * zoomEase;
                rotX = clamp(rotX, -1.35f, 1.35f);
                if (t >= 1f) {
                    rotY = panTargetY;
                    rotX = panTargetX;
                    zoom = panTargetZoom;
                    panActive = false;
                    staticCacheValid = false;
                }
                cameraMoving = true;
            } else if (!dragging && !scaling) {
                if (Math.abs(velY) > 0.015f || Math.abs(velX) > 0.015f) {
                    rotY += velY * dt;
                    rotX = clamp(rotX + velX * dt, -1.35f, 1.35f);
                    float decay = (float) Math.exp(-5.5 * dt);
                    velY *= decay;
                    velX *= decay;
                    if (Math.abs(velY) < 0.015f) velY = 0;
                    if (Math.abs(velX) < 0.015f) velX = 0;
                    cameraMoving = true;
                }
            }

            if (cameraMoving || dragging || scaling) {
                lastCamMoveMs = now;
                staticCacheValid = false;
                invalidate();
                requestAnimContinue();
            } else if (!Float.isNaN(pinLat)) {
                if (now - lastPinPulseMs >= pinPulseMinMs) {
                    lastPinPulseMs = now;
                    invalidate();
                }
                requestAnimContinue();
            }
            // Fully idle, no pin → stop the ticker (zero CPU)
        }
    };

    private void requestAnimContinue() {
        if (!tickerPosted) {
            tickerPosted = true;
            postOnAnimation(ticker);
        }
    }

    private void ensureStaticCache(int w, int h) {
        boolean camChanged = Float.isNaN(cacheRotY)
            || Math.abs(cacheRotY - rotY) > 0.0002f
            || Math.abs(cacheRotX - rotX) > 0.0002f
            || Math.abs(cacheZoom - zoom) > 0.0005f
            || cacheW != w || cacheH != h;
        if (staticCacheValid && !camChanged && staticCache != null) return;

        if (staticCache == null || cacheW != w || cacheH != h) {
            if (staticCache != null) staticCache.recycle();
            // RGB_565 is fine for dark globe; half the bandwidth of ARGB_8888
            staticCache = Bitmap.createBitmap(w, h, Bitmap.Config.ARGB_8888);
            staticCacheCanvas = new Canvas(staticCache);
            cacheW = w;
            cacheH = h;
        }
        staticCache.eraseColor(Color.TRANSPARENT);
        drawGlobeBodyAndBorders(staticCacheCanvas, w, h);
        cacheRotY = rotY;
        cacheRotX = rotX;
        cacheZoom = zoom;
        staticCacheValid = true;
    }

    @Override
    protected void onDraw(Canvas canvas) {
        super.onDraw(canvas);
        int w = getWidth();
        int h = getHeight();
        if (w <= 0 || h <= 0) return;

        boolean interactive = dragging || scaling || panActive
            || Math.abs(velY) > 0.015f || Math.abs(velX) > 0.015f;

        if (interactive) {
            drawGlobeBodyAndBorders(canvas, w, h);
        } else {
            ensureStaticCache(w, h);
            if (staticCache != null) {
                canvas.drawBitmap(staticCache, 0, 0, null);
            } else {
                drawGlobeBodyAndBorders(canvas, w, h);
            }
        }

        // Pin / badge always drawn on top (cheap)
        float cx = w * 0.5f;
        float cy = h * 0.5f + h * GLOBE_CENTER_Y_OFFSET;
        float r = Math.min(w, h) * 0.42f * zoom;
        float sy = (float) Math.sin(rotY);
        float cyR = (float) Math.cos(rotY);
        float sx = (float) Math.sin(rotX);
        float cxR = (float) Math.cos(rotX);
        float m00 = cyR, m02 = sy;
        float m10 = sy * sx, m11 = cxR, m12 = -cyR * sx;
        float m20 = -sy * cxR, m21 = sx, m22 = cyR * cxR;
        drawPinAndBadge(canvas, w, h, cx, cy, r, m00, m10, m11, m12, m20, m21, m22);
    }

    private void drawGlobeBodyAndBorders(Canvas canvas, int w, int h) {
        float cx = w * 0.5f;
        float cy = h * 0.5f + h * GLOBE_CENTER_Y_OFFSET;
        float r = Math.min(w, h) * 0.42f * zoom;

        if (cachedAtm == null || Math.abs(cachedAtmR - r) > 1.5f
            || Math.abs(cachedAtmCx - cx) > 0.5f || Math.abs(cachedAtmCy - cy) > 0.5f) {
            cachedAtmR = r;
            cachedAtmCx = cx;
            cachedAtmCy = cy;
            cachedAtm = new RadialGradient(
                cx, cy, r * 1.12f,
                new int[]{0x1000FF7F, 0x00000000},
                new float[]{0.88f, 1f},
                Shader.TileMode.CLAMP
            );
        }
        atmPaint.setShader(cachedAtm);
        canvas.drawCircle(cx, cy, r * 1.06f, atmPaint);
        atmPaint.setShader(null);

        canvas.drawCircle(cx, cy, r, bodyPaint);
        canvas.drawCircle(cx, cy, r + 3f, haloPaint);
        canvas.drawCircle(cx, cy, r, rimPaint);

        if (!bordersReady || rings.length == 0) return;

        float sy = (float) Math.sin(rotY);
        float cyR = (float) Math.cos(rotY);
        float sx = (float) Math.sin(rotX);
        float cxR = (float) Math.cos(rotX);
        float m00 = cyR, m01 = 0f, m02 = sy;
        float m10 = sy * sx, m11 = cxR, m12 = -cyR * sx;
        float m20 = -sy * cxR, m21 = sx, m22 = cyR * cxR;

        borderPath.rewind();
        for (int ri = 0; ri < rings.length; ri++) {
            float[] pts = rings[ri];
            if (pts == null) continue;
            int n = pts.length / 3;
            if (n < 2) continue;
            boolean started = false;
            boolean prevFront = false;
            for (int i = 0; i < n; i++) {
                int o = i * 3;
                float px = pts[o], py = pts[o + 1], pz = pts[o + 2];
                float zt = px * m20 + py * m21 + pz * m22;
                boolean front = zt >= -0.02f;
                if (!front) {
                    started = false;
                    prevFront = false;
                    continue;
                }
                float xt = px * m00 + py * m01 + pz * m02;
                float yt = px * m10 + py * m11 + pz * m12;
                float sx2 = cx + xt * r;
                float sy2 = cy + (-yt) * r;
                if (!started || !prevFront) {
                    borderPath.moveTo(sx2, sy2);
                    started = true;
                } else {
                    borderPath.lineTo(sx2, sy2);
                }
                prevFront = true;
            }
        }
        canvas.drawPath(borderPath, borderPaint);
    }

    private void drawPinAndBadge(
        Canvas canvas, int w, int h, float cx, float cy, float r,
        float m00, float m10, float m11, float m12,
        float m20, float m21, float m22
    ) {
        if (Float.isNaN(pinLat) || Float.isNaN(pinLon)) return;
        float[] p = latLngToVec3Windows(pinLat, pinLon, 1.01f);
        float yt = p[0] * m10 + p[1] * m11 + p[2] * m12;
        float zt = p[0] * m20 + p[1] * m21 + p[2] * m22;
        if (zt < 0f) return;

        float m02 = (float) Math.sin(rotY);
        float xt = p[0] * m00 + p[2] * m02;
        float px = cx + xt * r;
        float py = cy + (-yt) * r;
        float d = density();

        long now = SystemClock.uptimeMillis();
        float t = (now % 2200) / 2200f;
        for (int i = 0; i < 3; i++) {
            float phase = (t + i * 0.33f) % 1f;
            float wr = 3.2f * d + phase * 16f * d;
            int a = (int) ((1f - phase) * (1f - phase) * 0.62f * 255f);
            pinGlowPaint.setStrokeWidth(1.15f * d);
            pinGlowPaint.setColor(Color.argb(a, 0, 255, 127));
            canvas.drawCircle(px, py, wr, pinGlowPaint);
        }
        float pulse = 0.5f + 0.5f * (float) Math.sin(now / 1000.0 * 2.1);
        float core = 3.4f * d + pulse * 0.45f * d;
        pinPaint.setColor(Color.argb((int) ((0.16f + pulse * 0.10f) * 255), 0, 255, 127));
        canvas.drawCircle(px, py, core * 2.15f, pinPaint);
        pinPaint.setColor(BEACON);
        canvas.drawCircle(px, py, core, pinPaint);
        pinPaint.setColor(0xFFEAFFF4);
        canvas.drawCircle(px, py, core * 0.36f, pinPaint);
        pinPaint.setColor(BEACON);

        // Compact exit chip: flag + IP sit beside the glowing dot — not a HUD.
        boolean right = px < w * 0.62f;
        float textX = right ? px + 9f * d : px - 9f * d;
        labelPaint.setTextAlign(right ? Paint.Align.LEFT : Paint.Align.RIGHT);

        float lineY = py - 2.2f * d;
        if (pinFlag.length() > 0) {
            labelPaint.setTextSize(12.5f * d);
            labelPaint.setColor(0xFFFFFFFF);
            canvas.drawText(pinFlag, textX, lineY, labelPaint);
            lineY += 13f * d;
        }
        if (pinIp.length() > 0) {
            labelPaint.setTextSize(10.5f * d);
            labelPaint.setColor(0xFF00FF7F);
            labelPaint.setTypeface(Typeface.create("sans-serif-medium", Typeface.NORMAL));
            canvas.drawText(pinIp, textX, lineY, labelPaint);
            lineY += 12f * d;
        }
        if (pinCity.length() > 0) {
            labelPaint.setTextSize(9f * d);
            labelPaint.setColor(0xB8E8E8E8);
            canvas.drawText(pinCity, textX, lineY, labelPaint);
        }
        labelPaint.setTextAlign(Paint.Align.LEFT);
    }

    private float density() {
        return getResources().getDisplayMetrics().density;
    }

    private void syncPinPulseToDisplay() {
        float hz = 60f;
        try {
            if (Build.VERSION.SDK_INT >= 17) {
                android.view.Display display = null;
                if (Build.VERSION.SDK_INT >= 30) {
                    display = getDisplay();
                }
                if (display == null && getContext() instanceof android.app.Activity) {
                    display = ((android.app.Activity) getContext())
                        .getWindowManager().getDefaultDisplay();
                }
                if (display != null) hz = Math.max(60f, display.getRefreshRate());
            }
        } catch (Exception ignored) {
        }
        // Pin glow does not need panel-rate invalidates — 30 fps is plenty.
        pinPulseMinMs = 33;
        if (Build.VERSION.SDK_INT >= 35) {
            try {
                View.class.getMethod("setRequestedFrameRate", float.class).invoke(this, hz);
            } catch (Exception ignored) {
            }
        }
    }

    @Override
    protected void onAttachedToWindow() {
        super.onAttachedToWindow();
        syncPinPulseToDisplay();
        requestAnim();
    }

    @Override
    public boolean onTouchEvent(MotionEvent event) {
        boolean scaleHandled = scaleDetector.onTouchEvent(event);
        if (scaling || event.getPointerCount() > 1) {
            getParent().requestDisallowInterceptTouchEvent(true);
            return true;
        }
        switch (event.getActionMasked()) {
            case MotionEvent.ACTION_DOWN:
                dragging = true;
                panActive = false;
                velX = 0;
                velY = 0;
                lastX = event.getX();
                lastY = event.getY();
                lastMoveMs = SystemClock.uptimeMillis();
                getParent().requestDisallowInterceptTouchEvent(true);
                return true;
            case MotionEvent.ACTION_MOVE: {
                float dx = event.getX() - lastX;
                float dy = event.getY() - lastY;
                long now = SystemClock.uptimeMillis();
                float dt = Math.max(1f / 240f, (now - lastMoveMs) / 1000f);
                lastMoveMs = now;
                // Zoom-scaled orbit: closer = slower, farther = faster.
                float drag = ORBIT_DRAG / Math.max(0.85f, zoom);
                float rdx = dx * drag;
                float rdy = dy * drag;
                rotY += rdx;
                rotX = clamp(rotX + rdy, -1.35f, 1.35f);
                float instVx = rdx / dt;
                float instVy = rdy / dt;
                final float BLEND = 0.35f;
                velY = velY * (1f - BLEND) + instVx * BLEND;
                velX = velX * (1f - BLEND) + instVy * BLEND;
                velY = clamp(velY, -8f, 8f);
                velX = clamp(velX, -8f, 8f);
                lastX = event.getX();
                lastY = event.getY();
                requestAnim();
                return true;
            }
            case MotionEvent.ACTION_UP:
            case MotionEvent.ACTION_CANCEL:
                dragging = false;
                getParent().requestDisallowInterceptTouchEvent(false);
                requestAnim();
                return true;
        }
        return scaleHandled || super.onTouchEvent(event);
    }

    private static float clamp(float v, float lo, float hi) {
        return Math.max(lo, Math.min(hi, v));
    }

    @Override
    protected void onDetachedFromWindow() {
        super.onDetachedFromWindow();
        tickerPosted = false;
        if (staticCache != null) {
            staticCache.recycle();
            staticCache = null;
            staticCacheCanvas = null;
            staticCacheValid = false;
        }
    }

    @Override
    protected void onSizeChanged(int w, int h, int oldw, int oldh) {
        super.onSizeChanged(w, h, oldw, oldh);
        staticCacheValid = false;
    }
}
