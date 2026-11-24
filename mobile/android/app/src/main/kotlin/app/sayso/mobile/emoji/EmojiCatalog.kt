package app.sayso.mobile.emoji

/** The emoji keyboard's tabs, in order. [group] is the Unicode group in the data file. */
enum class EmojiCategory(val title: String, val icon: String, val group: String?) {
    RECENT("Recent", "🕘", null),
    SMILEYS("Smileys & Emotion", "😀", "Smileys & Emotion"),
    PEOPLE("People & Body", "👋", "People & Body"),
    ANIMALS("Animals & Nature", "🐻", "Animals & Nature"),
    FOOD("Food & Drink", "🍔", "Food & Drink"),
    TRAVEL("Travel & Places", "🚗", "Travel & Places"),
    ACTIVITIES("Activities", "⚽", "Activities"),
    OBJECTS("Objects", "💡", "Objects"),
    SYMBOLS("Symbols", "🔣", "Symbols"),
    FLAGS("Flags", "🏳️", "Flags"),
    ;

    companion object {
        fun forGroup(group: String): EmojiCategory? = entries.firstOrNull { it.group == group }
    }
}

/**
 * One emoji as the keyboard lists it, without a skin tone. [tones] holds its
 * five Fitzpatrick variants, lightest first, or is empty.
 */
data class Emoji(val value: String, val name: String, val keywords: String, val tones: List<String> = emptyList()) {
    val hasTones: Boolean get() = tones.isNotEmpty()

    /** All the forms the tone picker offers: the default (yellow) one, then the five tones. */
    val forms: List<String> get() = listOf(value) + tones

    internal val words: List<String> =
        (name.lowercase().split(' ').map { it.trim(':', ',', '(', ')', '“', '”', '"') } + keywords.split(' '))
            .filter { it.isNotEmpty() }
}

/**
 * The emoji the phone can draw, by category, from the data file that
 * mobile/tools/emojigen writes (see its DATA_SOURCES.md).
 */
class EmojiCatalog(val categories: Map<EmojiCategory, List<Emoji>>) {
    private val byForm: Map<String, Emoji> = buildMap {
        for (list in categories.values) for (e in list) for (form in e.forms) put(form, e)
    }

    val size: Int get() = categories.values.sumOf { it.size }

    /** The emoji that [form] (an emoji or one of its tone variants) belongs to. */
    fun find(form: String): Emoji? = byForm[form]

    /**
     * Emoji whose name or keywords have a word starting with each word of
     * [query]. Names that start with the query come first, then names with a
     * matching word, then keyword matches, each in the catalog's order.
     */
    fun search(query: String, limit: Int = SEARCH_LIMIT): List<Emoji> {
        val q = query.trim().lowercase()
        if (q.isEmpty()) return emptyList()
        val tokens = q.split(Regex("\\s+"))
        val nameStarts = ArrayList<Emoji>()
        val nameWord = ArrayList<Emoji>()
        val keyword = ArrayList<Emoji>()
        for (list in categories.values) {
            for (e in list) {
                if (!tokens.all { t -> e.words.any { it.startsWith(t) } }) continue
                val name = e.name.lowercase()
                when {
                    name.startsWith(q) -> nameStarts += e
                    name.split(' ').any { it.startsWith(tokens[0]) } -> nameWord += e
                    else -> keyword += e
                }
            }
        }
        return (nameStarts + nameWord + keyword).take(limit)
    }

    companion object {
        const val SEARCH_LIMIT = 60

        /**
         * Reads the data file. [drawable] says whether the font draws a
         * sequence as one glyph; an emoji it cannot draw is left out, and so
         * are its tones when any of them cannot be drawn.
         */
        fun parse(lines: Sequence<String>, drawable: (String) -> Boolean = { true }): EmojiCatalog {
            val categories = linkedMapOf<EmojiCategory, MutableList<Emoji>>()
            var current: MutableList<Emoji>? = null
            for (line in lines) {
                when {
                    line.isEmpty() || line.startsWith("#") -> {}
                    line.startsWith("@") ->
                        current = EmojiCategory.forGroup(line.substring(1))?.let { categories.getOrPut(it) { mutableListOf() } }
                    else -> {
                        val list = current ?: continue
                        val f = line.split('\t')
                        if (f.size < 2 || !drawable(f[0])) continue
                        val tones = f.getOrNull(3)?.split(' ').orEmpty()
                        val keepTones = tones.size == 5 && tones.all(drawable)
                        list += Emoji(f[0], f[1], f.getOrElse(2) { "" }, if (keepTones) tones else emptyList())
                    }
                }
            }
            return EmojiCatalog(categories)
        }
    }
}

/** Recently used emoji, newest first, at most [max]. Stored as one string. */
class EmojiRecents(private val max: Int = MAX, initial: List<String> = emptyList()) {
    private val list = ArrayList(initial.distinct().take(max))

    val items: List<String> get() = list

    fun add(form: String) {
        list.remove(form)
        list.add(0, form)
        while (list.size > max) list.removeAt(list.size - 1)
    }

    fun encode(): String = list.joinToString("\n")

    companion object {
        const val MAX = 32

        fun decode(stored: String?, max: Int = MAX): EmojiRecents =
            EmojiRecents(max, stored.orEmpty().split('\n').filter { it.isNotEmpty() })
    }
}

/** The skin tone last chosen for each emoji, so the grid shows it next time. */
class EmojiTones(initial: Map<String, String> = emptyMap()) {
    private val chosen = HashMap(initial)

    /** The form to show and type for [emoji]: the remembered tone when it still exists, else the default. */
    fun preferred(emoji: Emoji): String = chosen[emoji.value]?.takeIf { it in emoji.tones } ?: emoji.value

    fun choose(emoji: Emoji, form: String) {
        when {
            form == emoji.value -> chosen.remove(emoji.value)
            form in emoji.tones -> chosen[emoji.value] = form
        }
    }

    fun encode(): String = chosen.entries.sortedBy { it.key }.joinToString("\n") { "${it.key}\t${it.value}" }

    companion object {
        fun decode(stored: String?): EmojiTones = EmojiTones(
            stored.orEmpty().split('\n').mapNotNull { line ->
                val parts = line.split('\t')
                if (parts.size == 2 && parts[0].isNotEmpty()) parts[0] to parts[1] else null
            }.toMap(),
        )
    }
}

/** One row of the emoji grid: a category title or an emoji, shown as [form]. */
sealed interface EmojiGridItem {
    data class Header(val category: EmojiCategory) : EmojiGridItem
    data class Cell(val emoji: Emoji, val form: String) : EmojiGridItem
}

/** The grid's items and where each category starts. Recent comes first when it has anything. */
class EmojiGrid(val items: List<EmojiGridItem>, val starts: Map<EmojiCategory, Int>) {
    /** The category the item at [position] belongs to. */
    fun categoryAt(position: Int): EmojiCategory? =
        starts.entries.filter { it.value <= position }.maxByOrNull { it.value }?.key

    companion object {
        fun build(catalog: EmojiCatalog, recents: List<String>, tones: EmojiTones): EmojiGrid {
            val items = ArrayList<EmojiGridItem>(catalog.size + EmojiCategory.entries.size)
            val starts = linkedMapOf<EmojiCategory, Int>()
            val recent = recents.mapNotNull { form -> catalog.find(form)?.let { EmojiGridItem.Cell(it, form) } }
            if (recent.isNotEmpty()) {
                starts[EmojiCategory.RECENT] = items.size
                items += EmojiGridItem.Header(EmojiCategory.RECENT)
                items += recent
            }
            for ((category, list) in catalog.categories) {
                if (list.isEmpty()) continue
                starts[category] = items.size
                items += EmojiGridItem.Header(category)
                list.mapTo(items) { EmojiGridItem.Cell(it, tones.preferred(it)) }
            }
            return EmojiGrid(items, starts)
        }
    }
}
