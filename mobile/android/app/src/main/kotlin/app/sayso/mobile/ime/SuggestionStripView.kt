package app.sayso.mobile.ime

import android.annotation.SuppressLint
import android.content.Context
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.RectF
import android.graphics.Typeface
import android.text.TextUtils
import android.text.TextPaint
import android.view.MotionEvent
import android.view.View
import app.sayso.mobile.engine.Suggestion

/**
 * Three candidates, a revert chip after an autocorrect, and the microphone.
 * The candidate that space will commit is drawn in the accent color and underlined.
 */
@SuppressLint("ViewConstructor")
class SuggestionStripView(
    context: Context,
    private val onPick: (Int) -> Unit,
    private val onRevert: () -> Unit,
    private val onMic: () -> Unit,
) : View(context) {
    private val density = resources.displayMetrics.density
    private fun dp(v: Float) = v * density

    var palette: KeyboardPalette = KeyboardPalette.LIGHT
        set(value) { field = value; invalidate() }

    private var suggestions: List<Suggestion> = emptyList()
    private var revertWord: String? = null

    // Not inside apply: there `density` is TextPaint's own field (1.0), which made the text 17 px.
    private val text = TextPaint(Paint.ANTI_ALIAS_FLAG).also {
        it.textAlign = Paint.Align.CENTER
        it.textSize = dp(17f) * resources.configuration.fontScale
    }
    private val regular = Typeface.create("sans-serif", Typeface.NORMAL)
    private val bold = Typeface.create("sans-serif-medium", Typeface.NORMAL)
    private val paint = Paint(Paint.ANTI_ALIAS_FLAG)
    private val rect = RectF()

    private var pressedSlot = NONE

    fun show(suggestions: List<Suggestion>, revertWord: String?) {
        this.suggestions = suggestions.take(SLOTS)
        this.revertWord = revertWord
        invalidate()
    }

    override fun onMeasure(widthMeasureSpec: Int, heightMeasureSpec: Int) {
        setMeasuredDimension(MeasureSpec.getSize(widthMeasureSpec), dp(46f).toInt())
    }

    private val micWidth get() = dp(48f)
    private val chipWidth get() = if (revertWord != null) dp(96f) else 0f
    private val slotWidth get() = (width - micWidth - chipWidth) / SLOTS

    private fun slotAt(x: Float): Int = when {
        x >= width - micWidth -> MIC
        x < chipWidth -> REVERT
        else -> ((x - chipWidth) / slotWidth).toInt().coerceIn(0, SLOTS - 1)
    }

    @SuppressLint("ClickableViewAccessibility")
    override fun onTouchEvent(event: MotionEvent): Boolean {
        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN -> pressedSlot = slotAt(event.x)
            MotionEvent.ACTION_UP -> {
                val slot = slotAt(event.x)
                if (slot == pressedSlot) {
                    when {
                        slot == MIC -> onMic()
                        slot == REVERT -> onRevert()
                        slot < suggestions.size -> onPick(slot)
                    }
                }
                pressedSlot = NONE
            }
            MotionEvent.ACTION_CANCEL -> pressedSlot = NONE
        }
        invalidate()
        return true
    }

    override fun onDraw(canvas: Canvas) {
        paint.color = palette.ground
        canvas.drawRect(0f, 0f, width.toFloat(), height.toFloat(), paint)
        val cy = height / 2f
        val baseline = cy - (text.ascent() + text.descent()) / 2f

        revertWord?.let { word ->
            rect.set(dp(6f), dp(7f), chipWidth - dp(4f), height - dp(7f))
            paint.color = palette.accentWash
            canvas.drawRoundRect(rect, rect.height() / 2, rect.height() / 2, paint)
            text.typeface = regular
            text.color = palette.ink
            val label = TextUtils.ellipsize("↶ $word", text, rect.width() - dp(12f), TextUtils.TruncateAt.END)
            canvas.drawText(label, 0, label.length, rect.centerX(), baseline, text)
        }

        for (i in suggestions.indices) {
            val s = suggestions[i]
            val left = chipWidth + i * slotWidth
            if (i == pressedSlot) {
                paint.color = palette.pressedKey
                rect.set(left + dp(2f), dp(4f), left + slotWidth - dp(2f), height - dp(4f))
                canvas.drawRoundRect(rect, dp(6f), dp(6f), paint)
            }
            text.typeface = if (s.isAutoCommit) bold else regular
            text.color = if (s.isAutoCommit) palette.accent else palette.ink
            val label = TextUtils.ellipsize(s.text, text, slotWidth - dp(12f), TextUtils.TruncateAt.END)
            val cx = left + slotWidth / 2
            canvas.drawText(label, 0, label.length, cx, baseline, text)
            if (s.isAutoCommit) {
                val w = minOf(text.measureText(label, 0, label.length), slotWidth - dp(12f))
                paint.color = palette.accent
                canvas.drawRect(cx - w / 2, baseline + dp(4f), cx + w / 2, baseline + dp(6f), paint)
            }
            if (i > 0) {
                paint.color = palette.rule
                canvas.drawRect(left, dp(13f), left + dp(1f), height - dp(13f), paint)
            }
        }
        drawMic(canvas, width - micWidth / 2, cy)
    }

    /** The microphone is drawn dimmed: voice typing is not built yet, and a tap says so. */
    private fun drawMic(canvas: Canvas, cx: Float, cy: Float) {
        paint.color = palette.graphite
        paint.alpha = 110
        paint.style = Paint.Style.STROKE
        paint.strokeWidth = dp(1.6f)
        paint.strokeCap = Paint.Cap.ROUND
        rect.set(cx - dp(4.5f), cy - dp(10f), cx + dp(4.5f), cy + dp(3f))
        canvas.drawRoundRect(rect, dp(4.5f), dp(4.5f), paint)
        rect.set(cx - dp(8f), cy - dp(5f), cx + dp(8f), cy + dp(6.5f))
        canvas.drawArc(rect, 0f, 180f, false, paint)
        canvas.drawLine(cx, cy + dp(6.5f), cx, cy + dp(10f), paint)
        paint.style = Paint.Style.FILL
        paint.alpha = 255
    }

    private companion object {
        const val SLOTS = 3
        const val NONE = -1
        const val REVERT = -2
        const val MIC = -3
    }
}
