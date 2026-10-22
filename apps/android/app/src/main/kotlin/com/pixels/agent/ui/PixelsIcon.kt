package com.pixels.agent.ui

import androidx.annotation.DrawableRes
import androidx.compose.foundation.layout.size
import androidx.compose.material3.Icon
import androidx.compose.material3.LocalContentColor
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.unit.dp
import com.pixels.agent.R
import com.pixels.agent.platform.R as IconResources

/** Official Lucide assets; selected navigation items use the same geometry at weight 2.5. */
internal enum class Glyph(
    @param:DrawableRes val resource: Int,
    @param:DrawableRes val selectedResource: Int = resource,
) {
    BRAND(R.drawable.brand),
    HOME(IconResources.drawable.lucide_house, IconResources.drawable.lucide_house_bold),
    FOLDER(IconResources.drawable.lucide_folder, IconResources.drawable.lucide_folder_bold),
    ACTIVITY(IconResources.drawable.lucide_list, IconResources.drawable.lucide_list_bold),
    SETTINGS(IconResources.drawable.lucide_settings, IconResources.drawable.lucide_settings_bold),
    CHEVRON(IconResources.drawable.lucide_chevron_right),
    COPY(IconResources.drawable.lucide_copy),
    EYE(IconResources.drawable.lucide_eye),
    HIDE(IconResources.drawable.lucide_eye_off),
    ROTATE(IconResources.drawable.lucide_rotate_cw),
    PAUSE(IconResources.drawable.lucide_pause),
    CHECK(IconResources.drawable.lucide_check),
    SCREEN(IconResources.drawable.lucide_monitor),
    BELL(IconResources.drawable.lucide_bell),
    PHONE(IconResources.drawable.lucide_smartphone),
    LOCK(IconResources.drawable.lucide_lock_keyhole),
    PALETTE(IconResources.drawable.lucide_palette),
    GLOBE(IconResources.drawable.lucide_globe),
    LINK(IconResources.drawable.lucide_link),
    HELP(IconResources.drawable.lucide_circle_help),
    INFO(IconResources.drawable.lucide_info),
    MORE(IconResources.drawable.lucide_ellipsis_vertical),
    PLUS(IconResources.drawable.lucide_circle_plus),
    BACK(IconResources.drawable.lucide_arrow_left),
    DOCUMENT(IconResources.drawable.lucide_file_text),
    UPLOAD(IconResources.drawable.lucide_upload),
    POINTER(IconResources.drawable.lucide_mouse_pointer_2),
}

@Composable
internal fun PixelsIcon(
    glyph: Glyph,
    modifier: Modifier = Modifier,
    tint: Color = LocalContentColor.current,
    description: String? = null,
    bold: Boolean = false,
) {
    Icon(
        painter = painterResource(if (bold) glyph.selectedResource else glyph.resource),
        contentDescription = description,
        modifier = modifier.size(24.dp),
        // The app's brand is an image asset, not a functional icon or a redrawn logo.
        tint = if (glyph == Glyph.BRAND) Color.Unspecified else tint,
    )
}
