package app.sayso.mobile.ui

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.text.format.DateFormat
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.sayso.mobile.engine.VoiceCore
import app.sayso.mobile.engine.VoiceHistoryEntry
import kotlinx.coroutines.delay
import java.time.ZoneId
import java.util.Locale

@Composable
fun HistoryScreen(voice: VoiceCore) {
    val context = LocalContext.current
    var entries by remember { mutableStateOf(voice.history()) }
    var now by remember { mutableLongStateOf(System.currentTimeMillis()) }
    var confirmClear by remember { mutableStateOf(false) }
    var expanded by rememberSaveable { mutableStateOf(setOf<Long>()) }
    var copiedId by remember { mutableStateOf<Long?>(null) }

    OnResume {
        entries = voice.history()
        now = System.currentTimeMillis()
    }
    LaunchedEffect(copiedId) {
        if (copiedId != null) {
            delay(2000)
            copiedId = null
        }
    }

    val is24Hour = DateFormat.is24HourFormat(context)
    val zone = ZoneId.systemDefault()
    val locale = Locale.getDefault()

    LazyColumn(
        contentPadding = PaddingValues(horizontal = 20.dp, vertical = 20.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        item {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text("History", style = MaterialTheme.typography.headlineMedium, modifier = Modifier.weight(1f))
                if (entries.isNotEmpty()) TextButton(onClick = { confirmClear = true }) { Text("Clear all") }
            }
            Text(
                "Your recent dictations, newest first. Nothing is saved from password or private fields.",
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        if (entries.isEmpty()) {
            item {
                Text(
                    "No dictations yet. Tap the mic on the Sayso keyboard and speak; what you dictate shows up here.",
                    style = MaterialTheme.typography.bodyLarge,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(vertical = 24.dp),
                )
            }
        }
        items(entries, key = { it.id }) { entry ->
            HistoryCard(
                entry = entry,
                whenText = formatWhen(entry.timestampMs, now, zone, locale, is24Hour),
                expanded = entry.id in expanded,
                copied = copiedId == entry.id,
                onToggle = { expanded = if (entry.id in expanded) expanded - entry.id else expanded + entry.id },
                onCopy = {
                    copyToClipboard(context, entry.finalText)
                    copiedId = entry.id
                },
                onDelete = {
                    voice.deleteHistory(entry.id)
                    entries = voice.history()
                },
            )
        }
    }

    if (confirmClear) {
        AlertDialog(
            onDismissRequest = { confirmClear = false },
            title = { Text("Clear all history?") },
            text = { Text("This deletes all ${entries.size} dictations from this phone. It cannot be undone.") },
            confirmButton = {
                TextButton(onClick = {
                    voice.clearHistory()
                    entries = voice.history()
                    expanded = emptySet()
                    confirmClear = false
                }) { Text("Clear all", color = MaterialTheme.colorScheme.error) }
            },
            dismissButton = { TextButton(onClick = { confirmClear = false }) { Text("Keep") } },
        )
    }
}

@Composable
private fun HistoryCard(
    entry: VoiceHistoryEntry,
    whenText: String,
    expanded: Boolean,
    copied: Boolean,
    onToggle: () -> Unit,
    onCopy: () -> Unit,
    onDelete: () -> Unit,
) {
    Card(
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceContainerLow),
        modifier = Modifier.fillMaxWidth(),
    ) {
        Column(Modifier.clickable(onClick = onToggle).padding(start = 16.dp, end = 8.dp, top = 14.dp, bottom = 4.dp)) {
            Text(
                "$whenText · ${formatDuration(entry.durationMs)}",
                style = MaterialTheme.typography.labelLarge,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(end = 8.dp),
            )
            Text(
                entry.finalText.ifEmpty { "(empty)" },
                style = MaterialTheme.typography.bodyLarge,
                maxLines = if (expanded) Int.MAX_VALUE else 3,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.padding(top = 6.dp, end = 8.dp),
            )
            if (expanded) {
                Text(
                    "Raw transcript",
                    style = MaterialTheme.typography.labelMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(top = 12.dp),
                )
                Text(
                    entry.rawText.ifEmpty { "(nothing)" },
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.padding(top = 2.dp, end = 8.dp),
                )
            }
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    "${entry.engineId} · ${entry.styleId}",
                    style = MaterialTheme.typography.labelMedium,
                    color = MaterialTheme.colorScheme.outline,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f),
                )
                TextButton(onClick = onCopy) { Text(if (copied) "Copied" else "Copy") }
                TextButton(onClick = onDelete) { Text("Delete") }
            }
        }
    }
}

private fun copyToClipboard(context: Context, text: String) {
    val clipboard = context.getSystemService(ClipboardManager::class.java)
    clipboard.setPrimaryClip(ClipData.newPlainText("Sayso dictation", text))
}
