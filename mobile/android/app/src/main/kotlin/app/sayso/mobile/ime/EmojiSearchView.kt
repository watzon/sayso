package app.sayso.mobile.ime

import android.annotation.SuppressLint
import android.content.Context
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.RectF
import android.graphics.Typeface
import android.text.TextPaint
import android.text.TextUtils
import android.view.MotionEvent
import android.view.View
import android.view.ViewGroup
import android.widget.LinearLayout
import androidx.recyclerview.widget.LinearLayoutManager
import androidx.recyclerview.widget.RecyclerView
import app.sayso.mobile.emoji.Emoji

/**
 * Emoji search, above the letter keys: a row of results and the query. The
 * IME sends the keys it types to [type] and [deleteChar] instead of the
 * editor while this is open.
 */
@SuppressLint("ViewConstructor")
class EmojiSearchView(
    context: Context,
    private val store: EmojiStore,
    private val onEmoji: (String) -> Unit,
    private val onClose: () -> Unit,
) : LinearLayout(context) {
    private val density = resources.displayMetrics.density
    private fun dp(v: Float) = v * density

    var palette: KeyboardPalette = KeyboardPalette.LIGHT
        set(value) {
            field = value
            setBackgroundColor(value.ground)
            queryRow.invalidate()
            for (i in 0 until results.childCount) results.getChildAt(i).invalidate()
        }

    var query = ""
        private set

    private var found: List<Emoji> = emptyList()

    private val emojiPaint = TextPaint(Paint.ANTI_ALIAS_FLAG).also {
        it.textAlign = Paint.Align.CENTER
        it.textSize = dp(26f)
    }
    private val adapter = Adapter()
    private val results = RecyclerView(context).apply {
        layoutManager = LinearLayoutManager(context, LinearLayoutManager.HORIZONTAL, false)
        adapter = this@EmojiSearchView.adapter
        itemAnimator = null
        overScrollMode = OVER_SCROLL_NEVER
    }
    private val queryRow = QueryRow()

    init {
        orientation = VERTICAL
        addView(results, LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, dp(48f).toInt()))
        addView(queryRow, LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, dp(44f).toInt()))
    }

    fun open() {
        query = ""
        update()
    }

    fun type(text: String) {
        query += text.lowercase()
        update()
    }

    fun deleteChar() {
        if (query.isEmpty()) return
        query = query.dropLast(Character.charCount(query.codePointBefore(query.length)))
        update()
    }

    fun clear() {
        query = ""
        update()
    }

    private fun update() {
        val q = query
        store.catalog { catalog ->
            if (q != query) return@catalog
            found = if (q.isBlank()) store.recents.items.mapNotNull(catalog::find).distinct() else catalog.search(q)
            @SuppressLint("NotifyDataSetChanged")
            adapter.notifyDataSetChanged()
            results.scrollToPosition(0)
        }
        queryRow.invalidate()
    }

    private inner class Adapter : RecyclerView.Adapter<RecyclerView.ViewHolder>() {
        override fun getItemCount() = found.size

        override fun onCreateViewHolder(parent: ViewGroup, viewType: Int): RecyclerView.ViewHolder {
            val view = Result()
            view.layoutParams = RecyclerView.LayoutParams(dp(46f).toInt(), ViewGroup.LayoutParams.MATCH_PARENT)
            return object : RecyclerView.ViewHolder(view) {}
        }

        override fun onBindViewHolder(holder: RecyclerView.ViewHolder, position: Int) =
            (holder.itemView as Result).bind(found[position])
    }

    private inner class Result : View(context) {
        private var form = ""

        init {
            setOnClickListener {
                if (form.isNotEmpty()) {
                    store.used(form)
                    onEmoji(form)
                }
            }
        }

        fun bind(e: Emoji) {
            form = store.tones.preferred(e)
            contentDescription = e.name
            invalidate()
        }

        override fun drawableStateChanged() {
            super.drawableStateChanged()
            invalidate()
        }

        override fun onDraw(canvas: Canvas) {
            if (isPressed) {
                emojiPaint.color = palette.pressedKey
                canvas.drawRoundRect(dp(2f), dp(2f), width - dp(2f), height - dp(2f), dp(8f), dp(8f), emojiPaint)
            }
            emojiPaint.color = palette.ink
            canvas.drawText(form, width / 2f, height / 2f - (emojiPaint.ascent() + emojiPaint.descent()) / 2f, emojiPaint)
        }
    }

    private inner class QueryRow : View(context) {
        private val paint = Paint(Paint.ANTI_ALIAS_FLAG)
        private val text = TextPaint(Paint.ANTI_ALIAS_FLAG).also {
            it.textSize = dp(16f) * resources.configuration.fontScale
            it.typeface = Typeface.create("sans-serif", Typeface.NORMAL)
        }
        private val rect = RectF()
        private val back get() = dp(48f)

        override fun onDraw(canvas: Canvas) {
            paint.color = palette.ground
            canvas.drawRect(0f, 0f, width.toFloat(), height.toFloat(), paint)
            val cy = height / 2f
            // Back to the emoji grid.
            paint.color = palette.ink
            paint.style = Paint.Style.STROKE
            paint.strokeWidth = dp(2f)
            paint.strokeCap = Paint.Cap.ROUND
            val bx = back / 2f
            canvas.drawLine(bx - dp(7f), cy, bx + dp(7f), cy, paint)
            canvas.drawLine(bx - dp(7f), cy, bx - dp(2f), cy - dp(5f), paint)
            canvas.drawLine(bx - dp(7f), cy, bx - dp(2f), cy + dp(5f), paint)
            paint.style = Paint.Style.FILL

            rect.set(back, dp(5f), width - dp(8f), height - dp(5f))
            paint.color = palette.key
            canvas.drawRoundRect(rect, rect.height() / 2, rect.height() / 2, paint)
            val baseline = cy - (text.ascent() + text.descent()) / 2f
            val left = rect.left + dp(14f)
            val room = rect.width() - dp(28f)
            if (query.isEmpty()) {
                text.color = palette.graphite
                canvas.drawText("Search emoji", left, baseline, text)
            } else {
                text.color = palette.ink
                // The end of a long query stays in view, where the caret is.
                val shown = TextUtils.ellipsize(query, text, room - dp(4f), TextUtils.TruncateAt.START)
                canvas.drawText(shown, 0, shown.length, left, baseline, text)
                val caret = left + text.measureText(shown, 0, shown.length) + dp(1f)
                paint.color = palette.accent
                canvas.drawRect(caret, cy - dp(10f), caret + dp(2f), cy + dp(10f), paint)
            }
            if (query.isNotEmpty() && found.isEmpty()) {
                text.color = palette.graphite
                text.textAlign = Paint.Align.RIGHT
                canvas.drawText("No emoji", rect.right - dp(14f), baseline, text)
                text.textAlign = Paint.Align.LEFT
            }
        }

        @SuppressLint("ClickableViewAccessibility")
        override fun onTouchEvent(event: MotionEvent): Boolean {
            if (event.actionMasked == MotionEvent.ACTION_UP && event.x < back) onClose()
            return true
        }
    }
}
