import com.diffplug.gradle.spotless.SpotlessExtension

plugins {
    alias(libs.plugins.android.application) apply false
    alias(libs.plugins.android.library) apply false
    alias(libs.plugins.kotlin.jvm) apply false
    alias(libs.plugins.compose.compiler) apply false
    alias(libs.plugins.spotless)
}

spotless {
    kotlinGradle {
        target("*.gradle.kts", "*/build.gradle.kts")
        ktfmt("0.64").kotlinlangStyle()
    }
}

subprojects {
    apply(plugin = "com.diffplug.spotless")
    configure<SpotlessExtension> {
        kotlin {
            target("src/**/*.kt")
            ktfmt("0.64").kotlinlangStyle()
        }
    }
}
