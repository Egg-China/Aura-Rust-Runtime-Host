package dev.hmclce.runtime.rust;

import org.jackhuang.hmcl.plugin.runtime.PluginPlatformTarget;
import org.jackhuang.hmcl.plugin.runtime.PluginExecutionMode;
import org.jackhuang.hmcl.plugin.runtime.RuntimePayloadContext;
import org.jetbrains.annotations.NotNullByDefault;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.HashMap;
import java.util.Map;

/// Owns the optional Rust Host native library and its provider-wide JNI engine handle.
@NotNullByDefault
public final class RustNativeEngine implements RustRuntimeProvider.Engine {
    /// Canonical native library path loaded for this engine instance.
    private final Path loadedLibrary;

    /// Native binding implementation used after the library has been loaded.
    private final NativeBindings bindings;

    /// Java-side Runtime Bridge endpoint selected by the Host plugin.
    private final BridgeAdapter bridgeAdapter;

    /// Nonzero provider-wide native engine handle after initialization.
    private long handle;

    /// Whether teardown has already selected this engine instance.
    private boolean closed;

    /// Active Java payload contexts indexed by their opaque native payload ID.
    private final Map<String, RuntimePayloadContext> payloadContexts = new HashMap<>();

    /// Active Host sessions indexed independently from Java capability-token bytes.
    private final Map<Long, RuntimePayloadContext> sessionContexts = new HashMap<>();

    /// Active native payload IDs mapped to their Host session for exact teardown.
    private final Map<String, Long> payloadSessions = new HashMap<>();

    /// Next nonzero Host session handle; it never contains Java capability-token bytes.
    private long nextSession = 1L;

    /// Creates an unloaded provider-wide engine wrapper.
    ///
    /// @param loadedLibrary canonical native library path
    /// @param bindings JNI binding implementation
    /// @param bridgeAdapter Java-side Runtime Bridge endpoint
    private RustNativeEngine(Path loadedLibrary, NativeBindings bindings, BridgeAdapter bridgeAdapter) {
        this.loadedLibrary = loadedLibrary;
        this.bindings = bindings;
        this.bridgeAdapter = bridgeAdapter;
    }

    /// Loads the current platform library from the extracted plugin package.
    ///
    /// @param packageRoot extracted plugin package root
    /// @param platform exact launcher platform
    /// @return loaded engine wrapper
    /// @throws IOException if the native artifact is missing or escapes the package root
    public static RustNativeEngine load(Path packageRoot, PluginPlatformTarget platform) throws IOException {
        return load(
                packageRoot,
                platform,
                path -> System.load(path.toString()),
                new JniBindings(),
                new ContextBridgeAdapter()
        );
    }

    /// Loads the current platform library with one launcher-owned Runtime Bridge adapter.
    ///
    /// @param packageRoot extracted plugin package root
    /// @param platform exact launcher platform
    /// @param bridgeAdapter Java-side Runtime Bridge endpoint
    /// @return loaded engine wrapper
    /// @throws IOException if the native artifact is missing or escapes the package root
    static RustNativeEngine load(
            Path packageRoot,
            PluginPlatformTarget platform,
            BridgeAdapter bridgeAdapter
    ) throws IOException {
        return load(
                packageRoot,
                platform,
                path -> System.load(path.toString()),
                new JniBindings(),
                bridgeAdapter
        );
    }

    /// Loads a verified library with injectable boundary operations for deterministic tests.
    ///
    /// @param packageRoot extracted plugin package root
    /// @param platform exact launcher platform
    /// @param loader native library loader
    /// @param bindings native method bindings
    /// @return loaded engine wrapper
    /// @throws IOException if the native artifact is missing or escapes the package root
    static RustNativeEngine load(
            Path packageRoot,
            PluginPlatformTarget platform,
            NativeLibraryLoader loader,
            NativeBindings bindings
    ) throws IOException {
        return load(packageRoot, platform, loader, bindings, new UnavailableBridgeAdapter());
    }

    /// Loads a verified library with injectable native and Bridge boundaries for deterministic tests.
    ///
    /// @param packageRoot extracted plugin package root
    /// @param platform exact launcher platform
    /// @param loader native library loader
    /// @param bindings native method bindings
    /// @param bridgeAdapter Java-side Runtime Bridge endpoint
    /// @return loaded engine wrapper
    /// @throws IOException if the native artifact is missing or escapes the package root
    static RustNativeEngine load(
            Path packageRoot,
            PluginPlatformTarget platform,
            NativeLibraryLoader loader,
            NativeBindings bindings,
            BridgeAdapter bridgeAdapter
    ) throws IOException {
        Path library = resolveNativeLibrary(packageRoot, platform);
        loader.load(library);
        return new RustNativeEngine(library, bindings, bridgeAdapter);
    }

    /// Returns the stable package-relative library path for one supported release target.
    ///
    /// @param platform exact launcher platform
    /// @return package-relative native library path
    /// @throws IllegalArgumentException if no Rust Host artifact is published for the target
    static Path nativeLibraryPath(PluginPlatformTarget platform) {
        String filename;
        switch (platform.getId()) {
            case "windows-x64":
            case "windows-arm64":
                filename = "hmcl_rust_host_native.dll";
                break;
            case "linux-x64":
            case "linux-arm64":
                filename = "libhmcl_rust_host_native.so";
                break;
            case "macos-x64":
            case "macos-arm64":
                filename = "libhmcl_rust_host_native.dylib";
                break;
            default:
                throw new IllegalArgumentException("Unsupported Rust Runtime Host platform: " + platform.getId());
        }
        return Path.of("native", platform.getId(), filename);
    }

    /// Resolves a native engine to a canonical regular file confined beneath the package root.
    ///
    /// @param packageRoot extracted plugin package root
    /// @param platform exact launcher platform
    /// @return canonical native library path
    /// @throws IOException if the artifact is missing, irregular, or escapes through a symbolic link
    static Path resolveNativeLibrary(Path packageRoot, PluginPlatformTarget platform) throws IOException {
        Path canonicalRoot = packageRoot.toRealPath();
        Path candidate = canonicalRoot.resolve(nativeLibraryPath(platform)).normalize();
        if (!candidate.startsWith(canonicalRoot)) {
            throw new IOException("Rust Host native path escapes package root: " + candidate);
        }
        Path canonicalLibrary = candidate.toRealPath();
        if (!canonicalLibrary.startsWith(canonicalRoot)) {
            throw new IOException("Rust Host native symlink escapes package root: " + candidate);
        }
        if (!Files.isRegularFile(canonicalLibrary)) {
            throw new IOException("Rust Host native artifact is not a regular file: " + candidate);
        }
        return canonicalLibrary;
    }

    /// Creates provider-wide native state exactly once.
    ///
    /// @throws IOException if JNI returns an invalid handle or this engine is closed
    @Override
    public synchronized void initialize() throws IOException {
        if (closed) {
            throw new IOException("Rust Host native engine is closed");
        }
        if (handle != 0) {
            return;
        }
        long createdHandle = bindings.create(this);
        if (createdHandle == 0) {
            throw new IOException("Rust Host native engine returned a null handle");
        }
        handle = createdHandle;
    }

    /// Checks the native engine's current readiness.
    ///
    /// @return whether the native engine can accept payload work
    /// @throws IOException if the engine is not initialized or is already closed
    @Override
    public synchronized boolean healthCheck() throws IOException {
        if (closed || handle == 0) {
            throw new IOException("Rust Host native engine is not initialized");
        }
        return bindings.healthCheck(handle);
    }

    /// Loads one embedded payload while retaining its Java context only for call-time authority lookup.
    ///
    /// @param context immutable payload context
    /// @return opaque native payload ID
    /// @throws IOException if the engine is unavailable or native loading fails
    @Override
    public synchronized String loadPayload(RuntimePayloadContext context) throws IOException {
        requireAvailable();
        if (context.executionMode() != PluginExecutionMode.EMBEDDED) {
            throw new IOException("Rust embedded engine cannot load execution mode: " + context.executionMode());
        }
        long session = nextSession;
        if (session <= 0L || session == Long.MAX_VALUE) {
            throw new IOException("Rust Host session identifier space is exhausted");
        }
        nextSession++;
        Path entrypoint = context.packagePath().resolve(context.entrypoint()).normalize();
        sessionContexts.put(session, context);
        long nativePayload;
        try {
            nativePayload = bindings.loadPayload(
                    handle,
                    context.packagePath().toString(),
                    entrypoint.toString(),
                    session,
                    session
            );
        } catch (IOException | RuntimeException | Error exception) {
            sessionContexts.remove(session);
            throw exception;
        }
        if (nativePayload <= 0L) {
            sessionContexts.remove(session);
            throw new IOException("Rust Host native engine returned an invalid payload handle");
        }
        String payloadId = Long.toUnsignedString(nativePayload);
        if (payloadContexts.putIfAbsent(payloadId, context) != null) {
            sessionContexts.remove(session);
            throw new IOException("Rust Host native engine reused a live payload handle: " + payloadId);
        }
        payloadSessions.put(payloadId, session);
        return payloadId;
    }

    /// Enables one native payload.
    ///
    /// @param payloadId opaque native payload ID
    /// @throws IOException if the payload is unknown or initialization fails
    @Override
    public synchronized void enablePayload(String payloadId) throws IOException {
        bindings.enablePayload(handle, requirePayload(payloadId));
    }

    /// Disables one native payload.
    ///
    /// @param payloadId opaque native payload ID
    /// @throws IOException if the payload is unknown or transition fails
    @Override
    public synchronized void disablePayload(String payloadId) throws IOException {
        bindings.disablePayload(handle, requirePayload(payloadId));
    }

    /// Invokes one operation on an enabled embedded payload.
    ///
    /// @param payloadId opaque native payload ID
    /// @param operation canonical payload operation
    /// @param input canonical Bridge Value v1 bytes
    /// @param callbackId payload-local callback ID
    /// @return canonical Bridge Value v1 result bytes
    /// @throws IOException if the payload is unknown or invocation fails
    @Override
    public synchronized byte[] invokePayload(
            String payloadId,
            String operation,
            byte[] input,
            long callbackId
    ) throws IOException {
        return bindings.invokePayload(
                handle,
                requirePayload(payloadId),
                operation,
                input,
                callbackId
        );
    }

    /// Shuts down and unloads one native payload, dropping its Java session after success.
    ///
    /// @param payloadId opaque native payload ID
    /// @throws IOException if the payload is unknown or shutdown fails
    @Override
    public synchronized void unloadPayload(String payloadId) throws IOException {
        long nativePayload = requirePayload(payloadId);
        bindings.unloadPayload(handle, nativePayload);
        payloadContexts.remove(payloadId);
        Long session = payloadSessions.remove(payloadId);
        if (session != null) {
            sessionContexts.remove(session);
        }
    }

    /// Releases the provider-wide native handle exactly once.
    @Override
    public synchronized void close() {
        if (closed) {
            return;
        }
        closed = true;
        long closingHandle = handle;
        handle = 0;
        if (closingHandle != 0) {
            bindings.destroy(closingHandle);
        }
        payloadContexts.clear();
        payloadSessions.clear();
        sessionContexts.clear();
    }

    /// Invokes one Java Runtime Bridge operation for an active opaque Host session.
    ///
    /// @param session numeric Host session received from native code
    /// @param operation stable Runtime Bridge operation name
    /// @param input canonical wire-encoded Bridge input
    /// @return canonical wire-encoded Bridge result
    /// @throws IOException if the session is unknown or Bridge dispatch fails
    byte[] invokeBridge(long session, String operation, byte[] input) throws IOException {
        return bridgeAdapter.invoke(requireSession(session), operation, input);
    }

    /// Retains one launcher-owned Bridge handle for an active opaque Host session.
    ///
    /// @param session numeric Host session received from native code
    /// @param objectId launcher-owned object ID
    /// @param generation handle generation
    /// @throws IOException if the session is unknown or retain fails
    void retainBridgeHandle(long session, long objectId, long generation) throws IOException {
        bridgeAdapter.retainHandle(requireSession(session), objectId, generation);
    }

    /// Releases one launcher-owned Bridge handle for an active opaque Host session.
    ///
    /// @param session numeric Host session received from native code
    /// @param objectId launcher-owned object ID
    /// @param generation handle generation
    /// @throws IOException if the session is unknown or release fails
    void releaseBridgeHandle(long session, long objectId, long generation) throws IOException {
        bridgeAdapter.releaseHandle(requireSession(session), objectId, generation);
    }

    /// Returns the canonical owner library path for native owner binding tests.
    ///
    /// @return canonical loaded library path
    Path loadedLibrary() {
        return loadedLibrary;
    }

    /// Resolves one active Java payload context without touching its capability token supplier.
    ///
    /// @param session numeric Host session
    /// @return active Java payload context
    /// @throws IOException if the session is not active
    private synchronized RuntimePayloadContext requireSession(long session) throws IOException {
        RuntimePayloadContext context = sessionContexts.get(session);
        if (context == null) {
            throw new IOException("Unknown Rust Host session: " + Long.toUnsignedString(session));
        }
        return context;
    }

    /// Requires an initialized, open native engine.
    ///
    /// @throws IOException if the engine is unavailable
    private void requireAvailable() throws IOException {
        if (closed || handle == 0L) {
            throw new IOException("Rust Host native engine is not initialized");
        }
    }

    /// Parses and validates one active opaque payload ID.
    ///
    /// @param payloadId candidate payload ID
    /// @return positive native payload handle
    /// @throws IOException if the payload is unknown or malformed
    private long requirePayload(String payloadId) throws IOException {
        requireAvailable();
        if (!payloadContexts.containsKey(payloadId)) {
            throw new IOException("Unknown Rust payload handle: " + payloadId);
        }
        try {
            long nativePayload = Long.parseUnsignedLong(payloadId);
            if (nativePayload <= 0L) {
                throw new IOException("Invalid Rust payload handle: " + payloadId);
            }
            return nativePayload;
        } catch (NumberFormatException exception) {
            throw new IOException("Invalid Rust payload handle: " + payloadId, exception);
        }
    }

    /// Loads one already verified absolute native library path.
    @FunctionalInterface
    @NotNullByDefault
    interface NativeLibraryLoader {
        /// Loads the exact canonical native library.
        ///
        /// @param path canonical library path
        void load(Path path);
    }

    /// Adapts opaque Host sessions to launcher-owned Runtime Bridge operations.
    @NotNullByDefault
    interface BridgeAdapter {
        /// Invokes one wire-encoded Runtime Bridge operation.
        byte[] invoke(RuntimePayloadContext context, String operation, byte[] input) throws IOException;

        /// Retains one launcher-owned Bridge handle.
        void retainHandle(RuntimePayloadContext context, long objectId, long generation) throws IOException;

        /// Releases one launcher-owned Bridge handle.
        void releaseHandle(RuntimePayloadContext context, long objectId, long generation) throws IOException;
    }

    /// Rejects Bridge calls until the launcher supplies its public Runtime Bridge transport.
    @NotNullByDefault
    private static final class UnavailableBridgeAdapter implements BridgeAdapter {
        /// Rejects invocation because no launcher transport was supplied.
        @Override
        public byte[] invoke(RuntimePayloadContext context, String operation, byte[] input) throws IOException {
            throw new IOException("Launcher Runtime Bridge transport is unavailable");
        }

        /// Rejects retain because no launcher transport was supplied.
        @Override
        public void retainHandle(RuntimePayloadContext context, long objectId, long generation) throws IOException {
            throw new IOException("Launcher Runtime Bridge transport is unavailable");
        }

        /// Rejects release because no launcher transport was supplied.
        @Override
        public void releaseHandle(RuntimePayloadContext context, long objectId, long generation) throws IOException {
            throw new IOException("Launcher Runtime Bridge transport is unavailable");
        }
    }

    /// Delegates production calls to the launcher transport retained by each exact payload context.
    @NotNullByDefault
    private static final class ContextBridgeAdapter implements BridgeAdapter {
        /// Invokes one launcher Bridge operation without serializing the Java capability token.
        @Override
        public byte[] invoke(RuntimePayloadContext context, String operation, byte[] input) throws IOException {
            return context.bridgeTransport().invoke(context, operation, input);
        }

        /// Retains one launcher-owned handle through the context transport.
        @Override
        public void retainHandle(RuntimePayloadContext context, long objectId, long generation) throws IOException {
            context.bridgeTransport().retainHandle(context, objectId, generation);
        }

        /// Releases one launcher-owned handle through the context transport.
        @Override
        public void releaseHandle(RuntimePayloadContext context, long objectId, long generation) throws IOException {
            context.bridgeTransport().releaseHandle(context, objectId, generation);
        }
    }

    /// Provider-wide JNI operations implemented by the Rust native engine.
    @NotNullByDefault
    interface NativeBindings {
        /// Creates provider-wide native state.
        ///
        /// @return nonzero opaque engine handle
        long create(RustNativeEngine owner);

        /// Checks native engine readiness.
        ///
        /// @param handle nonzero engine handle
        /// @return whether the engine is healthy
        boolean healthCheck(long handle);

        /// Loads one embedded payload and returns its positive native handle.
        long loadPayload(
                long handle,
                String packageRoot,
                String entrypoint,
                long pluginId,
                long session
        ) throws IOException;

        /// Enables one loaded native payload.
        void enablePayload(long handle, long payloadId) throws IOException;

        /// Disables one enabled native payload.
        void disablePayload(long handle, long payloadId) throws IOException;

        /// Invokes one operation on an enabled embedded payload.
        byte[] invokePayload(
                long handle,
                long payloadId,
                String operation,
                byte[] input,
                long callbackId
        ) throws IOException;

        /// Shuts down and unloads one disabled native payload.
        void unloadPayload(long handle, long payloadId) throws IOException;

        /// Releases provider-wide native state.
        ///
        /// @param handle nonzero engine handle
        void destroy(long handle);
    }

    /// Production JNI binding table loaded from `hmcl-rust-host-native`.
    @NotNullByDefault
    private static final class JniBindings implements NativeBindings {
        /// Creates provider-wide Rust Host state through JNI.
        ///
        /// @return nonzero opaque engine handle
        private static native long createEngine(RustNativeEngine owner);

        /// Checks provider-wide Rust Host readiness through JNI.
        ///
        /// @param handle nonzero engine handle
        /// @return whether the engine is healthy
        private static native boolean checkEngineHealth(long handle);

        /// Loads and queries one embedded payload through JNI.
        private static native long loadEmbeddedPayload(
                long handle,
                String packageRoot,
                String entrypoint,
                long pluginId,
                long session
        ) throws IOException;

        /// Enables one embedded payload through JNI.
        private static native void enableEmbeddedPayload(long handle, long payloadId) throws IOException;

        /// Disables one embedded payload through JNI.
        private static native void disableEmbeddedPayload(long handle, long payloadId) throws IOException;

        /// Invokes one enabled embedded payload through JNI.
        private static native byte[] invokeEmbeddedPayload(
                long handle,
                long payloadId,
                String operation,
                byte[] input,
                long callbackId
        ) throws IOException;

        /// Unloads one embedded payload through JNI.
        private static native void unloadEmbeddedPayload(long handle, long payloadId) throws IOException;

        /// Releases provider-wide Rust Host state through JNI.
        ///
        /// @param handle nonzero engine handle
        private static native void destroyEngine(long handle);

        /// Calls the JNI engine constructor.
        ///
        /// @return nonzero opaque engine handle
        @Override
        public long create(RustNativeEngine owner) {
            return createEngine(owner);
        }

        /// Calls the JNI health endpoint.
        ///
        /// @param handle nonzero engine handle
        /// @return whether the engine is healthy
        @Override
        public boolean healthCheck(long handle) {
            return checkEngineHealth(handle);
        }

        /// Calls the JNI embedded payload loader.
        @Override
        public long loadPayload(
                long handle,
                String packageRoot,
                String entrypoint,
                long pluginId,
                long session
        ) throws IOException {
            return loadEmbeddedPayload(handle, packageRoot, entrypoint, pluginId, session);
        }

        /// Calls the JNI embedded payload initializer.
        @Override
        public void enablePayload(long handle, long payloadId) throws IOException {
            enableEmbeddedPayload(handle, payloadId);
        }

        /// Calls the JNI embedded payload disable transition.
        @Override
        public void disablePayload(long handle, long payloadId) throws IOException {
            disableEmbeddedPayload(handle, payloadId);
        }

        /// Calls the JNI embedded payload invocation endpoint.
        @Override
        public byte[] invokePayload(
                long handle,
                long payloadId,
                String operation,
                byte[] input,
                long callbackId
        ) throws IOException {
            return invokeEmbeddedPayload(handle, payloadId, operation, input, callbackId);
        }

        /// Calls the JNI embedded payload shutdown and unload transition.
        @Override
        public void unloadPayload(long handle, long payloadId) throws IOException {
            unloadEmbeddedPayload(handle, payloadId);
        }

        /// Calls the JNI engine destructor.
        ///
        /// @param handle nonzero engine handle
        @Override
        public void destroy(long handle) {
            destroyEngine(handle);
        }
    }
}
