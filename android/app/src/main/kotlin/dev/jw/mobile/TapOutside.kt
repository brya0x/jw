package dev.jw.mobile

import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.waitForUpOrCancellation
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.pointerInput

/**
 * Calls [onTap] after a tap anywhere on this, even on what is inside it that takes taps of its own (a card, selectable
 * text): it looks at the touch first and consumes nothing. A touch that moved is a scroll, not a tap.
 */
internal fun Modifier.onTapAnywhere(onTap: () -> Unit): Modifier = pointerInput(Unit) {
    awaitEachGesture {
        val down = awaitFirstDown(requireUnconsumed = false, pass = PointerEventPass.Initial)
        val up = waitForUpOrCancellation(PointerEventPass.Initial)
        if (up != null && (up.position - down.position).getDistance() < viewConfiguration.touchSlop) onTap()
    }
}
