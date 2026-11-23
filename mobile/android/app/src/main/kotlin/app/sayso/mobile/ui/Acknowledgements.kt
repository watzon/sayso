package app.sayso.mobile.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import java.io.IOException

/** The asset with SCOWL's license, bundled beside the typing model. */
private const val SCOWL_NOTICE_ASSET = "SCOWL-Copyright.txt"

/**
 * Credits for the data the typing model is built from, as its licenses ask
 * (mobile/tools/sslmgen/DATA_SOURCES.md).
 */
@Composable
fun Acknowledgements() {
    val context = LocalContext.current
    val notice = remember {
        try {
            context.assets.open(SCOWL_NOTICE_ASSET).bufferedReader().use { it.readText() }
        } catch (e: IOException) {
            null // the fake engine's build ships no model and no notice
        }
    }
    var showNotice by rememberSaveable { mutableStateOf(false) }
    val body = MaterialTheme.typography.bodyMedium
    val muted = MaterialTheme.colorScheme.onSurfaceVariant

    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Text("Acknowledgements", style = MaterialTheme.typography.titleMedium)
        Text("The typing model is built from open data.", style = body, color = muted)
        Text(
            "Sentences from Tatoeba (https://tatoeba.org), licensed under CC BY 2.0 FR " +
                "(https://creativecommons.org/licenses/by/2.0/fr/).",
            style = body,
        )
        Text(
            "Word lists from SCOWL (Spell Checker Oriented Word Lists) by Kevin Atkinson, " +
                "http://wordlist.aspell.net/, used under its license.",
            style = body,
        )
        if (notice != null) {
            TextButton(onClick = { showNotice = !showNotice }) {
                Text(if (showNotice) "Hide the SCOWL license" else "Show the SCOWL license")
            }
            if (showNotice) Text(notice, style = MaterialTheme.typography.bodySmall, color = muted)
        }
    }
}
