package app.sayso.mobile.keyboard

import app.sayso.mobile.engine.EngineKey
import app.sayso.mobile.engine.EngineLayout
import kotlin.math.hypot

enum class KeyKind { LETTER, CHARACTER, SHIFT, BACKSPACE, SYMBOLS, SYMBOLS_MORE, LETTERS, EMOJI, SPACE, ENTER }

enum class KeyboardMode { LETTERS, SYMBOLS, SYMBOLS_MORE }

object KeyCodes {
    const val SHIFT = -1
    const val SYMBOLS = -2
    const val LETTERS = -3
    const val SYMBOLS_MORE = -4
    const val BACKSPACE = -5
    const val EMOJI = -6
    const val ENTER = '\n'.code
    const val SPACE = ' '.code
}

/** A key before layout. [width] is in key units; a letter key is 1. */
data class KeySpec(
    val code: Int,
    val label: String,
    val kind: KeyKind,
    val width: Float = 1f,
    val longPress: List<String> = emptyList(),
)

/** A key laid out in keyboard view pixels. The rectangle is the drawn key, without the gap around it. */
data class Key(val spec: KeySpec, val left: Float, val top: Float, val right: Float, val bottom: Float) {
    val centerX: Float get() = (left + right) / 2f
    val centerY: Float get() = (top + bottom) / 2f
    val width: Float get() = right - left
    val height: Float get() = bottom - top
}

class Keyboard(val mode: KeyboardMode, val width: Float, val height: Float, val keys: List<Key>, private val gapX: Float, private val gapY: Float) {
    /** The key whose cell (key plus half the gap on each side) holds the point, else the nearest key. */
    fun keyAt(x: Float, y: Float): Key? =
        keys.firstOrNull { x >= it.left - gapX / 2 && x < it.right + gapX / 2 && y >= it.top - gapY / 2 && y < it.bottom + gapY / 2 }
            ?: keys.minByOrNull { hypot(x - it.centerX, y - it.centerY) }

    /** The letter keys, which are the only keys the engine resolves touches to. */
    fun engineLayout(): EngineLayout = EngineLayout(
        width,
        height,
        keys.filter { it.spec.kind == KeyKind.LETTER }.map { EngineKey(it.spec.code, it.spec.label, it.left, it.top, it.right, it.bottom) },
    )
}

object KeyboardLayouts {
    private val ACCENTS = mapOf(
        'a' to "àáâäæãåā", 'c' to "çćč", 'e' to "èéêëēę", 'i' to "ìíîïī", 'l' to "ł", 'n' to "ñń",
        'o' to "òóôöõøœō", 's' to "ßśš", 'u' to "ùúûüū", 'y' to "ýÿ", 'z' to "žźż",
    )

    fun rows(mode: KeyboardMode): List<List<KeySpec>> = when (mode) {
        KeyboardMode.LETTERS -> listOf(
            letters("qwertyuiop"),
            letters("asdfghjkl"),
            listOf(KeySpec(KeyCodes.SHIFT, "shift", KeyKind.SHIFT, 1.5f)) + letters("zxcvbnm") + backspace(),
            bottomRow(KeySpec(KeyCodes.SYMBOLS, "?123", KeyKind.SYMBOLS, 1.5f)),
        )
        KeyboardMode.SYMBOLS -> listOf(
            chars("1234567890"),
            chars("@#\$_&-+()/"),
            listOf(KeySpec(KeyCodes.SYMBOLS_MORE, "=\\<", KeyKind.SYMBOLS_MORE, 1.5f)) + chars("*\"':;!?") + backspace(),
            bottomRow(KeySpec(KeyCodes.LETTERS, "ABC", KeyKind.LETTERS, 1.5f)),
        )
        KeyboardMode.SYMBOLS_MORE -> listOf(
            chars("~`|•√π÷×¶∆"),
            chars("£€¥^°={}\\%"),
            listOf(KeySpec(KeyCodes.SYMBOLS, "?123", KeyKind.SYMBOLS, 1.5f)) + chars("©®™✓[]<") + backspace(),
            bottomRow(KeySpec(KeyCodes.LETTERS, "ABC", KeyKind.LETTERS, 1.5f)),
        )
    }

    /**
     * Lays out [mode] in a [width] x [height] area. Every row is scaled by the
     * widest row; shorter rows are centered.
     */
    fun build(mode: KeyboardMode, width: Float, height: Float, gapX: Float, gapY: Float): Keyboard {
        val rows = rows(mode)
        val units = rows.maxOf { row -> row.sumOf { it.width.toDouble() } }.toFloat()
        val unitWidth = width / units
        val rowHeight = height / rows.size
        val keys = ArrayList<Key>()
        rows.forEachIndexed { r, row ->
            var x = (width - row.sumOf { it.width.toDouble() }.toFloat() * unitWidth) / 2f
            val top = r * rowHeight
            for (spec in row) {
                val w = spec.width * unitWidth
                keys += Key(spec, x + gapX / 2, top + gapY / 2, x + w - gapX / 2, top + rowHeight - gapY / 2)
                x += w
            }
        }
        return Keyboard(mode, width, height, keys, gapX, gapY)
    }

    private fun letters(chars: String) = chars.map { c ->
        KeySpec(c.code, c.toString(), KeyKind.LETTER, longPress = ACCENTS[c].orEmpty().map(Char::toString))
    }

    private fun chars(chars: String) = chars.map { c -> KeySpec(c.code, c.toString(), KeyKind.CHARACTER) }

    private fun backspace() = listOf(KeySpec(KeyCodes.BACKSPACE, "delete", KeyKind.BACKSPACE, 1.5f))

    private fun bottomRow(modeKey: KeySpec) = listOf(
        modeKey,
        KeySpec(KeyCodes.EMOJI, "emoji", KeyKind.EMOJI),
        KeySpec(','.code, ",", KeyKind.CHARACTER),
        KeySpec(KeyCodes.SPACE, "space", KeyKind.SPACE, 4f),
        KeySpec('.'.code, ".", KeyKind.CHARACTER, longPress = listOf("?", "!", "'", "\"", "-", ":", ";")),
        KeySpec(KeyCodes.ENTER, "enter", KeyKind.ENTER, 1.5f),
    )
}
