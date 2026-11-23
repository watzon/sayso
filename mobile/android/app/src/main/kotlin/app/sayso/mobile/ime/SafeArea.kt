package app.sayso.mobile.ime

import androidx.core.graphics.Insets
import androidx.core.view.WindowInsetsCompat

/**
 * The padding that keeps the keys clear of the system bars and display cutouts.
 *
 * With edge-to-edge enforced (target SDK 35+) the input view reaches the bottom
 * of the screen. There the system draws the keyboard's own navigation bar: the
 * hide and switch-keyboard buttons, 48dp tall. navigationBars() reports only
 * the gesture handle (24dp); the full height comes as a caption bar, part of
 * systemBars(), and as tappableElement(). Taking the largest of them clears the
 * buttons in every navigation mode, and the sides clear landscape bars and
 * cutouts. The top is the app's side of the keyboard and needs none.
 */
internal fun keyboardSafeArea(systemBars: Insets, tappable: Insets, cutout: Insets): Insets {
    val all = Insets.max(Insets.max(systemBars, tappable), cutout)
    return Insets.of(all.left, 0, all.right, all.bottom)
}

internal fun keyboardSafeArea(insets: WindowInsetsCompat): Insets = keyboardSafeArea(
    systemBars = insets.getInsets(WindowInsetsCompat.Type.systemBars()),
    tappable = insets.getInsets(WindowInsetsCompat.Type.tappableElement()),
    cutout = insets.getInsets(WindowInsetsCompat.Type.displayCutout()),
)
