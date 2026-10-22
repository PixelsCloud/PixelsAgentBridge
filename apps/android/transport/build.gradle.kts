plugins { alias(libs.plugins.android.library) }

android {
    namespace = "com.pixels.agent.transport"
    compileSdk = 36
    defaultConfig { minSdk = 28 }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

kotlin { compilerOptions { freeCompilerArgs.add("-Xbackend-threads=1") } }

dependencies {
    implementation(project(":core"))
    implementation(libs.coroutines.android)
    implementation(libs.rustls.verifier)
    testImplementation(libs.junit)
}
