package app.sayso.mobile.ime

import android.content.Context
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.Typeface
import android.view.View
import app.sayso.mobile.engine.Diagnostics
import java.util.Locale

/**
 * Developer Diagnostics: candidate scores and the autocorrect margin, one line
 * each. With the native engine a candidate reads "text total=spatial+lm eEdits".
 */
class DiagnosticsPanelView(context: Context) : View(context) {
    private val density = resources.displayMetrics.density

    var palette: KeyboardPalette = KeyboardPalette.LIGHT
        set(value) { field = value; invalidate() }

    private var lines: List<String> = emptyList()

    private val text = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        typeface = Typeface.MONOSPACE
        textSize = 11f * density
    }

    fun show(d: Diagnostics) {
        val margin = String.format(Locale.US, "margin %.3f  threshold %.3f  %s", d.autocorrectMargin, d.autocorrectThreshold,
            if (d.candidates.any { it.isAutoCommit }) "→ autocorrect" else "no autocorrect")
        val candidates = when {
            d.candidates.isEmpty() -> "no candidates"
            d.details.isNotEmpty() -> d.details.zip(d.candidates).joinToString("  ") { (c, s) ->
                String.format(Locale.US, "%s%s %.1f=%.1f+%.1f e%d", if (s.isAutoCommit) "*" else "", c.text, c.total, c.spatial, c.languageModel, c.edits)
            }
            else -> d.candidates.joinToString("  ") { String.format(Locale.US, "%s%s %.3f", if (it.isAutoCommit) "*" else "", it.text, it.score) }
        }
        lines = listOf(candidates, margin)
        invalidate()
    }

    override fun onMeasure(widthMeasureSpec: Int, heightMeasureSpec: Int) {
        setMeasuredDimension(MeasureSpec.getSize(widthMeasureSpec), (LINES * 16 * density + 8 * density).toInt())
    }

    override fun onDraw(canvas: Canvas) {
        canvas.drawColor(palette.accentWash)
        text.color = palette.ink
        lines.forEachIndexed { i, line ->
            canvas.drawText(line, 8 * density, (i + 1) * 16 * density, text)
        }
    }

    private companion object {
        const val LINES = 2
    }
}
