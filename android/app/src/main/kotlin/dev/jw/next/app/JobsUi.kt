package dev.jw.next.app

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.defaultMinSize
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.dp
import dev.jw.next.client.Aviso
import dev.jw.next.client.AvisosTab
import dev.jw.next.client.ClientState
import dev.jw.next.client.JobsMark
import dev.jw.next.client.Navigator
import dev.jw.next.client.ago
import dev.jw.next.client.jobsOf
import dev.jw.next.client.kindLabel
import dev.jw.next.client.said
import dev.jw.next.client.time
import dev.jw.next.client.title
import dev.jw.next.client.took
import dev.jw.next.contract.Job
import dev.jw.next.contract.JobKind
import dev.jw.next.contract.JobState
import dev.jw.next.contract.Node
import kotlinx.coroutines.delay

private val Gutter = 16.dp

/** A clock that ticks while [active]: the spinners and the time a job has run move with it. */
@Composable
internal fun rememberClock(active: Boolean): Long {
    var now by remember { mutableLongStateOf(System.currentTimeMillis()) }
    LaunchedEffect(active) {
        while (active) {
            now = System.currentTimeMillis()
            delay(110)
        }
    }
    return now
}

internal fun spinOf(now: Long): String = Spin[((now / 110) % Spin.size).toInt()]

/** The mark beside the bell: a spinner while jobs run, then a green or red dot for what ended and was not shown (REQ-659). */
@Composable
internal fun BoxScope.JobsDot(mark: JobsMark, now: Long) {
    val (text, color) = when (mark) {
        JobsMark.None -> return
        is JobsMark.Running -> spinOf(now) to Palette.blue
        is JobsMark.Ended -> "●" to if (mark.failed) Palette.red else Palette.green
    }
    Mono(text, color, 11, bold = true, modifier = Modifier.align(Alignment.BottomEnd).padding(bottom = 7.dp, end = 5.dp))
}

/** What the bell says to a screen reader about the jobs. */
internal fun JobsMark.said(): String = when (this) {
    JobsMark.None -> ""
    is JobsMark.Running -> ", $count trabajos corriendo"
    is JobsMark.Ended -> if (failed) ", un trabajo falló" else ", $count trabajos terminaron"
}

/** The body of Avisos: the two tabs and what the one that shows lists (REQ-659). */
@Composable
internal fun AvisosBody(
    session: Node.Session,
    alerts: List<Aviso>,
    state: ClientState,
    nav: Navigator,
    set: (ClientState) -> Unit,
    now: Long,
    open: (Job) -> Unit,
) {
    val jobs = jobsOf(session)
    Column(Modifier.fillMaxSize()) {
        Row(Modifier.fillMaxWidth().padding(horizontal = Gutter, vertical = 8.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            TabButton("alertas", state.avisosTab == AvisosTab.Alertas, Modifier.weight(1f), { Mono(if (alerts.isEmpty()) "" else " ${alerts.size}", Palette.yellow, 12) }) {
                set(nav.openAvisos(state, AvisosTab.Alertas))
            }
            TabButton("trabajos", state.avisosTab == AvisosTab.Trabajos, Modifier.weight(1f), {
                when {
                    jobs.running.isNotEmpty() -> Mono(" ${spinOf(now)} ${jobs.running.size}", Palette.blue, 12)
                    jobs.ended.isNotEmpty() -> Mono(" ${jobs.ended.size}", Palette.dim, 12)
                }
            }) { set(nav.openAvisos(state, AvisosTab.Trabajos)) }
        }
        when (state.avisosTab) {
            AvisosTab.Alertas -> AlertsList(alerts, nav, state, set)
            AvisosTab.Trabajos -> JobsList(jobs.running, jobs.ended, now, open)
        }
    }
}

@Composable
private fun TabButton(label: String, selected: Boolean, modifier: Modifier, count: @Composable () -> Unit, click: () -> Unit) {
    Row(
        modifier.defaultMinSize(minHeight = 48.dp)
            .background(if (selected) Palette.sel else Palette.panel, RoundedCornerShape(10.dp))
            .clickable(role = Role.Tab, onClick = click),
        horizontalArrangement = Arrangement.Center,
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Mono(label, if (selected) Palette.fg else Palette.dim, 13, bold = selected)
        count()
    }
}

@Composable
private fun AlertsList(items: List<Aviso>, nav: Navigator, state: ClientState, set: (ClientState) -> Unit) {
    if (items.isEmpty()) {
        Box(Modifier.fillMaxSize().padding(Gutter), contentAlignment = Alignment.Center) {
            Mono("Nada espera algo de ti. Cuando algo pregunte o pida permiso, aparece aquí.", Palette.dim, 13, lines = 4)
        }
        return
    }
    AvisosList(items, nav, state, set)
}

@Composable
private fun JobsList(running: List<Job>, ended: List<Job>, now: Long, open: (Job) -> Unit) {
    LazyColumn(Modifier.fillMaxSize()) {
        item { Section("corriendo · ${running.size}") }
        if (running.isEmpty()) item { Mono("nada corre ahora", Palette.dim, 12, modifier = Modifier.padding(horizontal = Gutter, vertical = 8.dp)) }
        items(running, key = { it.id }) { job -> JobRow(job, now) { open(job) } }
        item { Section("terminados · ${ended.size}") }
        if (ended.isEmpty()) item { Mono("lo que termine aparece aquí hasta que lo descartes", Palette.dim, 12, lines = 2, modifier = Modifier.padding(horizontal = Gutter, vertical = 8.dp)) }
        items(ended, key = { it.id }) { job -> JobRow(job, now) { open(job) } }
    }
}

@Composable
private fun JobRow(job: Job, now: Long, open: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().defaultMinSize(minHeight = 56.dp).clickable(onClick = open).padding(horizontal = Gutter, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        val (glyph, color) = when (job.state) {
            JobState.Running -> spinOf(now) to Palette.blue
            JobState.Done -> "✓" to Palette.green
            JobState.Failed -> "✗" to Palette.red
        }
        Mono(glyph, color, 15, bold = true, modifier = Modifier.width(18.dp))
        Column(Modifier.weight(1f)) {
            Mono(job.title(), Palette.fg, 14, bold = true)
            Mono(job.kindLabel() + if (job.state == JobState.Failed) " · falló" else "", Palette.dim, 11)
        }
        Column(horizontalAlignment = Alignment.End) {
            Mono(took(job.time(now)), Palette.dim, 11)
            job.ago(now)?.let { Mono("hace ${took(it)}", Palette.dim, 11) }
        }
    }
    Hairline()
}

/** What a tap on a job opens: its result or its whole error, and what can be done with it (REQ-659). */
@Composable
internal fun BoxScope.JobSheet(
    source: AppSource,
    session: Node.Session,
    job: Job,
    state: ClientState,
    nav: Navigator,
    set: (ClientState) -> Unit,
    now: Long,
    dismiss: () -> Unit,
) {
    Sheet(dismiss) {
        Mono(job.title(), Palette.fg, 14, bold = true, lines = 2)
        when (job.state) {
            JobState.Running -> Mono("${spinOf(now)} corriendo · ${took(job.time(now))}", Palette.blue, 12)
            JobState.Done -> Mono("✓ terminado en ${took(job.time(now))}", Palette.green, 12)
            JobState.Failed -> Mono("✗ falló a los ${took(job.time(now))}", Palette.red, 12)
        }
        if (job.state == JobState.Failed) {
            Mono(job.said(), Palette.red, 12, lines = 10)
            if (job.kind == JobKind.RemoveWorkspace) Mono("Con cambios sin guardar, el terminal del PC lo quita con force.", Palette.dim, 12, lines = 3)
        } else {
            Mono(job.said(), Palette.dim, 12, lines = 3)
        }
        if (job.state == JobState.Running && job.cancellable) {
            SheetAction("×", "cancelar", Palette.red) {
                source.cancel(job.workspace) { r -> r.onFailure { source.say("✗ ${it.message ?: "el PC no pudo cancelar el trabajo"}") } }
                dismiss()
            }
        }
        val there = session.workspaces.any { it.id == job.workspace }
        if (there && (job.kind != JobKind.RemoveWorkspace || job.state == JobState.Failed)) {
            SheetAction("›", "ir al workspace") {
                dismiss()
                set(nav.openWorkspace(state, job.workspace))
            }
        }
        if (job.state != JobState.Running) {
            SheetAction("−", "descartar") {
                source.dismissJob(job.id) { r -> r.onFailure { source.say("✗ ${it.message ?: "el PC no pudo descartar el trabajo"}") } }
                dismiss()
            }
        }
        Mono("cerrar", Palette.fg, 14, modifier = Modifier.fillMaxWidth().clickable(onClick = dismiss).padding(vertical = 12.dp))
    }
}

/** The line Home says for a few seconds, over the list (REQ-661). */
@Composable
internal fun BoxScope.Notice(source: AppSource) {
    val text = source.notice.value
    LaunchedEffect(text) {
        if (text != null) {
            delay(3500)
            if (source.notice.value == text) source.notice.value = null
        }
    }
    if (text != null) {
        val color: Color = if (text.startsWith("✗")) Palette.red else if (text.startsWith("✓")) Palette.green else Palette.fg
        Mono(
            text,
            color,
            12,
            lines = 3,
            modifier = Modifier.align(Alignment.BottomCenter).fillMaxWidth().padding(horizontal = Gutter, vertical = 12.dp)
                .background(Palette.sel, RoundedCornerShape(10.dp)).padding(12.dp),
        )
    }
}
