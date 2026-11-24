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
        modelId = "parakeet",
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
    fun compatibleServerNeedsUrlAndModelButNoKey() {
        val compat = CloudConfig(asrProvider = CloudAsrProvider.OPENAI_COMPATIBLE)
        assertFalse(VoiceSettings(cloud = compat.copy(asrBaseUrl = "http://h:8000/v1")).cloudAsrConfigured)
        assertTrue(VoiceSettings(cloud = compat.copy(asrBaseUrl = "http://h:8000/v1", asrModel = "whisper")).cloudAsrConfigured)
    }

    @Test
    fun oneActiveModel() {
        val s = VoiceSettings().withActiveModel(ModelCatalog.PARAKEET_V3)
        assertEquals(ModelCatalog.PARAKEET_V3.id, s.modelId)
        assertEquals(s, s.withoutModel("other"))
        assertNull(s.withoutModel(ModelCatalog.PARAKEET_V3.id).modelId)
    }

    @Test
    fun anOlderInstallKeepsItsActiveModelAndDropsTheLiveOne() {
        val s = VoiceSettingsCodec.decode(
            mapOf("final_model" to ModelCatalog.PARAKEET_V3.id, "live_model" to "zipformer-en-streaming-2023-06-26"),
            emptyMap(),
        )
        assertEquals(ModelCatalog.PARAKEET_V3.id, s.modelId)
        assertFalse(VoiceSettingsCodec.encode(s).containsKey("live_model"))
    }
}
