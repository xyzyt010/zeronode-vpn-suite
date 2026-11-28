package io.zeronode.vpn;

import android.content.Context;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.view.MotionEvent;
import android.view.View;
import android.widget.ScrollView;

/**
 * ScrollView with a real, correctly-mapped vertical scrollbar.
 *
 * <p>Thumb length is the visible fraction ({@code viewport / content}) and
 * position is the exact scroll fraction ({@code scrollY / maxScroll}) — same
 * as a web page. The thumb is draggable 1:1 and tapping the track centres
 * the thumb under the finger. The bar is visible whenever content overflows
 * (popup lists) and hidden on full-screen pages that call
 * {@link #setThumbEnabled(boolean)} with {@code false}.
 *
 * <p>Geometry lives in {@link #thumbMetrics()}, used by both drawing and
 * touch, so the bar is always exactly where it is drawn.
 */
final class SleekScrollView extends ScrollView {
    private static final float MIN_THUMB_DP = 18f;
    private static final float BAR_HIT_DP = 14f;

    private final Paint thumbPaint = new Paint(Paint.ANTI_ALIAS_FLAG);
    private final Paint trackPaint = new Paint(Paint.ANTI_ALIAS_FLAG);
    private boolean thumbEnabled = true;
    private boolean draggingThumb;
    private float dragGrab;

    SleekScrollView(Context context) {
        super(context);
        setVerticalScrollBarEnabled(false);
        setHorizontalScrollBarEnabled(false);
        setScrollbarFadingEnabled(false);
        setOverScrollMode(View.OVER_SCROLL_NEVER);
        thumbPaint.setColor(0xB0FFFFFF);     // clearly visible thumb
        trackPaint.setColor(0x1FFFFFFF);     // faint full-height track
    }

    /** Turn the scrollbar on (popup lists) or off (full-screen pages). */
    void setThumbEnabled(boolean enabled) {
        if (thumbEnabled == enabled) return;
        thumbEnabled = enabled;
        draggingThumb = false;
        invalidate();
    }

    private float dp(float v) {
        return v * getResources().getDisplayMetrics().density;
    }

    /**
     * The single source of truth for thumb geometry, in view coordinates.
     *
     * @return {@code {trackTop, trackLength, thumbTop, thumbLength}}, or
     *         {@code null} when the content fits and there is no bar.
     */
    private float[] thumbMetrics() {
        View child = getChildAt(0);
        if (child == null) return null;
        int padTop = getPaddingTop();
        int padBottom = getPaddingBottom();
        int extent = getHeight() - padTop - padBottom;
        if (extent <= 0 || getWidth() <= 0) return null;
        int content = child.getHeight();
        int maxScroll = content - extent;
        if (maxScroll <= 0) return null;

        // Track inset a touch from the rounded popup edges.
        float inset = dp(2);
        float trackTop = padTop + inset;
        float trackLen = extent - inset * 2f;
        if (trackLen < dp(MIN_THUMB_DP)) {
            trackTop = padTop;
            trackLen = extent;
        }
        // Length = visible fraction; grown to a touchable minimum.
        float len = trackLen * (extent / (float) content);
        float minLen = dp(MIN_THUMB_DP);
        if (len < minLen) len = minLen;
        if (len > trackLen) len = trackLen;

        float t = maxScroll == 0 ? 0f : getScrollY() / (float) maxScroll;
        if (t < 0f) t = 0f;
        if (t > 1f) t = 1f;
        float top = trackTop + (trackLen - len) * t;
        return new float[]{trackTop, trackLen, top, len};
    }

    private boolean inBar(MotionEvent ev) {
        return ev.getX() >= getWidth() - dp(BAR_HIT_DP);
    }

    private boolean childHooked;

    @Override
    protected void onLayout(boolean changed, int l, int t, int r, int b) {
        super.onLayout(changed, l, t, r, b);
        View child = getChildAt(0);
        if (child != null && !childHooked) {
            childHooked = true;
            // Content height changes (profile rows added / enriched, Tor
            // countries filled after popup show) must re-map the thumb even
            // before the next scroll — otherwise the length/position is stale
            // and looks fake.
            child.addOnLayoutChangeListener(new View.OnLayoutChangeListener() {
                @Override public void onLayoutChange(View v, int left, int top,
                    int right, int bottom, int oldLeft, int oldTop,
                    int oldRight, int oldBottom) {
                    if (bottom - top != oldBottom - oldTop) invalidate();
                }
            });
        }
        invalidate();
    }

    @Override
    protected void onScrollChanged(int l, int t, int oldl, int oldt) {
        super.onScrollChanged(l, t, oldl, oldt);
        invalidate();
    }

    @Override
    protected void onSizeChanged(int w, int h, int oldw, int oldh) {
        super.onSizeChanged(w, h, oldw, oldh);
        invalidate();
    }

    @Override
    protected void dispatchDraw(Canvas canvas) {
        super.dispatchDraw(canvas);
        if (!thumbEnabled) return;
        float[] m = thumbMetrics();
        if (m == null) return;
        float trackTop = m[0];
        float trackLen = m[1];
        float top = m[2];
        float len = m[3];
        float width = dp(4);
        float right = getWidth() - dp(3);
        canvas.drawRoundRect(right - width, trackTop, right, trackTop + trackLen,
            width / 2f, width / 2f, trackPaint);
        canvas.drawRoundRect(right - width, top, right, top + len,
            width / 2f, width / 2f, thumbPaint);
    }

    private void scrollToThumbTop(float thumbTop, float[] m) {
        View child = getChildAt(0);
        if (child == null || m == null) return;
        int extent = getHeight() - getPaddingTop() - getPaddingBottom();
        if (extent <= 0) return;
        int maxScroll = child.getHeight() - extent;
        float travel = m[1] - m[3];
        if (travel <= 0f || maxScroll <= 0) return;
        float t = (thumbTop - m[0]) / travel;
        if (t < 0f) t = 0f;
        if (t > 1f) t = 1f;
        scrollTo(getScrollX(), Math.round(t * maxScroll));
    }

    @Override
    public boolean onInterceptTouchEvent(MotionEvent ev) {
        if (thumbEnabled && ev.getActionMasked() == MotionEvent.ACTION_DOWN && inBar(ev)) {
            float[] m = thumbMetrics();
            if (m != null) {
                float top = m[2];
                float len = m[3];
                float y = ev.getY();
                // On the thumb: grab at the touched point. On the empty track:
                // centre the thumb under the finger, then keep dragging.
                if (y >= top && y <= top + len) {
                    dragGrab = y - top;
                } else {
                    dragGrab = len / 2f;
                    scrollToThumbTop(y - dragGrab, m);
                }
                draggingThumb = true;
                if (getParent() != null) getParent().requestDisallowInterceptTouchEvent(true);
                return true;
            }
        }
        return super.onInterceptTouchEvent(ev);
    }

    @Override
    public boolean onTouchEvent(MotionEvent ev) {
        if (draggingThumb) {
            switch (ev.getActionMasked()) {
                case MotionEvent.ACTION_MOVE: {
                    float[] m = thumbMetrics();
                    if (m != null) scrollToThumbTop(ev.getY() - dragGrab, m);
                    return true;
                }
                case MotionEvent.ACTION_UP:
                case MotionEvent.ACTION_CANCEL:
                    draggingThumb = false;
                    if (getParent() != null) getParent().requestDisallowInterceptTouchEvent(false);
                    return true;
                default:
                    return true;
            }
        }
        return super.onTouchEvent(ev);
    }
}
