package app.sayso.mobile.settings

import app.sayso.mobile.asr.ModelCatalog
import app.sayso.mobile.engine.CloudAsrProvider
import app.sayso.mobile.engine.CloudConfig
import app.sayso.mobile.engine.CloudStyleProvider
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class VoiceSettingsTest {
    private val full = VoiceSettings(
        engine = VoiceEngineChoice.CLOUD,
        finalModelId = "parakeet",
        liveModelId = "zipformer",
        styleId = "email",
        handsFree = true,
        cloud = CloudConfig(
            asrProvider = CloudAsrProvider.GROQ,
            asrApiKey = "gsk-secret",
            asrModel = "whisper-large-v3",
            styleProvider = CloudStyleProvider.OPENAI_COMPATIBLE,
            styleApiKey = "sk-secret",
            styleBaseUrl = "https://llm.local/v1",
            styleModel = "small",
        ),
    )

    @Test
    fun emptyStorageGivesTheDefaults() {
        val s = VoiceSettingsCodec.decode(emptyMap(), emptyMap())
        assertEquals(VoiceSettings(), s)
        assertEquals(VoiceEngineChoice.AUTO, s.engine)
        assertEquals(CloudConfig.OFF, s.cloud)
        assertFalse(s.handsFree)
        assertFalse(s.cloudAsrConfigured)
    }

    @Test
    fun settingsRoundTrip() {
        val back = VoiceSettingsCodec.decode(VoiceSettingsCodec.encode(full), VoiceSettingsCodec.encodeSecrets(full))
        assertEquals(full, back)
    }

    @Test
    fun keysNeverGoToThePlainSettingsFile() {
        val plain = VoiceSettingsCodec.encode(full)
        assertFalse(plain.values.any { it.contains("secret") })
        assertEquals(setOf("gsk-secret", "sk-secret"), VoiceSettingsCodec.encodeSecrets(full).values.toSet())
        // Without the secrets file the keys are empty and cloud is not usable.
        assertFalse(VoiceSettingsCodec.decode(plain, emptyMap()).cloudAsrConfigured)
    }

    @Test
    fun unknownValuesFallBackToDefaults() {
        val s = VoiceSettingsCodec.decode(mapOf("engine" to "WARP", "hands_free" to "maybe", "cloud_asr_provider" to "X"), emptyMap())
        assertEquals(VoiceSettings(), s)
    }

    @Test
    fun cloudNeedsAProviderAndAKey() {
        assertFalse(VoiceSettings(cloud = CloudConfig(asrProvider = CloudAsrProvider.GROQ)).cloudAsrConfigured)
        assertFalse(VoiceSettings(cloud = CloudConfig(asrApiKey = "k")).cloudAsrConfigured)
        assertTrue(VoiceSettings(cloud = CloudConfig(asrProvider = CloudAsrProvider.GROQ, asrApiKey = "k")).cloudAsrConfigured)
    }

    @Test
    fun activeModelsByKind() {
        val s = VoiceSettings()
            .withActiveModel(ModelCatalog.PARAKEET_V3)
            .withActiveModel(ModelCatalog.ZIPFORMER_EN)
        assertEquals(ModelCatalog.PARAKEET_V3.id, s.finalModelId)
        assertEquals(ModelCatalog.ZIPFORMER_EN.id, s.liveModelId)
        val dropped = s.withoutModel(ModelCatalog.PARAKEET_V3.id)
        assertNull(dropped.finalModelId)
        assertEquals(ModelCatalog.ZIPFORMER_EN.id, dropped.liveModelId)
    }
}
