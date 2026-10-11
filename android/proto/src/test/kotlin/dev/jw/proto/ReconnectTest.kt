package dev.jw.proto

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class ReconnectTest {
    private fun retry(d: Decision) = (d as Decision.Retry).afterMs

    @Test fun backoffDoublesAndCaps() {
        val p = ReconnectPolicy()
        val delays = (0 until 7).map { retry(p.onFailure(Failure.Network, it * 1000L)) }
        assertEquals(listOf(5000L, 10000, 20000, 30000, 30000, 30000, 30000), delays)
    }

    @Test fun neverFiveFailuresInAMinute() {
        val p = ReconnectPolicy()
        var t = 0L
        val times = mutableListOf<Long>()
        repeat(8) {
            times += t
            t += retry(p.onFailure(Failure.Network, t))
        }
        for (i in 0..times.size - 5) assertTrue(times[i + 4] - times[i] > 60_000)
    }

    @Test fun finalErrorsNeverRetry() {
        for (c in listOf(ErrorCode.UNKNOWN, ErrorCode.REVOKED)) {
            assertEquals(Decision.Rescan, ReconnectPolicy().onFailure(Failure.Bridge(c), 0))
        }
    }

    @Test fun limitedWaitsTheWholeLockout() {
        assertEquals(300_000L, retry(ReconnectPolicy().onFailure(Failure.Bridge(ErrorCode.LIMITED), 0)))
    }

    @Test fun daemonErrorBacksOffAndKeepsToken() {
        assertEquals(5000L, retry(ReconnectPolicy().onFailure(Failure.Bridge(ErrorCode.DAEMON), 0)))
    }

    @Test fun dnsGraceThenGiveUp() {
        val p = ReconnectPolicy()
        assertEquals(5000L, retry(p.onFailure(Failure.Dns, 0)))
        assertEquals(5000L, retry(p.onFailure(Failure.Dns, 179_999)))
        assertEquals(Decision.GiveUpDns, p.onFailure(Failure.Dns, 180_000))
    }

    @Test fun dnsAfterFirstConnectIsOrdinaryBackoff() {
        val p = ReconnectPolicy()
        p.onConnected()
        assertEquals(5000L, retry(p.onFailure(Failure.Dns, 0)))
        assertEquals(10000L, retry(p.onFailure(Failure.Dns, 1_000_000)))
    }

    @Test fun connectedResetsBackoff() {
        val p = ReconnectPolicy()
        p.onFailure(Failure.Network, 0)
        p.onFailure(Failure.Network, 1)
        p.onConnected()
        assertEquals(5000L, retry(p.onFailure(Failure.Network, 2)))
    }
}
