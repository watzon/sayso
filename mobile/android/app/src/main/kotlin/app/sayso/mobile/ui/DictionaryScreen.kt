package app.sayso.mobile.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Button
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import app.sayso.mobile.engine.Replacement
import app.sayso.mobile.engine.TypingEngine

@Composable
fun DictionaryScreen(engine: TypingEngine) {
    var words by remember { mutableStateOf(engine.personalWords()) }
    var replacements by remember { mutableStateOf(engine.replacements()) }
    var newWord by rememberSaveable { mutableStateOf("") }
    var shortcut by rememberSaveable { mutableStateOf("") }
    var expansion by rememberSaveable { mutableStateOf("") }

    LazyColumn(Modifier.padding(horizontal = 20.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        item { SectionTitle("Personal words", "Words the keyboard always accepts and suggests, like names.") }
        item {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedTextField(newWord, { newWord = it }, Modifier.weight(1f), label = { Text("Word") }, singleLine = true)
                Button(
                    onClick = {
                        engine.addPersonalWord(newWord)
                        words = engine.personalWords()
                        newWord = ""
                    },
                    enabled = newWord.isNotBlank(),
                ) { Text("Add") }
            }
        }
        if (words.isEmpty()) item { EmptyLine("No personal words yet.") }
        items(words, key = { "w:$it" }) { word ->
            EntryRow(word) {
                engine.removePersonalWord(word)
                words = engine.personalWords()
            }
        }

        item { SectionTitle("Replacements", "Type the shortcut and a space to get the full text.") }
        item {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedTextField(shortcut, { shortcut = it }, Modifier.weight(0.8f), label = { Text("Shortcut") }, singleLine = true)
                OutlinedTextField(expansion, { expansion = it }, Modifier.weight(1.2f), label = { Text("Text") }, singleLine = true)
            }
        }
        item {
            Button(
                onClick = {
                    engine.putReplacement(Replacement(shortcut, expansion))
                    replacements = engine.replacements()
                    shortcut = ""
                    expansion = ""
                },
                enabled = shortcut.isNotBlank() && expansion.isNotBlank(),
            ) { Text("Add replacement") }
        }
        if (replacements.isEmpty()) item { EmptyLine("No replacements yet.") }
        items(replacements, key = { "r:${it.shortcut}" }) { r ->
            EntryRow("${r.shortcut}  →  ${r.expansion}") {
                engine.removeReplacement(r.shortcut)
                replacements = engine.replacements()
            }
        }
    }
}

@Composable
private fun SectionTitle(title: String, hint: String) {
    Text(title, style = MaterialTheme.typography.titleLarge, modifier = Modifier.padding(top = 20.dp))
    Text(hint, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
}

@Composable
private fun EmptyLine(text: String) {
    Text(text, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.padding(vertical = 8.dp))
}

@Composable
private fun EntryRow(text: String, onRemove: () -> Unit) {
    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
        Text(text, style = MaterialTheme.typography.bodyLarge, modifier = Modifier.weight(1f))
        TextButton(onClick = onRemove) { Text("Remove") }
    }
    HorizontalDivider(color = MaterialTheme.colorScheme.outlineVariant)
}
