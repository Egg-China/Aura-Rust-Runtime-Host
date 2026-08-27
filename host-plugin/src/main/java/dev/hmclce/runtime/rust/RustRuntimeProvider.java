package dev.hmclce.runtime.rust;

import org.jackhuang.hmcl.plugin.runtime.RuntimeProvider;
import org.jackhuang.hmcl.plugin.runtime.RuntimeProviderDeclaration;
import org.jackhuang.hmcl.plugin.runtime.RuntimeProviderDescriptor;
import org.jetbrains.annotations.NotNullByDefault;

import java.io.IOException;
import java.util.List;
import java.util.concurrent.atomic.AtomicBoolean;

/// Publishes the Rust runtime declaration and delegates native lifecycle negotiation to its engine.
@NotNullByDefault
public final class RustRuntimeProvider implements RuntimeProvider {
    /// Native engine owned by this Provider registration.
    private final Engine engine;

    /// Immutable descriptor whose identity and capabilities mirror the Host package manifest.
    private final RuntimeProviderDescriptor descriptor;

    /// Prevents duplicate provider-wide engine cleanup.
    private final AtomicBoolean closed = new AtomicBoolean();

    /// Creates one installed and enabled Rust Runtime Provider.
    ///
    /// @param providerId exact Host plugin ID
    /// @param version exact Host package version
    /// @param declarations exact manifest runtime declarations
    /// @param engine native engine owned by the Provider
    RustRuntimeProvider(
            String providerId,
            String version,
            List<RuntimeProviderDeclaration> declarations,
            Engine engine
    ) {
        this.engine = engine;
        this.descriptor = new RuntimeProviderDescriptor(
                providerId,
                version,
                declarations,
                true,
                true,
                100,
                false
        );
    }

    /// Returns the exact runtime contract copied from the Host manifest.
    ///
    /// @return immutable Provider descriptor
    @Override
    public RuntimeProviderDescriptor descriptor() {
        return descriptor;
    }

    /// Creates provider-wide native state before health negotiation.
    ///
    /// @throws IOException if native initialization fails
    @Override
    public void initialize() throws IOException {
        engine.initialize();
    }

    /// Delegates readiness negotiation to the native engine.
    ///
    /// @return whether the engine can accept payload work
    /// @throws IOException if native health negotiation fails
    @Override
    public boolean healthCheck() throws IOException {
        return engine.healthCheck();
    }

    /// Releases provider-wide native state exactly once.
    ///
    /// @throws IOException if native cleanup fails
    @Override
    public void close() throws IOException {
        if (closed.compareAndSet(false, true)) {
            engine.close();
        }
    }

    /// Minimal engine lifecycle used by the Java Host and implemented by the JNI owner.
    @NotNullByDefault
    interface Engine extends AutoCloseable {
        /// Creates provider-wide runtime state.
        ///
        /// @throws IOException if initialization fails
        void initialize() throws IOException;

        /// Checks whether the initialized engine can accept payload work.
        ///
        /// @return whether the engine is healthy
        /// @throws IOException if negotiation fails
        boolean healthCheck() throws IOException;

        /// Releases provider-wide runtime state.
        ///
        /// @throws IOException if cleanup fails
        @Override
        void close() throws IOException;
    }
}
