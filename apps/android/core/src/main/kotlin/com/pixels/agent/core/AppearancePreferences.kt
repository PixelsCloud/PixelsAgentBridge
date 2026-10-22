package com.pixels.agent.core

enum class Appearance {
    SYSTEM,
    LIGHT,
    DARK,
}

enum class AppLanguage {
    SYSTEM,
    CHINESE,
    ENGLISH,
}

data class AppearancePreferences(
    val appearance: Appearance = Appearance.SYSTEM,
    val language: AppLanguage = AppLanguage.SYSTEM,
    val startOnBoot: Boolean = true,
) {
    fun toToml(): String =
        "version = 1\nappearance = \"${appearance.name.lowercase()}\"\nlanguage = \"${language.name.lowercase()}\"\nstart_on_boot = $startOnBoot\n"

    companion object {
        /** Refuse unknown fields/versions instead of overwriting a newer client's preferences. */
        fun fromToml(text: String): AppearancePreferences {
            require(text.length <= 4096)
            val entries = linkedMapOf<String, String>()
            for (line in text.lineSequence()) {
                val content = line.substringBefore('#').trim()
                if (content.isEmpty()) continue
                val parts = content.split('=', limit = 2)
                require(parts.size == 2)
                val key = parts[0].trim()
                require(entries.put(key, parts[1].trim()) == null)
            }
            require(entries.keys.containsAll(setOf("version", "appearance", "language")))
            require(
                entries.keys.all {
                    it in setOf("version", "appearance", "language", "start_on_boot")
                }
            )
            require(entries["version"] == "1")
            return AppearancePreferences(
                Appearance.entries.firstOrNull {
                    "\"${it.name.lowercase()}\"" == entries["appearance"]
                } ?: throw IllegalArgumentException("unsupported_appearance"),
                AppLanguage.entries.firstOrNull {
                    "\"${it.name.lowercase()}\"" == entries["language"]
                } ?: throw IllegalArgumentException("unsupported_language"),
                entries["start_on_boot"]?.let {
                    it.toBooleanStrictOrNull()
                        ?: throw IllegalArgumentException("unsupported_start_on_boot")
                } ?: true,
            )
        }
    }
}
