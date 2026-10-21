package dev.jw.next.app

import android.content.Context
import dev.jw.next.client.Look

/** The phone's own choice of mode and themes: kept here and never sent to the PC (REQ-157). */
class LookStore(context: Context) {
    private val prefs = context.getSharedPreferences("look", Context.MODE_PRIVATE)

    fun load(): Look = Look.of(prefs.getString("mode", null), prefs.getString("dark", null), prefs.getString("light", null))

    fun save(look: Look) {
        prefs.edit().putString("mode", look.mode.id).putString("dark", look.dark).putString("light", look.light).apply()
    }
}
