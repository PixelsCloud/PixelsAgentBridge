package com.pixels.agent.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

class AppearancePreferencesTest {
    @Test
    fun roundTripsAllPreferences() {
        for (appearance in Appearance.entries) for (language in
            AppLanguage.entries) for (startOnBoot in listOf(true, false)) {
            val expected = AppearancePreferences(appearance, language, startOnBoot)
            assertEquals(expected, AppearancePreferences.fromToml(expected.toToml()))
        }
    }

    @Test
    fun rejectsUnknownVersionsFieldsDuplicatesAndOversizedDocuments() {
        val source = AppearancePreferences().toToml()
        for (invalid in
            listOf(
                source.replace("version = 1", "version = 2"),
                source + "future = true\n",
                source + "appearance = \"dark\"\n",
                source.replace("start_on_boot = true", "start_on_boot = \"true\""),
                source.replace("\"system\"", "\"unknown\""),
                source.repeat(100),
                "",
            )) {
            assertThrows(IllegalArgumentException::class.java) {
                AppearancePreferences.fromToml(invalid)
            }
        }
    }

    @Test
    fun olderPreferencesDefaultToStartupEnabled() {
        assertTrue(AppearancePreferences().startOnBoot)
        assertTrue(
            AppearancePreferences.fromToml(
                    "version = 1\nappearance = \"dark\"\nlanguage = \"chinese\"\n"
                )
                .startOnBoot
        )
    }

    @Test
    fun acceptsCommentsAndWhitespaceWithoutExecutingAnything() {
        assertEquals(
            AppearancePreferences(),
            AppearancePreferences.fromToml(
                "# preferences\n version = 1 # supported\n appearance = \"system\"\n language = \"system\"\n"
            ),
        )
    }
}
