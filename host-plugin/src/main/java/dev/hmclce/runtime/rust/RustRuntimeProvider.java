package dev.hmclce.runtime.rust;

import org.jackhuang.hmcl.plugin.runtime.RuntimeProvider;
import org.jackhuang.hmcl.plugin.runtime.RuntimeProviderDeclaration;
import org.jackhuang.hmcl.plugin.runtime.RuntimeProviderDescriptor;
import org.jackhuang.hmcl.plugin.runtime.RuntimePayloadContext;
import org.jackhuang.hmcl.plugin.runtime.RuntimePayloadHandle;
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

    /// Loads one embedded Rust payload and wraps its native ID in a Provider-owned opaque handle.
    ///
    /// @param context immutable payload loading context
    /// @return Provider-owned payload handle
    /// @throws IOException if native loading or ABI negotiation fails
    @Override
    public RuntimePayloadHandle loadPayload(RuntimePayloadContext context) throws IOException {
        String payloadId = engine.loadPayload(context);
        return new RuntimePayloadHandle(
                context.artifactIdentity().getPluginId(), descriptor.providerId(), payloadId);
    }

    /// Enables one loaded native payload.
    ///
    /// @param handle Provider-owned payload handle
    /// @throws IOException if native initialization fails
    @Override
    public void enablePayload(RuntimePayloadHandle handle) throws IOException {
        engine.enablePayload(requireOwnedHandle(handle));
    }

    /// Disables one enabled native payload.
    ///
    /// @param handle Provider-owned payload handle
    /// @throws IOException if the payload state is invalid
    @Override
    public void disablePayload(RuntimePayloadHandle handle) throws IOException {
        engine.disablePayload(requireOwnedHandle(handle));
    }

    /// Invokes one raw-byte operation on an enabled Provider-owned payload.
    ///
    /// @param handle Provider-owned payload handle
    /// @param operation canonical payload operation
    /// @param input canonical Bridge Value v1 bytes
    /// @param callbackId payload-local callback ID
    /// @return canonical Bridge Value v1 result bytes
    /// @throws IOException if ownership validation or native invocation fails
    @Override
    public byte[] invokePayload(
            RuntimePayloadHandle handle,
            String operation,
            byte[] input,
            long callbackId
    ) throws IOException {
        return engine.invokePayload(requireOwnedHandle(handle), operation, input, callbackId);
    }

    /// Shuts down and unloads one disabled native payload.
    ///
    /// @param handle Provider-owned payload handle
    /// @throws IOException if native shutdown fails
    @Override
    public void unloadPayload(RuntimePayloadHandle handle) throws IOException {
        engine.unloadPayload(requireOwnedHandle(handle));
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

    /// Requires one handle issued by this exact Provider identity.
    ///
    /// @param handle candidate payload handle
    /// @return opaque native payload ID
    /// @throws IOException if another Provider issued the handle
    private String requireOwnedHandle(RuntimePayloadHandle handle) throws IOException {
        if (!descriptor.providerId().equals(handle.providerId())) {
            throw new IOException("Rust Runtime Host received a foreign payload handle: " + handle.providerId());
        }
        return handle.payloadId();
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

        /// Loads and negotiates one exact embedded payload without exposing Java token bytes.
        ///
        /// @param context immutable payload context
        /// @return opaque native payload ID
        /// @throws IOException if loading fails
        String loadPayload(RuntimePayloadContext context) throws IOException;

        /// Enables one loaded payload.
        ///
        /// @param payloadId opaque native payload ID
        /// @throws IOException if initialization fails
        void enablePayload(String payloadId) throws IOException;

        /// Disables one enabled payload.
        ///
        /// @param payloadId opaque native payload ID
        /// @throws IOException if transition fails
        void disablePayload(String payloadId) throws IOException;

        /// Invokes one raw-byte operation on an enabled payload.
        ///
        /// @param payloadId opaque native payload ID
        /// @param operation canonical payload operation
        /// @param input canonical Bridge Value v1 bytes
        /// @param callbackId payload-local callback ID
        /// @return canonical Bridge Value v1 result bytes
        /// @throws IOException if invocation fails
        byte[] invokePayload(
                String payloadId,
                String operation,
                byte[] input,
                long callbackId
        ) throws IOException;

        /// Shuts down and unloads one disabled payload.
        ///
        /// @param payloadId opaque native payload ID
        /// @throws IOException if shutdown fails
        void unloadPayload(String payloadId) throws IOException;

        /// Releases provider-wide runtime state.
        ///
        /// @throws IOException if cleanup fails
        @Override
        void close() throws IOException;
    }
}
