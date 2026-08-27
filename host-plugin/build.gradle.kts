import org.gradle.api.tasks.bundling.AbstractArchiveTask
import org.gradle.api.tasks.bundling.Zip

plugins {
    java
}

repositories {
    mavenCentral()
}

val hmclJar = System.getenv("HMCL_JAR")?.let(::file)
    ?: error("Set HMCL_JAR to the Aura Launcher Next Shadow JAR")

dependencies {
    compileOnly(files(hmclJar))
    testImplementation(files(hmclJar))
    testImplementation(platform("org.junit:junit-bom:5.11.4"))
    testImplementation("org.junit.jupiter:junit-jupiter")
    testRuntimeOnly("org.junit.platform:junit-platform-launcher")
}

tasks.withType<JavaCompile>().configureEach {
    options.release.set(17)
}

tasks.withType<Test>().configureEach {
    useJUnitPlatform()
    systemProperty("hmcl.host.projectDir", projectDir.absolutePath)
    systemProperty("hmcl.launcher.jar", hmclJar.absolutePath)
}

tasks.withType<AbstractArchiveTask>().configureEach {
    isPreserveFileTimestamps = false
    isReproducibleFileOrder = true
}

tasks.jar {
    archiveBaseName.set("hmcl-rust-runtime-host")
}

val nativeLibrary = providers.environmentVariable("HMCL_RUST_NATIVE_LIBRARY")
val processHost = providers.environmentVariable("HMCL_RUST_PROCESS_HOST")
val nativePlatform = providers.environmentVariable("HMCL_RUST_PLATFORM")

tasks.register<Zip>("packageNpl") {
    dependsOn(tasks.jar)
    archiveFileName.set("dev.hmclce.runtime.rust-host-v0.1.0.npl")
    destinationDirectory.set(layout.buildDirectory.dir("npl"))
    from("plugin.json")
    into("libs") {
        from(tasks.jar)
    }
    into(nativePlatform.map { "native/$it" }) {
        from(nativeLibrary)
        from(processHost)
    }
    doFirst {
        val platform = nativePlatform.orNull
            ?: error("Set HMCL_RUST_PLATFORM to the native artifact platform")
        val library = nativeLibrary.orNull?.let(::file)
            ?: error("Set HMCL_RUST_NATIVE_LIBRARY to the native engine")
        val process = processHost.orNull?.let(::file)
            ?: error("Set HMCL_RUST_PROCESS_HOST to the isolated process Host")
        require(platform in setOf(
            "windows-x64", "windows-arm64", "linux-x64", "linux-arm64", "macos-x64", "macos-arm64"
        )) { "Unsupported Rust Host platform: $platform" }
        require(library.isFile) { "Rust Host native engine does not exist: $library" }
        require(process.isFile) { "Rust process Host does not exist: $process" }
        val expectedLibraryName = when {
            platform.startsWith("windows-") -> "hmcl_rust_host_native.dll"
            platform.startsWith("linux-") -> "libhmcl_rust_host_native.so"
            else -> "libhmcl_rust_host_native.dylib"
        }
        val expectedProcessName = if (platform.startsWith("windows-")) {
            "hmcl-rust-host-process.exe"
        } else {
            "hmcl-rust-host-process"
        }
        require(library.name == expectedLibraryName) {
            "Rust Host native engine for $platform must be named $expectedLibraryName"
        }
        require(process.name == expectedProcessName) {
            "Rust process Host for $platform must be named $expectedProcessName"
        }
    }
}
