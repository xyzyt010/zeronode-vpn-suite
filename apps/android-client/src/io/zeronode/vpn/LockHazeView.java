package io.zeronode.vpn;

import android.content.Context;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.graphics.RadialGradient;
import android.graphics.Shader;
import android.view.View;

/**
 * The very small in-theme haze behind the top status glyph.
 *
 * <p>Deliberately not a shape, not a border, not a filled box — it is a soft
 * radial glow that is brightest just under the lock and falls off to fully
 * transparent, so there is no visible boundary/edge. Colours are animated by
 * {@link MainActivity} as the session state changes (red idle, purple Tor,
 * green when an encrypted tunnel is live).
 */
final class LockHazeView extends View {
    private final Paint hazePaint = new Paint(Paint.ANTI_ALIAS_FLAG);
    private int coreColor = 0x66FF5A5A;
    private int edgeColor = 0x00FF5A5A;
    private Shader shader;
    private int shaderW;
    private int shaderH;

    LockHazeView(Context context) {
        super(context);
        // No hardware layer: this allocates a new RadialGradient on each colour
        // step anyway; skipping the extra layer keeps memory low (RAM budget).
        setLayerType(View.LAYER_TYPE_NONE, null);
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

    @Override
    protected void onSizeChanged(int w, int h, int oldw, int oldh) {
        super.onSizeChanged(w, h, oldw, oldh);
        shader = null;
    }

    @Override
    protected void onDraw(Canvas canvas) {
        int w = getWidth();
        int h = getHeight();
        if (w <= 0 || h <= 0) return;
        if (shader == null || shaderW != w || shaderH != h) {
            // Glow centre sits a touch above vertical middle (over the lock),
            // radius generous so it fades well below the glyph with no edge.
            float cx = w * 0.5f;
            float cy = h * 0.42f;
            float radius = Math.max(w, h) * 1.05f;
            shader = new RadialGradient(
                cx, cy, radius,
                new int[]{coreColor, edgeColor, 0x00000000},
                new float[]{0f, 0.42f, 1f},
                Shader.TileMode.CLAMP);
            shaderW = w;
            shaderH = h;
            hazePaint.setShader(shader);
        }
        canvas.drawRect(0f, 0f, w, h, hazePaint);
    }
}
