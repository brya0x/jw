package dev.jw.next.app

import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.exclude
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.ime
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.remember
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.platform.LocalViewConfiguration

/** Whether the tap in flight landed on something that keeps the keyboard up (REQ-236). */
internal class KeyboardTaps {
    var keeps = false
}

private val LocalKeyboardTaps = staticCompositionLocalOf { KeyboardTaps() }

/**
 * The whole app is a keyboard avoider and a tap anywhere else puts the keyboard away (REQ-236,
 * REQ-237): its content rises with the keyboard, and a tap that is not on a text field (or on
 * what [keepsKeyboard] marks) takes the focus from the field. A scroll or a drag does not.
 */
@Composable
internal fun KeyboardRoot(modifier: Modifier, content: @Composable BoxScope.() -> Unit) {
    val taps = remember { KeyboardTaps() }
    val focus = LocalFocusManager.current
    val keyboard = LocalSoftwareKeyboardController.current
    val slop = LocalViewConfiguration.current.touchSlop
    CompositionLocalProvider(LocalKeyboardTaps provides taps) {
        Box(
            modifier.imePadding().pointerInput(focus, keyboard) {
                awaitEachGesture {
                    // Parents see the press before their children: this clears what a child sets on the same press.
                    awaitFirstDown(requireUnconsumed = false, pass = PointerEventPass.Initial)
                    taps.keeps = false
                    var moved = false
                    do {
                        val event = awaitPointerEvent(PointerEventPass.Final)
                        if (event.changes.any { (it.position - it.previousPosition).getDistance() > slop }) moved = true
                    } while (event.changes.any { it.pressed })
                    if (!moved && !taps.keeps) {
                        focus.clearFocus()
                        keyboard?.hide()
                    }
                }
            },
            content = content,
        )
    }
}

/** A tap on this keeps the keyboard up: it is a text field, or something that goes with one (the composer's send). */
@Composable
internal fun Modifier.keepsKeyboard(): Modifier {
    val taps = LocalKeyboardTaps.current
    return pointerInput(taps) {
        awaitEachGesture {
            awaitFirstDown(requireUnconsumed = false, pass = PointerEventPass.Initial)
            taps.keeps = true
        }
    }
}

/**
 * The navigation bar's room, unless the keyboard is up: the keyboard covers the bar, and
 * [KeyboardRoot] has already made room for the keyboard, so padding for the bar too would leave a gap.
 */
@Composable
internal fun Modifier.navigationBarsUnlessKeyboard(): Modifier =
    windowInsetsPadding(WindowInsets.navigationBars.exclude(WindowInsets.ime))
