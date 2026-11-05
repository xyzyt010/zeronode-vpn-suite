package io.zeronode.vpn;

import android.animation.ValueAnimator;
import android.content.Context;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.view.MotionEvent;
import android.view.View;
import android.view.animation.DecelerateInterpolator;
import android.widget.ScrollView;

/**
 * ScrollView with a web-style overlay scrollbar.
 *
 * <p>The thumb is mapped exactly like a desktop web page: its length is the
 * visible fraction of the content and its position is the exact scroll
 * fraction. Dragging the thumb scrolls 1:1, and touching anywhere else on the
 * bar jumps the thumb under the finger and keeps dragging from there — the bar
 * is never a dead, static decoration.
 *
 * <p>Like mobile web (and the desktop client), the bar is invisible at rest,
 * appears while the list is scrolling or being dragged, then fades away. Only
 * popup/list surfaces enable it; full-screen pages call
 * {@link #setThumbEnabled(boolean) setThumbEnabled(false)}.
 */
final class SleekScrollView extends ScrollView {
    private static final int MIN_THUMB_DP = 24;
    private static final long HIDE_DELAY_MS = 850L;

    private final Paint thumbPaint = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint trackPaint = new Paint(Paint.ANTI_ALIAS_FLAG);
    private boolean thumbEnabled = true;
    private boolean draggingThumb;
    private float dragGrab;
    private float thumbAlpha;
    private ValueAnimator fadeAnim;

    private final Runnable hideThumb = new Runnable() {
        @Override
        public void run() {
            if (draggingThumb) return;
            fadeThumbTo(0f);
        }
    };

    SleekScrollView(Context context) {
        super(context);
        setVerticalScrollBarEnabled(false);
        setHorizontalScrollBarEnabled(false);
        setScrollbarFadingEnabled(false);
        setOverScrollMode(View.OVER_SCROLL_NEVER);
        thumbPaint.setColor(0xCCFFFFFF);
        trackPaint.setColor(0x14FFFFFF);
    }

    /** Turn the floating thumb on (popup lists) or off (full-screen pages). */
    void setThumbEnabled(boolean enabled) {
        if (thumbEnabled == enabled) return;
        thumbEnabled = enabled;
        if (!enabled) {
            draggingThumb = false;
            thumbAlpha = 0f;
            removeCallbacks(hideThumb);
            if (fadeAnim != null) {
                fadeAnim.cancel();
                fadeAnim = null;
            }
        }
        invalidate();
    }

    private float dp(float v) {
        return v * getResources().getDisplayMetrics().density;
    }

    private void fadeThumbTo(float target) {
        if (fadeAnim != null) {
            fadeAnim.cancel();
            fadeAnim = null;
        }
        final float from = thumbAlpha;
        if (from == target) return;
        fadeAnim = ValueAnimator.ofFloat(from, target);
        fadeAnim.setDuration(target == 0f ? 260L : 90L);
        fadeAnim.setInterpolator(new DecelerateInterpolator());
        fadeAnim.addUpdateListener(new ValueAnimator.AnimatorUpdateListener() {
            @Override public void onAnimationUpdate(ValueAnimator animation) {
                thumbAlpha = (Float) animation.getAnimatedValue();
                invalidate();
            }
        });
        fadeAnim.start();
    }

    /** Reveal the bar and restart the auto-hide countdown. */
    private void wakeThumb() {
        if (!thumbEnabled) return;
        removeCallbacks(hideThumb);
        if (thumbAlpha < 1f) fadeThumbTo(1f);
        invalidate();
        postDelayed(hideThumb, HIDE_DELAY_MS);
    }

    /**
     * One source of truth for the thumb geometry, used by drawing and by touch
     * handling so the bar is always exactly where it is drawn.
     *
     * @return {@code {trackTop, trackLength, thumbTop, thumbLength}} in view
     *         coordinates, or null when the content fits and no bar exists.
     */
    private float[] thumbMetrics() {
        View child = getChildAt(0);
        if (child == null) return null;
        int extent = getHeight() - getPaddingTop() - getPaddingBottom();
        if (extent <= 0 || getWidth() <= 0) return null;
        int content = child.getHeight();
        int maxScroll = content - extent;
        if (maxScroll <= 0) return null;

        // Track is inset from the rounded popup edges so the thumb never sits
        // in a corner cut.
        float trackTop = getPaddingTop() + dp(4);
        float trackLen = extent - dp(8);
        if (trackLen < dp(MIN_THUMB_DP)) {
            trackTop = getPaddingTop();
            trackLen = extent;
        }
        // Visible fraction of the content, grown to a touchable minimum. This
        // is the same thumb length a web page's scrollbar would use.
        float len = trackLen * (extent / (float) content);
        float minLen = dp(MIN_THUMB_DP);
        if (len < minLen) len = minLen;
        if (len > trackLen) len = trackLen;

        float t = getScrollY() / (float) maxScroll;
        if (t < 0f) t = 0f;
        if (t > 1f) t = 1f;
        float top = trackTop + (trackLen - len) * t;
        return new float[]{trackTop, trackLen, top, len};
    }

    private boolean inBar(MotionEvent ev) {
        return ev.getX() >= getWidth() - dp(24);
    }

    @Override
    protected void onScrollChanged(int l, int t, int oldl, int oldt) {
        super.onScrollChanged(l, t, oldl, oldt);
        if (thumbEnabled) wakeThumb();
    }

    @Override
    protected void onSizeChanged(int w, int h, int oldw, int oldh) {
        super.onSizeChanged(w, h, oldw, oldh);
        invalidate();
    }

    @Override
    protected void dispatchDraw(Canvas canvas) {
        super.dispatchDraw(canvas);
        if (!thumbEnabled || thumbAlpha <= 0.01f) return;
        float[] m = thumbMetrics();
        if (m == null) return;
        float trackTop = m[0];
        float trackLen = m[1];
        float top = m[2];
        float len = m[3];
        float width = dp(3.5f);
        float right = getWidth() - dp(3);
        int alpha = (int) (thumbAlpha * 255f);

        // Faint track, same web-page proportions.
        trackPaint.setAlpha((int) (alpha * 0.10f));
        canvas.drawRoundRect(right - width, trackTop, right, trackTop + trackLen,
            width / 2f, width / 2f, trackPaint);

        thumbPaint.setAlpha((int) (alpha * 0.80f));
        canvas.drawRoundRect(right - width, top, right, top + len,
            width / 2f, width / 2f, thumbPaint);
    }

    @Override
    public boolean onInterceptTouchEvent(MotionEvent ev) {
        if (thumbEnabled && ev.getActionMasked() == MotionEvent.ACTION_DOWN && inBar(ev)) {
            float[] m = thumbMetrics();
            if (m != null) {
                float top = m[2];
                float len = m[3];
                float y = ev.getY();
                // Touching the thumb grabs it at the exact point touched.
                // Touching the empty track centres the thumb under the finger
                // first, then keeps dragging — a tap always does something.
                if (y >= top && y <= top + len) {
                    dragGrab = y - top;
                } else {
                    dragGrab = len / 2f;
                    scrollToThumbTop(y - dragGrab, m);
                }
                draggingThumb = true;
                wakeThumb();
                if (getParent() != null) {
                    getParent().requestDisallowInterceptTouchEvent(true);
                }
                return true;
            }
        }
        return super.onInterceptTouchEvent(ev);
    }

    private void scrollToThumbTop(float thumbTop, float[] m) {
        View child = getChildAt(0);
        if (child == null || m == null) return;
        int extent = getHeight() - getPaddingTop() - getPaddingBottom();
        int maxScroll = child.getHeight() - extent;
        float travel = m[1] - m[3];
        if (travel <= 0f || maxScroll <= 0) return;
        float t = (thumbTop - m[0]) / travel;
        if (t < 0f) t = 0f;
        if (t > 1f) t = 1f;
        scrollTo(getScrollX(), Math.round(t * maxScroll));
    }

    @Override
    public boolean onTouchEvent(MotionEvent ev) {
        if (draggingThumb) {
            switch (ev.getActionMasked()) {
                case MotionEvent.ACTION_MOVE: {
                    float[] m = thumbMetrics();
                    if (m != null) {
                        float target = ev.getY() - dragGrab;
                        scrollToThumbTop(target, m);
                        wakeThumb();
                    }
                    return true;
                }
                case MotionEvent.ACTION_UP:
                case MotionEvent.ACTION_CANCEL:
                    draggingThumb = false;
                    if (getParent() != null) {
                        getParent().requestDisallowInterceptTouchEvent(false);
                    }
                    removeCallbacks(hideThumb);
                    postDelayed(hideThumb, HIDE_DELAY_MS);
                    return true;
                default:
                    return true;
            }
        }
        return super.onTouchEvent(ev);
    }
}
