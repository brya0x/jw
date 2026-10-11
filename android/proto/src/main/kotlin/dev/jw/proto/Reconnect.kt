package dev.jw.proto

/** Why a connection attempt ended. */
sealed interface Failure {
    /** The host did not resolve. */
    data object Dns : Failure

    /** Socket error, silent close or timeout of the transport. */
    data object Network : Failure

    /** The bridge said `error` with this code, while authing with a stored device. */
    class Bridge(val code: String) : Failure
}

sealed interface Decision {
    /** Try again after this many milliseconds. */
    class Retry(val afterMs: Long) : Decision

    /** Drop the stored device and go back to scanning. */
    data object Rescan : Decision

    /** Stop retrying and tell the person: the tunnel host never resolved. */
    data object GiveUpDns : Decision
}

/**
 * Pure reconnect policy: no clock, no sleeping. The caller passes `nowMs` and waits.
 *
 * - `unknown` and `revoked` are final: [Decision.Rescan], never a retry (REQ-5 lockout).
 * - `limited` waits the whole lockout (5 min).
 * - A DNS failure before the first connect is retried every [dnsRetryMs] for [dnsGraceMs].
 * - Anything else backs off 5 s, 10 s, ... capped at [maxMs]; never five failures in 60 s.
 */
class ReconnectPolicy(
    private val dnsGraceMs: Long = 180_000,
    private val dnsRetryMs: Long = 5_000,
    private val baseMs: Long = 5_000,
    private val maxMs: Long = 30_000,
    private val limitedMs: Long = 300_000,
) {
    private var connectedOnce = false
    private var firstFailureAt: Long? = null
    private var attempts = 0

    /** A connection reached `ready`. */
    fun onConnected() {
        connectedOnce = true
        firstFailureAt = null
        attempts = 0
    }

    fun onFailure(failure: Failure, nowMs: Long): Decision {
        if (failure is Failure.Bridge) {
            when {
                ErrorCode.isFinal(failure.code) -> return Decision.Rescan
                failure.code == ErrorCode.LIMITED -> return Decision.Retry(limitedMs)
            }
        }
        val start = firstFailureAt ?: nowMs.also { firstFailureAt = it }
        if (failure is Failure.Dns && !connectedOnce) {
            if (nowMs - start >= dnsGraceMs) return Decision.GiveUpDns
            return Decision.Retry(dnsRetryMs)
        }
        val delay = minOf(maxMs, baseMs shl minOf(attempts, 10))
        attempts++
        return Decision.Retry(delay)
    }
}
