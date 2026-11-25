package app.sayso.mobile.ui

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.consumeWindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.PrimaryScrollableTabRow
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Tab
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import app.sayso.mobile.engine.TypingEngine
import app.sayso.mobile.engine.createVoiceCore
import app.sayso.mobile.settings.KeyboardSettingsStore
import app.sayso.mobile.telemetry.Telemetry

data class KeyboardStatus(val enabled: Boolean, val selected: Boolean)

private val TABS = listOf("Setup", "Voice", "Dictionary", "History", "Settings")

@Composable
fun SaysoApp(
    status: KeyboardStatus,
    store: KeyboardSettingsStore,
    engine: TypingEngine,
    onEnableKeyboard: () -> Unit,
    onSwitchKeyboard: () -> Unit,
) {
    val context = LocalContext.current
    var settings by remember { mutableStateOf(store.load()) }
    val telemetry = remember { Telemetry.get(context) }
    var telemetryOn by remember { mutableStateOf(telemetry.enabled) }
    val setTelemetry = { on: Boolean ->
        telemetry.enabled = on
        telemetryOn = on
    }
    val voice = remember { createVoiceCore(context) }
    var tab by rememberSaveable { mutableIntStateOf(0) }
    SaysoTheme {
        Scaffold(modifier = Modifier.fillMaxSize()) { padding ->
            // Edge-to-edge windows no longer resize for the keyboard: shrink for it here,
            // less the navigation bar the Scaffold padding already covers.
            Column(Modifier.fillMaxSize().padding(padding).consumeWindowInsets(padding).imePadding()) {
                PrimaryScrollableTabRow(selectedTabIndex = tab, edgePadding = 8.dp) {
                    TABS.forEachIndexed { i, title ->
                        Tab(selected = tab == i, onClick = { tab = i }, text = { Text(title) })
                    }
                }
                when (tab) {
                    0 -> SetupScreen(status, onEnableKeyboard, onSwitchKeyboard, telemetryOn, setTelemetry)
                    1 -> VoiceScreen(voice)
                    2 -> DictionaryScreen(engine)
                    3 -> HistoryScreen(voice)
                    else -> SettingsScreen(
                        settings = settings,
                        onChange = {
                            settings = it
                            store.save(it)
                        },
                        onResetLearned = engine::resetLearnedData,
                        telemetry = telemetryOn,
                        onTelemetry = setTelemetry,
                    )
                }
            }
        }
    }
}
