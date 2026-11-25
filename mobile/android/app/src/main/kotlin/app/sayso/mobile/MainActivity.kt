package app.sayso.mobile

import android.Manifest
import android.content.ComponentName
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Bundle
import android.provider.Settings
import android.view.inputmethod.InputMethodManager
import androidx.activity.ComponentActivity
import androidx.activity.result.contract.ActivityResultContracts
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.runtime.mutableStateOf
import app.sayso.mobile.engine.SaysoEngine
import app.sayso.mobile.settings.KeyboardSettingsStore
import app.sayso.mobile.telemetry.Telemetry
import app.sayso.mobile.ui.KeyboardStatus
import app.sayso.mobile.ui.SaysoApp

class MainActivity : ComponentActivity() {
    private val status = mutableStateOf(KeyboardStatus(enabled = false, selected = false))

    private val micPermission = registerForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        // Denied for good: Android shows no prompt any more, so open the app's settings instead.
        if (!granted && !shouldShowRequestPermissionRationale(Manifest.permission.RECORD_AUDIO)) {
            startActivity(Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.fromParts("package", packageName, null)))
        }
    }

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
        if (savedInstanceState == null) requestMicIfAsked(intent)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        requestMicIfAsked(intent)
    }

    /** The keyboard opens the app with [EXTRA_REQUEST_MIC] when its mic is tapped without the permission. */
    private fun requestMicIfAsked(intent: Intent?) {
        if (intent?.getBooleanExtra(EXTRA_REQUEST_MIC, false) != true) return
        intent.removeExtra(EXTRA_REQUEST_MIC)
        if (checkSelfPermission(Manifest.permission.RECORD_AUDIO) != PackageManager.PERMISSION_GRANTED) {
            micPermission.launch(Manifest.permission.RECORD_AUDIO)
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
        // Like the desktop's end of onboarding: nothing is sent before the keyboard is set up.
        if (enabled) Telemetry.get(this).launched()
    }

    companion object {
        const val EXTRA_REQUEST_MIC = "app.sayso.mobile.extra.REQUEST_MIC"
    }
}
