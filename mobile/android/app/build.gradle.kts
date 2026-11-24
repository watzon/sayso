plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.compose)
}

// The engine: the native core (JNI to libsayso_core.so) when it has been built for
// both ABIs (mobile/scripts/build-core-android.sh), else the Kotlin fake.
// -PsaysoNative=true|false forces one. -PsaysoCoreDir points at a folder with one
// subfolder per ABI holding libsayso_core.so (default mobile/build/jniLibs).
// -PsaysoHostLibDir: a folder with the core and the JNI library built for this
// computer (build-android.sh makes it); the JVM tests then drive the real core.
val coreAbis = listOf("arm64-v8a", "x86_64")
val saysoCoreDir = file(providers.gradleProperty("saysoCoreDir").getOrElse("../../build/jniLibs"))
val coreBuilt = coreAbis.all { File(saysoCoreDir, "$it/libsayso_core.so").exists() }
val saysoNative = providers.gradleProperty("saysoNative").map { it.toBoolean() }.getOrElse(coreBuilt)
val saysoHostLibDir = providers.gradleProperty("saysoHostLibDir").orNull
val modelsDir = file("../../models")

// Local speech recognition in the :asr process: the sherpa-onnx Android AAR that
// mobile/scripts/fetch-sherpa-onnx.sh downloads and checks. Without it the build
// has no local engine (src/nosherpa) and voice uses the system recognizer.
// -PsaysoSherpaAar points at another copy.
val sherpaAar = file(providers.gradleProperty("saysoSherpaAar").getOrElse("../../build/sherpa-onnx/sherpa-onnx-2.0.3.aar"))
val saysoSherpa = sherpaAar.exists()
// -PsaysoAsrTestDir=<dir>: lib/ with the sherpa-onnx JNI library for this computer and
// models/<id>/ with catalog models; SherpaLocalAsrTest then decodes real speech.
val saysoAsrTestDir = providers.gradleProperty("saysoAsrTestDir").orNull

// The license texts of sherpa-onnx and ONNX Runtime, shown in the acknowledgements
// when the APK carries them; the same files ship with the desktop app.
val speechLicenses = layout.buildDirectory.dir("generated/speechLicenses")
val copySpeechLicenses = tasks.register<Sync>("copySpeechLicenses") {
    from(file("../../../packaging/licenses")) {
        include("sherpa-onnx.txt", "onnxruntime.txt")
    }
    into(speechLicenses.map { it.dir("licenses") })
}

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
        // sherpa-onnx ships four ABIs; the app supports these two.
        ndk { abiFilters += coreAbis }
        if (saysoNative) {
            externalNativeBuild {
                cmake {
                    arguments += "-DSAYSO_CORE_DIR=${saysoCoreDir.absolutePath}"
                    arguments += "-DSAYSO_CORE_INCLUDE=${file("../../core/capi").absolutePath}"
                }
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
            kotlin.directories.add(if (saysoSherpa) "src/sherpa/kotlin" else "src/nosherpa/kotlin")
            if (saysoSherpa) assets.directories.add(speechLicenses.get().asFile.absolutePath)
            if (saysoNative) {
                jniLibs.directories.add(saysoCoreDir.absolutePath)
                // en-US.sslm, and SCOWL-Copyright.txt for the acknowledgements.
                assets.directories.add(modelsDir.absolutePath)
            }
        }
        if (saysoNative) getByName("test").kotlin.directories.add("src/testNative/kotlin")
        if (saysoSherpa) getByName("test").kotlin.directories.add("src/testSherpa/kotlin")
    }

    // The core maps the model straight out of the APK, which needs it stored uncompressed.
    androidResources {
        noCompress += "sslm"
    }

    // Compress native libraries in the APK: sherpa-onnx's library is about 25 MB
    // per ABI and shrinks to about 10 MB. Assets are not affected, so the .sslm
    // model stays stored uncompressed.
    packaging {
        jniLibs { useLegacyPackaging = true }
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
        unitTests.all { test ->
            test.systemProperty("sayso.model", File(modelsDir, "en-US.sslm").absolutePath)
            val libPath = listOfNotNull(
                saysoHostLibDir?.let { file(it).absolutePath },
                saysoAsrTestDir?.let { File(file(it), "lib").absolutePath },
            )
            if (libPath.isNotEmpty()) test.systemProperty("java.library.path", libPath.joinToString(File.pathSeparator))
            if (saysoHostLibDir != null) test.systemProperty("sayso.hostLibs", "true")
            if (saysoAsrTestDir != null) test.systemProperty("sayso.asrTestDir", file(saysoAsrTestDir).absolutePath)
        }
    }
}

if (saysoSherpa) tasks.named("preBuild") { dependsOn(copySpeechLicenses) }

dependencies {
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.recyclerview)
    implementation(libs.androidx.activity.compose)
    implementation(platform(libs.compose.bom))
    implementation(libs.compose.ui)
    implementation(libs.compose.material3)
    implementation(libs.compose.ui.tooling.preview)
    if (saysoSherpa) implementation(files(sherpaAar))
    testImplementation(libs.junit)
}
