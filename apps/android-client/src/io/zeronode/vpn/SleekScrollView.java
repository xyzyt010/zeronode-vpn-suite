package io.zeronode.vpn;

import android.content.Context;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.view.MotionEvent;
import android.view.View;
import android.widget.ScrollView;

/**
 * ScrollView with an optional thin position indicator.
 *
 * <p>The thumb uses the same mapping a web page uses: the thumb's length is
 * the visible fraction of the content, and its position is the exact scroll
 * fraction — nothing is cached between frames, so the bar always reports the
 * true scroll location and dragging it scrolls 1:1.
 *
 * <p>Full-screen pages disable the thumb entirely; only popup/list surfaces
 * keep it, and they are mapped exactly the same way.
 */
final class SleekScrollView extends ScrollView {
    private static final int MIN_THUMB_DP = 24;

    private final Paint thumbPaint = new Paint(Paint.ANTI_ALIAS_FLAG);
    private boolean thumbEnabled = true;
    private boolean draggingThumb;
    private float dragGrab;

    SleekScrollView(Context context) {
        super(context);
        setVerticalScrollBarEnabled(false);
        setHorizontalScrollBarEnabled(false);
        setScrollbarFadingEnabled(false);
        setOverScrollMode(View.OVER_SCROLL_NEVER);
        thumbPaint.setColor(0xB3FFFFFF);
    }

    /** Turn the floating thumb on (popup lists) or off (full-screen pages). */
    void setThumbEnabled(boolean enabled) {
        if (thumbEnabled == enabled) return;
        thumbEnabled = enabled;
        if (!enabled) draggingThumb = false;
        invalidate();
    }

    private float dp(float v) {
        return v * getResources().getDisplayMetrics().density;
    }

    /**
     * One source of truth for the thumb geometry, used by drawing and by touch
     * handling so the bar is always exactly where it is drawn.
     *
     * @return {@code {top, length}} in view coordinates, or null when the
     *         content fits and no thumb should be shown.
     */
    private float[] thumbMetrics() {
        View child = getChildAt(0);
        if (child == null) return null;
        int extent = getHeight() - getPaddingTop() - getPaddingBottom();
        int content = child.getHeight();
        int maxScroll = content - extent;
        if (extent <= 0 || maxScroll <= 0) return null;
        // Visible fraction of the content, grown to a touchable minimum.
        float len = extent * (extent / (float) content);
        float minLen = dp(MIN_THUMB_DP);
        if (len < minLen) len = minLen;
        if (len > extent) len = extent;
        float maxScrollF = maxScroll;
        float t = getScrollY() / maxScrollF;
        if (t < 0f) t = 0f;
        if (t > 1f) t = 1f;
        float top = getPaddingTop() + (extent - len) * t;
        return new float[]{top, len};
    }

    @Override
    protected void onScrollChanged(int l, int t, int oldl, int oldt) {
        super.onScrollChanged(l, t, oldl, oldt);
        // Keep the thumb glued to the scroll position every frame it moves.
        if (thumbEnabled) invalidate();
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
        float width = dp(4);
        float right = getWidth() - dp(2);
        canvas.drawRoundRect(right - width, m[0], right, m[0] + m[1],
            dp(2), dp(2), thumbPaint);
    }

    @Override
    public boolean onInterceptTouchEvent(MotionEvent ev) {
        if (thumbEnabled && ev.getActionMasked() == MotionEvent.ACTION_DOWN) {
            float[] m = thumbMetrics();
            if (m != null && ev.getX() >= getWidth() - dp(26)) {
                float y = ev.getY();
                if (y >= m[0] - dp(12) && y <= m[0] + m[1] + dp(12)) {
                    draggingThumb = true;
                    dragGrab = y - m[0];
                    if (getParent() != null) {
                        getParent().requestDisallowInterceptTouchEvent(true);
                    }
                    return true;
                }
            }
        }
        return super.onInterceptTouchEvent(ev);
    }

    @Override
    public boolean onTouchEvent(MotionEvent ev) {
        if (draggingThumb) {
            switch (ev.getActionMasked()) {
                case MotionEvent.ACTION_MOVE: {
                    View child = getChildAt(0);
                    if (child == null) return true;
                    int extent = getHeight() - getPaddingTop() - getPaddingBottom();
                    int maxScroll = child.getHeight() - extent;
                    float[] m = thumbMetrics();
                    float len = m != null ? m[1] : dp(MIN_THUMB_DP);
                    float travel = extent - len;
                    if (travel > 0f && maxScroll > 0) {
                        float target = ev.getY() - dragGrab - getPaddingTop();
                        float t = target / travel;
                        if (t < 0f) t = 0f;
                        if (t > 1f) t = 1f;
                        scrollTo(getScrollX(), Math.round(t * maxScroll));
                    }
                    return true;
                }
                case MotionEvent.ACTION_UP:
                case MotionEvent.ACTION_CANCEL:
                    draggingThumb = false;
                    if (getParent() != null) {
                        getParent().requestDisallowInterceptTouchEvent(false);
                    }
                    return true;
                default:
                    return true;
            }
        }
        return super.onTouchEvent(ev);
    }
}
