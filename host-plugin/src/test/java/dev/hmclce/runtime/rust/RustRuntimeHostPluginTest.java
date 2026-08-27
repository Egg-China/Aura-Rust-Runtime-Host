package dev.hmclce.runtime.rust;

import org.jackhuang.hmcl.plugin.PluginManifest;
import org.jackhuang.hmcl.plugin.runtime.PluginPlatformTarget;
import org.jackhuang.hmcl.plugin.runtime.RuntimeProvider;
import org.jackhuang.hmcl.plugin.runtime.RuntimeProviderDescriptor;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

import java.io.IOException;
import java.io.Reader;
import java.nio.charset.StandardCharsets;
import java.nio.file.FileSystemException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.jar.JarFile;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertSame;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;
import static org.junit.jupiter.api.Assumptions.assumeTrue;

/// Verifies the optional Rust Runtime Host package boundary and bootstrap lifecycle.
final class RustRuntimeHostPluginTest {
    /// Temporary filesystem root used by native path tests.
    @TempDir
    Path temporaryDirectory;

    /// A descriptor built by the Host must exactly mirror its schema-v5 package declaration.
    @Test
    void providerDescriptorMatchesManifest() throws IOException {
        PluginManifest manifest = readManifest();
        RustRuntimeProvider provider = new RustRuntimeProvider(
                manifest.getId(), manifest.getVersion(), manifest.getProvidesRuntimes(), new FakeEngine());

        RuntimeProviderDescriptor descriptor = provider.descriptor();

        assertEquals(manifest.getId(), descriptor.providerId());
        assertEquals(manifest.getVersion(), descriptor.version());
        assertEquals(manifest.getProvidesRuntimes(), List.copyOf(descriptor.capabilities().values()));
        assertTrue(descriptor.installed());
        assertTrue(descriptor.enabled());
        assertFalse(descriptor.reserved());
    }

    /// Every supported build target must select one stable package-relative native engine path.
    @Test
    void mapsEverySupportedPlatformToItsNativeLibrary() {
        assertEquals(Path.of("native/windows-x64/hmcl_rust_host_native.dll"),
                RustNativeEngine.nativeLibraryPath(PluginPlatformTarget.parse("windows-x64")));
        assertEquals(Path.of("native/windows-arm64/hmcl_rust_host_native.dll"),
                RustNativeEngine.nativeLibraryPath(PluginPlatformTarget.parse("windows-arm64")));
        assertEquals(Path.of("native/linux-x64/libhmcl_rust_host_native.so"),
                RustNativeEngine.nativeLibraryPath(PluginPlatformTarget.parse("linux-x64")));
        assertEquals(Path.of("native/linux-arm64/libhmcl_rust_host_native.so"),
                RustNativeEngine.nativeLibraryPath(PluginPlatformTarget.parse("linux-arm64")));
        assertEquals(Path.of("native/macos-x64/libhmcl_rust_host_native.dylib"),
                RustNativeEngine.nativeLibraryPath(PluginPlatformTarget.parse("macos-x64")));
        assertEquals(Path.of("native/macos-arm64/libhmcl_rust_host_native.dylib"),
                RustNativeEngine.nativeLibraryPath(PluginPlatformTarget.parse("macos-arm64")));
        assertThrows(IllegalArgumentException.class,
                () -> RustNativeEngine.nativeLibraryPath(PluginPlatformTarget.parse("windows-x86")));
    }

    /// Native loading must use the canonical file beneath the extracted package root exactly once.
    @Test
    void loadsCanonicalNativeLibraryOnce() throws IOException {
        Path expected = createNativeLibrary(temporaryDirectory, "windows-x64", "hmcl_rust_host_native.dll");
        AtomicInteger loads = new AtomicInteger();
        List<Path> loadedPaths = new ArrayList<>();

        RustNativeEngine engine = RustNativeEngine.load(
                temporaryDirectory,
                PluginPlatformTarget.parse("windows-x64"),
                path -> {
                    loads.incrementAndGet();
                    loadedPaths.add(path);
                },
                new FakeNativeBindings());

        assertEquals(1, loads.get());
        assertEquals(List.of(expected.toRealPath()), loadedPaths);
        engine.close();
    }

    /// Resolving a symlink outside the package root must fail before native code is loaded.
    @Test
    void rejectsNativeSymlinkEscape() throws IOException {
        Path nativeDirectory = temporaryDirectory.resolve("native/windows-x64");
        Files.createDirectories(nativeDirectory);
        Path outside = Files.createTempFile("hmcl-rust-host-outside", ".dll");
        Path link = nativeDirectory.resolve("hmcl_rust_host_native.dll");
        try {
            Files.createSymbolicLink(link, outside);
        } catch (UnsupportedOperationException | FileSystemException exception) {
            assumeTrue(false, "Symbolic links are unavailable: " + exception.getMessage());
        }
        try {
            assertThrows(IOException.class, () -> RustNativeEngine.resolveNativeLibrary(
                    temporaryDirectory, PluginPlatformTarget.parse("windows-x64")));
        } finally {
            Files.deleteIfExists(outside);
        }
    }

    /// Host load must publish one Provider and unload must release registration before the engine.
    @Test
    void registersOnceAndUnloadsInReverseOrder() throws IOException {
        PluginManifest manifest = readManifest();
        FakeEngine engine = new FakeEngine();
        List<String> events = new ArrayList<>();
        AtomicInteger registrations = new AtomicInteger();
        RustRuntimeHostPlugin plugin = new RustRuntimeHostPlugin((root, platform) -> engine,
                () -> PluginPlatformTarget.parse("windows-x64"));

        plugin.load(new RustRuntimeHostPlugin.HostContext() {
            @Override
            public PluginManifest manifest() {
                return manifest;
            }

            @Override
            public Path packageDirectory() {
                return temporaryDirectory;
            }

            @Override
            public RustRuntimeHostPlugin.Registration register(RuntimeProvider provider) {
                registrations.incrementAndGet();
                assertEquals(manifest.getProvidesRuntimes(),
                        List.copyOf(provider.descriptor().capabilities().values()));
                return () -> events.add("registration");
            }
        });
        engine.closeEvent = () -> events.add("engine");

        plugin.onUnload();
        plugin.onUnload();

        assertEquals(1, registrations.get());
        assertEquals(List.of("registration", "engine"), events);
        assertSame(manifest, plugin.getManifest());
    }

    /// Provider health failures must remain observable and its native engine cleanup must be idempotent.
    @Test
    void propagatesHealthFailureAndClosesEngineOnce() throws IOException {
        FakeEngine engine = new FakeEngine();
        engine.healthy = false;
        RustRuntimeProvider provider = new RustRuntimeProvider(
                readManifest().getId(), "0.1.0", readManifest().getProvidesRuntimes(), engine);

        provider.initialize();
        assertFalse(provider.healthCheck());
        provider.close();
        provider.close();

        assertEquals(1, engine.initializeCalls);
        assertEquals(1, engine.healthCalls);
        assertEquals(1, engine.closeCalls);
    }

    /// A failed registration must immediately close the loaded native engine.
    @Test
    void registrationFailureClosesEngine() throws IOException {
        PluginManifest manifest = readManifest();
        FakeEngine engine = new FakeEngine();
        RustRuntimeHostPlugin plugin = new RustRuntimeHostPlugin((root, platform) -> engine,
                () -> PluginPlatformTarget.parse("windows-x64"));

        assertThrows(IllegalStateException.class, () -> plugin.load(new RustRuntimeHostPlugin.HostContext() {
            @Override
            public PluginManifest manifest() {
                return manifest;
            }

            @Override
            public Path packageDirectory() {
                return temporaryDirectory;
            }

            @Override
            public RustRuntimeHostPlugin.Registration register(RuntimeProvider provider) {
                throw new IllegalStateException("registration rejected");
            }
        }));

        assertEquals(1, engine.closeCalls);
    }

    /// The launcher distribution must not bundle this optional Host or any Rust native engine.
    @Test
    void launcherShadowJarDoesNotBundleRustHost() throws IOException {
        Path launcherJar = Path.of(System.getProperty("hmcl.launcher.jar"));
        try (JarFile jar = new JarFile(launcherJar.toFile())) {
            assertFalse(jar.stream().anyMatch(entry ->
                    entry.getName().startsWith("dev/hmclce/runtime/rust/")
                            || entry.getName().contains("hmcl_rust_host_native")
                            || entry.getName().endsWith(".dylib")));
        }
    }

    /// Reads the authoritative manifest from the Host project root.
    private static PluginManifest readManifest() throws IOException {
        Path projectDirectory = Path.of(System.getProperty("hmcl.host.projectDir"));
        try (Reader reader = Files.newBufferedReader(projectDirectory.resolve("plugin.json"), StandardCharsets.UTF_8)) {
            return PluginManifest.fromJson(reader);
        }
    }

    /// Creates one native library fixture at the exact packaged target path.
    private static Path createNativeLibrary(Path root, String platform, String filename) throws IOException {
        Path library = root.resolve("native").resolve(platform).resolve(filename);
        Files.createDirectories(library.getParent());
        Files.write(library, new byte[]{0x48, 0x4d, 0x43, 0x4c});
        return library;
    }

    /// In-memory provider engine used to observe lifecycle behavior without native code.
    private static final class FakeEngine implements RustRuntimeProvider.Engine {
        /// Whether health negotiation succeeds.
        private boolean healthy = true;

        /// Initialization call count.
        private int initializeCalls;

        /// Health call count.
        private int healthCalls;

        /// Close call count.
        private int closeCalls;

        /// Optional close event recorder.
        private Runnable closeEvent = () -> {
        };

        /// Records engine initialization.
        @Override
        public void initialize() {
            initializeCalls++;
        }

        /// Returns the configured health result.
        @Override
        public boolean healthCheck() {
            healthCalls++;
            return healthy;
        }

        /// Records the first engine close.
        @Override
        public void close() {
            if (closeCalls++ == 0) {
                closeEvent.run();
            }
        }
    }

    /// In-memory JNI bindings used to verify native library path loading independently of JNI.
    private static final class FakeNativeBindings implements RustNativeEngine.NativeBindings {
        /// Returns a stable nonzero engine handle.
        @Override
        public long create() {
            return 1;
        }

        /// Reports a healthy engine.
        @Override
        public boolean healthCheck(long handle) {
            return true;
        }

        /// Accepts engine teardown.
        @Override
        public void destroy(long handle) {
        }
    }
}
