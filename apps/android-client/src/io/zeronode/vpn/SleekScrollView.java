package io.zeronode.vpn;

import android.content.Context;
import android.graphics.Canvas;
import android.graphics.Paint;
import android.view.MotionEvent;
import android.view.View;
import android.widget.ScrollView;

/**
 * ScrollView with a thin position indicator.
 *
 * <p>The bar maps the visible window onto the full content height, so it shows
 * exactly how far into a section the user is. It is a marker first, but it can
 * also be dragged to jump — like a desktop scrollbar.
 */
final class SleekScrollView extends ScrollView {
    private final Paint thumbPaint = new Paint(Paint.ANTI_ALIAS_FLAG);
    private float thumbTop;
    private float thumbLen;
    private boolean hasThumb;
    private boolean draggingThumb;
    private float dragGrab;

    SleekScrollView(Context context) {
        super(context);
        setVerticalScrollBarEnabled(false);
        setHorizontalScrollBarEnabled(false);
        setScrollbarFadingEnabled(false);
        setOverScrollMode(View.OVER_SCROLL_NEVER);
        thumbPaint.setColor(0xA6FFFFFF);
    }

    private float dp(float v) {
        return v * getResources().getDisplayMetrics().density;
    }

    private void measureThumb() {
        int extent = getHeight();
        int range = getChildCount() > 0 ? getChildAt(0).getHeight() : 0;
        hasThumb = extent > 0 && range > extent;
        if (!hasThumb) {
            thumbLen = 0f;
            thumbTop = 0f;
            return;
        }
        thumbLen = Math.max(dp(28), extent * (float) extent / range);
        float travel = extent - thumbLen;
        float maxScroll = range - extent;
        float t = maxScroll <= 0f ? 0f : Math.max(0f, Math.min(1f, getScrollY() / maxScroll));
        thumbTop = t * travel;
    }

    @Override
    protected void dispatchDraw(Canvas canvas) {
        super.dispatchDraw(canvas);
        measureThumb();
        if (!hasThumb) return;
        float right = getWidth() - dp(2);
        canvas.drawRoundRect(right - dp(3), thumbTop, right, thumbTop + thumbLen,
            dp(1.5f), dp(1.5f), thumbPaint);
    }

    @Override
    public boolean onInterceptTouchEvent(MotionEvent ev) {
        if (ev.getActionMasked() == MotionEvent.ACTION_DOWN) {
            measureThumb();
            if (hasThumb && ev.getX() >= getWidth() - dp(28)) {
                float y = ev.getY();
                if (y >= thumbTop - dp(12) && y <= thumbTop + thumbLen + dp(12)) {
                    draggingThumb = true;
                    dragGrab = y - thumbTop;
                    if (getParent() != null) getParent().requestDisallowInterceptTouchEvent(true);
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
                    float y = ev.getY() - dragGrab;
                    int extent = getHeight();
                    int range = getChildCount() > 0 ? getChildAt(0).getHeight() : 0;
                    float travel = extent - thumbLen;
                    float maxScroll = range - extent;
                    if (travel > 0f && maxScroll > 0f) {
                        float t = Math.max(0f, Math.min(1f, y / travel));
                        scrollTo(getScrollX(), (int) (t * maxScroll));
                    }
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
