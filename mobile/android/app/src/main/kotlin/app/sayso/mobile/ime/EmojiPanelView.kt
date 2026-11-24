package app.sayso.mobile.ime

import android.annotation.SuppressLint
import android.content.Context
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.Path
import android.graphics.RectF
import android.graphics.Typeface
import android.os.Handler
import android.os.Looper
import android.text.TextPaint
import android.view.MotionEvent
import android.view.View
import android.view.ViewGroup
import android.widget.FrameLayout
import android.widget.LinearLayout
import androidx.recyclerview.widget.GridLayoutManager
import androidx.recyclerview.widget.RecyclerView
import app.sayso.mobile.emoji.Emoji
import app.sayso.mobile.emoji.EmojiCatalog
import app.sayso.mobile.emoji.EmojiCategory
import app.sayso.mobile.emoji.EmojiGrid
import app.sayso.mobile.emoji.EmojiGridItem

/**
 * The emoji keyboard: category tabs, a recycled grid of every emoji the font
 * can draw (recent ones first), and a bar with ABC, search, and delete. Hold
 * an emoji with skin tones to pick one; the grid then shows that tone.
 */
@SuppressLint("ViewConstructor")
class EmojiPanelView(
    context: Context,
    private val store: EmojiStore,
    private val onEmoji: (String) -> Unit,
    private val onBackspace: () -> Unit,
    private val onLetters: () -> Unit,
    private val onSearch: () -> Unit,
) : FrameLayout(context) {
    private val density = resources.displayMetrics.density
    private fun dp(v: Float) = v * density

    var palette: KeyboardPalette = KeyboardPalette.LIGHT
        set(value) {
            field = value
            setBackgroundColor(value.ground)
            tabs.invalidate()
            bar.invalidate()
            picker.invalidate()
            for (i in 0 until recycler.childCount) recycler.getChildAt(i).invalidate()
        }

    private var catalog: EmojiCatalog? = null
    private var grid = EmojiGrid(emptyList(), emptyMap())
    private var active = EmojiCategory.SMILEYS

    private val emojiPaint = TextPaint(Paint.ANTI_ALIAS_FLAG).also {
        it.textAlign = Paint.Align.CENTER
        it.textSize = dp(27f)
    }
    private val labelPaint = TextPaint(Paint.ANTI_ALIAS_FLAG).also {
        it.textSize = dp(12f) * resources.configuration.fontScale
        it.typeface = Typeface.create("sans-serif-medium", Typeface.NORMAL)
    }

    private val tabs = Tabs()
    private val bar = Bar()
    private val picker = TonePicker()
    private val layoutManager = GridLayoutManager(context, COLUMNS_MIN)
    private val adapter = Adapter()
    private val recycler = RecyclerView(context).apply {
        layoutManager = this@EmojiPanelView.layoutManager
        adapter = this@EmojiPanelView.adapter
        setHasFixedSize(true)
        itemAnimator = null
        setItemViewCacheSize(24)
        recycledViewPool.setMaxRecycledViews(TYPE_CELL, 96)
        overScrollMode = OVER_SCROLL_NEVER
        clipToPadding = false
        setPadding(dp(4f).toInt(), 0, dp(4f).toInt(), 0)
    }

    init {
        layoutManager.spanSizeLookup = object : GridLayoutManager.SpanSizeLookup() {
            override fun getSpanSize(position: Int) =
                if (grid.items.getOrNull(position) is EmojiGridItem.Header) layoutManager.spanCount else 1
        }
        recycler.addOnScrollListener(object : RecyclerView.OnScrollListener() {
            override fun onScrolled(view: RecyclerView, dx: Int, dy: Int) {
                val first = layoutManager.findFirstVisibleItemPosition()
                // At the very end the last category may be too short to reach the top.
                val position = if (!view.canScrollVertically(1)) grid.items.size - 1 else first
                grid.categoryAt(position)?.let { setActive(it) }
            }
        })
        recycler.addOnLayoutChangeListener { _, left, _, right, _, oldLeft, _, oldRight, _ ->
            if (right - left != oldRight - oldLeft) {
                val columns = ((right - left) / dp(CELL_DP)).toInt().coerceAtLeast(COLUMNS_MIN)
                if (columns != layoutManager.spanCount) recycler.post { layoutManager.spanCount = columns }
            }
        }
        val column = LinearLayout(context).apply {
            orientation = LinearLayout.VERTICAL
            addView(tabs, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, dp(42f).toInt()))
            addView(recycler, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, 0, 1f))
            addView(bar, LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, dp(46f).toInt()))
        }
        addView(column, LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT))
        addView(picker, LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT))
        picker.visibility = GONE
    }

    /** Builds the grid again, with the latest recents, each time the panel opens. */
    fun open() {
        picker.visibility = GONE
        store.catalog { c ->
            catalog = c
            grid = EmojiGrid.build(c, store.recents.items, store.tones)
            @SuppressLint("NotifyDataSetChanged")
            adapter.notifyDataSetChanged()
            layoutManager.scrollToPositionWithOffset(0, 0)
            setActive(grid.starts.keys.firstOrNull() ?: EmojiCategory.SMILEYS)
        }
    }

    private fun setActive(category: EmojiCategory) {
        if (category == active) return
        active = category
        tabs.invalidate()
    }

    private fun jumpTo(category: EmojiCategory) {
        val start = grid.starts[category] ?: return
        recycler.stopScroll()
        layoutManager.scrollToPositionWithOffset(start, 0)
        setActive(category)
    }

    private fun pick(cell: EmojiGridItem.Cell) {
        store.used(cell.form)
        onEmoji(cell.form)
    }

    private fun showTones(cell: EmojiCell, item: EmojiGridItem.Cell) {
        val where = IntArray(2)
        val mine = IntArray(2)
        cell.getLocationInWindow(where)
        getLocationInWindow(mine)
        picker.show(item.emoji, where[0] - mine[0] + cell.width / 2f, (where[1] - mine[1]).toFloat(), cell.height.toFloat())
    }

    private inner class Adapter : RecyclerView.Adapter<RecyclerView.ViewHolder>() {
        override fun getItemCount() = grid.items.size

        override fun getItemViewType(position: Int) =
            if (grid.items[position] is EmojiGridItem.Header) TYPE_HEADER else TYPE_CELL

        override fun onCreateViewHolder(parent: ViewGroup, viewType: Int): RecyclerView.ViewHolder {
            val view = if (viewType == TYPE_HEADER) Header() else EmojiCell()
            val height = if (viewType == TYPE_HEADER) dp(28f) else dp(CELL_DP)
            view.layoutParams = RecyclerView.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, height.toInt())
            return object : RecyclerView.ViewHolder(view) {}
        }

        override fun onBindViewHolder(holder: RecyclerView.ViewHolder, position: Int) {
            when (val item = grid.items[position]) {
                is EmojiGridItem.Header -> (holder.itemView as Header).bind(item.category)
                is EmojiGridItem.Cell -> (holder.itemView as EmojiCell).bind(item)
            }
        }
    }

    private inner class Header : View(context) {
        private var title = ""

        fun bind(category: EmojiCategory) {
            title = category.title
            invalidate()
        }

        override fun onDraw(canvas: Canvas) {
            labelPaint.color = palette.graphite
            labelPaint.textAlign = Paint.Align.LEFT
            canvas.drawText(title, dp(8f), height - dp(8f), labelPaint)
        }
    }

    private inner class EmojiCell : View(context) {
        private var item: EmojiGridItem.Cell? = null
        private val mark = Path()

        init {
            setOnClickListener { item?.let(::pick) }
            setOnLongClickListener {
                val i = item ?: return@setOnLongClickListener false
                if (!i.emoji.hasTones) return@setOnLongClickListener false
                showTones(this, i)
                true
            }
        }

        fun bind(cell: EmojiGridItem.Cell) {
            item = cell
            contentDescription = cell.emoji.name
            invalidate()
        }

        override fun drawableStateChanged() {
            super.drawableStateChanged()
            invalidate()
        }

        override fun onDraw(canvas: Canvas) {
            val cell = item ?: return
            if (isPressed) {
                emojiPaint.color = palette.pressedKey
                canvas.drawRoundRect(dp(2f), dp(2f), width - dp(2f), height - dp(2f), dp(8f), dp(8f), emojiPaint)
            }
            emojiPaint.color = palette.ink
            val baseline = height / 2f - (emojiPaint.ascent() + emojiPaint.descent()) / 2f
            canvas.drawText(cell.form, width / 2f, baseline, emojiPaint)
            if (cell.emoji.hasTones) {
                // A small corner mark: hold for skin tones.
                mark.reset()
                mark.moveTo(width - dp(5f), height - dp(9f))
                mark.lineTo(width - dp(5f), height - dp(5f))
                mark.lineTo(width - dp(9f), height - dp(5f))
                mark.close()
                labelPaint.color = palette.graphite
                canvas.drawPath(mark, labelPaint)
            }
        }
    }

    private inner class Tabs : View(context) {
        private val paint = Paint(Paint.ANTI_ALIAS_FLAG)
        private val icon = TextPaint(Paint.ANTI_ALIAS_FLAG).also {
            it.textAlign = Paint.Align.CENTER
            it.textSize = dp(19f)
        }
        private val rect = RectF()
        private val categories = EmojiCategory.entries

        override fun onDraw(canvas: Canvas) {
            paint.color = palette.ground
            canvas.drawRect(0f, 0f, width.toFloat(), height.toFloat(), paint)
            val w = width.toFloat() / categories.size
            val baseline = height / 2f - (icon.ascent() + icon.descent()) / 2f
            categories.forEachIndexed { i, c ->
                val cx = (i + 0.5f) * w
                val selected = c == active
                if (selected) {
                    paint.color = palette.accentWash
                    rect.set(cx - dp(17f), dp(4f), cx + dp(17f), height - dp(4f))
                    canvas.drawRoundRect(rect, dp(10f), dp(10f), paint)
                }
                icon.alpha = when {
                    selected -> 255
                    c in grid.starts -> 150
                    else -> 60
                }
                canvas.drawText(c.icon, cx, baseline, icon)
            }
            paint.color = palette.rule
            canvas.drawRect(0f, height - dp(1f), width.toFloat(), height.toFloat(), paint)
        }

        @SuppressLint("ClickableViewAccessibility")
        override fun onTouchEvent(event: MotionEvent): Boolean {
            if (event.actionMasked == MotionEvent.ACTION_UP) {
                val i = (event.x / (width.toFloat() / categories.size)).toInt().coerceIn(0, categories.size - 1)
                jumpTo(categories[i])
            }
            return true
        }
    }

    private inner class Bar : View(context) {
        private val face = Paint(Paint.ANTI_ALIAS_FLAG)
        private val text = TextPaint(Paint.ANTI_ALIAS_FLAG).also {
            it.textAlign = Paint.Align.CENTER
            it.textSize = dp(15f) * resources.configuration.fontScale
            it.typeface = Typeface.create("sans-serif-medium", Typeface.NORMAL)
        }
        private val rect = RectF()
        private val buttonWidth get() = dp(72f)
        private val repeat = Handler(Looper.getMainLooper())
        private var repeated = false
        private val repeater = object : Runnable {
            override fun run() {
                repeated = true
                onBackspace()
                repeat.postDelayed(this, REPEAT_MS)
            }
        }

        override fun onDraw(canvas: Canvas) {
            face.color = palette.modifierKey
            canvas.drawRect(0f, 0f, width.toFloat(), height.toFloat(), face)
            text.color = palette.ink
            val baseline = height / 2f - (text.ascent() + text.descent()) / 2f
            canvas.drawText("ABC", buttonWidth / 2, baseline, text)
            canvas.drawText("⌫", width - buttonWidth / 2, baseline, text)
            face.color = palette.key
            rect.set(buttonWidth, dp(7f), width - buttonWidth, height - dp(7f))
            canvas.drawRoundRect(rect, rect.height() / 2, rect.height() / 2, face)
            text.color = palette.graphite
            canvas.drawText("Search emoji", rect.centerX(), baseline, text)
        }

        @SuppressLint("ClickableViewAccessibility")
        override fun onTouchEvent(event: MotionEvent): Boolean {
            val onDelete = event.x > width - buttonWidth
            when (event.actionMasked) {
                MotionEvent.ACTION_DOWN -> if (onDelete) {
                    repeated = false
                    repeat.postDelayed(repeater, REPEAT_DELAY_MS)
                }
                MotionEvent.ACTION_UP -> {
                    repeat.removeCallbacks(repeater)
                    when {
                        event.x < buttonWidth -> onLetters()
                        onDelete -> if (!repeated) onBackspace()
                        else -> onSearch()
                    }
                }
                MotionEvent.ACTION_CANCEL -> repeat.removeCallbacks(repeater)
            }
            return true
        }
    }

    /** The skin tones of one emoji in a row above it (below it on the top row). Tap one, or outside to close. */
    private inner class TonePicker : View(context) {
        private var emoji: Emoji? = null
        private val card = RectF()
        private val paint = Paint(Paint.ANTI_ALIAS_FLAG)
        private val option get() = dp(44f)

        fun show(e: Emoji, cx: Float, cellTop: Float, cellHeight: Float) {
            emoji = e
            val w = option * e.forms.size + dp(8f)
            // This view is GONE until now, so its own width is 0: place it within the panel's.
            val left = tonePickerLeft(cx, w, this@EmojiPanelView.width.toFloat(), dp(4f))
            val h = option + dp(8f)
            val top = if (cellTop - h - dp(4f) >= 0) cellTop - h - dp(4f) else cellTop + cellHeight + dp(4f)
            card.set(left, top, left + w, top + h)
            visibility = VISIBLE
            invalidate()
        }

        override fun onDraw(canvas: Canvas) {
            val e = emoji ?: return
            paint.color = palette.keyShadow
            canvas.drawRoundRect(card.left, card.top + dp(2f), card.right, card.bottom + dp(2f), dp(12f), dp(12f), paint)
            paint.color = palette.key
            canvas.drawRoundRect(card, dp(12f), dp(12f), paint)
            val chosen = store.tones.preferred(e)
            val baseline = card.centerY() - (emojiPaint.ascent() + emojiPaint.descent()) / 2f
            e.forms.forEachIndexed { i, form ->
                val cx = card.left + dp(4f) + (i + 0.5f) * option
                if (form == chosen) {
                    paint.color = palette.accentWash
                    canvas.drawRoundRect(cx - option / 2 + dp(2f), card.top + dp(6f), cx + option / 2 - dp(2f), card.bottom - dp(6f), dp(8f), dp(8f), paint)
                }
                emojiPaint.color = palette.ink
                canvas.drawText(form, cx, baseline, emojiPaint)
            }
        }

        @SuppressLint("ClickableViewAccessibility")
        override fun onTouchEvent(event: MotionEvent): Boolean {
            if (event.actionMasked != MotionEvent.ACTION_UP) return true
            val e = emoji
            if (e != null && card.contains(event.x, event.y)) {
                val i = ((event.x - card.left - dp(4f)) / option).toInt().coerceIn(0, e.forms.size - 1)
                val form = e.forms[i]
                store.chooseTone(e, form)
                store.used(form)
                onEmoji(form)
                refreshCellsOf(e)
            }
            visibility = GONE
            return true
        }
    }

    /** Shows the chosen tone in the emoji's own category; the recent row keeps what was typed. */
    private fun refreshCellsOf(e: Emoji) {
        val items = grid.items.toMutableList()
        val changed = items.indices.filter { i ->
            val item = items[i]
            item is EmojiGridItem.Cell && item.emoji == e && grid.categoryAt(i) != EmojiCategory.RECENT
        }
        for (i in changed) items[i] = (items[i] as EmojiGridItem.Cell).copy(form = store.tones.preferred(e))
        grid = EmojiGrid(items, grid.starts)
        changed.forEach(adapter::notifyItemChanged)
    }

    private companion object {
        const val TYPE_HEADER = 0
        const val TYPE_CELL = 1
        const val CELL_DP = 46f
        const val COLUMNS_MIN = 7
        const val REPEAT_DELAY_MS = 400L
        const val REPEAT_MS = 60L
    }
}

/** The left edge of the tone picker: centered on the emoji, kept inside the panel when it fits. */
internal fun tonePickerLeft(anchorX: Float, pickerWidth: Float, panelWidth: Float, margin: Float): Float {
    val max = panelWidth - pickerWidth - margin
    return if (max < margin) maxOf(0f, (panelWidth - pickerWidth) / 2) else (anchorX - pickerWidth / 2).coerceIn(margin, max)
}
