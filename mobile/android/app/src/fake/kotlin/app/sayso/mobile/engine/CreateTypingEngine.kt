package app.sayso.mobile.engine

import android.content.Context
import java.io.File

fun createTypingEngine(context: Context): TypingEngine =
    FakeTypingEngine(File(context.filesDir, "personal-dictionary.tsv"))
