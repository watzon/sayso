package app.sayso.mobile.ime

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File

/**
 * Two mistakes that left the suggestion strip and the diagnostics panel blank.
 * The JVM tests have no Robolectric, so these read the sources.
 */
class ImeDrawingTest {
    private val dir = File("src/main/kotlin/app/sayso/mobile/ime")

    private fun sources(): List<File> {
        val files = dir.listFiles { f -> f.extension == "kt" }.orEmpty().toList()
        assertTrue("no sources in ${dir.absolutePath}", files.any { it.name == "KeyboardView.kt" })
        return files
    }

    /**
     * The input view sets clipChildren = false so key previews can draw above
     * the keys. A child's canvas is then not clipped to its bounds, and
     * drawColor fills the whole input view: the keyboard, drawn last, painted
     * over the strip and the panel. Fill the view's bounds with drawRect.
     */
    @Test
    fun imeViewsNeverFillTheWholeCanvas() {
        val unbounded = Regex("""\.(drawColor|drawPaint|drawARGB|drawRGB)\(""")
        val hits = sources().flatMap { f ->
            f.readLines().mapIndexedNotNull { i, line -> if (unbounded.containsMatchIn(line)) "${f.name}:${i + 1}: ${line.trim()}" else null }
        }
        assertEquals("unbounded fills in IME views", emptyList<String>(), hits)
    }

    /**
     * TextPaint has a public `density` field (1.0). Inside TextPaint().apply { }
     * that field shadows the view's density, so a size in dp came out in px.
     */
    @Test
    fun textPaintApplyBlocksDoNotReadDensity() {
        val hits = sources().flatMap { f ->
            val lines = f.readLines()
            val found = mutableListOf<String>()
            var inBlock = false
            lines.forEachIndexed { i, line ->
                if (line.contains("TextPaint(") && line.contains(".apply")) inBlock = true
                if (inBlock && Regex("""\bdensity\b""").containsMatchIn(line)) found += "${f.name}:${i + 1}: ${line.trim()}"
                if (inBlock && line.trim() == "}") inBlock = false
            }
            found
        }
        assertEquals("density read inside TextPaint().apply", emptyList<String>(), hits)
    }
}
