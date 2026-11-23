package app.sayso.mobile

import android.content.ComponentName
import android.content.Intent
import android.os.Bundle
import android.provider.Settings
import android.view.inputmethod.InputMethodManager
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.runtime.mutableStateOf
import app.sayso.mobile.engine.SaysoEngine
import app.sayso.mobile.settings.KeyboardSettingsStore
import app.sayso.mobile.ui.KeyboardStatus
import app.sayso.mobile.ui.SaysoApp

class MainActivity : ComponentActivity() {
    private val status = mutableStateOf(KeyboardStatus(enabled = false, selected = false))

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        val store = KeyboardSettingsStore(this)
        val engine = SaysoEngine.get(this)
        setContent {
            SaysoApp(
                status = status.value,
                store = store,
                engine = engine,
                onEnableKeyboard = { startActivity(Intent(Settings.ACTION_INPUT_METHOD_SETTINGS)) },
                onSwitchKeyboard = { getSystemService(InputMethodManager::class.java).showInputMethodPicker() },
            )
        }
    }

    override fun onResume() {
        super.onResume()
        refreshStatus()
    }

    // The keyboard picker is a dialog, so returning from it changes focus, not the lifecycle.
    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (hasFocus) refreshStatus()
    }

    private fun refreshStatus() {
        val imm = getSystemService(InputMethodManager::class.java)
        val enabled = imm.enabledInputMethodList.any { it.packageName == packageName }
        val current = Settings.Secure.getString(contentResolver, Settings.Secure.DEFAULT_INPUT_METHOD)
        val selected = current?.let { ComponentName.unflattenFromString(it)?.packageName } == packageName
        status.value = KeyboardStatus(enabled, selected)
    }
}
