package io.zeronode.vpn;

import android.content.ContentResolver;
import android.content.Context;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.graphics.Rect;
import android.net.Uri;
import android.webkit.WebView;
import android.webkit.WebViewClient;

import java.io.ByteArrayOutputStream;
import java.io.InputStream;
import java.nio.charset.StandardCharsets;

/** Turn a gallery/camera/SVG pick into a square 192px launcher bitmap. */
final class IconFactory {
    interface Ready {
        void onReady(Bitmap bmp);
        void onError(String message);
    }

    private IconFactory() {}

    static void fromUri(final Context ctx, final Uri uri, final Ready ready) {
        if (uri == null) {
            ready.onError("No image selected.");
            return;
        }
        new Thread(new Runnable() {
            @Override public void run() {
                try {
                    ContentResolver cr = ctx.getContentResolver();
                    String mime = cr.getType(uri);
                    InputStream in = cr.openInputStream(uri);
                    if (in == null) {
                        postError(ctx, ready, "Could not open image.");
                        return;
                    }
                    byte[] data = readAll(in);
                    in.close();
                    boolean svg = (mime != null && mime.contains("svg"))
                        || looksLikeSvg(data);
                    if (svg) {
                        rasterizeSvg(ctx, new String(data, StandardCharsets.UTF_8), ready);
                        return;
                    }
                    Bitmap raw = BitmapFactory.decodeByteArray(data, 0, data.length);
                    if (raw == null) {
                        postError(ctx, ready, "Unsupported image. Use PNG, JPEG, WebP, or SVG.");
                        return;
                    }
                    final Bitmap square = toAppIcon(raw);
                    if (raw != square) raw.recycle();
                    ctx.getMainLooper();
                    ((android.app.Activity) ctx).runOnUiThread(new Runnable() {
                        @Override public void run() { ready.onReady(square); }
                    });
                } catch (Exception e) {
                    postError(ctx, ready, e.getMessage() != null ? e.getMessage() : "Image failed");
                }
            }
        }, "zn-icon-factory").start();
    }

    static Bitmap toAppIcon(Bitmap src) {
        int size = 192;
        int w = src.getWidth();
        int h = src.getHeight();
        int side = Math.min(w, h);
        int x = (w - side) / 2;
        int y = (h - side) / 2;
        Bitmap cropped = Bitmap.createBitmap(src, x, y, side, side);
        Bitmap out = Bitmap.createBitmap(size, size, Bitmap.Config.ARGB_8888);
        Canvas c = new Canvas(out);
        Paint p = new Paint(Paint.ANTI_ALIAS_FLAG | Paint.FILTER_BITMAP_FLAG);
        c.drawBitmap(cropped, new Rect(0, 0, side, side), new Rect(0, 0, size, size), p);
        if (cropped != src) cropped.recycle();
        return out;
    }

    private static void rasterizeSvg(final Context ctx, final String svg, final Ready ready) {
        ((android.app.Activity) ctx).runOnUiThread(new Runnable() {
            @Override public void run() {
                try {
                    final WebView wv = new WebView(ctx);
                    wv.setBackgroundColor(0x00000000);
                    wv.getSettings().setJavaScriptEnabled(false);
                    wv.layout(0, 0, 192, 192);
                    String html = "<html><head><meta name='viewport' content='width=192'/>"
                        + "<style>html,body{margin:0;padding:0;background:#0000;width:192px;height:192px;overflow:hidden}"
                        + "svg,img{width:192px;height:192px;display:block}</style></head><body>"
                        + svg + "</body></html>";
                    wv.setWebViewClient(new WebViewClient() {
                        @Override public void onPageFinished(WebView view, String url) {
                            view.postDelayed(new Runnable() {
                                @Override public void run() {
                                    Bitmap b = Bitmap.createBitmap(192, 192, Bitmap.Config.ARGB_8888);
                                    Canvas c = new Canvas(b);
                                    view.draw(c);
                                    ready.onReady(toAppIcon(b));
                                }
                            }, 80);
                        }
                    });
                    wv.loadDataWithBaseURL(null, html, "text/html", "utf-8", null);
                } catch (Exception e) {
                    ready.onError("Could not read SVG.");
                }
            }
        });
    }

    private static boolean looksLikeSvg(byte[] data) {
        if (data == null || data.length < 8) return false;
        String head = new String(data, 0, Math.min(data.length, 256), StandardCharsets.UTF_8)
            .trim()
            .toLowerCase();
        return head.contains("<svg");
    }

    private static byte[] readAll(InputStream in) throws java.io.IOException {
        ByteArrayOutputStream bos = new ByteArrayOutputStream();
        byte[] buf = new byte[8192];
        int n;
        while ((n = in.read(buf)) > 0) bos.write(buf, 0, n);
        return bos.toByteArray();
    }

    private static void postError(Context ctx, final Ready ready, final String msg) {
        ((android.app.Activity) ctx).runOnUiThread(new Runnable() {
            @Override public void run() { ready.onError(msg); }
        });
    }
}
