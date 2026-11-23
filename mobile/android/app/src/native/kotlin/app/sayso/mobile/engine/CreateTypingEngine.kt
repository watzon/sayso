package app.sayso.mobile.engine

import android.content.Context

// Built only with -PsaysoNative=true. NativeTypingEngine is the JNI bridge to
// libsayso_core.so; add it beside this file. See mobile/android/README.md.
fun createTypingEngine(context: Context): TypingEngine = NativeTypingEngine(context)
