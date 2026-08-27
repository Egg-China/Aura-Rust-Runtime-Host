package dev.hmclce.runtime.rust;

import org.jackhuang.hmcl.plugin.Plugin;
import org.jackhuang.hmcl.plugin.PluginContext;
import org.jackhuang.hmcl.plugin.PluginManifest;
import org.jackhuang.hmcl.plugin.runtime.PluginPlatformTarget;
import org.jackhuang.hmcl.plugin.runtime.RuntimeProvider;
import org.jackhuang.hmcl.plugin.runtime.RuntimeProviderRegistration;
import org.jetbrains.annotations.NotNullByDefault;
import org.jetbrains.annotations.Nullable;

import java.io.IOException;
import java.io.UncheckedIOException;
import java.nio.file.Path;
import java.util.Objects;
import java.util.function.Supplier;

/// Java bootstrap for the separately packaged optional Rust Runtime Host.
@NotNullByDefault
public final class RustRuntimeHostPlugin implements Plugin {
    /// Opens one platform-specific native engine from the package root.
    private final EngineFactory engineFactory;

    /// Supplies the exact launcher platform used for artifact selection.
    private final Supplier<PluginPlatformTarget> platformSupplier;

    /// Authoritative package manifest captured during load and retained for the Plugin API.
    private @Nullable PluginManifest manifest;

    /// Active Provider registration, if load completed.
    private @Nullable Registration registration;

    /// Loaded native engine, if load completed.
    private @Nullable RustRuntimeProvider.Engine engine;

    /// Creates a production bootstrap using current-platform JNI loading.
    public RustRuntimeHostPlugin() {
        this(RustNativeEngine::load, PluginPlatformTarget::current);
    }

    /// Creates a bootstrap with injectable package boundaries for lifecycle tests.
    ///
    /// @param engineFactory native engine factory
    /// @param platformSupplier exact platform supplier
    RustRuntimeHostPlugin(
            EngineFactory engineFactory,
            Supplier<PluginPlatformTarget> platformSupplier
    ) {
        this.engineFactory = engineFactory;
        this.platformSupplier = platformSupplier;
    }

    /// Loads the native library and publishes one manifest-matched Rust Provider.
    ///
    /// @param context launcher-owned plugin context
    @Override
    public void onLoad(PluginContext context) {
        try {
            load(new PluginHostContext(context));
        } catch (IOException exception) {
            throw new UncheckedIOException("Failed to load the Rust Runtime Host", exception);
        }
    }

    /// Performs the testable Host load transaction and rolls back an unregistered engine on failure.
    ///
    /// @param context minimal launcher Host context
    /// @throws IOException if native loading or rollback cleanup fails
    synchronized void load(HostContext context) throws IOException {
        if (registration != null || engine != null) {
            throw new IllegalStateException("Rust Runtime Host is already loaded");
        }
        PluginManifest loadedManifest = context.manifest();
        RustRuntimeProvider.Engine openedEngine = engineFactory.open(
                context.packageDirectory(), platformSupplier.get());
        try {
            RustRuntimeProvider provider = new RustRuntimeProvider(
                    loadedManifest.getId(),
                    loadedManifest.getVersion(),
                    loadedManifest.getProvidesRuntimes(),
                    openedEngine
            );
            Registration openedRegistration = context.register(provider);
            manifest = loadedManifest;
            engine = openedEngine;
            registration = openedRegistration;
        } catch (RuntimeException | Error exception) {
            try {
                openedEngine.close();
            } catch (IOException closeException) {
                exception.addSuppressed(closeException);
            }
            throw exception;
        }
    }

    /// Runtime Provider activation is owned by the launcher Supervisor after `onLoad` returns.
    @Override
    public void onEnable() {
    }

    /// Dependent payload shutdown is owned by the launcher Supervisor before Host unload.
    @Override
    public void onDisable() {
    }

    /// Releases the Provider registration followed by any remaining native engine state.
    @Override
    public synchronized void onUnload() {
        @Nullable Registration closingRegistration = registration;
        @Nullable RustRuntimeProvider.Engine closingEngine = engine;
        registration = null;
        engine = null;

        @Nullable IOException failure = null;
        if (closingRegistration != null) {
            try {
                closingRegistration.close();
            } catch (IOException exception) {
                failure = exception;
            }
        }
        if (closingEngine != null) {
            try {
                closingEngine.close();
            } catch (IOException exception) {
                if (failure == null) {
                    failure = exception;
                } else {
                    failure.addSuppressed(exception);
                }
            }
        }
        if (failure != null) {
            throw new UncheckedIOException("Failed to unload the Rust Runtime Host", failure);
        }
    }

    /// Returns the authoritative package manifest captured during `onLoad`.
    ///
    /// @return Host package manifest
    /// @throws IllegalStateException before the Host has loaded
    @Override
    public PluginManifest getManifest() {
        return Objects.requireNonNull(manifest, "Rust Runtime Host has not loaded");
    }

    /// Minimal Host context kept separate from the final launcher context for deterministic tests.
    @NotNullByDefault
    interface HostContext {
        /// Returns the authoritative Host package manifest.
        ///
        /// @return package manifest
        PluginManifest manifest();

        /// Returns the read-only extracted Host package root.
        ///
        /// @return package root
        Path packageDirectory();

        /// Publishes one manifest-matched Runtime Provider.
        ///
        /// @param provider Provider implementation
        /// @return Host-owned registration
        Registration register(RuntimeProvider provider);
    }

    /// Close-only view of a launcher-owned Provider registration.
    @FunctionalInterface
    @NotNullByDefault
    interface Registration extends AutoCloseable {
        /// Stops dependents and unregisters the Provider.
        ///
        /// @throws IOException if teardown fails
        @Override
        void close() throws IOException;
    }

    /// Opens one exact platform native engine.
    @FunctionalInterface
    @NotNullByDefault
    interface EngineFactory {
        /// Loads an engine from the extracted package.
        ///
        /// @param packageRoot extracted package root
        /// @param platform exact launcher platform
        /// @return loaded native engine
        /// @throws IOException if native loading fails
        RustRuntimeProvider.Engine open(Path packageRoot, PluginPlatformTarget platform) throws IOException;
    }

    /// Adapts the final launcher context without exposing it to tests or alternate loaders.
    @NotNullByDefault
    private static final class PluginHostContext implements HostContext {
        /// Launcher-owned plugin context.
        private final PluginContext context;

        /// Creates one context adapter.
        ///
        /// @param context launcher-owned context
        private PluginHostContext(PluginContext context) {
            this.context = context;
        }

        /// Returns the authoritative Host manifest.
        ///
        /// @return Host manifest
        @Override
        public PluginManifest manifest() {
            return context.getManifest();
        }

        /// Returns the extracted package root.
        ///
        /// @return package root
        @Override
        public Path packageDirectory() {
            return context.getPackageDirectory();
        }

        /// Registers the Runtime Provider and narrows its handle to close-only ownership.
        ///
        /// @param provider Provider implementation
        /// @return close-only registration
        @Override
        public Registration register(RuntimeProvider provider) {
            RuntimeProviderRegistration registered = context.registerRuntimeProvider(provider);
            return registered::close;
        }
    }
}
