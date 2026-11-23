package app.sayso.mobile.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.selection.selectableGroup
import androidx.compose.foundation.selection.toggleable
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.unit.dp
import app.sayso.mobile.asr.AsrModel
import app.sayso.mobile.asr.AsrModelKind
import app.sayso.mobile.asr.LOCAL_ASR_BUILT
import app.sayso.mobile.asr.ModelManager
import app.sayso.mobile.asr.ModelState
import app.sayso.mobile.engine.CloudAsrProvider
import app.sayso.mobile.engine.CloudStyleProvider
import app.sayso.mobile.engine.VoiceCore
import app.sayso.mobile.settings.VoiceEngineChoice
import app.sayso.mobile.settings.VoiceSettings
import app.sayso.mobile.settings.VoiceSettingsStore

@Composable
fun VoiceScreen(voice: VoiceCore) {
    val context = LocalContext.current
    val store = remember { VoiceSettingsStore(context) }
    val manager = remember { ModelManager.forApp(context) }
    val deviceRam = remember { ModelManager.deviceTotalRam(context) }
    var settings by remember { mutableStateOf(store.load()) }
    val styles = remember { voice.styles() }
    // Bumped when files change outside a download (delete, resume), so the model states are read again.
    var diskTick by remember { mutableIntStateOf(0) }

    fun update(change: (VoiceSettings) -> VoiceSettings) {
        settings = change(settings)
        store.save(settings)
    }

    OnResume {
        settings = store.load()
        diskTick++
    }

    Column(
        Modifier.verticalScroll(rememberScrollState()).padding(20.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        Text("Voice", style = MaterialTheme.typography.headlineMedium)
        Text(
            "Tap the mic on the Sayso keyboard to dictate. Choose how speech becomes text.",
            style = MaterialTheme.typography.bodyLarge,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )

        Section("Engine")
        Column(Modifier.selectableGroup()) {
            EngineOption(
                "On this phone",
                if (LOCAL_ASR_BUILT) "A downloaded model below. Until one is active, Android's speech recognition."
                else "This build has no local engine, so Android's speech recognition is used.",
                settings.engine == VoiceEngineChoice.AUTO,
            ) { update { it.copy(engine = VoiceEngineChoice.AUTO) } }
            EngineOption(
                "Android speech recognition",
                "The recognizer built into Android. It may send audio to its provider.",
                settings.engine == VoiceEngineChoice.SYSTEM,
            ) { update { it.copy(engine = VoiceEngineChoice.SYSTEM) } }
            EngineOption(
                "Cloud",
                if (settings.cloudAsrConfigured) "${asrProviderName(settings.cloud.asrProvider)}. Never used in password or private fields."
                else "Set up a speech provider under Cloud first. Until then, on this phone.",
                settings.engine == VoiceEngineChoice.CLOUD,
            ) { update { it.copy(engine = VoiceEngineChoice.CLOUD) } }
        }

        if (LOCAL_ASR_BUILT) {
            Section("Models", "Downloaded once over the internet, then run on this phone.")
            val active = remember(settings, diskTick) { manager.activeModels() }
            for (model in manager.catalog) {
                val download = ModelDownloads.status[model.id]
                val state = remember(diskTick, download) { manager.state(model) }
                ModelCard(
                    model = model,
                    state = state,
                    download = download,
                    active = model == active.final || model == active.live,
                    ramWarning = manager.ramAdvisory(model, deviceRam)?.let {
                        "This phone has ${formatRam(it.deviceRamBytes)} of memory; this model wants ${formatRam(model.recommendedRamBytes)}. It may be slow or stop."
                    },
                    onDownload = {
                        ModelDownloads.start(manager, model) {
                            // The first model of its kind becomes the active one.
                            val now = manager.activeModels()
                            val noneOfKind = if (model.kind == AsrModelKind.OFFLINE) now.final == null else now.live == null
                            if (noneOfKind) manager.setActive(model)
                            settings = store.load()
                            diskTick++
                        }
                    },
                    onCancel = { ModelDownloads.cancel(model) },
                    onDelete = {
                        manager.delete(model)
                        ModelDownloads.status.remove(model.id)
                        settings = store.load()
                        diskTick++
                    },
                    onSetActive = {
                        manager.setActive(model)
                        settings = store.load()
                    },
                )
            }
        }

        Section("Style", "How the text is cleaned up. Styles that rewrite text need a cloud style provider.")
        val selectedStyle = effectiveStyleId(styles, settings.styleId)
        Column(Modifier.selectableGroup()) {
            for (style in styles) {
                EngineOption(style.name, null, style.id == selectedStyle) { update { it.copy(styleId = style.id) } }
            }
        }

        Row(
            Modifier.fillMaxWidth().toggleable(
                value = settings.handsFree,
                role = Role.Switch,
                onValueChange = { on -> update { it.copy(handsFree = on) } },
            ).padding(vertical = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Column(Modifier.weight(1f)) {
                Text("Hands-free", style = MaterialTheme.typography.titleMedium)
                Text(
                    "Done also sends the message and starts listening again.",
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            Switch(checked = settings.handsFree, onCheckedChange = null)
        }

        Section("Cloud", "Off unless you choose a provider. Keys stay on this phone and are never backed up.")
        CloudCard(
            title = "Speech to text",
            providers = CloudAsrProvider.entries,
            provider = settings.cloud.asrProvider,
            providerName = ::asrProviderName,
            hints = asrHints(settings.cloud.asrProvider),
            off = CloudAsrProvider.OFF,
            apiKey = settings.cloud.asrApiKey,
            baseUrl = settings.cloud.asrBaseUrl,
            model = settings.cloud.asrModel,
            onProvider = { p -> update { it.copy(cloud = it.cloud.copy(asrProvider = p)) } },
            onChange = { key, url, m -> update { it.copy(cloud = it.cloud.copy(asrApiKey = key, asrBaseUrl = url, asrModel = m)) } },
        )
        CloudCard(
            title = "Styles",
            providers = CloudStyleProvider.entries,
            provider = settings.cloud.styleProvider,
            providerName = ::styleProviderName,
            hints = styleHints(settings.cloud.styleProvider),
            off = CloudStyleProvider.OFF,
            apiKey = settings.cloud.styleApiKey,
            baseUrl = settings.cloud.styleBaseUrl,
            model = settings.cloud.styleModel,
            onProvider = { p -> update { it.copy(cloud = it.cloud.copy(styleProvider = p)) } },
            onChange = { key, url, m -> update { it.copy(cloud = it.cloud.copy(styleApiKey = key, styleBaseUrl = url, styleModel = m)) } },
        )
    }
}

@Composable
private fun Section(title: String, hint: String? = null) {
    Column(Modifier.padding(top = 8.dp)) {
        Text(title, style = MaterialTheme.typography.titleLarge)
        if (hint != null) Text(hint, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

@Composable
private fun EngineOption(title: String, body: String?, selected: Boolean, onSelect: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().selectable(selected = selected, role = Role.RadioButton, onClick = onSelect).padding(vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        RadioButton(selected = selected, onClick = null)
        Column(Modifier.padding(start = 12.dp)) {
            Text(title, style = MaterialTheme.typography.bodyLarge)
            if (body != null) Text(body, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
}

@Composable
private fun ModelCard(
    model: AsrModel,
    state: ModelState,
    download: DownloadStatus?,
    active: Boolean,
    ramWarning: String?,
    onDownload: () -> Unit,
    onCancel: () -> Unit,
    onDelete: () -> Unit,
    onSetActive: () -> Unit,
) {
    val badge = modelBadge(state, download, active)
    var confirmDelete by remember { mutableStateOf(false) }
    Card(colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceContainerLow)) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(model.name, style = MaterialTheme.typography.titleMedium, modifier = Modifier.weight(1f))
                Badge(badge)
            }
            Text(
                "${modelRole(model)} · ${formatBytes(model.downloadBytes)}",
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Text("License: ${model.license}", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.outline)
            if (ramWarning != null) Text(ramWarning, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.error)
            when {
                download is DownloadStatus.Running -> {
                    val pct = downloadPercent(download.downloadedBytes, download.totalBytes)
                    LinearProgressIndicator(progress = { pct / 100f }, modifier = Modifier.fillMaxWidth())
                    Text(
                        "${formatBytes(download.downloadedBytes)} of ${formatBytes(download.totalBytes)} · $pct%",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                badge == ModelBadge.PAUSED && state is ModelState.Partial -> Text(
                    "${formatBytes(state.downloadedBytes)} of ${formatBytes(state.totalBytes)} downloaded.",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                download is DownloadStatus.Failed -> Text(
                    "Download failed: ${download.message}",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.error,
                )
            }
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                when (badge) {
                    ModelBadge.DOWNLOADING -> OutlinedButton(onClick = onCancel) { Text("Cancel") }
                    ModelBadge.NOT_DOWNLOADED -> Button(onClick = onDownload) { Text("Download") }
                    ModelBadge.PAUSED, ModelBadge.FAILED -> Button(onClick = onDownload) {
                        Text(if (state is ModelState.Partial) "Resume" else "Try again")
                    }
                    ModelBadge.DOWNLOADED -> Button(onClick = onSetActive) { Text("Set active") }
                    ModelBadge.ACTIVE -> {}
                }
                if (badge != ModelBadge.DOWNLOADING && state != ModelState.NotInstalled) {
                    TextButton(onClick = { confirmDelete = true }) { Text("Delete") }
                }
            }
        }
    }
    if (confirmDelete) {
        AlertDialog(
            onDismissRequest = { confirmDelete = false },
            title = { Text("Delete ${model.name}?") },
            text = { Text("This frees ${formatBytes(model.downloadBytes)}. You can download it again later.") },
            confirmButton = {
                TextButton(onClick = {
                    confirmDelete = false
                    onDelete()
                }) { Text("Delete", color = MaterialTheme.colorScheme.error) }
            },
            dismissButton = { TextButton(onClick = { confirmDelete = false }) { Text("Keep") } },
        )
    }
}

@Composable
private fun Badge(badge: ModelBadge) {
    val (bg, fg) = when (badge) {
        ModelBadge.ACTIVE -> MaterialTheme.colorScheme.primary to MaterialTheme.colorScheme.onPrimary
        ModelBadge.FAILED -> MaterialTheme.colorScheme.errorContainer to MaterialTheme.colorScheme.onErrorContainer
        ModelBadge.DOWNLOADED, ModelBadge.DOWNLOADING -> MaterialTheme.colorScheme.primaryContainer to MaterialTheme.colorScheme.onPrimaryContainer
        else -> MaterialTheme.colorScheme.surfaceVariant to MaterialTheme.colorScheme.onSurfaceVariant
    }
    Surface(color = bg, contentColor = fg, shape = MaterialTheme.shapes.small) {
        Text(badge.label, style = MaterialTheme.typography.labelMedium, modifier = Modifier.padding(horizontal = 8.dp, vertical = 3.dp))
    }
}

@Composable
private fun <P> CloudCard(
    title: String,
    providers: List<P>,
    provider: P,
    providerName: (P) -> String,
    hints: CloudFieldHints,
    off: P,
    apiKey: String,
    baseUrl: String,
    model: String,
    onProvider: (P) -> Unit,
    onChange: (apiKey: String, baseUrl: String, model: String) -> Unit,
) {
    var menu by remember { mutableStateOf(false) }
    var showKey by remember { mutableStateOf(false) }
    Card(colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceContainerLow)) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(title, style = MaterialTheme.typography.titleMedium, modifier = Modifier.weight(1f))
                Box {
                    OutlinedButton(onClick = { menu = true }) { Text(providerName(provider)) }
                    DropdownMenu(expanded = menu, onDismissRequest = { menu = false }) {
                        for (p in providers) {
                            DropdownMenuItem(text = { Text(providerName(p)) }, onClick = {
                                menu = false
                                onProvider(p)
                            })
                        }
                    }
                }
            }
            if (provider != off) {
                OutlinedTextField(
                    value = apiKey,
                    onValueChange = { onChange(it.trim(), baseUrl, model) },
                    label = { Text(if (hints.needsBaseUrl) "API key (if the server needs one)" else "API key") },
                    singleLine = true,
                    visualTransformation = if (showKey) VisualTransformation.None else PasswordVisualTransformation(),
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password, autoCorrectEnabled = false),
                    trailingIcon = { TextButton(onClick = { showKey = !showKey }) { Text(if (showKey) "Hide" else "Show") } },
                    modifier = Modifier.fillMaxWidth(),
                )
                OutlinedTextField(
                    value = baseUrl,
                    onValueChange = { onChange(apiKey, it.trim(), model) },
                    label = { Text(if (hints.needsBaseUrl) "Server URL" else "Server URL (optional)") },
                    placeholder = { Text(hints.baseUrl, color = MaterialTheme.colorScheme.outline) },
                    singleLine = true,
                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri, autoCorrectEnabled = false),
                    modifier = Modifier.fillMaxWidth(),
                )
                OutlinedTextField(
                    value = model,
                    onValueChange = { onChange(apiKey, baseUrl, it.trim()) },
                    label = { Text(if (hints.needsModel) "Model" else "Model (optional)") },
                    placeholder = { Text(hints.model, color = MaterialTheme.colorScheme.outline) },
                    singleLine = true,
                    keyboardOptions = KeyboardOptions(autoCorrectEnabled = false),
                    modifier = Modifier.fillMaxWidth(),
                )
            }
        }
    }
}
