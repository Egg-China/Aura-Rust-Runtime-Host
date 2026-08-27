package dev.hmclce.runtime.rust;

import org.jackhuang.hmcl.plugin.PluginArtifactIdentity;
import org.jackhuang.hmcl.plugin.PluginDataObject;
import org.jackhuang.hmcl.plugin.PluginDataValue;
import org.jackhuang.hmcl.plugin.PluginHookEvent;
import org.jackhuang.hmcl.plugin.PluginHookPoint;
import org.jackhuang.hmcl.plugin.PluginHookResult;
import org.jackhuang.hmcl.plugin.PluginManifest;
import org.jackhuang.hmcl.plugin.PluginSecretAccess;
import org.jackhuang.hmcl.plugin.bridge.BridgeValue;
import org.jackhuang.hmcl.plugin.bridge.PluginCapabilityToken;
import org.jackhuang.hmcl.plugin.bridge.PluginPermissionAuthority;
import org.jackhuang.hmcl.plugin.bridge.RuntimeBridgeWireCodec;
import org.jackhuang.hmcl.plugin.runtime.PluginExecutionMode;
import org.jackhuang.hmcl.plugin.runtime.PluginPlatformTarget;
import org.jackhuang.hmcl.plugin.runtime.RuntimeProvider;
import org.jackhuang.hmcl.plugin.runtime.RuntimeProviderDescriptor;
import org.jackhuang.hmcl.plugin.runtime.RuntimeBridgeTransport;
import org.jackhuang.hmcl.plugin.runtime.RuntimeFeature;
import org.jackhuang.hmcl.plugin.runtime.RuntimePayloadContext;
import org.jackhuang.hmcl.plugin.runtime.RuntimePayloadHandle;
import org.jetbrains.annotations.Nullable;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

import java.io.IOException;
import java.io.Reader;
import java.math.BigDecimal;
import java.nio.charset.StandardCharsets;
import java.nio.file.FileSystemException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;
import java.time.Duration;
import java.time.Instant;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Set;
import java.util.UUID;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.jar.JarFile;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertInstanceOf;
import static org.junit.jupiter.api.Assertions.assertNull;
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
        assertTrue(manifest.getProvidesRuntimes().get(0).getFeatures().contains(RuntimeFeature.HOOKS));
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

    /// Hook dispatch must cross the native boundary as one exact language-neutral envelope.
    @Test
    void dispatchesHookThroughCanonicalNativeOperation() throws Exception {
        PluginManifest manifest = readManifest();
        FakeEngine engine = new FakeEngine();
        engine.invocationResult = hookWire(mapOf(
                "contractVersion", BridgeValue.integer(1L),
                "action", BridgeValue.string("unchanged")
        ));
        RuntimeProvider provider = new RustRuntimeProvider(
                manifest.getId(), manifest.getVersion(), manifest.getProvidesRuntimes(), engine);
        RuntimeProvider.HookInvoker invoker = assertInstanceOf(RuntimeProvider.HookInvoker.class, provider);
        RuntimePayloadHandle handle = new RuntimePayloadHandle(
                "dev.hmclce.test.rust-payload", manifest.getId(), "native-41");
        PluginCapabilityToken token = hookToken();
        PluginHookEvent event = hookEvent();

        PluginHookResult result = invoker.invokeHook(handle, token, event, Duration.ofMillis(275));

        Map<String, BridgeValue> expectedData = new LinkedHashMap<>();
        expectedData.put("enabled", BridgeValue.bool(true));
        expectedData.put("attempts", BridgeValue.integer(3L));
        Map<String, BridgeValue> expectedEvent = new LinkedHashMap<>();
        expectedEvent.put("contractVersion", BridgeValue.integer(1L));
        expectedEvent.put("dispatchId", BridgeValue.string("dispatch-rust-42"));
        expectedEvent.put("point", BridgeValue.string("before-game-launch"));
        expectedEvent.put("occurredAt", BridgeValue.string("2026-08-27T12:34:56Z"));
        expectedEvent.put("data", BridgeValue.map(expectedData));
        assertEquals(PluginHookResult.Action.UNCHANGED, result.action());
        assertEquals("native-41", engine.invokedPayloadId);
        assertEquals("hook.before-game-launch", engine.invokedOperation);
        assertEquals(0L, engine.invokedCallbackId);
        assertEquals(BridgeValue.map(expectedEvent), RuntimeBridgeWireCodec.decode(engine.invokedInput));
    }

    /// Hook dispatch must decode every valid action and classify malformed native output as absent.
    @Test
    void decodesNativeHookResultsWithoutAcceptingMalformedOutput() throws Exception {
        PluginManifest manifest = readManifest();
        FakeEngine engine = new FakeEngine();
        RuntimeProvider provider = new RustRuntimeProvider(
                manifest.getId(), manifest.getVersion(), manifest.getProvidesRuntimes(), engine);
        RuntimeProvider.HookInvoker invoker = assertInstanceOf(RuntimeProvider.HookInvoker.class, provider);
        RuntimePayloadHandle handle = new RuntimePayloadHandle(
                "dev.hmclce.test.rust-payload", manifest.getId(), "native-41");
        PluginCapabilityToken token = hookToken();
        PluginHookEvent event = hookEvent();

        engine.invocationResult = hookWire(mapOf(
                "contractVersion", BridgeValue.integer(1L),
                "action", BridgeValue.string("unchanged")
        ));
        assertEquals(PluginHookResult.Action.UNCHANGED,
                invoker.invokeHook(handle, token, event, Duration.ofSeconds(1)).action());

        engine.invocationResult = hookWire(mapOf(
                "contractVersion", BridgeValue.integer(1L),
                "action", BridgeValue.string("replace"),
                "data", BridgeValue.map(mapOf("attempts", BridgeValue.integer(7L))),
                "protectedSecrets", BridgeValue.map(mapOf(
                        "access-token", BridgeValue.string("replacement")))
        ));
        PluginHookResult replacement = invoker.invokeHook(handle, token, event, Duration.ofSeconds(1));
        assertEquals(PluginHookResult.Action.REPLACE, replacement.action());
        assertEquals(new BigDecimal("7"), replacement.data().requireNumber("attempts"));
        assertEquals(Map.of("access-token", "replacement"), replacement.protectedSecrets());

        engine.invocationResult = hookWire(mapOf(
                "contractVersion", BridgeValue.integer(1L),
                "action", BridgeValue.string("cancel"),
                "reasonCode", BridgeValue.string("runtime-policy"),
                "message", BridgeValue.string("Blocked by Rust plugin")
        ));
        PluginHookResult cancellation = invoker.invokeHook(handle, token, event, Duration.ofSeconds(1));
        assertEquals(PluginHookResult.Action.CANCEL, cancellation.action());
        assertEquals("runtime-policy", cancellation.reasonCode());
        assertEquals("Blocked by Rust plugin", cancellation.message());

        engine.invocationResult = new byte[]{0x01, 0x02};
        assertNull(invoker.invokeHook(handle, token, event, Duration.ofSeconds(1)));
    }

    /// Hook dispatch must reject invalid Java authority, deadlines, and Provider ownership before native invocation.
    @Test
    void rejectsInvalidHookInvocationBoundary() throws Exception {
        PluginManifest manifest = readManifest();
        FakeEngine engine = new FakeEngine();
        engine.invocationResult = hookWire(mapOf(
                "contractVersion", BridgeValue.integer(1L),
                "action", BridgeValue.string("unchanged")
        ));
        RuntimeProvider provider = new RustRuntimeProvider(
                manifest.getId(), manifest.getVersion(), manifest.getProvidesRuntimes(), engine);
        RuntimeProvider.HookInvoker invoker = assertInstanceOf(RuntimeProvider.HookInvoker.class, provider);
        RuntimePayloadHandle handle = new RuntimePayloadHandle(
                "dev.hmclce.test.rust-payload", manifest.getId(), "native-41");
        PluginHookEvent event = hookEvent();

        assertThrows(NullPointerException.class,
                () -> invoker.invokeHook(handle, null, event, Duration.ofSeconds(1)));
        assertThrows(IllegalArgumentException.class,
                () -> invoker.invokeHook(handle, hookToken(), event, Duration.ZERO));
        assertThrows(IllegalArgumentException.class,
                () -> invoker.invokeHook(handle, hookToken(), event, Duration.ofMillis(-1)));
        RuntimePayloadHandle foreign = new RuntimePayloadHandle(
                "dev.hmclce.test.rust-payload", "dev.hmclce.runtime.foreign", "native-41");
        assertThrows(IOException.class,
                () -> invoker.invokeHook(foreign, hookToken(), event, Duration.ofSeconds(1)));
        assertNull(engine.invokedOperation);
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

    /// Creates one live opaque token whose Java identity must stop at the Provider boundary.
    ///
    /// @return live test capability token
    private static PluginCapabilityToken hookToken() {
        return new PluginPermissionAuthority().issue(
                new PluginArtifactIdentity("dev.hmclce.test.rust-payload", "1.0.0", "e".repeat(64)),
                PluginExecutionMode.EMBEDDED,
                Set.of(),
                "runtime.payload",
                Duration.ofMinutes(1)
        );
    }

    /// Creates one deterministic Hook event with ordinary data and a denied Java secret accessor.
    ///
    /// @return deterministic Hook event
    private static PluginHookEvent hookEvent() {
        Map<String, PluginDataValue> data = new LinkedHashMap<>();
        data.put("enabled", PluginDataValue.bool(true));
        data.put("attempts", PluginDataValue.number(new BigDecimal("3")));
        return new PluginHookEvent(
                1,
                "dispatch-rust-42",
                PluginHookPoint.BEFORE_GAME_LAUNCH,
                Instant.parse("2026-08-27T12:34:56Z"),
                PluginDataObject.of(data),
                PluginSecretAccess.denied("dev.hmclce.test.rust-payload")
        );
    }

    /// Creates one insertion-ordered Bridge map from alternating string keys and values.
    ///
    /// @param entries alternating string keys and Bridge values
    /// @return insertion-ordered Bridge map
    private static Map<String, BridgeValue> mapOf(Object... entries) {
        Map<String, BridgeValue> values = new LinkedHashMap<>();
        for (int index = 0; index < entries.length; index += 2) {
            values.put((String) entries[index], (BridgeValue) entries[index + 1]);
        }
        return values;
    }

    /// Encodes one manually constructed external Hook result.
    ///
    /// @param values exact result fields
    /// @return canonical Bridge Value v1 wire bytes
    /// @throws IOException if the test fixture cannot be encoded
    private static byte[] hookWire(Map<String, BridgeValue> values) throws IOException {
        return RuntimeBridgeWireCodec.encode(BridgeValue.map(values));
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

        /// Native payload identifier from the latest generic invocation.
        private @Nullable String invokedPayloadId;

        /// Native operation from the latest generic invocation.
        private @Nullable String invokedOperation;

        /// Wire input from the latest generic invocation.
        private byte @Nullable [] invokedInput;

        /// Callback identifier from the latest generic invocation.
        private long invokedCallbackId = Long.MIN_VALUE;

        /// Optional scripted result returned by the next and subsequent generic invocations.
        private byte @Nullable [] invocationResult;

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

        /// Records raw-byte payload invocations and returns the configured result or an input echo.
        @Override
        public byte[] invokePayload(String payloadId, String operation, byte[] input, long callbackId) {
            payloadEvents.add("invoke:" + payloadId + ":" + operation + ":" + callbackId);
            invokedPayloadId = payloadId;
            invokedOperation = operation;
            invokedInput = input.clone();
            invokedCallbackId = callbackId;
            return invocationResult == null ? input.clone() : invocationResult.clone();
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
