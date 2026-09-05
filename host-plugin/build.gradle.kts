import org.gradle.api.tasks.bundling.AbstractArchiveTask
import org.gradle.api.tasks.bundling.Zip
import groovy.json.JsonOutput
import groovy.json.JsonSlurper

plugins {
    java
}

repositories {
    mavenCentral()
}

val hmclJar = System.getenv("HMCL_JAR")?.let(::file)
    ?: error("Set HMCL_JAR to the Aura Launcher Next Shadow JAR")
val launchHook = providers.environmentVariable("AURA_RUST_LAUNCH_HOOK")

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
    launchHook.orNull?.let { systemProperty("aura.rust.launchHook", it) }
    systemProperty("aura.rust.launchHookSample", file("../examples/launch-hook").absolutePath)
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
val generatedNplManifest = layout.buildDirectory.file("generated/npl/plugin.json")

val generateNplManifest = tasks.register("generateNplManifest") {
    inputs.file("plugin.json")
    inputs.property("nativePlatform", nativePlatform)
    outputs.file(generatedNplManifest)
    doLast {
        val platform = nativePlatform.orNull
            ?: error("Set HMCL_RUST_PLATFORM to the native artifact platform")
        val manifest = JsonSlurper().parse(file("plugin.json")) as MutableMap<String, Any>
        manifest["platforms"] = listOf(platform)
        val output = generatedNplManifest.get().asFile
        output.parentFile.mkdirs()
        output.writeText(JsonOutput.toJson(manifest) + "\n", Charsets.UTF_8)
    }
}

tasks.register<Zip>("packageNpl") {
    dependsOn(tasks.jar, generateNplManifest)
    archiveFileName.set("dev.hmclce.runtime.rust-host-v0.2.0-beta.1.npl")
    destinationDirectory.set(layout.buildDirectory.dir("npl"))
    from(generatedNplManifest)
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
