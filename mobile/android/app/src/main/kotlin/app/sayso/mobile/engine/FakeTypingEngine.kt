package app.sayso.mobile.engine

import java.io.File
import kotlin.math.abs
import kotlin.math.hypot
import kotlin.math.min

/**
 * A small engine written in Kotlin so the keyboard works before the native
 * core is wired in. Nearest-key hit testing with a learned per-key offset, a
 * short English word list, keyboard-aware edit distance for autocorrect, and a
 * few next-word pairs. [storage] keeps personal words and replacements; null
 * keeps them in memory.
 */
class FakeTypingEngine(private val storage: File? = null) : TypingEngine {
    private var layout = EngineLayout(0f, 0f, emptyList())
    private var flags = EditorFlags(privacy = false, autoCapitalize = true, inputKind = InputKind.TEXT, suggestionsAllowed = true)
    private var preferences = EnginePreferences()

    private val learned = HashMap<Int, Offset>()
    private val word = StringBuilder()
    private var previousWord: String? = null
    private var lastAutocorrect: AppliedCorrection? = null
    private val rejected = HashSet<String>()

    private val personal = LinkedHashSet<String>()
    private val replacementMap = LinkedHashMap<String, String>()

    private var lastCandidates: List<Suggestion> = emptyList()
    private var lastMargin = 0f

    init {
        load()
    }

    override fun setLayout(layout: EngineLayout) {
        this.layout = layout
    }

    override fun setEditorFlags(flags: EditorFlags) {
        this.flags = flags
    }

    override fun setPreferences(preferences: EnginePreferences) {
        this.preferences = preferences
    }

    override fun setContext(textBeforeCursor: String) {
        lastAutocorrect = null
        word.clear()
        var end = textBeforeCursor.length
        var start = end
        while (start > 0 && isWordChar(textBeforeCursor[start - 1])) start--
        word.append(textBeforeCursor, start, end)
        end = start
        while (end > 0 && !isWordChar(textBeforeCursor[end - 1])) end--
        start = end
        while (start > 0 && isWordChar(textBeforeCursor[start - 1])) start--
        previousWord = if (start < end) textBeforeCursor.substring(start, end).lowercase() else null
    }

    override fun onTouch(x: Float, y: Float, timeMs: Long, shifted: Boolean): Int {
        lastAutocorrect = null
        val key = layout.keys.minByOrNull { key ->
            val offset = learned[key.code] ?: Offset.ZERO
            hypot(x - (key.centerX + offset.dx), y - (key.centerY + offset.dy))
        } ?: return TypingEngine.NO_KEY
        if (!flags.privacy) learn(key, x, y)
        val char = key.code.toChar()
        word.append(if (shifted) char.uppercaseChar() else char)
        return key.code
    }

    override fun onText(text: String) {
        lastAutocorrect = null
        if (text.all(::isWordChar)) word.append(text) else word.clear()
    }

    override fun onBackspace() {
        lastAutocorrect = null
        if (word.isNotEmpty()) word.setLength(word.length - 1)
    }

    override fun onSpaceOrPunct(char: Char): CommitAction {
        val typed = word.toString()
        word.clear()
        lastAutocorrect = null
        val separator = char.toString()
        if (typed.isEmpty()) return CommitAction(0, separator)

        val expansion = replacementMap[typed.lowercase()]
        val corrected = when {
            expansion != null -> expansion
            !correctionsOn() -> null
            else -> rank(typed).firstOrNull { it.isAutoCommit }?.let { matchCase(it.text, typed) }
        }
        if (corrected == null || corrected == typed) {
            previousWord = typed.lowercase()
            return CommitAction(0, separator)
        }
        previousWord = corrected.lowercase()
        lastAutocorrect = AppliedCorrection(typed, corrected, separator)
        return CommitAction(typed.length, corrected + separator, wasAutocorrect = true, original = typed)
    }

    override fun suggestions(): List<Suggestion> {
        if (!flags.suggestionsAllowed) return emptyList()
        if (word.isNotEmpty()) return rank(word.toString())
        if (!preferences.nextWordPrediction) return emptyList()
        val previous = previousWord ?: return emptyList()
        return NEXT_WORDS[previous].orEmpty().take(MAX_SUGGESTIONS).mapIndexed { i, next ->
            Suggestion(next, 1f - i * 0.1f, isAutoCommit = false)
        }
    }

    override fun pickSuggestion(index: Int): CommitAction {
        val picked = suggestions().getOrNull(index) ?: return CommitAction(0, "")
        val typed = word.toString()
        val text = if (typed.isEmpty()) picked.text else matchCase(picked.text, typed)
        word.clear()
        lastAutocorrect = null
        previousWord = text.lowercase()
        return CommitAction(typed.length, "$text ")
    }

    override fun revertLastAutocorrect(): CommitAction? {
        val last = lastAutocorrect ?: return null
        lastAutocorrect = null
        rejected += last.original.lowercase()
        previousWord = last.original.lowercase()
        return CommitAction(last.corrected.length + last.separator.length, last.original + last.separator)
    }

    override fun personalWords(): List<String> = personal.toList()

    override fun addPersonalWord(word: String) {
        val trimmed = word.trim()
        if (trimmed.isEmpty() || !personal.add(trimmed)) return
        save()
    }

    override fun removePersonalWord(word: String) {
        if (personal.remove(word)) save()
    }

    override fun replacements(): List<Replacement> = replacementMap.map { (k, v) -> Replacement(k, v) }

    override fun putReplacement(replacement: Replacement) {
        val shortcut = replacement.shortcut.trim().lowercase()
        val expansion = replacement.expansion.trim()
        if (shortcut.isEmpty() || expansion.isEmpty() || shortcut.any { !isWordChar(it) }) return
        replacementMap[shortcut] = expansion
        save()
    }

    override fun removeReplacement(shortcut: String) {
        if (replacementMap.remove(shortcut) != null) save()
    }

    override fun resetLearnedData() {
        learned.clear()
        rejected.clear()
        previousWord = null
        lastCandidates = emptyList()
        lastMargin = 0f
    }

    override fun diagnostics(): Diagnostics = Diagnostics(
        keys = layout.keys.map { key ->
            val area = hitArea(key)
            val offset = learned[key.code] ?: Offset.ZERO
            KeyDiagnostics(key.code, area[0] + offset.dx, area[1] + offset.dy, area[2] + offset.dx, area[3] + offset.dy, offset.dx, offset.dy)
        },
        candidates = lastCandidates,
        autocorrectMargin = lastMargin,
        autocorrectThreshold = threshold(),
    )

    private fun correctionsOn() = preferences.autocorrect != AutocorrectMode.OFF && flags.suggestionsAllowed

    private fun threshold() = when (preferences.autocorrect) {
        AutocorrectMode.STRICT -> 0.35f
        else -> 0.15f
    }

    private fun learn(key: EngineKey, x: Float, y: Float) {
        val old = learned[key.code] ?: Offset.ZERO
        val maxX = (key.right - key.left) / 2f
        val maxY = (key.bottom - key.top) / 2f
        val dx = (old.dx * (1 - LEARN_RATE) + (x - key.centerX) * LEARN_RATE).coerceIn(-maxX, maxX)
        val dy = (old.dy * (1 - LEARN_RATE) + (y - key.centerY) * LEARN_RATE).coerceIn(-maxY, maxY)
        learned[key.code] = Offset(dx, dy)
    }

    /** The key's rectangle grown to halfway to its neighbors, or to the keyboard edge. */
    private fun hitArea(key: EngineKey): FloatArray {
        val sameRow = layout.keys.filter { abs(it.centerY - key.centerY) < 1f && it !== key }
        val left = sameRow.filter { it.right <= key.left }.maxOfOrNull { (it.right + key.left) / 2f } ?: 0f
        val right = sameRow.filter { it.left >= key.right }.minOfOrNull { (it.left + key.right) / 2f } ?: layout.width
        val above = layout.keys.filter { it.bottom <= key.top }.maxOfOrNull { (it.bottom + key.top) / 2f } ?: 0f
        val below = layout.keys.filter { it.top >= key.bottom }.minOfOrNull { (it.top + key.bottom) / 2f } ?: layout.height
        return floatArrayOf(left, above, right, below)
    }

    private fun rank(typed: String): List<Suggestion> {
        val lower = typed.lowercase()
        val known = lower in DICTIONARY_INDEX || personal.any { it.lowercase() == lower }
        val scored = ArrayList<Pair<String, Float>>()
        val vocabulary = personal.map { it to PERSONAL_FREQUENCY } + DICTIONARY.mapIndexed { i, w -> w to frequency(i) }
        val seen = HashSet<String>()
        for ((candidate, freq) in vocabulary) {
            val candidateLower = candidate.lowercase()
            if (!seen.add(candidateLower)) continue
            val cost = when {
                candidateLower == lower -> 0f
                candidateLower.startsWith(lower) && lower.length >= 2 -> COMPLETION_COST
                else -> editCost(lower, candidateLower)
            }
            if (cost <= MAX_COST) scored += candidate to freq * (1f - 0.45f * cost)
        }
        if (!known) scored += typed to UNKNOWN_LITERAL_SCORE
        scored.sortByDescending { it.second }

        val best = scored.firstOrNull()
        val margin = if (best == null) 0f else best.second - (scored.getOrNull(1)?.second ?: 0f)
        val autoCommit = best != null && correctionsOn() && !known &&
            best.first.lowercase() != lower && lower !in rejected && margin >= threshold()

        var top = scored.take(MAX_SUGGESTIONS)
        if (!known && top.none { it.first == typed }) top = top.take(MAX_SUGGESTIONS - 1) + (typed to UNKNOWN_LITERAL_SCORE)
        val result = top.mapIndexed { i, (text, score) ->
            Suggestion(matchCase(text, typed), score, isAutoCommit = autoCommit && i == 0)
        }
        lastCandidates = result
        lastMargin = margin
        return result
    }

    /** Damerau-Levenshtein with cheaper substitutions between neighboring keys. */
    private fun editCost(a: String, b: String): Float {
        if (abs(a.length - b.length) > 1) return Float.MAX_VALUE
        val d = Array(a.length + 1) { FloatArray(b.length + 1) }
        for (i in 0..a.length) d[i][0] = i.toFloat()
        for (j in 0..b.length) d[0][j] = j.toFloat()
        for (i in 1..a.length) {
            for (j in 1..b.length) {
                val sub = when {
                    a[i - 1] == b[j - 1] -> 0f
                    neighbors(a[i - 1], b[j - 1]) -> 0.5f
                    else -> 1f
                }
                var cost = min(min(d[i - 1][j] + 1f, d[i][j - 1] + 1f), d[i - 1][j - 1] + sub)
                if (i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1]) {
                    cost = min(cost, d[i - 2][j - 2] + 0.6f)
                }
                d[i][j] = cost
            }
        }
        return d[a.length][b.length]
    }

    private fun neighbors(a: Char, b: Char): Boolean {
        val ka = layout.keys.firstOrNull { it.code == a.code } ?: return false
        val kb = layout.keys.firstOrNull { it.code == b.code } ?: return false
        val width = ka.right - ka.left
        return hypot(ka.centerX - kb.centerX, ka.centerY - kb.centerY) < width * 1.6f
    }

    private fun load() {
        val file = storage ?: return
        if (!file.exists()) return
        for (line in file.readLines()) {
            val parts = line.split('\t')
            when {
                parts.size == 2 && parts[0] == "w" -> personal += parts[1]
                parts.size == 3 && parts[0] == "r" -> replacementMap[parts[1]] = parts[2]
            }
        }
    }

    private fun save() {
        val file = storage ?: return
        val lines = personal.map { "w\t$it" } + replacementMap.map { (k, v) -> "r\t$k\t$v" }
        file.parentFile?.mkdirs()
        file.writeText(lines.joinToString("\n"))
    }

    private data class Offset(val dx: Float, val dy: Float) {
        companion object {
            val ZERO = Offset(0f, 0f)
        }
    }

    private data class AppliedCorrection(val original: String, val corrected: String, val separator: String)

    companion object {
        private const val MAX_SUGGESTIONS = 3
        private const val LEARN_RATE = 0.1f
        private const val MAX_COST = 1f
        private const val COMPLETION_COST = 0.5f
        private const val PERSONAL_FREQUENCY = 0.9f
        private const val UNKNOWN_LITERAL_SCORE = 0.25f

        fun matchCase(candidate: String, typed: String): String = when {
            typed.length > 1 && typed.all { !it.isLetter() || it.isUpperCase() } -> candidate.uppercase()
            typed.firstOrNull()?.isUpperCase() == true -> candidate.replaceFirstChar { it.uppercaseChar() }
            else -> candidate
        }

        private fun frequency(rank: Int) = 1f - 0.5f * rank / DICTIONARY.size

        /** Most frequent first. */
        private val DICTIONARY = listOf(
            "the", "I", "to", "and", "a", "you", "of", "it", "is", "in", "that", "for", "on", "my", "me", "this",
            "be", "have", "with", "not", "are", "was", "but", "so", "just", "we", "do", "at", "can", "if", "what",
            "all", "your", "I'm", "it's", "don't", "get", "like", "know", "will", "up", "out", "no", "yes", "go",
            "about", "one", "there", "they", "think", "how", "when", "time", "good", "now", "see", "from", "or",
            "he", "she", "here", "want", "would", "make", "been", "could", "really", "back", "then", "work", "day",
            "need", "more", "going", "today", "love", "thanks", "thank", "please", "can't", "right", "people",
            "some", "come", "him", "her", "them", "say", "well", "got", "way", "new", "also", "only", "very", "an",
            "much", "where", "why", "who", "said", "home", "tomorrow", "night", "morning", "great", "sure",
            "sorry", "okay", "maybe", "still", "let", "take", "again", "first", "last", "next", "call", "tell",
            "feel", "look", "help", "give", "keep", "find", "thing", "things", "something", "nothing", "because",
            "before", "after", "should", "lunch", "dinner", "meeting", "later", "soon", "week", "weekend",
            "phone", "message", "text", "keyboard", "voice", "type", "typing", "word", "words", "hello", "hey",
            "friend", "family", "school", "happy", "nice", "fine", "best", "better", "little", "long",
        )
        private val DICTIONARY_INDEX = DICTIONARY.map { it.lowercase() }.toSet()

        private val NEXT_WORDS = mapOf(
            "i" to listOf("think", "am", "will"),
            "i'm" to listOf("going", "not", "on"),
            "thank" to listOf("you", "goodness"),
            "how" to listOf("are", "is", "about"),
            "see" to listOf("you", "it", "what"),
            "on" to listOf("my", "the", "it"),
            "good" to listOf("morning", "night", "luck"),
            "what" to listOf("is", "are", "time"),
            "in" to listOf("the", "a", "my"),
            "talk" to listOf("to", "soon", "later"),
        )
    }
}
