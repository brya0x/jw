package dev.jw.next.client

import dev.jw.next.contract.Node

/** The chip of an engine that runs another claude than the one installed (REQ-856); null while the node does not say both. */
fun engineChip(conv: Node.Conversation): String? {
    val running = conv.engineVersion ?: return null
    val newer = conv.engineNewer ?: return null
    return "motor $running ↑ $newer"
}

/** What the sheet of that chip says: which claude the conversation runs, which is installed, and what to do. */
fun engineSaid(conv: Node.Conversation): String {
    val running = conv.engineVersion ?: return ""
    val newer = conv.engineNewer ?: return ""
    return "Esta conversación corre claude $running; en la PC está instalado $newer. Cierra la conversación y ábrela: retoma donde estaba y usa la nueva."
}
