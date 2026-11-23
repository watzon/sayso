package app.sayso.mobile.engine

import android.content.Context

// The native build. Until the Go core exposes the voice C ABI this is the
// Kotlin stand-in; the core work replaces it with NativeVoiceCore.
@Suppress("UNUSED_PARAMETER")
fun createVoiceCore(context: Context): VoiceCore = FakeVoiceCore()
