package io.zeronode.vpn;

import android.content.Context;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.graphics.RadialGradient;
import android.graphics.Shader;
import android.widget.FrameLayout;

/**
 * The status glyph container: a small in-theme haze drawn behind a centred lock.
 *
 * <p>This is a {@link FrameLayout} whose size is determined entirely by its
 * children (wrap-content) and paints a soft radial glow <em>behind</em> them in
 * {@link #dispatchDraw(Canvas)}. Because the glow is drawn onto this view's own
 * canvas instead of via a MATCH_PARENT child, it can never inflate the badge —
 * the previous "haze covering the whole screen" bug was exactly a MATCH_PARENT
 * child inside a wrap-content FrameLayout, which Android sizes to the full
 * available space.
 *
 * <p>No border, no shape, no filled box: the glow is brightest just under the
 * lock and fades to fully transparent, so there is no visible boundary.
 * Colours are animated by {@link MainActivity} as the session changes.
 */
final class LockHazeView extends FrameLayout {
    private final Paint hazePaint = new Paint(Paint.ANTI_ALIAS_FLAG);
    private int coreColor = 0x59FF5A5A;   // ~35% alpha: translucent, not a block
    private int edgeColor = 0x00FF5A5A;   // fully transparent at the rim
    private Shader shader;
    private int shaderW;
    private int shaderH;

    LockHazeView(Context context) {
        super(context);
        setClipChildren(false);
        setClipToPadding(false);
        setWillNotDraw(false);
    }

    void setHaze(int core, int edge) {
        if (coreColor == core && edgeColor == edge && shader != null) {
            return;
        }
        coreColor = core;
        edgeColor = edge;
        shader = null;
        invalidate();
    }

    float glowCoreAlpha() {
        return (coreColor >>> 24) / 255f;
    }

    @Override
    protected void onSizeChanged(int w, int h, int oldw, int oldh) {
        super.onSizeChanged(w, h, oldw, oldh);
        shader = null;
    }

    @Override
    protected void dispatchDraw(Canvas canvas) {
        int w = getWidth();
        int h = getHeight();
        if (w > 0 && h > 0) {
            if (shader == null || shaderW != w || shaderH != h) {
                // Centre a touch above middle (over the lock), radius generous
                // so it fades smoothly around and below the glyph.
                float cx = w * 0.5f;
                float cy = h * 0.34f;
                float radius = Math.max(w, h) * 0.85f;
                shader = new RadialGradient(
                    cx, cy, radius,
                    new int[]{coreColor, edgeColor, 0x00000000},
                    new float[]{0f, 0.45f, 1f},
                    Shader.TileMode.CLAMP);
                shaderW = w;
                shaderH = h;
                hazePaint.setShader(shader);
            }
            canvas.drawRect(0f, 0f, w, h, hazePaint);
        }
        super.dispatchDraw(canvas);
    }
}
