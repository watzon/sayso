package app.sayso.mobile.engine

import android.content.Context

/**
 * The native build: [NativeVoiceCore] on the process's typing engine
 * ([SaysoEngine]), so voice and keyboard share the personal dictionary and
 * the journal folder. Call it on the main thread, like [SaysoEngine.get].
 */
fun createVoiceCore(context: Context): VoiceCore {
    val typing = SaysoEngine.get(context)
    return if (typing is NativeTypingEngine) NativeVoiceCore(typing.handle) else FakeVoiceCore()
}
