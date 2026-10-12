package dev.jw.proto

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class ControlTest {
    @Test fun aThemeChosenOnThePhoneWinsOverTheSystemOnlyWhileThePcHasIt() {
        val night = ThemeInfo("Tokyo Night", true, mapOf("bg" to "#1a1b26"))
        val p = Palettes(Palettes.FALLBACK.dark, Palettes.FALLBACK.light, listOf(night))
        val (c, dark) = p.resolve("Tokyo Night", false)
        assertEquals("#1a1b26", c["bg"])
        assertTrue(dark)
        assertEquals(Palettes.FALLBACK.light["bg"], p.resolve("Gone", false).first["bg"])
        assertEquals(Palettes.FALLBACK.dark["bg"], p.resolve(null, true).first["bg"])
    }

    @Test fun encodesPairAndAuth() {
        val p = Control.encode(Hello.Pair("a".repeat(32), "Pixel", "482913"))
        assertEquals("""{"t":"pair","token":"${"a".repeat(32)}","name":"Pixel","code":"482913"}""", p)
        assertEquals("""{"t":"auth","device":"d"}""", Control.encode(Hello.Auth("d")))
    }

    @Test fun parsesPairedReadyError() {
        val paired = Control.parse("""{"t":"paired","device":"${"f".repeat(64)}","id":"abcd1234"}""") as ServerMessage.Paired
        assertEquals("abcd1234", paired.id)
        val ready = Control.parse("""{"t":"ready","bridge":1,"protocol":13,"read_only":true,"future":1}""") as ServerMessage.Ready
        assertEquals(13, ready.protocol)
        assertTrue(ready.readOnly)
        assertEquals(Palettes.FALLBACK.dark["bg"], ready.palettes.dark["bg"])
        val err = Control.parse("""{"t":"error","code":"revoked","msg":"x"}""") as ServerMessage.Error
        assertTrue(ErrorCode.isFinal(err.code))
        assertFalse(ErrorCode.isFinal(ErrorCode.LIMITED))
    }

    @Test fun unknownTagAndGarbage() {
        assertTrue(Control.parse("""{"t":"diff"}""") is ServerMessage.Other)
        assertNull(Control.parse("nope"))
        assertNull(Control.parse("""{"t":"paired"}"""))
    }

    @Test fun palettesFromReadyOverrideRoleByRole() {
        val r = Control.parse(
            """{"t":"ready","protocol":13,"palettes":[{"name":"Other","dark":true,"colors":{"bg":"#111111"}},{"name":"One Dark","dark":true,"colors":{"bg":"#000000","fg":"nothex"}}]}""",
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
        assertEquals("guys-slowly.trycloudflare.com", q!!.tunnel)
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

    @Test fun endpointsKeepOnlyWhatIsValid() {
        val none = Control.parse("""{"lan":null,"t":"endpoints","tunnel":null}""") as ServerMessage.Endpoints
        assertNull(none.lan); assertNull(none.tunnel)
        val fp = "a".repeat(64)
        val some = Control.parse(
            """{"lan":{"fp":"$fp","host":"192.168.1.23","port":48211},"t":"endpoints","tunnel":"a-b.trycloudflare.com"}""",
        ) as ServerMessage.Endpoints
        assertEquals("192.168.1.23:48211", some.lan!!.hostPort)
        assertEquals(fp, some.lan!!.fp)
        assertEquals("a-b.trycloudflare.com", some.tunnel)
        // A host the app never connects to is dropped, and does not take the rest with it.
        val bad = Control.parse(
            """{"lan":{"fp":"$fp","host":"8.8.8.8","port":1},"t":"endpoints","tunnel":"evil.example.com"}""",
        ) as ServerMessage.Endpoints
        assertNull(bad.lan); assertNull(bad.tunnel)
        val oneBad = Control.parse(
            """{"lan":{"fp":"short","host":"10.0.0.2","port":1},"t":"endpoints","tunnel":"ok.trycloudflare.com"}""",
        ) as ServerMessage.Endpoints
        assertNull(oneBad.lan); assertEquals("ok.trycloudflare.com", oneBad.tunnel)
    }
}
