package dev.jw.next.app

import dev.jw.next.client.Candidate
import dev.jw.next.client.Pairing
import dev.jw.next.client.PcState
import dev.jw.next.contract.BridgeMsg
import dev.jw.next.contract.ContractJson
import dev.jw.next.contract.Denied
import dev.jw.next.contract.PhoneMsg
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit
import okhttp3.Request
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener

/** What one look at a PC found: how it answered, and its name when it said one. */
data class Seen(val state: PcState, val host: String?)

private const val PROBE_WAIT_SECONDS = 6L

/**
 * Look at a paired PC the way the phone connects to it: open the WebSocket and authenticate,
 * and as soon as the bridge says `ready` hang up (no data is loaded, REQ-87). The PC's
 * addresses are tried in order. A PC that does not answer fails before authenticating, so it
 * never counts against the PC's limit on failed attempts. Blocks: call it off the UI thread.
 */
fun lookAt(pairing: Pairing): Seen {
    for (c in pairing.candidates()) {
        val seen = once(c, pairing)
        if (seen.state != PcState.Offline) return seen
    }
    return Seen(PcState.Offline, null)
}

private fun once(c: Candidate, p: Pairing): Seen {
    val answer = LinkedBlockingQueue<Seen>()
    val ws = linkClient(c, p.fp).newWebSocket(
        Request.Builder().url(c.url).build(),
        object : WebSocketListener() {
            override fun onOpen(webSocket: WebSocket, response: Response) {
                webSocket.send(ContractJson.encodeToString(PhoneMsg.serializer(), PhoneMsg.Auth(p.device, p.secret)))
            }

            override fun onMessage(webSocket: WebSocket, text: String) {
                if (messageType(text) !in BRIDGE_TYPES) return
                when (val msg = ContractJson.decodeFromString(BridgeMsg.serializer(), text)) {
                    is BridgeMsg.Ready -> answer.offer(Seen(PcState.Online, msg.host))
                    is BridgeMsg.DeniedMsg -> answer.offer(
                        // A lockout is the PC being cautious, not this phone being unpaired.
                        Seen(if (msg.reason == Denied.BadSecret || msg.reason == Denied.Revoked) PcState.Revoked else PcState.Offline, null),
                    )
                    else -> {}
                }
            }

            override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
                webSocket.close(1000, null)
            }

            override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
                answer.offer(Seen(PcState.Offline, null))
            }
        },
    )
    val seen = answer.poll(PROBE_WAIT_SECONDS, TimeUnit.SECONDS) ?: Seen(PcState.Offline, null)
    ws.cancel()
    return seen
}
