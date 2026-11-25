package app.sayso.mobile.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import app.sayso.mobile.engine.AutocorrectMode
import app.sayso.mobile.settings.KeyboardSettings

@Composable
fun SettingsScreen(
    settings: KeyboardSettings,
    onChange: (KeyboardSettings) -> Unit,
    onResetLearned: () -> Unit,
    telemetry: Boolean,
    onTelemetry: (Boolean) -> Unit,
) {
    var confirmReset by rememberSaveable { mutableStateOf(false) }
    var resetDone by rememberSaveable { mutableStateOf(false) }

    Column(
        Modifier.verticalScroll(rememberScrollState()).padding(20.dp),
        verticalArrangement = Arrangement.spacedBy(20.dp),
    ) {
        Toggle("Haptic feedback", "A short vibration on each key press.", settings.haptics) {
            onChange(settings.copy(haptics = it))
        }
        Choice(
            title = "Autocorrect",
            hint = "Strict corrects only when one word is a clear winner.",
            options = AutocorrectMode.entries,
            selected = settings.autocorrect,
            label = { it.name.lowercase().replaceFirstChar(Char::uppercaseChar) },
        ) { onChange(settings.copy(autocorrect = it)) }
        Toggle("Next-word prediction", "Suggest a word after you finish one.", settings.nextWordPrediction) {
            onChange(settings.copy(nextWordPrediction = it))
        }
        Toggle("Double-space period", "Tap space twice after a word to type a period and a space.", settings.doubleSpacePeriod) {
            onChange(settings.copy(doubleSpacePeriod = it))
        }
        Toggle(
            "Developer diagnostics",
            "Draw hit targets and learned offsets on the keyboard, with candidate scores above it.",
            settings.diagnostics,
        ) { onChange(settings.copy(diagnostics = it)) }
        TelemetryToggle(telemetry, onTelemetry)

        Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Text("Learned data", style = MaterialTheme.typography.titleMedium)
            Text(
                if (resetDone) "Learned data cleared." else "Clears what the keyboard learned about where you touch and what you type. Personal words and replacements stay.",
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            OutlinedButton(
                onClick = { confirmReset = true },
                colors = ButtonDefaults.outlinedButtonColors(contentColor = MaterialTheme.colorScheme.error),
            ) { Text("Reset learned data") }
        }

        Acknowledgements()
    }

    if (confirmReset) {
        AlertDialog(
            onDismissRequest = { confirmReset = false },
            title = { Text("Reset learned data?") },
            text = { Text("The keyboard forgets your touch offsets and typing history. This cannot be undone.") },
            confirmButton = {
                TextButton(
                    onClick = {
                        onResetLearned()
                        confirmReset = false
                        resetDone = true
                    },
                    colors = ButtonDefaults.textButtonColors(contentColor = MaterialTheme.colorScheme.error),
                ) { Text("Reset") }
            },
            dismissButton = { TextButton(onClick = { confirmReset = false }) { Text("Cancel") } },
        )
    }
}

/** The telemetry switch, the same in Setup and Settings. */
@Composable
fun TelemetryToggle(checked: Boolean, onChange: (Boolean) -> Unit) = Toggle(
    "Anonymous usage telemetry",
    "Tells TelemetryDeck when Sayso starts and when a dictation finishes, with the version, system, model, and kind of style. Never your words, audio, or prompts.",
    checked,
    onChange,
)

@Composable
private fun Toggle(title: String, hint: String, checked: Boolean, onChange: (Boolean) -> Unit) {
    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f).padding(end = 12.dp)) {
            Text(title, style = MaterialTheme.typography.titleMedium)
            Text(hint, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        Switch(checked = checked, onCheckedChange = onChange)
    }
}

@Composable
private fun <T> Choice(title: String, hint: String?, options: List<T>, selected: T, label: (T) -> String, onSelect: (T) -> Unit) {
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(title, style = MaterialTheme.typography.titleMedium)
        if (hint != null) Text(hint, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        SingleChoiceSegmentedButtonRow(Modifier.fillMaxWidth()) {
            options.forEachIndexed { i, option ->
                SegmentedButton(
                    selected = option == selected,
                    onClick = { onSelect(option) },
                    shape = SegmentedButtonDefaults.itemShape(i, options.size),
                ) { Text(label(option)) }
            }
        }
    }
}
