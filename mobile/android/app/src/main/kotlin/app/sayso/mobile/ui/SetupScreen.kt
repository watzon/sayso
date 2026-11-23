package app.sayso.mobile.ui

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
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp

@Composable
fun SetupScreen(status: KeyboardStatus, onEnableKeyboard: () -> Unit, onSwitchKeyboard: () -> Unit) {
    var playground by rememberSaveable { mutableStateOf("") }
    Column(
        Modifier.verticalScroll(rememberScrollState()).padding(20.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        Text("Sayso Keyboard", style = MaterialTheme.typography.headlineMedium)
        Text(
            "Set it up in three steps.",
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
        Card(colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceContainerLow)) {
            Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                StepTitle(3, "Typing playground", done = false)
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
