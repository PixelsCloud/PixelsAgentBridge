pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}

dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        google()
        mavenCentral()
        maven {
            url = uri("../../.build/android/maven")
            content { includeGroup("rustls") }
        }
    }
}

rootProject.name = "PixelsAndroid"

include(":app", ":core", ":platform", ":transport")
