package app.sayso.mobile.emoji

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File

class EmojiCatalogTest {
    private val sample = """
        # comment
        @Smileys & Emotion
        😀	grinning face	cheerful grin happy smile
        😂	face with tears of joy	laugh lol
        @People & Body
        👋	waving hand	bye hello hi wave	👋🏻 👋🏼 👋🏽 👋🏾 👋🏿
        🫶	heart hands	love	🫶🏻 🫶🏼 🫶🏽 🫶🏾 🫶🏿
        @Flags
        🇺🇸	flag: United States
    """.trimIndent()

    private fun catalog(drawable: (String) -> Boolean = { true }) = EmojiCatalog.parse(sample.lineSequence(), drawable)

    @Test
    fun parsesGroupsIntoCategories() {
        val c = catalog()
        assertEquals(listOf(EmojiCategory.SMILEYS, EmojiCategory.PEOPLE, EmojiCategory.FLAGS), c.categories.keys.toList())
        assertEquals(5, c.size)
        assertEquals(listOf("👋🏻", "👋🏼", "👋🏽", "👋🏾", "👋🏿"), c.find("👋")!!.tones)
        assertEquals("👋", c.find("👋🏽")!!.value)
    }

    @Test
    fun emojiTheFontCannotDrawAreLeftOut() {
        val c = catalog { it != "😂" && it != "🫶🏿" }
        assertNull(c.find("😂"))
        assertEquals(1, c.categories.getValue(EmojiCategory.SMILEYS).size)
        assertTrue("one missing tone drops all its tones", c.find("🫶")!!.tones.isEmpty())
        assertTrue(c.find("👋")!!.hasTones)
    }

    @Test
    fun searchMatchesWordPrefixesOfNamesAndKeywords() {
        val c = catalog()
        assertEquals(listOf("👋"), c.search("wav").map { it.value })
        assertEquals(listOf("👋"), c.search("HELLO").map { it.value })
        assertEquals(listOf("🇺🇸"), c.search("united st").map { it.value })
        assertEquals(listOf("😂"), c.search("lol").map { it.value })
        assertTrue(c.search("zebra").isEmpty())
        assertTrue(c.search("  ").isEmpty())
    }

    @Test
    fun searchPutsNameMatchesBeforeKeywordMatches() {
        val c = EmojiCatalog.parse(
            sequenceOf("@Smileys & Emotion", "😍\tsmiling face with heart-eyes\tlove", "❤️\tred heart\tlove", "💌\tlove letter\tmail"),
        )
        assertEquals(listOf("💌", "😍", "❤️"), c.search("love").map { it.value })
    }

    @Test
    fun recentsKeepTheNewestFirstWithoutDuplicatesAndRoundTrip() {
        val r = EmojiRecents(max = 3)
        listOf("😀", "👋", "😀", "🍕", "🎉").forEach(r::add)
        assertEquals(listOf("🎉", "🍕", "😀"), r.items)
        assertEquals(r.items, EmojiRecents.decode(r.encode(), max = 3).items)
        assertTrue(EmojiRecents.decode(null).items.isEmpty())
    }

    @Test
    fun aChosenToneIsRememberedPerEmojiAndRoundTrips() {
        val c = catalog()
        val wave = c.find("👋")!!
        val tones = EmojiTones()
        assertEquals("👋", tones.preferred(wave))
        tones.choose(wave, "👋🏾")
        assertEquals("👋🏾", tones.preferred(wave))
        assertEquals("👋🏾", EmojiTones.decode(tones.encode()).preferred(wave))
        assertEquals("🫶", tones.preferred(c.find("🫶")!!))
        tones.choose(wave, "👋")
        assertEquals("👋", tones.preferred(wave))
        tones.choose(wave, "😀")
        assertEquals("not a form of this emoji", "👋", tones.preferred(wave))
    }

    @Test
    fun theGridStartsWithRecentsAndKnowsWhereEachCategoryStarts() {
        val c = catalog()
        val tones = EmojiTones().apply { choose(c.find("👋")!!, "👋🏻") }
        val grid = EmojiGrid.build(c, listOf("👋🏽", "🦄-gone"), tones)
        assertEquals(EmojiGridItem.Header(EmojiCategory.RECENT), grid.items[0])
        assertEquals("👋🏽", (grid.items[1] as EmojiGridItem.Cell).form)
        assertEquals(2, grid.starts[EmojiCategory.SMILEYS])
        val wave = grid.items.filterIsInstance<EmojiGridItem.Cell>().first { it.emoji.value == "👋" && it.form != "👋🏽" }
        assertEquals("the grid shows the chosen tone", "👋🏻", wave.form)
        assertEquals(EmojiCategory.PEOPLE, grid.categoryAt(grid.starts.getValue(EmojiCategory.PEOPLE) + 1))
        assertEquals(EmojiCategory.RECENT, grid.categoryAt(1))
        assertFalse(EmojiCategory.RECENT in EmojiGrid.build(c, emptyList(), tones).starts)
    }

    /** The data file the APK ships: the full standard set, every category, clean tone sets. */
    @Test
    fun theBundledDataIsTheFullUnicodeSet() {
        val file = File("src/main/assets/emoji/emoji.txt")
        assertTrue("missing ${file.absolutePath}", file.isFile)
        val c = EmojiCatalog.parse(file.readLines().asSequence())
        assertTrue("${c.size} emoji", c.size in 1_800..2_200)
        for (category in EmojiCategory.entries - EmojiCategory.RECENT) {
            assertTrue("$category is empty", c.categories[category].orEmpty().size > 50)
        }
        val all = c.categories.values.flatten()
        assertEquals("no duplicates", all.size, all.map { it.value }.toSet().size)
        assertTrue("skin tones", all.count { it.hasTones } > 300)
        val modifiers = 0x1F3FB..0x1F3FF
        for (e in all) {
            assertFalse("${e.name} has a tone in its base form", e.value.codePoints().anyMatch { it in modifiers })
            e.tones.forEachIndexed { i, t ->
                assertTrue("${e.name} tone $i", t.codePoints().anyMatch { it == modifiers.first + i })
            }
        }
        assertEquals("👋", c.search("waving").first().value)
        assertTrue(c.search("pizza").any { it.value == "🍕" })
        assertTrue(c.search("japan").any { it.value == "🇯🇵" })
    }
}
