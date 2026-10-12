package dev.jw.mobile

import android.content.Context
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue

/** Which bytes y and n send (OPEN-5). Not secret: plain preferences. */
class AnswerPrefs(context: Context) {
    private val prefs = context.getSharedPreferences("answers", Context.MODE_PRIVATE)

    var approve: String?
        get() = prefs.getString("approve", null)
        set(v) = prefs.edit().putString("approve", v).apply()

    var reject: String?
        get() = prefs.getString("reject", null)
        set(v) = prefs.edit().putString("reject", v).apply()

    /** The theme chosen on the phone, by the name the PC lists it under; null follows the system's dark or light. */
    var theme: String? by mutableStateOf(prefs.getString("theme", null))
        private set

    fun chooseTheme(name: String?) {
        theme = name
        prefs.edit().putString("theme", name).apply()
    }

    /** "Fit to phone" (REQ-24): off until the person turns it on. */
    var fitToPhone: Boolean
        get() = prefs.getBoolean("fit", false)
        set(v) = prefs.edit().putBoolean("fit", v).apply()
}
