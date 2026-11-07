plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.plugin.compose")
}

android {
    namespace = "dev.alia.client"
    compileSdk = 37

    defaultConfig {
        applicationId = "dev.alia.client"
        minSdk = 37
        targetSdk = 37
        versionCode = 1
        versionName = "0.0.0"
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_21
        targetCompatibility = JavaVersion.VERSION_21
    }

    buildFeatures {
        compose = true
    }
}

kotlin {
    compilerOptions {
        jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_21)
    }
}

dependencies {
    implementation(platform("androidx.compose:compose-bom:2026.09.00"))
    implementation("androidx.compose.ui:ui")
    implementation("androidx.activity:activity-compose:1.13.0")
    implementation("androidx.compose.material3:material3:1.5.0-rc01")
}
