package dev.hmclce.runtime.rust;

import org.jackhuang.hmcl.plugin.runtime.PluginPlatformTarget;
import org.jetbrains.annotations.NotNullByDefault;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;

/// Owns the optional Rust Host native library and its provider-wide JNI engine handle.
@NotNullByDefault
public final class RustNativeEngine implements RustRuntimeProvider.Engine {
    /// Native binding implementation used after the library has been loaded.
    private final NativeBindings bindings;

    /// Nonzero provider-wide native engine handle after initialization.
    private long handle;

    /// Whether teardown has already selected this engine instance.
    private boolean closed;

    /// Creates an unloaded provider-wide engine wrapper.
    ///
    /// @param bindings JNI binding implementation
    private RustNativeEngine(NativeBindings bindings) {
        this.bindings = bindings;
    }

    /// Loads the current platform library from the extracted plugin package.
    ///
    /// @param packageRoot extracted plugin package root
    /// @param platform exact launcher platform
    /// @return loaded engine wrapper
    /// @throws IOException if the native artifact is missing or escapes the package root
    public static RustNativeEngine load(Path packageRoot, PluginPlatformTarget platform) throws IOException {
        return load(packageRoot, platform, path -> System.load(path.toString()), new JniBindings());
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
        Path library = resolveNativeLibrary(packageRoot, platform);
        loader.load(library);
        return new RustNativeEngine(bindings);
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
        long createdHandle = bindings.create();
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

    /// Provider-wide JNI operations implemented by the Rust native engine.
    @NotNullByDefault
    interface NativeBindings {
        /// Creates provider-wide native state.
        ///
        /// @return nonzero opaque engine handle
        long create();

        /// Checks native engine readiness.
        ///
        /// @param handle nonzero engine handle
        /// @return whether the engine is healthy
        boolean healthCheck(long handle);

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
        private static native long createEngine();

        /// Checks provider-wide Rust Host readiness through JNI.
        ///
        /// @param handle nonzero engine handle
        /// @return whether the engine is healthy
        private static native boolean checkEngineHealth(long handle);

        /// Releases provider-wide Rust Host state through JNI.
        ///
        /// @param handle nonzero engine handle
        private static native void destroyEngine(long handle);

        /// Calls the JNI engine constructor.
        ///
        /// @return nonzero opaque engine handle
        @Override
        public long create() {
            return createEngine();
        }

        /// Calls the JNI health endpoint.
        ///
        /// @param handle nonzero engine handle
        /// @return whether the engine is healthy
        @Override
        public boolean healthCheck(long handle) {
            return checkEngineHealth(handle);
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
