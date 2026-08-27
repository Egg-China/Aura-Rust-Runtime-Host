package dev.hmclce.runtime.rust;

import org.jackhuang.hmcl.plugin.PluginManifest;
import org.jackhuang.hmcl.plugin.PluginArtifactIdentity;
import org.jackhuang.hmcl.plugin.runtime.PluginExecutionMode;
import org.jackhuang.hmcl.plugin.runtime.PluginPlatformTarget;
import org.jackhuang.hmcl.plugin.runtime.RuntimeProvider;
import org.jackhuang.hmcl.plugin.runtime.RuntimeProviderDescriptor;
import org.jackhuang.hmcl.plugin.runtime.RuntimeBridgeTransport;
import org.jackhuang.hmcl.plugin.runtime.RuntimePayloadContext;
import org.jackhuang.hmcl.plugin.runtime.RuntimePayloadHandle;
import org.jetbrains.annotations.Nullable;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

import java.io.IOException;
import java.io.Reader;
import java.nio.charset.StandardCharsets;
import java.nio.file.FileSystemException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;
import java.util.ArrayList;
import java.util.List;
import java.util.UUID;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.jar.JarFile;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
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

    /// Provider payload lifecycle calls must preserve owner identity and delegate by opaque native ID.
    @Test
    void delegatesPayloadLifecycleToNativeEngine() throws IOException {
        PluginManifest manifest = readManifest();
        FakeEngine engine = new FakeEngine();
        RustRuntimeProvider provider = new RustRuntimeProvider(
                manifest.getId(), manifest.getVersion(), manifest.getProvidesRuntimes(), engine);
        RuntimePayloadContext context = new RuntimePayloadContext(
                new PluginArtifactIdentity("dev.hmclce.test.rust-payload", "1.0.0", "a".repeat(64)),
                temporaryDirectory,
                "payload/plugin.dll",
                PluginExecutionMode.EMBEDDED,
                temporaryDirectory.resolve("data"),
                () -> {
                    throw new AssertionError("Java capability token bytes must not cross into Rust");
                }
        );

        RuntimePayloadHandle handle = provider.loadPayload(context);
        provider.enablePayload(handle);
        provider.disablePayload(handle);
        provider.unloadPayload(handle);

        assertSame(context, engine.loadedContext);
        assertEquals("dev.hmclce.test.rust-payload", handle.ownerPluginId());
        assertEquals(manifest.getId(), handle.providerId());
        assertEquals("native-41", handle.payloadId());
        assertEquals(List.of("enable:native-41", "disable:native-41", "unload:native-41"),
                engine.payloadEvents);
    }

    /// JNI Bridge callbacks must resolve only the Java context mapped to their opaque Host session.
    @Test
    void resolvesBridgeCallsThroughOpaqueSessionHandle() throws IOException {
        Path library = createNativeLibrary(temporaryDirectory, "windows-x64", "hmcl_rust_host_native.dll");
        FakeNativeBindings bindings = new FakeNativeBindings();
        List<String> calls = new ArrayList<>();
        RustNativeEngine engine = RustNativeEngine.load(
                temporaryDirectory,
                PluginPlatformTarget.parse("windows-x64"),
                ignored -> {
                },
                bindings,
                new RustNativeEngine.BridgeAdapter() {
                    @Override
                    public byte[] invoke(RuntimePayloadContext context, String operation, byte[] input) {
                        calls.add(context.artifactIdentity().getPluginId() + ":" + operation);
                        return input.clone();
                    }

                    @Override
                    public void retainHandle(RuntimePayloadContext context, long objectId, long generation) {
                        calls.add("retain:" + objectId + ":" + generation);
                    }

                    @Override
                    public void releaseHandle(RuntimePayloadContext context, long objectId, long generation) {
                        calls.add("release:" + objectId + ":" + generation);
                    }
                }
        );
        engine.initialize();
        assertEquals(library.toRealPath(), bindings.loadedOwnerLibrary);
        RuntimePayloadContext context = new RuntimePayloadContext(
                new PluginArtifactIdentity("dev.hmclce.test.bridge-payload", "1.0.0", "b".repeat(64)),
                temporaryDirectory,
                "payload/plugin.dll",
                PluginExecutionMode.EMBEDDED,
                temporaryDirectory.resolve("data"),
                () -> {
                    throw new AssertionError("Opaque session mapping must not resolve token bytes");
                }
        );
        String payloadId = engine.loadPayload(context);

        assertArrayEquals(new byte[]{1, 2, 3},
                engine.invokeBridge(1L, "core.launcher-version", new byte[]{1, 2, 3}));
        engine.retainBridgeHandle(1L, 7L, 9L);
        engine.releaseBridgeHandle(1L, 7L, 9L);
        engine.unloadPayload(payloadId);

        assertEquals(List.of(
                "dev.hmclce.test.bridge-payload:core.launcher-version",
                "retain:7:9",
                "release:7:9"
        ), calls);
    }

    /// Rolls back every unpublished Java session after native load failures or duplicate handles.
    @Test
    void rollsBackSessionLookupAfterEveryLoadPublicationFailure() throws IOException {
        createNativeLibrary(temporaryDirectory, "windows-x64", "hmcl_rust_host_native.dll");
        FakeNativeBindings bindings = new FakeNativeBindings();
        RustNativeEngine engine = RustNativeEngine.load(
                temporaryDirectory,
                PluginPlatformTarget.parse("windows-x64"),
                ignored -> {
                },
                bindings,
                new RustNativeEngine.BridgeAdapter() {
                    @Override
                    public byte[] invoke(RuntimePayloadContext context, String operation, byte[] input) {
                        return input.clone();
                    }

                    @Override
                    public void retainHandle(RuntimePayloadContext context, long objectId, long generation) {
                    }

                    @Override
                    public void releaseHandle(RuntimePayloadContext context, long objectId, long generation) {
                    }
                }
        );
        engine.initialize();
        RuntimePayloadContext context = new RuntimePayloadContext(
                new PluginArtifactIdentity("dev.hmclce.test.rollback", "1.0.0", "d".repeat(64)),
                temporaryDirectory,
                "payload/plugin.dll",
                PluginExecutionMode.EMBEDDED,
                temporaryDirectory.resolve("data"),
                () -> {
                    throw new AssertionError("Session rollback must not inspect token bytes");
                }
        );

        bindings.loadFailure = new AssertionError("Requested native linkage failure");
        assertThrows(AssertionError.class, () -> engine.loadPayload(context));
        assertUnknownSession(engine, 1L);

        bindings.payloadHandle = 0L;
        assertThrows(IOException.class, () -> engine.loadPayload(context));
        assertUnknownSession(engine, 2L);

        bindings.payloadHandle = 7L;
        assertEquals("7", engine.loadPayload(context));
        assertThrows(IOException.class, () -> engine.loadPayload(context));
        assertArrayEquals(new byte[]{4, 5}, engine.invokeBridge(3L, "probe", new byte[]{4, 5}));
        assertUnknownSession(engine, 4L);

        engine.unloadPayload("7");
        assertUnknownSession(engine, 3L);
        engine.close();
    }

    /// The production JNI table must drive a real embedded Rust payload and its Java Bridge callbacks.
    @Test
    void loadsEmbeddedPayloadThroughRealJniBridge() throws IOException {
        String nativeArtifact = System.getenv("HMCL_RUST_NATIVE_LIBRARY");
        String fixtureArtifact = System.getenv("HMCL_RUST_EMBEDDED_FIXTURE");
        assumeTrue(nativeArtifact != null && fixtureArtifact != null,
                "Set native Host and embedded fixture artifacts to run JNI integration");
        PluginPlatformTarget platform = PluginPlatformTarget.current();
        assumeTrue(platform.getId().startsWith("windows-"), "Current integration fixture targets Windows");

        Path integrationRoot = Path.of(
                System.getProperty("hmcl.host.projectDir"),
                "build",
                "jni-test",
                UUID.randomUUID().toString()
        );
        Path nativeLibrary = integrationRoot.resolve(RustNativeEngine.nativeLibraryPath(platform));
        Files.createDirectories(nativeLibrary.getParent());
        Files.copy(Path.of(nativeArtifact), nativeLibrary, StandardCopyOption.REPLACE_EXISTING);
        Path payloadLibrary = integrationRoot.resolve("payload").resolve("plugin.dll");
        Files.createDirectories(payloadLibrary.getParent());
        Files.copy(Path.of(fixtureArtifact), payloadLibrary, StandardCopyOption.REPLACE_EXISTING);
        List<String> calls = new ArrayList<>();
        RustNativeEngine engine = RustNativeEngine.load(integrationRoot, platform);
        engine.initialize();
        RuntimePayloadContext context = new RuntimePayloadContext(
                new PluginArtifactIdentity("dev.hmclce.test.real-jni", "1.0.0", "c".repeat(64)),
                integrationRoot,
                "payload/plugin.dll",
                PluginExecutionMode.EMBEDDED,
                integrationRoot.resolve("data"),
                () -> {
                    throw new AssertionError("JNI must preserve opaque Java session lookup");
                },
                new RuntimeBridgeTransport() {
                    @Override
                    public byte[] invoke(RuntimePayloadContext current, String operation, byte[] input) {
                        calls.add(current.artifactIdentity().getPluginId() + ":" + operation);
                        return input.clone();
                    }

                    @Override
                    public void retainHandle(RuntimePayloadContext current, long objectId, long generation) {
                        calls.add("retain:" + objectId + ":" + generation);
                    }

                    @Override
                    public void releaseHandle(RuntimePayloadContext current, long objectId, long generation) {
                        calls.add("release:" + objectId + ":" + generation);
                    }
                }
        );

        String payload = engine.loadPayload(context);
        engine.enablePayload(payload);
        byte[] wireNull = {(byte) 0x92, 0x00, (byte) 0xc0};
        assertArrayEquals(wireNull, engine.invokePayload(payload, "bridge", wireNull, 41L));
        byte[] wireHandle = {
                (byte) 0x92, 0x08, (byte) 0x93,
                (byte) 0xcf, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x07,
                (byte) 0xcf, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x09,
                (byte) 0xdb, 0x00, 0x00, 0x00, 0x07, 'u', 'i', '.', 'p', 'a', 'g', 'e'
        };
        assertArrayEquals(wireHandle, engine.invokePayload(payload, "handle", wireHandle, 43L));
        engine.disablePayload(payload);
        engine.unloadPayload(payload);
        engine.close();

        assertEquals(List.of(
                "dev.hmclce.test.real-jni:initialize",
                "dev.hmclce.test.real-jni:fixture.bridge",
                "retain:7:9",
                "release:7:9",
                "dev.hmclce.test.real-jni:shutdown"
        ), calls);
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

    /// Verifies that one opaque Host session no longer resolves to a Java payload context.
    ///
    /// @param engine engine under test
    /// @param session expected unknown session
    private static void assertUnknownSession(RustNativeEngine engine, long session) {
        IOException exception = assertThrows(IOException.class,
                () -> engine.invokeBridge(session, "probe", new byte[0]));
        assertTrue(exception.getMessage().contains("Unknown Rust Host session"));
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

        /// Last payload context accepted by the fake native engine.
        private RuntimePayloadContext loadedContext;

        /// Payload lifecycle events after loading.
        private final List<String> payloadEvents = new ArrayList<>();

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

        /// Records one payload load without resolving its capability token supplier.
        @Override
        public String loadPayload(RuntimePayloadContext context) {
            loadedContext = context;
            return "native-41";
        }

        /// Records payload enablement.
        @Override
        public void enablePayload(String payloadId) {
            payloadEvents.add("enable:" + payloadId);
        }

        /// Records payload disablement.
        @Override
        public void disablePayload(String payloadId) {
            payloadEvents.add("disable:" + payloadId);
        }

        /// Echoes raw-byte payload invocations.
        @Override
        public byte[] invokePayload(String payloadId, String operation, byte[] input, long callbackId) {
            payloadEvents.add("invoke:" + payloadId + ":" + operation + ":" + callbackId);
            return input.clone();
        }

        /// Records payload unload.
        @Override
        public void unloadPayload(String payloadId) {
            payloadEvents.add("unload:" + payloadId);
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
        /// Exact owner native library observed before engine construction.
        private Path loadedOwnerLibrary;

        /// Payload handle returned by the next and subsequent native loads.
        private long payloadHandle = 1L;

        /// Optional linkage-style failure thrown by the next native load.
        private @Nullable Error loadFailure;

        /// Returns a stable nonzero engine handle.
        @Override
        public long create(RustNativeEngine owner) {
            loadedOwnerLibrary = owner.loadedLibrary();
            return 1;
        }

        /// Reports a healthy engine.
        @Override
        public boolean healthCheck(long handle) {
            return true;
        }

        /// Returns a stable positive payload handle.
        @Override
        public long loadPayload(
                long handle,
                String packageRoot,
                String entrypoint,
                long pluginId,
                long session
        ) {
            @Nullable Error failure = loadFailure;
            loadFailure = null;
            if (failure != null) {
                throw failure;
            }
            return payloadHandle;
        }

        /// Accepts payload enablement.
        @Override
        public void enablePayload(long handle, long payloadId) {
        }

        /// Accepts payload disablement.
        @Override
        public void disablePayload(long handle, long payloadId) {
        }

        /// Echoes one fake native payload invocation.
        @Override
        public byte[] invokePayload(
                long handle,
                long payloadId,
                String operation,
                byte[] input,
                long callbackId
        ) {
            return input.clone();
        }

        /// Accepts payload unload.
        @Override
        public void unloadPayload(long handle, long payloadId) {
        }

        /// Accepts engine teardown.
        @Override
        public void destroy(long handle) {
        }
    }
}
