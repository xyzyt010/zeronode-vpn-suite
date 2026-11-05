package io.zeronode.vpn;

import android.content.Context;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.graphics.Path;
import android.graphics.RadialGradient;
import android.graphics.RectF;
import android.graphics.Shader;
import android.view.View;

/**
 * The hazy background of the top lock capsule.
 *
 * <p>Not a flat fill and not a bordered box: a soft radial haze that is denser
 * in the middle and falls off toward the rounded edge, so the bar reads as a
 * blurred translucent blob rather than a painted rectangle. Colours are
 * animated by {@link MainActivity} when the session state changes.
 */
final class LockHazeView extends View {
    private final Paint hazePaint = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Path clip = new Path();
    private final RectF bounds = new RectF();
    private int coreColor = 0x99FF5A5A;
    private int edgeColor = 0x33B71C1C;
    private float radius;
    private Shader shader;

    LockHazeView(Context context) {
        super(context);
        setLayerType(View.LAYER_TYPE_HARDWARE, null);
    }

    void setHaze(int core, int edge, float radiusPx) {
        if (coreColor == core && edgeColor == edge && radius == radiusPx && shader != null) {
            return;
        }
        coreColor = core;
        edgeColor = edge;
        radius = radiusPx;
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
        bounds.set(0f, 0f, w, h);
        clip.reset();
        clip.addRoundRect(bounds, radius, radius, Path.Direction.CW);
        if (shader == null) {
            // Denser core, translucent edge — the soft falloff is what makes
            // the capsule look hazy instead of solid.
            shader = new RadialGradient(
                w * 0.5f, h * 0.46f,
                Math.max(w, h) * 0.62f,
                new int[]{coreColor, edgeColor},
                new float[]{0f, 1f},
                Shader.TileMode.CLAMP);
            hazePaint.setShader(shader);
        }
        canvas.save();
        canvas.clipPath(clip);
        canvas.drawRect(bounds, hazePaint);
        canvas.restore();
    }
}
