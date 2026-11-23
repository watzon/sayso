package app.sayso.mobile.engine

import org.junit.Assert.assertEquals
import org.junit.Test

class CommitActionTest {
    @Test
    fun thenAppendsWhenTheNextActionDeletesNothing() {
        assertEquals(CommitAction(2, "ab c"), CommitAction(2, "ab").then(CommitAction(0, " c")))
    }

    @Test
    fun thenDeletesFromTheFirstActionsTextFirst() {
        // "the " written, then the space swapped for ". ".
        assertEquals(CommitAction(3, "the. "), CommitAction(3, "the ").then(CommitAction(1, ". ")))
    }

    @Test
    fun thenDeletesBeforeTheFirstActionWhenItsTextRunsOut() {
        assertEquals(CommitAction(5, "xyz"), CommitAction(2, "ab").then(CommitAction(5, "xyz")))
    }

    @Test
    fun thenCountsUtf16Units() {
        // An emoji outside the BMP is two UTF-16 units, as Android's editor counts.
        assertEquals(CommitAction(0, "a"), CommitAction(0, "a😀").then(CommitAction(2, "")))
    }
}
