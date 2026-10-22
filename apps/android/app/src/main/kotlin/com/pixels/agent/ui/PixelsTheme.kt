package com.pixels.agent.ui

import android.content.res.Configuration
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.remember
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalContext
import com.pixels.agent.core.AppLanguage
import com.pixels.agent.core.Appearance
import com.pixels.agent.core.AppearancePreferences
import java.util.Locale

private val light =
    lightColorScheme(
        primary = Color(0xFF218574),
        onPrimary = Color.White,
        primaryContainer = Color(0xFFDFF1E9),
        onPrimaryContainer = Color(0xFF1E7B6C),
        tertiary = Color(0xFF2CA777),
        background = Color(0xFFF5F7F9),
        onBackground = Color(0xFF162637),
        surface = Color.White,
        onSurface = Color(0xFF162637),
        surfaceVariant = Color(0xFFE9F6F1),
        onSurfaceVariant = Color(0xFF718796),
        outline = Color(0xFFD4E0E5),
        outlineVariant = Color(0xFFEAF0F3),
        error = Color(0xFFB9474B),
        onError = Color.White,
    )
private val dark =
    darkColorScheme(
        primary = Color(0xFF93D7C2),
        onPrimary = Color(0xFF162637),
        primaryContainer = Color(0xFF345047),
        onPrimaryContainer = Color(0xFF93D7C2),
        tertiary = Color(0xFF84DFB5),
        background = Color(0xFF202329),
        onBackground = Color(0xFFF0F3F5),
        surface = Color(0xFF2B3038),
        onSurface = Color(0xFFF0F3F5),
        surfaceVariant = Color(0xFF333A43),
        onSurfaceVariant = Color(0xFFAFB9C2),
        outline = Color(0xFF434B55),
        outlineVariant = Color(0xFF3A424C),
        error = Color(0xFFFFB0B2),
    )

@Composable
internal fun PixelsTheme(preferences: AppearancePreferences, content: @Composable () -> Unit) {
    val context = LocalContext.current
    val configuration = LocalConfiguration.current
    val localized =
        remember(configuration, preferences.language) {
            Configuration(configuration).apply {
                when (preferences.language) {
                    AppLanguage.CHINESE -> setLocale(Locale.SIMPLIFIED_CHINESE)
                    AppLanguage.ENGLISH -> setLocale(Locale.ENGLISH)
                    AppLanguage.SYSTEM -> Unit
                }
            }
        }
    val localizedContext =
        remember(context, localized) { context.createConfigurationContext(localized) }
    val useDark =
        when (preferences.appearance) {
            Appearance.SYSTEM -> isSystemInDarkTheme()
            Appearance.LIGHT -> false
            Appearance.DARK -> true
        }
    CompositionLocalProvider(
        LocalContext provides localizedContext,
        LocalConfiguration provides localized,
    ) {
        MaterialTheme(colorScheme = if (useDark) dark else light, content = content)
    }
}
