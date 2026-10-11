package dev.jw.mobile

import android.content.Context

/** Which bytes y and n send (OPEN-5). Not secret: plain preferences. */
class AnswerPrefs(context: Context) {
    private val prefs = context.getSharedPreferences("answers", Context.MODE_PRIVATE)

    var approve: String?
        get() = prefs.getString("approve", null)
        set(v) = prefs.edit().putString("approve", v).apply()

    var reject: String?
        get() = prefs.getString("reject", null)
        set(v) = prefs.edit().putString("reject", v).apply()
}
