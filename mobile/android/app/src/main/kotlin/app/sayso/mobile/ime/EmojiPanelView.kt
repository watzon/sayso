package app.sayso.mobile.ime

import android.annotation.SuppressLint
import android.content.Context
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.Typeface
import android.view.MotionEvent
import android.view.View
import android.widget.LinearLayout
import android.widget.ScrollView

/** A scrolling grid of common emoji with a bar to go back to letters and to delete. */
@SuppressLint("ViewConstructor")
class EmojiPanelView(
    context: Context,
    private val onEmoji: (String) -> Unit,
    private val onBackspace: () -> Unit,
    private val onLetters: () -> Unit,
) : LinearLayout(context) {
    private val density = resources.displayMetrics.density

    var palette: KeyboardPalette = KeyboardPalette.LIGHT
        set(value) {
            field = value
            setBackgroundColor(value.ground)
            grid.invalidate()
            bar.invalidate()
        }

    private val grid = Grid()
    private val bar = Bar()
    private val scroll = ScrollView(context).apply { addView(grid) }

    init {
        orientation = VERTICAL
        addView(scroll, LayoutParams(LayoutParams.MATCH_PARENT, 0, 1f))
        addView(bar, LayoutParams(LayoutParams.MATCH_PARENT, (48 * density).toInt()))
    }

    private inner class Grid : View(context) {
        private val cell = 48 * density
        private val paint = Paint(Paint.ANTI_ALIAS_FLAG).apply { textAlign = Paint.Align.CENTER; textSize = 28 * density }

        private val columns get() = (width / cell).toInt().coerceAtLeast(1)

        override fun onMeasure(widthMeasureSpec: Int, heightMeasureSpec: Int) {
            val w = MeasureSpec.getSize(widthMeasureSpec)
            val cols = (w / cell).toInt().coerceAtLeast(1)
            val rows = (EMOJI.size + cols - 1) / cols
            setMeasuredDimension(w, (rows * cell).toInt())
        }

        override fun onDraw(canvas: Canvas) {
            val cols = columns
            val cellW = width.toFloat() / cols
            EMOJI.forEachIndexed { i, e ->
                val cx = (i % cols + 0.5f) * cellW
                val cy = (i / cols + 0.5f) * cell
                canvas.drawText(e, cx, cy - (paint.ascent() + paint.descent()) / 2f, paint)
            }
        }

        @SuppressLint("ClickableViewAccessibility")
        override fun onTouchEvent(event: MotionEvent): Boolean {
            // The ScrollView cancels the touch when it scrolls, so an up event is a tap.
            if (event.actionMasked == MotionEvent.ACTION_UP) {
                val cols = columns
                val col = (event.x / (width.toFloat() / cols)).toInt().coerceIn(0, cols - 1)
                val index = (event.y / cell).toInt() * cols + col
                EMOJI.getOrNull(index)?.let(onEmoji)
            }
            return true
        }
    }

    private inner class Bar : View(context) {
        private val face = Paint()
        private val text = Paint(Paint.ANTI_ALIAS_FLAG).apply {
            textAlign = Paint.Align.CENTER
            textSize = 15 * density
            typeface = Typeface.create("sans-serif-medium", Typeface.NORMAL)
        }
        private val buttonWidth get() = 72 * density

        override fun onDraw(canvas: Canvas) {
            face.color = palette.modifierKey
            canvas.drawRect(0f, 0f, width.toFloat(), height.toFloat(), face)
            text.color = palette.ink
            val baseline = height / 2f - (text.ascent() + text.descent()) / 2f
            canvas.drawText("ABC", buttonWidth / 2, baseline, text)
            canvas.drawText("⌫", width - buttonWidth / 2, baseline, text)
        }

        @SuppressLint("ClickableViewAccessibility")
        override fun onTouchEvent(event: MotionEvent): Boolean {
            if (event.actionMasked == MotionEvent.ACTION_UP) {
                when {
                    event.x < buttonWidth -> onLetters()
                    event.x > width - buttonWidth -> onBackspace()
                }
            }
            return true
        }
    }

    private companion object {
        val EMOJI = listOf(
            "😀", "😂", "🥲", "😊", "😍", "🥰", "😘", "😎", "🤔", "😅", "😭", "😴", "🙃", "😉", "😬", "🤯",
            "😡", "🥳", "😇", "🤗", "🙄", "😐", "😢", "😱", "👍", "👎", "👏", "🙌", "🙏", "💪", "👋", "🤝",
            "✌️", "🤞", "👌", "👀", "❤️", "🧡", "💛", "💚", "💙", "💜", "🖤", "💔", "✨", "🔥", "🎉", "💯",
            "✅", "❌", "⭐", "☀️", "🌙", "🌧️", "☕", "🍕", "🍺", "🎂", "🐶", "🐱", "🚗", "✈️", "🏠", "💻",
        )
    }
}
