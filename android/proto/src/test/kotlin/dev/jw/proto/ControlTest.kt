package dev.jw.proto

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class ControlTest {
    @Test fun encodesPairAndAuth() {
        val p = Control.encode(Hello.Pair("a".repeat(32), "Pixel", "482913"))
        assertEquals("""{"t":"pair","token":"${"a".repeat(32)}","name":"Pixel","code":"482913"}""", p)
        assertEquals("""{"t":"auth","device":"d"}""", Control.encode(Hello.Auth("d")))
    }

    @Test fun parsesPairedReadyError() {
        val paired = Control.parse("""{"t":"paired","device":"${"f".repeat(64)}","id":"abcd1234"}""") as ServerMessage.Paired
        assertEquals("abcd1234", paired.id)
        val ready = Control.parse("""{"t":"ready","bridge":1,"protocol":10,"read_only":true,"future":1}""") as ServerMessage.Ready
        assertEquals(10, ready.protocol)
        assertTrue(ready.readOnly)
        assertEquals(Palettes.FALLBACK.dark["bg"], ready.palettes.dark["bg"])
        val err = Control.parse("""{"t":"error","code":"revoked","msg":"x"}""") as ServerMessage.Error
        assertTrue(ErrorCode.isFinal(err.code))
        assertFalse(ErrorCode.isFinal(ErrorCode.LIMITED))
    }

    @Test fun unknownTagAndGarbage() {
        assertTrue(Control.parse("""{"t":"workspaces"}""") is ServerMessage.Other)
        assertNull(Control.parse("nope"))
        assertNull(Control.parse("""{"t":"paired"}"""))
    }

    @Test fun palettesFromReadyOverrideRoleByRole() {
        val r = Control.parse(
            """{"t":"ready","protocol":10,"palettes":[{"name":"Other","dark":true,"colors":{"bg":"#111111"}},{"name":"One Dark","dark":true,"colors":{"bg":"#000000","fg":"nothex"}}]}""",
        ) as ServerMessage.Ready
        assertEquals("#000000", r.palettes.dark["bg"])
        assertEquals(Palettes.FALLBACK.dark["fg"], r.palettes.dark["fg"])
        assertEquals(Palettes.FALLBACK.light["bg"], r.palettes.light["bg"])
    }

    @Test fun secretsStayOutOfToString() {
        val s = Hello.Pair("t".repeat(32), "n", "123456").toString() + Hello.Auth("devtoken") +
            ServerMessage.Paired("devtoken", "id")
        assertFalse(s.contains("tttt"))
        assertFalse(s.contains("123456"))
        assertFalse(s.contains("devtoken"))
    }

    @Test fun qrParsing() {
        val token = "0123456789abcdef0123456789abcdef"
        val q = QrPayload.parse("jw://guys-slowly.trycloudflare.com/$token")
        assertNotNull(q)
        assertEquals("guys-slowly.trycloudflare.com", q!!.host)
        assertEquals(token, q.token)
        assertNull(QrPayload.parse("https://x.com/$token"))
        assertNull(QrPayload.parse("jw://host/short"))
        for (h in listOf(
            "evil.com", "a.trycloudflare.com.evil.com", "trycloudflare.com", "a.trycloudflare.com:8080",
            "user@a.trycloudflare.com", "a..trycloudflare.com", "a.trycloudflare.com.", "-a.trycloudflare.com",
            "a.evil.com/x.trycloudflare.com",
        )) assertNull(h, QrPayload.parse("jw://$h/$token"))
        assertNull(QrPayload.parse("jw://host/${token.uppercase()}"))
        assertFalse(q.toString().contains(token))
    }
}
