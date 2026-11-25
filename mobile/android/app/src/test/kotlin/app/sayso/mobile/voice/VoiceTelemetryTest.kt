package app.sayso.mobile.voice

import app.sayso.mobile.engine.CloudAsrProvider
import app.sayso.mobile.engine.CloudConfig
import app.sayso.mobile.telemetry.DictationSignal
import org.junit.Assert.assertEquals
import org.junit.Test

class VoiceTelemetryTest {
    private fun cloud(provider: CloudAsrProvider, model: String) = CloudConfig(asrProvider = provider, asrApiKey = "sk-secret", asrModel = model, asrBaseUrl = "https://jane.example/v1")

    @Test
    fun namesOnlyPublicModels() {
        assertEquals(DictationSignal("whisper-base-en", "local", "raw"), dictationSignal(VoiceEngine.LOCAL, "whisper-base-en", CloudConfig.OFF, styled = false))
        assertEquals(DictationSignal("android_system", "system", "ai_cloud"), dictationSignal(VoiceEngine.SYSTEM, null, CloudConfig.OFF, styled = true))
        assertEquals(DictationSignal("groq:whisper-large-v3", "cloud", "raw"), dictationSignal(VoiceEngine.CLOUD, null, cloud(CloudAsrProvider.GROQ, " whisper-large-v3 "), styled = false))
        assertEquals(DictationSignal("open_ai", "cloud", "raw"), dictationSignal(VoiceEngine.CLOUD, null, cloud(CloudAsrProvider.OPENAI_WHISPER, ""), styled = false))
        assertEquals(DictationSignal("open_ai_compatible", "cloud", "raw"), dictationSignal(VoiceEngine.CLOUD, null, cloud(CloudAsrProvider.OPENAI_COMPATIBLE, "jane-finetune"), styled = false))
        assertEquals(DictationSignal("groq", "cloud", "raw"), dictationSignal(VoiceEngine.CLOUD, null, cloud(CloudAsrProvider.GROQ, "jane-finetune"), styled = false))
        assertEquals(DictationSignal("other", "cloud", "raw"), dictationSignal(VoiceEngine.CLOUD, null, CloudConfig.OFF, styled = false))
        assertEquals(DictationSignal("other", "local", "raw"), dictationSignal(VoiceEngine.LOCAL, null, CloudConfig.OFF, styled = false))
    }
}
