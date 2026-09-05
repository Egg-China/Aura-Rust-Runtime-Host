package dev.hmclce.runtime.rust;

import org.jackhuang.hmcl.plugin.PluginArtifactIdentity;
import org.jackhuang.hmcl.plugin.PluginDataObject;
import org.jackhuang.hmcl.plugin.PluginDataValue;
import org.jackhuang.hmcl.plugin.PluginHookEvent;
import org.jackhuang.hmcl.plugin.PluginHookPoint;
import org.jackhuang.hmcl.plugin.PluginHookResult;
import org.jackhuang.hmcl.plugin.PluginPatchDeclaration;
import org.jackhuang.hmcl.plugin.PluginPatchInvocation;
import org.jackhuang.hmcl.plugin.PluginPatchResult;
import org.jackhuang.hmcl.plugin.PluginSecretAccess;
import org.jackhuang.hmcl.plugin.bridge.PluginCapabilityToken;
import org.jackhuang.hmcl.plugin.bridge.PluginPermissionAuthority;
import org.jackhuang.hmcl.plugin.runtime.PluginExecutionMode;
import org.jackhuang.hmcl.plugin.runtime.RuntimeFeature;
import org.jackhuang.hmcl.plugin.runtime.RuntimePatchWireCodec;
import org.jackhuang.hmcl.plugin.runtime.RuntimePayloadContext;
import org.jackhuang.hmcl.plugin.runtime.RuntimePayloadHandle;
import org.jackhuang.hmcl.plugin.runtime.RuntimeProvider;
import org.jackhuang.hmcl.plugin.runtime.RuntimeProviderDeclaration;
import org.jetbrains.annotations.NotNullByDefault;
import org.jetbrains.annotations.Nullable;
import org.jetbrains.annotations.Unmodifiable;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.Timeout;
import org.junit.jupiter.api.io.TempDir;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.time.Duration;
import java.time.Instant;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.Set;
import java.util.concurrent.TimeUnit;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertInstanceOf;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

/// Verifies Hook and Patch callbacks against the real Rust process Host and launch-hook cdylib.
@NotNullByDefault
final class RustRuntimeProcessIntegrationTest {
    /// Isolated package and data root owned by the test payload.
    @TempDir
    Path temporaryDirectory;

    /// Runs the complete process lifecycle, rejects an unknown same-Provider handle, and proves one Patch response
    /// cannot be decoded twice.
    ///
    /// @throws Exception if process startup, lifecycle, Hook, Patch, or cleanup fails
    @Test
    @Timeout(value = 60L, unit = TimeUnit.SECONDS)
    void invokesCanonicalHookAndPatchAcrossRealRustProcess() throws Exception {
        RustRuntimeProvider provider = productionProvider(processHost());
        @Nullable RuntimePayloadHandle handle = null;
        boolean enabled = false;
        try {
            provider.initialize();
            assertTrue(provider.healthCheck());
            handle = provider.loadPayload(payloadContext());
            provider.enablePayload(handle);
            enabled = true;

            RuntimeProvider.HookInvoker hookInvoker = assertInstanceOf(RuntimeProvider.HookInvoker.class, provider);
            RuntimePayloadHandle unknown = new RuntimePayloadHandle(
                    handle.ownerPluginId(), handle.providerId(), "unknown-rust-process-payload");
            assertThrows(IOException.class,
                    () -> hookInvoker.invokeHook(unknown, capabilityToken(), hookEvent(), Duration.ofSeconds(2L)));
            PluginHookResult hookResult = Objects.requireNonNull(
                    hookInvoker.invokeHook(handle, capabilityToken(), hookEvent(), Duration.ofSeconds(2L)),
                    "Rust process Hook payload returned a malformed result"
            );
            assertEquals(PluginHookResult.Action.UNCHANGED, hookResult.action());
            assertUnchangedAfterFileNamePatch(provider, handle);

            provider.disablePayload(handle);
            enabled = false;
            provider.unloadPayload(handle);
            handle = null;
        } finally {
            closePayload(provider, handle, enabled);
        }
    }

    /// Invokes the observational after Patch and proves its invocation-local handle table expires after decoding.
    ///
    /// @param provider real Rust Provider
    /// @param handle enabled payload handle
    /// @throws IOException if the process or Patch wire exchange fails
    private void assertUnchangedAfterFileNamePatch(RustRuntimeProvider provider, RuntimePayloadHandle handle)
            throws IOException {
        PluginPatchDeclaration declaration = new PluginPatchDeclaration(
                "org.jackhuang.hmcl.util.io.FileUtils", "getName", PluginPatchDeclaration.PatchType.AFTER,
                List.of("java.nio.file.Path"));
        PluginPatchInvocation invocation = PluginPatchInvocation.after(
                declaration, null, List.of(temporaryDirectory.resolve("profile.json")), "profile.json");
        try (RuntimePatchWireCodec codec = new RuntimePatchWireCodec()) {
            byte @Unmodifiable [] response = provider.invokePayload(
                    handle, "aura.patch.v1", codec.encodeInvocation(invocation), 0L);
            assertEquals(PluginPatchResult.Action.UNCHANGED, codec.decodeResult(response, invocation).action());
            assertThrows(IOException.class, () -> codec.decodeResult(response, invocation));
        }
    }

    /// Closes a partially completed lifecycle without retaining a failed process child.
    ///
    /// @param provider real Provider to close
    /// @param handle loaded payload handle, or `null` after unload
    /// @param enabled whether the payload remains enabled
    /// @throws IOException if cleanup cannot complete
    private static void closePayload(RustRuntimeProvider provider, @Nullable RuntimePayloadHandle handle, boolean enabled)
            throws IOException {
        try {
            if (handle != null) {
                if (enabled) {
                    provider.disablePayload(handle);
                }
                provider.unloadPayload(handle);
            }
        } finally {
            provider.close();
        }
    }

    /// Creates the production hybrid Provider and real process-session factory.
    ///
    /// @param executable exact Rust process Host executable
    /// @return production-backed Rust Provider
    private static RustRuntimeProvider productionProvider(Path executable) {
        @Unmodifiable List<RuntimeProviderDeclaration> declarations = List.of(new RuntimeProviderDeclaration(
                "rust", Set.of(1), 1, Set.of(PluginExecutionMode.ISOLATED),
                Set.of(RuntimeFeature.BRIDGE, RuntimeFeature.HOOKS, RuntimeFeature.PATCHES, RuntimeFeature.NATIVE)));
        return new RustRuntimeProvider("dev.hmclce.runtime.rust-host", "0.2.0-beta.1", declarations,
                new RustRuntimeEngine(new IsolatedProcessEngine(), executable, RustRuntimeEngine::startProcessPayload));
    }

    /// Copies the compiled sample into an isolated package while guarding the Java-only token supplier.
    ///
    /// @return process payload context for the compiled launch-hook cdylib
    /// @throws IOException if the required sample artifact or metadata is unavailable
    private RuntimePayloadContext payloadContext() throws IOException {
        Path sampleRoot = Path.of(requireSystemProperty("aura.rust.launchHookSample")).toRealPath();
        Path library = launchHookLibrary();
        Path packageRoot = Files.createDirectories(temporaryDirectory.resolve("package"));
        Files.copy(sampleRoot.resolve("plugin.json"), packageRoot.resolve("plugin.json"));
        Files.copy(library, packageRoot.resolve(library.getFileName()));
        return new RuntimePayloadContext(
                new PluginArtifactIdentity("dev.hmclce.example.rust.launch-hook", "1.0.0", "a".repeat(64)),
                packageRoot, library.getFileName().toString(), PluginExecutionMode.ISOLATED,
                temporaryDirectory.resolve("data"),
                () -> { throw new AssertionError("Rust process test must not resolve a JVM capability token"); }
        );
    }

    /// Creates a Java-only Hook token for the process-boundary regression.
    ///
    /// @return opaque launcher-issued capability token
    private static PluginCapabilityToken capabilityToken() {
        PluginArtifactIdentity identity = new PluginArtifactIdentity(
                "dev.hmclce.example.rust.launch-hook", "1.0.0", "a".repeat(64));
        return new PluginPermissionAuthority().issue(
                identity, PluginExecutionMode.ISOLATED, Set.of(), "runtime.payload", Duration.ofMinutes(1L));
    }

    /// Creates a valid Hook event that the observational sample leaves unchanged.
    ///
    /// @return deterministic Hook event
    private static PluginHookEvent hookEvent() {
        return new PluginHookEvent(1, "rust-process-hook-42", PluginHookPoint.BEFORE_GAME_LAUNCH,
                Instant.parse("2026-09-05T00:00:00Z"),
                PluginDataObject.of(Map.of("enabled", PluginDataValue.bool(true))),
                PluginSecretAccess.denied("dev.hmclce.example.rust.launch-hook"));
    }

    /// Resolves the mandatory Rust process Host path.
    ///
    /// @return canonical Rust process Host executable
    /// @throws IOException if the configured process Host is not a regular file
    private static Path processHost() throws IOException {
        Path executable = Path.of(requireEnvironment("HMCL_RUST_PROCESS_HOST")).toAbsolutePath().normalize();
        if (!Files.isRegularFile(executable)) {
            throw new IOException("HMCL_RUST_PROCESS_HOST does not name a regular file: " + executable);
        }
        return executable.toRealPath();
    }

    /// Resolves the mandatory compiled sample library forwarded from the environment.
    ///
    /// @return canonical Rust launch-hook cdylib
    /// @throws IOException if the configured sample is not a regular file
    private static Path launchHookLibrary() throws IOException {
        Path library = Path.of(requireSystemProperty("aura.rust.launchHook")).toAbsolutePath().normalize();
        if (!Files.isRegularFile(library)) {
            throw new IOException("AURA_RUST_LAUNCH_HOOK does not name a regular file: " + library);
        }
        return library.toRealPath();
    }

    /// Returns one nonblank mandatory Gradle-forwarded setting.
    ///
    /// @param name exact setting name
    /// @return nonblank setting value
    private static String requireSystemProperty(String name) {
        @Nullable String value = System.getProperty(name);
        if (value == null || value.isBlank()) {
            throw new IllegalStateException("Set required Rust process integration configuration: " + name);
        }
        return value;
    }

    /// Returns one nonblank mandatory native integration environment setting.
    ///
    /// @param name exact environment variable name
    /// @return nonblank setting value
    private static String requireEnvironment(String name) {
        @Nullable String value = System.getenv(name);
        if (value == null || value.isBlank()) {
            throw new IllegalStateException("Set required Rust process integration configuration: " + name);
        }
        return value;
    }

    /// Supplies only the provider-wide embedded lifecycle required by the hybrid process route.
    @NotNullByDefault
    private static final class IsolatedProcessEngine implements RustRuntimeProvider.Engine {
        /// Accepts provider initialization because this integration exercises only isolated payloads.
        @Override
        public void initialize() {
        }

        /// Reports isolated process startup readiness.
        ///
        /// @return always true for this isolated-only boundary
        @Override
        public boolean healthCheck() {
            return true;
        }

        /// Rejects unexpected embedded loading.
        ///
        /// @param context unused embedded payload context
        /// @return never returns
        @Override
        public String loadPayload(RuntimePayloadContext context) {
            throw new AssertionError("process integration must not load an embedded payload");
        }

        /// Rejects unexpected embedded enablement.
        ///
        /// @param payloadId unused embedded payload ID
        @Override
        public void enablePayload(String payloadId) {
            throw new AssertionError("process integration must not enable an embedded payload");
        }

        /// Rejects unexpected embedded disablement.
        ///
        /// @param payloadId unused embedded payload ID
        @Override
        public void disablePayload(String payloadId) {
            throw new AssertionError("process integration must not disable an embedded payload");
        }

        /// Rejects unexpected embedded invocation.
        ///
        /// @param payloadId unused embedded payload ID
        /// @param operation unused embedded operation
        /// @param input unused embedded Bridge input
        /// @param callbackId unused embedded callback ID
        /// @return never returns
        @Override
        public byte[] invokePayload(String payloadId, String operation, byte[] input, long callbackId) {
            throw new AssertionError("process integration must not invoke an embedded payload");
        }

        /// Rejects unexpected embedded unloading.
        ///
        /// @param payloadId unused embedded payload ID
        @Override
        public void unloadPayload(String payloadId) {
            throw new AssertionError("process integration must not unload an embedded payload");
        }

        /// Performs no embedded cleanup because this test owns no embedded payload.
        @Override
        public void close() {
        }
    }
}
