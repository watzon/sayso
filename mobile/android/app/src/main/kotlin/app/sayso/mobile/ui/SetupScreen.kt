package app.sayso.mobile.ui

import android.Manifest
import android.app.Activity
import android.content.Context
import android.content.ContextWrapper
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.provider.Settings
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp

@Composable
fun SetupScreen(status: KeyboardStatus, onEnableKeyboard: () -> Unit, onSwitchKeyboard: () -> Unit) {
    var playground by rememberSaveable { mutableStateOf("") }
    val context = LocalContext.current
    var micGranted by remember { mutableStateOf(hasMic(context)) }
    // Survives rotation, not process death: after a restart one more tap asks Android again,
    // which answers at once when the denial is permanent.
    var micAsked by rememberSaveable { mutableStateOf(false) }
    var micRationale by remember { mutableStateOf(false) }
    val micLauncher = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        micGranted = granted
        micAsked = true
        micRationale = context.findActivity()?.shouldShowRequestPermissionRationale(Manifest.permission.RECORD_AUDIO) == true
    }
    OnResume {
        micGranted = hasMic(context)
        micRationale = context.findActivity()?.shouldShowRequestPermissionRationale(Manifest.permission.RECORD_AUDIO) == true
    }
    val mic = micStep(micGranted, micAsked, micRationale)
    Column(
        Modifier.verticalScroll(rememberScrollState()).padding(20.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        Text("Sayso Keyboard", style = MaterialTheme.typography.headlineMedium)
        Text(
            "Set it up in four steps.",
            style = MaterialTheme.typography.bodyLarge,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Step(
            number = 1,
            title = "Enable Sayso Keyboard",
            body = "Turn on Sayso Keyboard in the system list of keyboards. Android warns that a keyboard can read what you type; Sayso keeps it on this phone.",
            done = status.enabled,
            action = "Open keyboard settings",
            onAction = onEnableKeyboard,
        )
        Step(
            number = 2,
            title = "Switch to Sayso",
            body = "Choose Sayso Keyboard as the keyboard you type with.",
            done = status.selected,
            action = "Choose keyboard",
            onAction = onSwitchKeyboard,
            enabled = status.enabled,
        )
        Step(
            number = 3,
            title = "Allow the microphone",
            body = when (mic) {
                MicStep.BLOCKED -> "Microphone access is turned off for Sayso. Open its app settings, choose Permissions, and allow the microphone."
                else -> "Sayso needs the microphone for voice dictation from the keyboard. Speech is recognized on this phone unless you turn on a cloud engine."
            },
            done = mic == MicStep.GRANTED,
            action = if (mic == MicStep.BLOCKED) "Open app settings" else "Allow microphone",
            onAction = {
                if (mic == MicStep.BLOCKED) {
                    context.startActivity(
                        Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.fromParts("package", context.packageName, null)),
                    )
                } else {
                    micLauncher.launch(Manifest.permission.RECORD_AUDIO)
                }
            },
        )
        Card(colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceContainerLow)) {
            Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                StepTitle(4, "Typing playground", done = false)
                OutlinedTextField(
                    value = playground,
                    onValueChange = { playground = it },
                    modifier = Modifier.fillMaxWidth().heightIn(min = 120.dp),
                    placeholder = { Text("Type here to try the keyboard") },
                )
            }
        }
    }
}

@Composable
private fun Step(number: Int, title: String, body: String, done: Boolean, action: String, onAction: () -> Unit, enabled: Boolean = true) {
    Card(colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceContainerLow)) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            StepTitle(number, title, done)
            Text(body, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
            if (!done) Button(onClick = onAction, enabled = enabled) { Text(action) }
        }
    }
}

@Composable
private fun StepTitle(number: Int, title: String, done: Boolean) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        Surface(
            shape = CircleShape,
            color = if (done) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.surfaceVariant,
            contentColor = if (done) MaterialTheme.colorScheme.onPrimary else MaterialTheme.colorScheme.onSurface,
            modifier = Modifier.size(28.dp),
        ) {
            Row(horizontalArrangement = Arrangement.Center, verticalAlignment = Alignment.CenterVertically) {
                Text(if (done) "✓" else number.toString(), style = MaterialTheme.typography.labelLarge)
            }
        }
        Spacer(Modifier.width(12.dp))
        Text(title, style = MaterialTheme.typography.titleMedium, modifier = Modifier.weight(1f))
        if (done) Text("Done", style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.primary)
    }
}

enum class MicStep {
    GRANTED,

    /** Not granted, and asking shows Android's prompt. */
    ASK,

    /** Denied for good: Android shows no prompt any more, only app settings can grant it. */
    BLOCKED,
}

/**
 * Before the first request Android reports no rationale, the same as after a
 * permanent denial, so only a denial seen in this session counts as blocked.
 */
fun micStep(granted: Boolean, askedBefore: Boolean, showRationale: Boolean): MicStep = when {
    granted -> MicStep.GRANTED
    askedBefore && !showRationale -> MicStep.BLOCKED
    else -> MicStep.ASK
}

private fun hasMic(context: Context) =
    context.checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED

private tailrec fun Context.findActivity(): Activity? = when (this) {
    is Activity -> this
    is ContextWrapper -> baseContext.findActivity()
    else -> null
}
