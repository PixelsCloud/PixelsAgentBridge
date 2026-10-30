plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.compose.compiler)
}

android {
    // Release optimization while updating the existing USB development identity.
    val deviceIdentity = providers.gradleProperty("pixelsDeviceIdentity").orNull == "true"
    testBuildType = "release"
    if (deviceIdentity && providers.gradleProperty("pixelsDeviceTests").orNull == "true") {
        sourceSets["release"].java.srcDir("src/debug/kotlin")
        sourceSets["release"].manifest.srcFile("src/debug/AndroidManifest.xml")
    }
    namespace = "com.pixels.agent"
    compileSdk = 36
    ndkVersion = "29.0.14206865"
    defaultConfig {
        applicationId = "com.pixels.agent"
        minSdk = 28
        targetSdk = 36
        versionCode = providers.gradleProperty("pixelsVersionCode").map(String::toInt).getOrElse(1)
        versionName = providers.gradleProperty("pixelsVersionName").getOrElse("0.1.0-dev")
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
        ndk {
            abiFilters +=
                if (deviceIdentity) listOf(providers.gradleProperty("pixelsDeviceAbi").get())
                else listOf("arm64-v8a", "x86_64")
        }
    }
    val releaseKeystore = providers.environmentVariable("PAB_ANDROID_KEYSTORE_FILE")
    signingConfigs {
        if (releaseKeystore.isPresent) {
            create("officialRelease") {
                storeFile = file(releaseKeystore.get())
                storePassword = providers.environmentVariable("PAB_ANDROID_KEYSTORE_PASSWORD").get()
                keyAlias = providers.environmentVariable("PAB_ANDROID_KEY_ALIAS").get()
                keyPassword = providers.environmentVariable("PAB_ANDROID_KEY_PASSWORD").get()
            }
        }
    }
    buildTypes {
        debug { applicationIdSuffix = ".debug" }
        release {
            if (deviceIdentity) {
                applicationIdSuffix = ".debug"
                signingConfig = signingConfigs.getByName("debug")
            } else if (releaseKeystore.isPresent)
                signingConfig = signingConfigs.getByName("officialRelease")
        }
    }
    buildFeatures { compose = true }
    sourceSets["main"]
        .assets
        .srcDir(rootProject.file("../../.build/android/document-runtime/assets"))
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

kotlin { compilerOptions { freeCompilerArgs.add("-Xbackend-threads=1") } }

dependencies {
    implementation(project(":core"))
    implementation(project(":platform"))
    implementation(project(":transport"))
    implementation(platform(libs.compose.bom))
    implementation(libs.compose.material3)
    implementation(libs.compose.ui)
    implementation(libs.activity.compose)
    implementation(libs.lifecycle.viewmodel.compose)
    implementation(libs.lifecycle.runtime.compose)
    implementation(libs.coroutines.android)
    // Verified and staged by build_android.py; standard flavor has no OpenMP fan-out.
    runtimeOnly(
        files(rootProject.file("../../.build/android/document-runtime/tesseract4android-4.9.0.aar"))
    )
    androidTestImplementation(libs.test.junit)
    androidTestImplementation(libs.test.runner)
    androidTestImplementation(libs.test.uiautomator)
}
