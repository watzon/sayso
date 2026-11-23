plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.compose)
}

// -PsaysoNative=true swaps the fake engine for the JNI bridge to libsayso_core.so.
// -PsaysoCoreDir points at a folder with one subfolder per ABI holding libsayso_core.so.
val saysoNative = providers.gradleProperty("saysoNative").map { it.toBoolean() }.getOrElse(false)
val saysoCoreDir = providers.gradleProperty("saysoCoreDir").getOrElse("../../core/build/android")

android {
    namespace = "app.sayso.mobile"
    compileSdk {
        version = release(37) { minorApiLevel = 1 }
    }
    ndkVersion = "27.1.12297006"

    defaultConfig {
        applicationId = "app.sayso.mobile"
        minSdk = 26
        targetSdk = 37
        versionCode = 1
        versionName = "0.1.0"
        if (saysoNative) {
            ndk { abiFilters += listOf("arm64-v8a", "x86_64") }
            externalNativeBuild {
                cmake { arguments += "-DSAYSO_CORE_DIR=${file(saysoCoreDir).absolutePath}" }
            }
        }
    }

    if (saysoNative) {
        externalNativeBuild {
            cmake { path = file("src/main/cpp/CMakeLists.txt") }
        }
    }

    sourceSets {
        getByName("main") {
            kotlin.directories.add(if (saysoNative) "src/native/kotlin" else "src/fake/kotlin")
            if (saysoNative) jniLibs.directories.add(file(saysoCoreDir).absolutePath)
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    buildFeatures {
        compose = true
    }

    testOptions {
        unitTests.isReturnDefaultValues = true
    }
}

dependencies {
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.activity.compose)
    implementation(platform(libs.compose.bom))
    implementation(libs.compose.ui)
    implementation(libs.compose.material3)
    implementation(libs.compose.ui.tooling.preview)
    testImplementation(libs.junit)
}
