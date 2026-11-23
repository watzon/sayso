package app.sayso.mobile.engine

import android.content.Context

// The native build: the shared Go typing core through JNI. See mobile/android/README.md.
fun createTypingEngine(context: Context): TypingEngine = NativeTypingEngine.create(context)
