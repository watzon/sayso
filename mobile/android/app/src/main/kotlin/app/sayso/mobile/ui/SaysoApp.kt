package app.sayso.mobile.ui

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.PrimaryTabRow
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
import app.sayso.mobile.engine.TypingEngine
import app.sayso.mobile.settings.KeyboardSettingsStore

data class KeyboardStatus(val enabled: Boolean, val selected: Boolean)

private val TABS = listOf("Setup", "Dictionary", "Settings")

@Composable
fun SaysoApp(
    status: KeyboardStatus,
    store: KeyboardSettingsStore,
    engine: TypingEngine,
    onEnableKeyboard: () -> Unit,
    onSwitchKeyboard: () -> Unit,
) {
    var settings by remember { mutableStateOf(store.load()) }
    var tab by rememberSaveable { mutableIntStateOf(0) }
    SaysoTheme(settings.theme) {
        Scaffold(modifier = Modifier.fillMaxSize()) { padding ->
            Column(Modifier.padding(padding)) {
                PrimaryTabRow(selectedTabIndex = tab) {
                    TABS.forEachIndexed { i, title ->
                        Tab(selected = tab == i, onClick = { tab = i }, text = { Text(title) })
                    }
                }
                when (tab) {
                    0 -> SetupScreen(status, onEnableKeyboard, onSwitchKeyboard)
                    1 -> DictionaryScreen(engine)
                    else -> SettingsScreen(
                        settings = settings,
                        onChange = {
                            settings = it
                            store.save(it)
                        },
                        onResetLearned = engine::resetLearnedData,
                    )
                }
            }
        }
    }
}
