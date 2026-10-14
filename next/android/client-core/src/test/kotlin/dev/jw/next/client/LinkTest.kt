package dev.jw.next.client

import dev.jw.next.contract.Denied
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertNull
import kotlin.test.assertTrue

/** REQ-81 to REQ-83 on the phone's side. */
class LinkTest {
    private val token = "00112233445566778899aabbccddeeff"
    private val fp = "ab".repeat(32)

    @Test
    fun aLinkNamesTheNetworkAddressTheFingerprintAndTheTunnel() {
        val both = parsePairLink("jwn://192.168.1.20:7443/$token?fp=$fp&t=quiet-river-1.trycloudflare.com", "jwn")!!
        assertEquals(PairLink(token, HostPort("192.168.1.20", 7443), fp, "quiet-river-1.trycloudflare.com"), both)
        assertEquals(PairLink(token, null, null, "a-b.trycloudflare.com"), parsePairLink("  jwn://a-b.trycloudflare.com/$token \n", "jwn"))
        assertEquals(PairLink(token, HostPort("10.0.0.5", 1), fp, null), parsePairLink("jwn://10.0.0.5:1/$token?fp=$fp", "jwn"))
    }

    @Test
    fun aLinkThatPointsAnywhereElseIsNotALink() {
        val bad = listOf(
            "jwn://8.8.8.8:7443/$token?fp=$fp",
            "jwn://192.168.1.20:7443/$token",
            "jwn://192.168.1.20:7443/$token?fp=abc",
            "jwn://evil.example.com/$token",
            "jwn://trycloudflare.com/$token",
            "jwn://a.trycloudflare.com.evil.com/$token",
            "jwn://192.168.1.20:7443/$token?fp=$fp&t=evil.example.com",
            "jwn://a.trycloudflare.com/short",
            "jwn://a.trycloudflare.com/$token?fp=$fp",
            "jwn://192.168.1.20:0/$token?fp=$fp",
            "jwn://192.168.1.020:7443/$token?fp=$fp",
            "https://a.trycloudflare.com/$token",
            "",
        )
        for (text in bad) assertNull(parsePairLink(text, "jwn"), text)
    }

    @Test
    fun privateAddressesAreTheThreeRanges() {
        assertTrue(isPrivateV4("10.1.2.3") && isPrivateV4("172.16.0.1") && isPrivateV4("172.31.9.9") && isPrivateV4("192.168.0.1"))
        assertFalse(isPrivateV4("172.32.0.1") || isPrivateV4("127.0.0.1") || isPrivateV4("0.0.0.0") || isPrivateV4("192.169.0.1") || isPrivateV4("1.2.3"))
    }

    @Test
    fun aPairingTriesTheNetworkFirstAndRemembersOnlyAllowedEndpoints() {
        val p = Pairing("d", "s", HostPort("192.168.1.20", 7443), fp, "a.trycloudflare.com")
        assertEquals(listOf(Candidate("wss://192.168.1.20:7443/", true), Candidate("wss://a.trycloudflare.com/", false)), p.candidates())
        val moved = p.withEndpoints("192.168.1.30:7443", "b.trycloudflare.com")
        assertEquals(HostPort("192.168.1.30", 7443), moved.lan)
        assertEquals("b.trycloudflare.com", moved.tunnel)
        val hostile = p.withEndpoints("8.8.8.8:7443", "evil.example.com")
        assertEquals(p, hostile, "a host the phone must not trust is ignored")
        val gone = p.withEndpoints(null, null)
        assertNull(gone.lan)
        assertNull(gone.tunnel)
        assertEquals(emptyList(), Pairing("d", "s").candidates())
        assertEquals(emptyList(), Pairing("d", "s", HostPort("192.168.1.20", 7443), null).candidates(), "no pin, no network address")
    }

    @Test
    fun aLinkIsTriedTheSameWayAsAPairing() {
        val both = parsePairLink("jwn://192.168.1.20:7443/$token?fp=$fp&t=a-b.trycloudflare.com", "jwn")!!
        assertEquals(listOf(Candidate("wss://192.168.1.20:7443/", true), Candidate("wss://a-b.trycloudflare.com/", false)), both.candidates())
        assertEquals(listOf(Candidate("wss://a-b.trycloudflare.com/", false)), parsePairLink("jwn://a-b.trycloudflare.com/$token", "jwn")!!.candidates())
    }

    @Test
    fun aPairingSurvivesItsJson() {
        val p = Pairing("d", "s", HostPort("10.0.0.5", 7443), fp, "a.trycloudflare.com")
        assertEquals(p, Pairing.fromJson(p.toJson()))
        assertNull(Pairing.fromJson("not json"))
        assertNull(Pairing.fromJson(null))
    }

    @Test
    fun aCertificateMatchesOnlyItsOwnFingerprint() {
        val der = byteArrayOf(1, 2, 3)
        val fp = sha256Hex(der)
        assertEquals("039058c6f2c0cb492c533b0a4d14ef77cc0f78abccced5287d84a1a2011cfb81", fp)
        assertTrue(certMatches(der, fp))
        assertFalse(certMatches(byteArrayOf(1, 2, 4), fp))
        assertFalse(certMatches(der, "0".repeat(64)))
    }

    @Test
    fun theCodeIsSixDigitsAndEveryDenialHasAScreen() {
        repeat(20) { assertTrue(Regex("^[0-9]{6}$").matches(pairingCode())) }
        assertEquals(
            listOf(LinkFail.Expired, LinkFail.Declined, LinkFail.Declined, LinkFail.Revoked, LinkFail.Revoked, LinkFail.Locked),
            listOf(Denied.BadToken, Denied.Declined, Denied.Timeout, Denied.BadSecret, Denied.Revoked, Denied.LockedOut).map { it.toFail() },
        )
    }
}
