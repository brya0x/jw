plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
}

// The product identity lives in ../identity.toml; read it here instead of repeating it.
fun identity(key: String): String {
    val line = file("../../identity.toml").readLines().first { it.trim().startsWith("$key =") }
    return line.substringAfter("=").substringBefore("#").trim().trim('"')
}

android {
    namespace = "dev.jw.next.app"
    compileSdk = 36

    defaultConfig {
        applicationId = identity("android_application_id")
        minSdk = 26
        targetSdk = 36
        versionCode = 1
        versionName = "0.0.1"
        manifestPlaceholders["appLabel"] = identity("label")
        buildConfigField("String", "LINK_SCHEME", "\"${identity("name")}\"")
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    buildFeatures {
        compose = true
        buildConfig = true
    }

    // The shared fixtures ship in the app until it talks to a daemon: assets/fixtures/**.
    sourceSets["main"].assets.srcDir(rootDir.resolve("../contract"))
}

kotlin {
    jvmToolchain(17)
}

dependencies {
    implementation(project(":client-core"))
    implementation(platform(libs.compose.bom))
    implementation(libs.compose.ui)
    implementation(libs.compose.foundation)
    implementation(libs.compose.material3)
    implementation(libs.activity.compose)
    implementation(libs.okhttp)
}
