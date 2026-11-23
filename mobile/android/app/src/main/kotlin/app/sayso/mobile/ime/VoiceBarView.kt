package app.sayso.mobile.ime

import android.annotation.SuppressLint
import android.content.Context
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.Path
import android.graphics.RectF
import android.graphics.Typeface
import android.os.SystemClock
import android.text.TextPaint
import android.text.TextUtils
import android.view.MotionEvent
import android.view.View
import app.sayso.mobile.voice.VoiceController
import app.sayso.mobile.voice.VoiceState

/**
 * The keyboard's voice mode, in place of the keys: the engine and style, the
 * time, a live waveform of the real microphone level, the live text, and
 * Cancel, hands-free, and Done. Redraws every frame while a dictation runs.
 */
@SuppressLint("ViewConstructor")
class VoiceBarView(
    context: Context,
    private val voice: VoiceController,
    private val onCancel: () -> Unit,
    private val onDone: () -> Unit,
    private val onRetry: () -> Unit,
    private val onHandsFree: () -> Unit,
) : View(context) {
    private val density = resources.displayMetrics.density
    private fun dp(v: Float) = v * density

    var palette: KeyboardPalette = KeyboardPalette.LIGHT
        set(value) { field = value; invalidate() }

    private val paint = Paint(Paint.ANTI_ALIAS_FLAG)
    private val text = TextPaint(Paint.ANTI_ALIAS_FLAG).also {
        it.textSize = dp(15f) * resources.configuration.fontScale
        it.typeface = Typeface.create("sans-serif", Typeface.NORMAL)
    }
    private val small = TextPaint(Paint.ANTI_ALIAS_FLAG).also {
        it.textSize = dp(13f) * resources.configuration.fontScale
        it.typeface = Typeface.create("sans-serif-medium", Typeface.NORMAL)
    }
    private val rect = RectF()
    private val path = Path()

    // Waveform history, oldest first, as a ring.
    private val bars = FloatArray(BARS)
    private var head = 0
    private var smooth = 0f

    private var pressed = NONE

    private val buttonRadius get() = dp(26f)
    private val buttonY get() = height - dp(14f) - buttonRadius
    private val cancelX get() = dp(16f) + buttonRadius
    private val doneX get() = width - dp(16f) - buttonRadius
    private val handsFreeRect: RectF
        get() = RectF(width / 2f - dp(64f), buttonY - dp(17f), width / 2f + dp(64f), buttonY + dp(17f))

    private fun hit(x: Float, y: Float): Int {
        if (y < buttonY - buttonRadius - dp(8f)) return NONE
        return when {
            x < cancelX + buttonRadius + dp(8f) -> CANCEL
            x > doneX - buttonRadius - dp(8f) -> DONE
            handsFreeRect.contains(x, y) -> HANDS_FREE
            else -> NONE
        }
    }

    @SuppressLint("ClickableViewAccessibility")
    override fun onTouchEvent(event: MotionEvent): Boolean {
        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN -> pressed = hit(event.x, event.y)
            MotionEvent.ACTION_UP -> {
                val target = hit(event.x, event.y)
                if (target == pressed) {
                    val failed = voice.machine.state is VoiceState.Failed
                    when (target) {
                        CANCEL -> onCancel()
                        DONE -> if (failed) onRetry() else onDone()
                        HANDS_FREE -> if (!failed) onHandsFree()
                    }
                }
                pressed = NONE
            }
            MotionEvent.ACTION_CANCEL -> pressed = NONE
        }
        invalidate()
        return true
    }

    override fun onDraw(canvas: Canvas) {
        paint.color = palette.ground
        canvas.drawRect(0f, 0f, width.toFloat(), height.toFloat(), paint)
        val state = voice.machine.state
        val now = SystemClock.elapsedRealtime()

        val listening = state is VoiceState.Listening
        pushLevel(if (listening) voice.level else 0f)

        drawHeader(canvas, state, now)
        val waveTop = dp(52f)
        val waveBottom = buttonY - buttonRadius - dp(44f)
        when (state) {
            is VoiceState.Failed -> drawMessage(canvas, state.message, (waveTop + waveBottom) / 2f, palette.danger)
            else -> drawWave(canvas, waveTop, waveBottom, state is VoiceState.Finishing)
        }
        drawLiveText(canvas, state, waveBottom + dp(24f))
        drawButtons(canvas, state)

        if (state is VoiceState.Listening || state is VoiceState.Finishing) postInvalidateOnAnimation()
    }

    private fun pushLevel(level: Float) {
        // Rise fast, fall slowly, so syllables read as peaks.
        smooth += (level - smooth) * if (level > smooth) 0.5f else 0.12f
        bars[head] = smooth
        head = (head + 1) % BARS
    }

    private fun drawHeader(canvas: Canvas, state: VoiceState, now: Long) {
        val pill = when (state) {
            is VoiceState.Listening, is VoiceState.Finishing ->
                listOf(voice.engineLabel, voice.styleLabel).filter { it.isNotEmpty() }.joinToString(" · ")
            else -> null
        }
        if (pill != null) {
            val w = minOf(small.measureText(pill) + dp(24f), width * 0.6f)
            rect.set(dp(12f), dp(12f), dp(12f) + w, dp(40f))
            paint.color = palette.accentWash
            canvas.drawRoundRect(rect, rect.height() / 2, rect.height() / 2, paint)
            small.color = palette.ink
            small.textAlign = Paint.Align.CENTER
            val label = TextUtils.ellipsize(pill, small, w - dp(16f), TextUtils.TruncateAt.END)
            canvas.drawText(label, 0, label.length, rect.centerX(), baseline(rect.centerY(), small), small)
        }
        val elapsed = when (state) {
            is VoiceState.Listening -> now - state.startedAtMs
            is VoiceState.Finishing -> state.stoppedAtMs - state.startedAtMs
            else -> null
        }
        if (elapsed != null) {
            val secs = elapsed / 1000
            small.color = palette.graphite
            small.textAlign = Paint.Align.RIGHT
            canvas.drawText("%d:%02d".format(secs / 60, secs % 60), width - dp(16f), baseline(dp(26f), small), small)
        }
    }

    private fun drawWave(canvas: Canvas, top: Float, bottom: Float, dimmed: Boolean) {
        val cy = (top + bottom) / 2f
        val maxHalf = (bottom - top) / 2f
        val left = dp(24f)
        val step = (width - 2 * left) / BARS
        paint.color = palette.accent
        paint.alpha = if (dimmed) 90 else 255
        paint.strokeWidth = step * 0.55f
        paint.strokeCap = Paint.Cap.ROUND
        for (i in 0 until BARS) {
            val v = bars[(head + i) % BARS]
            val half = maxOf(dp(1.5f), v * maxHalf)
            val x = left + (i + 0.5f) * step
            canvas.drawLine(x, cy - half, x, cy + half, paint)
        }
        paint.alpha = 255
    }

    private fun drawLiveText(canvas: Canvas, state: VoiceState, y: Float) {
        val (line, color) = when (state) {
            is VoiceState.Listening -> when {
                state.partial.isNotEmpty() -> state.partial to palette.ink
                state.notice != null -> state.notice to palette.graphite
                else -> "Listening" to palette.graphite
            }
            is VoiceState.Finishing -> (state.partial.ifEmpty { "Writing" }) to palette.graphite
            else -> return
        }
        text.color = color
        text.textAlign = Paint.Align.CENTER
        // In live text the newest words matter, so cut from the start; a notice reads from its start.
        val isNotice = state is VoiceState.Listening && state.partial.isEmpty() && state.notice != null
        val cut = if (isNotice) TextUtils.TruncateAt.END else TextUtils.TruncateAt.START
        val shown = TextUtils.ellipsize(line, text, width - dp(32f), cut)
        canvas.drawText(shown, 0, shown.length, width / 2f, y, text)
    }

    private fun drawMessage(canvas: Canvas, message: String, cy: Float, color: Int) {
        small.color = color
        small.textAlign = Paint.Align.CENTER
        val shown = TextUtils.ellipsize(message, small, width - dp(32f), TextUtils.TruncateAt.END)
        canvas.drawText(shown, 0, shown.length, width / 2f, baseline(cy, small), small)
    }

    private fun drawButtons(canvas: Canvas, state: VoiceState) {
        val y = buttonY
        val r = buttonRadius
        val failed = state is VoiceState.Failed

        // Cancel
        paint.style = Paint.Style.FILL
        paint.color = if (pressed == CANCEL) palette.pressedKey else palette.modifierKey
        canvas.drawCircle(cancelX, y, r, paint)
        stroke(palette.ink)
        val a = dp(7f)
        canvas.drawLine(cancelX - a, y - a, cancelX + a, y + a, paint)
        canvas.drawLine(cancelX - a, y + a, cancelX + a, y - a, paint)
        paint.style = Paint.Style.FILL

        // Hands-free
        if (!failed) {
            val hf = handsFreeRect
            paint.color = if (voice.handsFree) palette.accentWash else palette.modifierKey
            canvas.drawRoundRect(hf, hf.height() / 2, hf.height() / 2, paint)
            paint.color = if (voice.handsFree) palette.accent else palette.graphite
            canvas.drawCircle(hf.left + dp(16f), hf.centerY(), dp(4f), paint)
            small.color = palette.ink
            small.textAlign = Paint.Align.CENTER
            canvas.drawText("Hands-free", hf.centerX() + dp(6f), baseline(hf.centerY(), small), small)
        }

        // Done, Send, or Retry
        val finishing = state is VoiceState.Finishing
        paint.color = when {
            finishing -> palette.modifierKey
            pressed == DONE -> palette.pressedKey
            else -> palette.accent
        }
        canvas.drawCircle(doneX, y, r, paint)
        val ink = if (finishing) palette.graphite else palette.onAccent
        when {
            finishing -> {
                paint.color = ink
                val t = (SystemClock.elapsedRealtime() / 300 % 3).toInt()
                for (i in -1..1) {
                    paint.alpha = if (i + 1 == t) 255 else 100
                    canvas.drawCircle(doneX + i * dp(8f), y, dp(3f), paint)
                }
                paint.alpha = 255
            }
            failed -> {
                stroke(ink)
                paint.style = Paint.Style.STROKE
                rect.set(doneX - dp(8f), y - dp(8f), doneX + dp(8f), y + dp(8f))
                canvas.drawArc(rect, -60f, 300f, false, paint)
                paint.style = Paint.Style.FILL
                path.reset()
                path.moveTo(doneX + dp(4f), y - dp(12f))
                path.lineTo(doneX + dp(9f), y - dp(6f))
                path.lineTo(doneX + dp(2f), y - dp(4f))
                path.close()
                paint.color = ink
                canvas.drawPath(path, paint)
            }
            voice.doneSends -> {
                // A paper plane: send.
                paint.color = ink
                path.reset()
                path.moveTo(doneX - dp(9f), y - dp(8f))
                path.lineTo(doneX + dp(10f), y)
                path.lineTo(doneX - dp(9f), y + dp(8f))
                path.lineTo(doneX - dp(5f), y)
                path.close()
                canvas.drawPath(path, paint)
            }
            else -> {
                stroke(ink)
                canvas.drawLine(doneX - dp(8f), y, doneX - dp(2f), y + dp(6f), paint)
                canvas.drawLine(doneX - dp(2f), y + dp(6f), doneX + dp(9f), y - dp(6f), paint)
            }
        }
        paint.style = Paint.Style.FILL
    }

    private fun stroke(color: Int) {
        paint.color = color
        paint.style = Paint.Style.STROKE
        paint.strokeWidth = dp(2f)
        paint.strokeCap = Paint.Cap.ROUND
    }

    private fun baseline(cy: Float, p: Paint) = cy - (p.ascent() + p.descent()) / 2f

    private companion object {
        const val BARS = 48
        const val NONE = 0
        const val CANCEL = 1
        const val DONE = 2
        const val HANDS_FREE = 3
    }
}
