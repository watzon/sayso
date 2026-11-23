package app.sayso.mobile.ime

import android.annotation.SuppressLint
import android.content.Context
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.Path
import android.graphics.RectF
import android.graphics.Typeface
import android.view.HapticFeedbackConstants
import android.view.MotionEvent
import android.view.View
import app.sayso.mobile.engine.Diagnostics
import app.sayso.mobile.keyboard.Key
import app.sayso.mobile.keyboard.KeyCodes
import app.sayso.mobile.keyboard.KeyKind
import app.sayso.mobile.keyboard.Keyboard
import app.sayso.mobile.keyboard.KeyboardLayouts
import app.sayso.mobile.keyboard.KeyboardMode
import app.sayso.mobile.keyboard.Shift
import kotlin.math.abs
import kotlin.math.hypot

interface KeyboardActionListener {
    /** A letter key was tapped at the given point. The engine decides which letter it was. */
    fun onLetter(key: Key, x: Float, y: Float, timeMs: Long)
    fun onText(text: String)
    fun onSpecialKey(code: Int, timeMs: Long)
    fun onSpace()
    fun onSpaceLongPress()
    fun onBackspace()
    fun onDeleteWord()
    fun onCursorMove(steps: Int)
    fun onKeyboardLaidOut(keyboard: Keyboard)
}

/**
 * The key grid, drawn on a Canvas. Previews and the accent picker are drawn
 * above the view's top edge, into the suggestion strip, so the parent must not
 * clip its children.
 */
@SuppressLint("ViewConstructor")
class KeyboardView(context: Context, private val listener: KeyboardActionListener) : View(context) {
    private val density = resources.displayMetrics.density
    private val scaledDensity = density * resources.configuration.fontScale
    private fun dp(v: Float) = v * density
    private fun sp(v: Float) = v * scaledDensity

    var palette: KeyboardPalette = KeyboardPalette.LIGHT
        set(value) { field = value; invalidate() }
    var hapticsEnabled = true
    var shift: Shift = Shift.OFF
        set(value) { if (field != value) { field = value; invalidate() } }
    var enterLabel: String = "return"
        set(value) { field = value; invalidate() }
    var diagnostics: Diagnostics? = null
        set(value) { field = value; invalidate() }

    var mode: KeyboardMode = KeyboardMode.LETTERS
        set(value) {
            if (field == value) return
            field = value
            cancelAllTouches()
            relayout()
        }

    var keyboard: Keyboard? = null
        private set

    private val rowHeight = dp(54f)
    private val gapX = dp(6f)
    private val gapY = dp(10f)
    private val radius = dp(6f)

    private val fill = Paint(Paint.ANTI_ALIAS_FLAG)
    private val stroke = Paint(Paint.ANTI_ALIAS_FLAG).apply { style = Paint.Style.STROKE; strokeWidth = dp(1.6f); strokeCap = Paint.Cap.ROUND; strokeJoin = Paint.Join.ROUND }
    private val text = Paint(Paint.ANTI_ALIAS_FLAG).apply { textAlign = Paint.Align.CENTER }
    private val letterFace = Typeface.create("sans-serif", Typeface.NORMAL)
    private val labelFace = Typeface.create("sans-serif-medium", Typeface.NORMAL)
    private val outline = Paint(Paint.ANTI_ALIAS_FLAG).apply { style = Paint.Style.STROKE; strokeWidth = dp(1f) }
    private val arrow = Paint(Paint.ANTI_ALIAS_FLAG).apply { style = Paint.Style.STROKE; strokeWidth = dp(1.5f); strokeCap = Paint.Cap.ROUND }
    private val rect = RectF()
    private val path = Path()

    private val pointers = HashMap<Int, Pointer>()

    private enum class Phase { PRESSED, ACCENTS, TRACKPAD, REPEAT, WORD_DELETE, DONE }

    private inner class Pointer(val key: Key, val downX: Float, val downY: Float, val downTime: Long) {
        var phase = Phase.PRESSED
        var accents: List<String> = emptyList()
        var accentIndex = 0
        var trackX = downX
        val longPress = Runnable { onLongPress(this) }
        val repeat: Runnable = object : Runnable {
            override fun run() {
                if (phase != Phase.PRESSED && phase != Phase.REPEAT) return
                phase = Phase.REPEAT
                listener.onBackspace()
                postDelayed(this, REPEAT_INTERVAL_MS)
            }
        }
    }

    override fun onMeasure(widthMeasureSpec: Int, heightMeasureSpec: Int) {
        val rows = KeyboardLayouts.rows(mode).size
        setMeasuredDimension(MeasureSpec.getSize(widthMeasureSpec), (rows * rowHeight).toInt() + paddingBottom)
    }

    override fun onSizeChanged(w: Int, h: Int, oldw: Int, oldh: Int) = relayout()

    private fun relayout() {
        if (width == 0) return
        val built = KeyboardLayouts.build(mode, width.toFloat(), (height - paddingBottom).toFloat(), gapX, gapY)
        keyboard = built
        listener.onKeyboardLaidOut(built)
        invalidate()
    }

    @SuppressLint("ClickableViewAccessibility")
    override fun onTouchEvent(event: MotionEvent): Boolean {
        val board = keyboard ?: return false
        val index = event.actionIndex
        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN, MotionEvent.ACTION_POINTER_DOWN -> {
                // A new finger commits any key still held by another, so fast rolling input keeps its order.
                for (p in pointers.values) if (p.phase == Phase.PRESSED) commit(p)
                val x = event.getX(index)
                val y = event.getY(index)
                val key = board.keyAt(x, y) ?: return true
                val pointer = Pointer(key, x, y, event.eventTime)
                pointers[event.getPointerId(index)] = pointer
                onDown(pointer)
            }
            MotionEvent.ACTION_MOVE -> for (i in 0 until event.pointerCount) {
                pointers[event.getPointerId(i)]?.let { onMove(it, event.getX(i), event.getY(i)) }
            }
            MotionEvent.ACTION_UP, MotionEvent.ACTION_POINTER_UP -> {
                pointers.remove(event.getPointerId(index))?.let { onUp(it) }
            }
            MotionEvent.ACTION_CANCEL -> cancelAllTouches()
        }
        invalidateWithPopups()
        return true
    }

    private fun onDown(p: Pointer) {
        if (hapticsEnabled) performHapticFeedback(HapticFeedbackConstants.KEYBOARD_TAP)
        when (p.key.spec.kind) {
            KeyKind.SHIFT -> { listener.onSpecialKey(KeyCodes.SHIFT, p.downTime); p.phase = Phase.DONE }
            KeyKind.BACKSPACE -> { listener.onBackspace(); postDelayed(p.repeat, LONG_PRESS_MS) }
            KeyKind.SPACE -> postDelayed(p.longPress, LONG_PRESS_MS)
            else -> if (p.key.spec.longPress.isNotEmpty()) postDelayed(p.longPress, LONG_PRESS_MS)
        }
    }

    private fun onMove(p: Pointer, x: Float, y: Float) {
        when (p.phase) {
            Phase.PRESSED -> when (p.key.spec.kind) {
                KeyKind.SPACE -> if (abs(x - p.downX) > dp(16f)) {
                    removeCallbacks(p.longPress)
                    p.phase = Phase.TRACKPAD
                    p.trackX = x
                }
                KeyKind.BACKSPACE -> if (p.downX - x > dp(24f)) startWordDelete(p, x)
                else -> Unit
            }
            Phase.REPEAT -> if (p.downX - x > dp(24f)) startWordDelete(p, x)
            Phase.TRACKPAD -> {
                val step = dp(10f)
                val steps = ((x - p.trackX) / step).toInt()
                if (steps != 0) {
                    listener.onCursorMove(steps)
                    p.trackX += steps * step
                }
            }
            Phase.WORD_DELETE -> if (p.trackX - x > dp(40f)) {
                listener.onDeleteWord()
                p.trackX = x
            }
            Phase.ACCENTS -> {
                val popup = accentPopupRect(p)
                val cell = popup.width() / p.accents.size
                p.accentIndex = ((x - popup.left) / cell).toInt().coerceIn(0, p.accents.lastIndex)
            }
            Phase.DONE -> Unit
        }
    }

    private fun startWordDelete(p: Pointer, x: Float) {
        removeCallbacks(p.repeat)
        p.phase = Phase.WORD_DELETE
        p.trackX = x
        listener.onDeleteWord()
    }

    private fun onUp(p: Pointer) {
        removeCallbacks(p.longPress)
        removeCallbacks(p.repeat)
        when (p.phase) {
            Phase.PRESSED -> commit(p)
            Phase.ACCENTS -> listener.onText(cased(p.accents[p.accentIndex]))
            else -> Unit
        }
        p.phase = Phase.DONE
    }

    private fun commit(p: Pointer) {
        removeCallbacks(p.longPress)
        p.phase = Phase.DONE
        val spec = p.key.spec
        when (spec.kind) {
            KeyKind.LETTER -> listener.onLetter(p.key, p.downX, p.downY, p.downTime)
            KeyKind.CHARACTER -> listener.onText(spec.label)
            KeyKind.SPACE -> listener.onSpace()
            KeyKind.SHIFT, KeyKind.BACKSPACE -> Unit
            else -> listener.onSpecialKey(spec.code, p.downTime)
        }
    }

    private fun onLongPress(p: Pointer) {
        if (p.phase != Phase.PRESSED) return
        if (p.key.spec.kind == KeyKind.SPACE) {
            p.phase = Phase.DONE
            listener.onSpaceLongPress()
            return
        }
        if (hapticsEnabled) performHapticFeedback(HapticFeedbackConstants.LONG_PRESS)
        p.phase = Phase.ACCENTS
        p.accents = p.key.spec.longPress
        p.accentIndex = 0
        invalidateWithPopups()
    }

    private fun cancelAllTouches() {
        for (p in pointers.values) {
            removeCallbacks(p.longPress)
            removeCallbacks(p.repeat)
        }
        pointers.clear()
        invalidateWithPopups()
    }

    // Previews and the accent picker draw above this view's bounds, which its own invalidate does not repaint.
    private fun invalidateWithPopups() {
        invalidate()
        rootView.invalidate()
    }

    override fun onDetachedFromWindow() {
        cancelAllTouches()
        super.onDetachedFromWindow()
    }

    private fun cased(label: String) = if (shift != Shift.OFF) label.uppercase() else label

    // Drawing

    override fun onDraw(canvas: Canvas) {
        val board = keyboard ?: return
        canvas.drawColor(palette.ground)
        val pressed = pointers.values.filter { it.phase != Phase.DONE }.map { it.key }.toSet()
        for (key in board.keys) drawKey(canvas, key, key in pressed)
        diagnostics?.let { drawDiagnostics(canvas, it) }
        for (p in pointers.values) {
            when {
                p.phase == Phase.ACCENTS -> drawAccents(canvas, p)
                p.phase == Phase.PRESSED && (p.key.spec.kind == KeyKind.LETTER || p.key.spec.kind == KeyKind.CHARACTER) -> drawPreview(canvas, p.key)
            }
        }
    }

    private fun drawKey(canvas: Canvas, key: Key, pressed: Boolean) {
        val spec = key.spec
        val accentKey = spec.kind == KeyKind.ENTER
        val modifier = spec.kind !in PLAIN_KEYS && !accentKey
        val face = when {
            accentKey -> palette.accent
            pressed -> palette.pressedKey
            modifier -> palette.modifierKey
            else -> palette.key
        }
        rect.set(key.left, key.top + dp(1.5f), key.right, key.bottom + dp(1.5f))
        fill.color = palette.keyShadow
        canvas.drawRoundRect(rect, radius, radius, fill)
        rect.set(key.left, key.top, key.right, key.bottom)
        fill.color = face
        canvas.drawRoundRect(rect, radius, radius, fill)

        val ink = if (accentKey) palette.onAccent else palette.ink
        when (spec.kind) {
            KeyKind.LETTER -> drawLabel(canvas, cased(spec.label), key.centerX, key.centerY, sp(22f), letterFace, ink)
            KeyKind.CHARACTER -> drawLabel(canvas, spec.label, key.centerX, key.centerY, sp(20f), letterFace, ink)
            KeyKind.SHIFT -> drawShift(canvas, key, ink)
            KeyKind.BACKSPACE -> drawBackspace(canvas, key, ink)
            KeyKind.EMOJI -> drawEmojiGlyph(canvas, key, ink)
            KeyKind.SPACE -> drawLabel(canvas, "sayso", key.centerX, key.centerY, sp(13f), labelFace, palette.graphite)
            KeyKind.ENTER -> if (enterLabel == "return") drawReturn(canvas, key, ink) else drawLabel(canvas, enterLabel, key.centerX, key.centerY, sp(14f), labelFace, ink)
            else -> drawLabel(canvas, spec.label, key.centerX, key.centerY, sp(14f), labelFace, ink)
        }
    }

    private fun drawLabel(canvas: Canvas, label: String, cx: Float, cy: Float, size: Float, face: Typeface, color: Int) {
        text.textSize = size
        text.typeface = face
        text.color = color
        canvas.drawText(label, cx, cy - (text.ascent() + text.descent()) / 2f, text)
    }

    private fun drawShift(canvas: Canvas, key: Key, color: Int) {
        val s = dp(9f)
        val cx = key.centerX
        val cy = key.centerY - dp(1f)
        path.reset()
        path.moveTo(cx, cy - s)
        path.lineTo(cx + s, cy)
        path.lineTo(cx + s * 0.45f, cy)
        path.lineTo(cx + s * 0.45f, cy + s * 0.8f)
        path.lineTo(cx - s * 0.45f, cy + s * 0.8f)
        path.lineTo(cx - s * 0.45f, cy)
        path.lineTo(cx - s, cy)
        path.close()
        val filled = shift != Shift.OFF
        if (filled) {
            fill.color = color
            canvas.drawPath(path, fill)
        } else {
            stroke.color = color
            canvas.drawPath(path, stroke)
        }
        if (shift == Shift.LOCKED) {
            stroke.color = color
            canvas.drawLine(cx - s * 0.6f, cy + s * 1.25f, cx + s * 0.6f, cy + s * 1.25f, stroke)
        }
    }

    private fun drawBackspace(canvas: Canvas, key: Key, color: Int) {
        val h = dp(8f)
        val w = dp(11f)
        val cx = key.centerX + dp(2f)
        val cy = key.centerY
        path.reset()
        path.moveTo(cx - w - h * 0.8f, cy)
        path.lineTo(cx - w, cy - h)
        path.lineTo(cx + w, cy - h)
        path.lineTo(cx + w, cy + h)
        path.lineTo(cx - w, cy + h)
        path.close()
        stroke.color = color
        canvas.drawPath(path, stroke)
        val x = dp(3.5f)
        canvas.drawLine(cx - x, cy - x, cx + x, cy + x, stroke)
        canvas.drawLine(cx - x, cy + x, cx + x, cy - x, stroke)
    }

    private fun drawReturn(canvas: Canvas, key: Key, color: Int) {
        val s = dp(8f)
        val cx = key.centerX
        val cy = key.centerY
        stroke.color = color
        path.reset()
        path.moveTo(cx + s, cy - s)
        path.lineTo(cx + s, cy + s * 0.3f)
        path.lineTo(cx - s, cy + s * 0.3f)
        canvas.drawPath(path, stroke)
        canvas.drawLine(cx - s, cy + s * 0.3f, cx - s * 0.45f, cy - s * 0.25f, stroke)
        canvas.drawLine(cx - s, cy + s * 0.3f, cx - s * 0.45f, cy + s * 0.85f, stroke)
    }

    private fun drawEmojiGlyph(canvas: Canvas, key: Key, color: Int) {
        val r = dp(9f)
        val cx = key.centerX
        val cy = key.centerY
        stroke.color = color
        canvas.drawCircle(cx, cy, r, stroke)
        fill.color = color
        canvas.drawCircle(cx - r * 0.35f, cy - r * 0.25f, dp(1.2f), fill)
        canvas.drawCircle(cx + r * 0.35f, cy - r * 0.25f, dp(1.2f), fill)
        rect.set(cx - r * 0.5f, cy - r * 0.2f, cx + r * 0.5f, cy + r * 0.55f)
        canvas.drawArc(rect, 20f, 140f, false, stroke)
    }

    private fun previewRect(key: Key): RectF {
        val w = key.width + dp(12f)
        val h = key.height * 1.1f
        val left = (key.centerX - w / 2).coerceIn(0f, width - w)
        return RectF(left, key.top - h - dp(4f), left + w, key.top - dp(4f))
    }

    private fun drawPreview(canvas: Canvas, key: Key) {
        val r = previewRect(key)
        drawBubble(canvas, r)
        val label = if (key.spec.kind == KeyKind.LETTER) cased(key.spec.label) else key.spec.label
        drawLabel(canvas, label, r.centerX(), r.centerY(), sp(30f), letterFace, palette.ink)
    }

    private fun accentPopupRect(p: Pointer): RectF {
        val cell = p.key.width + dp(4f)
        val w = cell * p.accents.size
        val h = p.key.height * 1.1f
        val left = (p.key.left - dp(2f)).coerceIn(0f, (width - w).coerceAtLeast(0f))
        return RectF(left, p.key.top - h - dp(4f), left + w, p.key.top - dp(4f))
    }

    private fun drawAccents(canvas: Canvas, p: Pointer) {
        val r = accentPopupRect(p)
        drawBubble(canvas, r)
        val cell = r.width() / p.accents.size
        p.accents.forEachIndexed { i, accent ->
            val cx = r.left + cell * (i + 0.5f)
            var color = palette.ink
            if (i == p.accentIndex) {
                fill.color = palette.accent
                rect.set(r.left + cell * i + dp(3f), r.top + dp(3f), r.left + cell * (i + 1) - dp(3f), r.bottom - dp(3f))
                canvas.drawRoundRect(rect, radius, radius, fill)
                color = palette.onAccent
            }
            drawLabel(canvas, cased(accent), cx, r.centerY(), sp(24f), letterFace, color)
        }
    }

    private fun drawBubble(canvas: Canvas, r: RectF) {
        fill.color = palette.keyShadow
        rect.set(r.left, r.top + dp(2f), r.right, r.bottom + dp(2f))
        canvas.drawRoundRect(rect, radius * 1.5f, radius * 1.5f, fill)
        fill.color = palette.key
        canvas.drawRoundRect(r, radius * 1.5f, radius * 1.5f, fill)
    }

    private fun drawDiagnostics(canvas: Canvas, d: Diagnostics) {
        val board = keyboard ?: return
        outline.color = palette.accent
        outline.alpha = 150
        arrow.color = palette.danger
        for (k in d.keys) {
            canvas.drawRect(k.left, k.top, k.right, k.bottom, outline)
            if (hypot(k.learnedDx, k.learnedDy) < dp(0.5f)) continue
            val key = board.keys.firstOrNull { it.spec.code == k.code && it.spec.kind == KeyKind.LETTER } ?: continue
            val tx = key.centerX + k.learnedDx
            val ty = key.centerY + k.learnedDy
            canvas.drawLine(key.centerX, key.centerY, tx, ty, arrow)
            canvas.drawCircle(tx, ty, dp(2f), arrow)
        }
    }

    private companion object {
        const val LONG_PRESS_MS = 400L
        const val REPEAT_INTERVAL_MS = 50L
        val PLAIN_KEYS = setOf(KeyKind.LETTER, KeyKind.CHARACTER, KeyKind.SPACE)
    }
}
