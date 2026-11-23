package app.sayso.mobile.engine

import android.content.Context

@Suppress("UNUSED_PARAMETER")
fun createVoiceCore(context: Context): VoiceCore = FakeVoiceCore()
