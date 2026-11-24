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
 * The dictation strip, drawn over the suggestion strip while a dictation runs
 * or its error shows; the keys below stay on screen. Cancel (×), a waveform of
 * the real microphone level with the time, and Stop. After Stop it reads
 * "Transcribing…"; after an error, the message and Retry. Redraws every frame
 * while a dictation runs. Layout and hit areas come from [VoiceStrip].
 */
@SuppressLint("ViewConstructor")
class VoiceBarView(
    context: Context,
    private val voice: VoiceController,
    private val onCancel: () -> Unit,
    private val onStop: () -> Unit,
    private val onRetry: () -> Unit,
) : View(context) {
    private val density = resources.displayMetrics.density
    private fun dp(v: Float) = v * density

    var palette: KeyboardPalette = KeyboardPalette.LIGHT
        set(value) { field = value; invalidate() }

    private val paint = Paint(Paint.ANTI_ALIAS_FLAG)
    private val small = TextPaint(Paint.ANTI_ALIAS_FLAG).also {
        it.textSize = dp(14f) * resources.configuration.fontScale
        it.typeface = Typeface.create("sans-serif-medium", Typeface.NORMAL)
    }
    private val rect = RectF()
    private val path = Path()

    // Waveform history, oldest first, as a ring.
    private val bars = FloatArray(BARS)
    private var head = 0
    private var smooth = 0f

    private var pressed = VoiceStripAction.NONE

    private val button get() = dp(VoiceStrip.BUTTON_DP)
    private val radius get() = dp(16f)

    // One strip tall: the keys below stay on screen.
    override fun onMeasure(widthMeasureSpec: Int, heightMeasureSpec: Int) {
        setMeasuredDimension(MeasureSpec.getSize(widthMeasureSpec), dp(STRIP_HEIGHT_DP).toInt())
    }

    @SuppressLint("ClickableViewAccessibility")
    override fun onTouchEvent(event: MotionEvent): Boolean {
        val state = voice.machine.state
        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN -> pressed = VoiceStrip.hit(event.x, width.toFloat(), density, state)
            MotionEvent.ACTION_UP -> {
                val target = VoiceStrip.hit(event.x, width.toFloat(), density, state)
                if (target == pressed) {
                    when (target) {
                        VoiceStripAction.CANCEL -> onCancel()
                        VoiceStripAction.STOP -> onStop()
                        VoiceStripAction.RETRY -> onRetry()
                        VoiceStripAction.NONE -> {}
                    }
                }
                pressed = VoiceStripAction.NONE
            }
            MotionEvent.ACTION_CANCEL -> pressed = VoiceStripAction.NONE
        }
        invalidate()
        return true
    }

    override fun onDraw(canvas: Canvas) {
        paint.color = palette.ground
        canvas.drawRect(0f, 0f, width.toFloat(), height.toFloat(), paint)
        val state = voice.machine.state
        val now = SystemClock.elapsedRealtime()
        pushLevel(if (state is VoiceState.Listening) voice.level else 0f)

        val cy = height / 2f
        drawCancel(canvas, button / 2f, cy)
        drawPrimary(canvas, VoiceStrip.primary(state), width - button / 2f, cy)

        val left = button + dp(4f)
        val right = width - button - dp(4f)
        when (val status = VoiceStrip.status(state, voice.styleLabel, now)) {
            is VoiceStripStatus.Recording -> {
                small.color = palette.graphite
                small.textAlign = Paint.Align.RIGHT
                canvas.drawText(status.elapsed, right, baseline(cy, small), small)
                val timeWidth = small.measureText("0:00") + dp(12f)
                drawWave(canvas, left, right - timeWidth, cy)
            }
            is VoiceStripStatus.Message -> {
                small.color = if (status.tone == VoiceStripStatus.Tone.DANGER) palette.danger else palette.graphite
                // A slow pulse while the text is on its way, so the strip never looks stuck.
                if (state is VoiceState.Finishing) small.alpha = 150 + (105 * pulse(now)).toInt()
                small.textAlign = Paint.Align.CENTER
                val shown = TextUtils.ellipsize(status.text, small, right - left, TextUtils.TruncateAt.END)
                canvas.drawText(shown, 0, shown.length, (left + right) / 2f, baseline(cy, small), small)
                small.alpha = 255
            }
            null -> {}
        }

        if (voice.machine.active) postInvalidateOnAnimation()
    }

    private fun pushLevel(level: Float) {
        // Rise fast, fall slowly, so syllables read as peaks.
        smooth += (level - smooth) * if (level > smooth) 0.5f else 0.12f
        bars[head] = smooth
        head = (head + 1) % BARS
    }

    private fun drawWave(canvas: Canvas, left: Float, right: Float, cy: Float) {
        if (right <= left) return
        val maxHalf = height / 2f - dp(10f)
        val step = (right - left) / BARS
        paint.color = palette.accent
        paint.strokeWidth = step * 0.55f
        paint.strokeCap = Paint.Cap.ROUND
        for (i in 0 until BARS) {
            val v = bars[(head + i) % BARS]
            val half = maxOf(dp(1.5f), v * maxHalf)
            val x = left + (i + 0.5f) * step
            canvas.drawLine(x, cy - half, x, cy + half, paint)
        }
    }

    private fun drawCancel(canvas: Canvas, cx: Float, cy: Float) {
        paint.style = Paint.Style.FILL
        paint.color = if (pressed == VoiceStripAction.CANCEL) palette.pressedKey else palette.modifierKey
        canvas.drawCircle(cx, cy, radius, paint)
        stroke(palette.ink)
        val a = dp(5f)
        canvas.drawLine(cx - a, cy - a, cx + a, cy + a, paint)
        canvas.drawLine(cx - a, cy + a, cx + a, cy - a, paint)
        paint.style = Paint.Style.FILL
    }

    private fun drawPrimary(canvas: Canvas, action: VoiceStripAction, cx: Float, cy: Float) {
        paint.style = Paint.Style.FILL
        if (action == VoiceStripAction.NONE) return
        paint.color = if (pressed == action) palette.pressedKey else palette.accent
        canvas.drawCircle(cx, cy, radius, paint)
        val ink = if (pressed == action) palette.ink else palette.onAccent
        when (action) {
            VoiceStripAction.STOP -> {
                paint.color = ink
                val a = dp(5f)
                rect.set(cx - a, cy - a, cx + a, cy + a)
                canvas.drawRoundRect(rect, dp(1.5f), dp(1.5f), paint)
            }
            VoiceStripAction.RETRY -> {
                stroke(ink)
                rect.set(cx - dp(6f), cy - dp(6f), cx + dp(6f), cy + dp(6f))
                canvas.drawArc(rect, -60f, 300f, false, paint)
                paint.style = Paint.Style.FILL
                path.reset()
                path.moveTo(cx + dp(3f), cy - dp(9.5f))
                path.lineTo(cx + dp(7.5f), cy - dp(4.5f))
                path.lineTo(cx + dp(1.5f), cy - dp(3f))
                path.close()
                paint.color = ink
                canvas.drawPath(path, paint)
            }
            else -> {}
        }
        paint.style = Paint.Style.FILL
    }

    private fun pulse(now: Long): Float = (1f + kotlin.math.sin(now / 250.0).toFloat()) / 2f

    private fun stroke(color: Int) {
        paint.color = color
        paint.style = Paint.Style.STROKE
        paint.strokeWidth = dp(2f)
        paint.strokeCap = Paint.Cap.ROUND
    }

    private fun baseline(cy: Float, p: Paint) = cy - (p.ascent() + p.descent()) / 2f

    private companion object {
        const val BARS = 40
    }
}
