package io.zeronode.vpn;

import android.animation.ValueAnimator;
import android.content.Context;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.graphics.Path;
import android.graphics.PorterDuff;
import android.graphics.PorterDuffXfermode;
import android.graphics.RectF;
import android.view.View;
import android.view.animation.AccelerateDecelerateInterpolator;

/**
 * Minimal physical padlock.
 *
 * <p>The shackle is hinged at its left leg. Opening first lifts the shackle
 * slightly, then rotates the free leg away. Closing reverses the order in
 * visual terms: the free leg rotates back over the body and then drops into
 * it. The full state change lasts 750 ms, long enough to read as a mechanism
 * rather than an icon swap.</p>
 */
final class LockIconView extends View {
    static final int MUTED_RED = 0xFFCE7B73;
    static final int MUTED_GREEN = 0xFF7FAE8B;
    static final int MUTED_PURPLE = 0xFFAE97C7;

    private static final long MECHANISM_MS = 750L;

    private final Paint fillPaint = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint strokePaint = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint clearPaint = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Path shacklePath = new Path();
    private final RectF shackleArc = new RectF();
    private final RectF bodyRect = new RectF();

    private ValueAnimator animator;
    private float openAmount = 1f;
    private boolean locked;
    private int shownColor = MUTED_RED;
    private int fromColor = MUTED_RED;
    private int targetColor = MUTED_RED;

    LockIconView(Context context) {
        super(context);
        setBackgroundColor(android.graphics.Color.TRANSPARENT);
        setClickable(false);
        setFocusable(false);

        fillPaint.setStyle(Paint.Style.FILL);
        strokePaint.setStyle(Paint.Style.STROKE);
        strokePaint.setStrokeCap(Paint.Cap.ROUND);
        strokePaint.setStrokeJoin(Paint.Join.ROUND);
        clearPaint.setXfermode(new PorterDuffXfermode(PorterDuff.Mode.CLEAR));
    }

    void setState(boolean nextLocked, int color, boolean animate) {
        if (animator != null) {
            animator.cancel();
            animator = null;
        }
        fromColor = shownColor;
        targetColor = color;
        final float fromOpen = openAmount;
        final float toOpen = nextLocked ? 0f : 1f;
        locked = nextLocked;
        if (!animate || (fromOpen == toOpen && fromColor == targetColor)) {
            openAmount = toOpen;
            shownColor = targetColor;
            invalidate();
            return;
        }
        animator = ValueAnimator.ofFloat(0f, 1f);
        animator.setDuration(MECHANISM_MS);
        animator.setInterpolator(new AccelerateDecelerateInterpolator());
        animator.addUpdateListener(new ValueAnimator.AnimatorUpdateListener() {
            @Override public void onAnimationUpdate(ValueAnimator animation) {
                float f = (Float) animation.getAnimatedValue();
                openAmount = fromOpen + (toOpen - fromOpen) * f;
                shownColor = lerpArgb(fromColor, targetColor, f);
                invalidate();
            }
        });
        animator.addListener(new android.animation.AnimatorListenerAdapter() {
            @Override public void onAnimationEnd(android.animation.Animator animation) {
                openAmount = toOpen;
                shownColor = targetColor;
                animator = null;
                invalidate();
            }
            @Override public void onAnimationCancel(android.animation.Animator animation) {
                animator = null;
            }
        });
        animator.start();
    }

    boolean isLocked() {
        return locked;
    }

    @Override
    protected void onDraw(Canvas canvas) {
        super.onDraw(canvas);
        int width = getWidth();
        int height = getHeight();
        if (width <= 0 || height <= 0) return;

        float size = Math.min(width, height);
        float u = size / 32f;
        float cx = width * 0.5f;
        float bodyLeft = cx - 8.2f * u;
        float bodyRight = cx + 8.2f * u;
        float bodyTop = height * 0.5f - 0.4f * u;
        float bodyBottom = bodyTop + 11.2f * u;

        // Shackle geometry in a 32-unit design box.
        float leftX = cx - 5.1f * u;
        float rightX = cx + 5.1f * u;
        float shackleTop = bodyTop - 9.0f * u;
        float hingeY = bodyTop + 0.8f * u;

        float liftPhase = smooth01(openAmount / 0.42f);
        float rotatePhase = smooth01((openAmount - 0.38f) / 0.62f);
        float lift = liftPhase * 1.55f * u;
        float rotation = -34f * rotatePhase;

        int layer = canvas.saveLayer(0, 0, width, height, null);

        strokePaint.setColor(shownColor);
        strokePaint.setStrokeWidth(2.45f * u);
        shackleArc.set(leftX, shackleTop, rightX, shackleTop + 10.2f * u);
        shacklePath.reset();
        shacklePath.moveTo(leftX, hingeY);
        shacklePath.lineTo(leftX, shackleTop + 5.1f * u);
        shacklePath.arcTo(shackleArc, 180f, 180f, false);
        shacklePath.lineTo(rightX, hingeY);

        canvas.save();
        canvas.translate(0f, -lift);
        canvas.rotate(rotation, leftX, hingeY);
        canvas.drawPath(shacklePath, strokePaint);
        canvas.restore();

        fillPaint.setColor(shownColor);
        bodyRect.set(bodyLeft, bodyTop, bodyRight, bodyBottom);
        canvas.drawRoundRect(bodyRect, 2.1f * u, 2.1f * u, fillPaint);

        // A real transparent keyhole, cut through the filled body.
        float keyX = cx;
        float keyY = bodyTop + 4.35f * u;
        canvas.drawCircle(keyX, keyY, 1.55f * u, clearPaint);
        bodyRect.set(keyX - 0.82f * u, keyY + 0.7f * u,
            keyX + 0.82f * u, keyY + 3.35f * u);
        canvas.drawRoundRect(bodyRect, 0.82f * u, 0.82f * u, clearPaint);

        canvas.restoreToCount(layer);
    }

    private static float smooth01(float value) {
        float f = Math.max(0f, Math.min(1f, value));
        return f * f * (3f - 2f * f);
    }

    private static int lerpArgb(int from, int to, float f) {
        int a = (int) (android.graphics.Color.alpha(from)
            + (android.graphics.Color.alpha(to) - android.graphics.Color.alpha(from)) * f);
        int r = (int) (android.graphics.Color.red(from)
            + (android.graphics.Color.red(to) - android.graphics.Color.red(from)) * f);
        int g = (int) (android.graphics.Color.green(from)
            + (android.graphics.Color.green(to) - android.graphics.Color.green(from)) * f);
        int b = (int) (android.graphics.Color.blue(from)
            + (android.graphics.Color.blue(to) - android.graphics.Color.blue(from)) * f);
        return (a << 24) | (r << 16) | (g << 8) | b;
    }
}
