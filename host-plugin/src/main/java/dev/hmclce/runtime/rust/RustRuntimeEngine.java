package dev.hmclce.runtime.rust;

import org.jackhuang.hmcl.plugin.runtime.PluginExecutionMode;
import org.jackhuang.hmcl.plugin.runtime.PluginPlatformTarget;
import org.jackhuang.hmcl.plugin.runtime.RuntimePayloadContext;
import org.jetbrains.annotations.NotNullByDefault;
import org.jetbrains.annotations.Nullable;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.time.Duration;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Objects;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;

/// Routes schema-v5 Rust payloads to embedded JNI or one isolated process per payload.
@NotNullByDefault
public final class RustRuntimeEngine implements RustRuntimeProvider.Engine {
    /// Default deadline for non-Hook isolated invocations.
    private static final Duration DEFAULT_INVOCATION_TIMEOUT = Duration.ofSeconds(30L);

    /// Existing provider-wide embedded JNI engine.
    private final RustRuntimeProvider.Engine embeddedEngine;

    /// Canonical isolated process Host executable.
    private final Path processExecutable;

    /// Shared daemon deadline scheduler for isolated payload supervisors.
    private final ScheduledExecutorService scheduler;

    /// Injectable isolated payload process boundary.
    private final IsolatedPayloadFactory isolatedFactory;

    /// Active Host IDs mapped to their selected backend in load order.
    private final Map<String, PayloadBackend> payloads = new LinkedHashMap<>();

    /// Next positive Host-generated public payload ID.
    private long nextPayloadId = 1L;

    /// Whether provider-wide cleanup has selected this engine.
    private boolean closed;

    /// Creates one hybrid engine with injectable backend boundaries.
    ///
    /// @param embeddedEngine provider-wide embedded engine
    /// @param processExecutable exact isolated process executable
    /// @param scheduler shared isolated deadline scheduler
    /// @param isolatedFactory isolated payload starter
    RustRuntimeEngine(
            RustRuntimeProvider.Engine embeddedEngine,
            Path processExecutable,
            ScheduledExecutorService scheduler,
            IsolatedPayloadFactory isolatedFactory
    ) {
        this.embeddedEngine = embeddedEngine;
        this.processExecutable = processExecutable;
        this.scheduler = scheduler;
        this.isolatedFactory = isolatedFactory;
    }

    /// Loads both platform artifacts and creates one production hybrid engine.
    ///
    /// @param packageRoot extracted Runtime Host package root
    /// @param platform exact launcher platform
    /// @return unloaded hybrid engine
    /// @throws IOException if either required platform artifact is absent or escapes the package root
    static RustRuntimeEngine load(Path packageRoot, PluginPlatformTarget platform) throws IOException {
        Path processExecutable = resolveProcessHost(packageRoot, platform);
        RustNativeEngine embeddedEngine = RustNativeEngine.load(packageRoot, platform);
        ScheduledExecutorService scheduler = Executors.newSingleThreadScheduledExecutor(runnable -> {
            Thread thread = new Thread(runnable, "rust-isolated-runtime-deadline");
            thread.setDaemon(true);
            return thread;
        });
        return new RustRuntimeEngine(
                embeddedEngine,
                processExecutable,
                scheduler,
                (executable, context, deadlines) -> RustIsolatedPayload.start(
                        executable, context, ProcessBuilder::start, deadlines)
        );
    }

    /// Returns the stable package-relative isolated executable path for one release target.
    ///
    /// @param platform exact launcher platform
    /// @return package-relative process Host path
    /// @throws IllegalArgumentException if the platform is unsupported
    static Path processHostPath(PluginPlatformTarget platform) {
        String filename;
        switch (platform.getId()) {
            case "windows-x64":
            case "windows-arm64":
                filename = "hmcl-rust-host-process.exe";
                break;
            case "linux-x64":
            case "linux-arm64":
            case "macos-x64":
            case "macos-arm64":
                filename = "hmcl-rust-host-process";
                break;
            default:
                throw new IllegalArgumentException(
                        "Unsupported Rust Runtime Host platform: " + platform.getId());
        }
        return Path.of("native", platform.getId(), filename);
    }

    /// Resolves the isolated executable to a canonical regular file confined beneath the package root.
    ///
    /// @param packageRoot extracted Runtime Host package root
    /// @param platform exact launcher platform
    /// @return canonical isolated process executable
    /// @throws IOException if the artifact is missing, irregular, or escapes through a symbolic link
    static Path resolveProcessHost(Path packageRoot, PluginPlatformTarget platform) throws IOException {
        Path canonicalRoot = packageRoot.toRealPath();
        Path candidate = canonicalRoot.resolve(processHostPath(platform)).normalize();
        if (!candidate.startsWith(canonicalRoot)) {
            throw new IOException("Rust process Host path escapes package root: " + candidate);
        }
        Path canonicalExecutable = candidate.toRealPath();
        if (!canonicalExecutable.startsWith(canonicalRoot)) {
            throw new IOException("Rust process Host symlink escapes package root: " + candidate);
        }
        if (!Files.isRegularFile(canonicalExecutable)) {
            throw new IOException("Rust process Host artifact is not a regular file: " + candidate);
        }
        return canonicalExecutable;
    }

    /// Initializes provider-wide embedded state.
    ///
    /// @throws IOException if closed or embedded initialization fails
    @Override
    public synchronized void initialize() throws IOException {
        requireOpen();
        embeddedEngine.initialize();
    }

    /// Checks provider-wide embedded health used by both routing modes.
    ///
    /// @return whether the shared ABI engine is healthy
    /// @throws IOException if closed or embedded health negotiation fails
    @Override
    public synchronized boolean healthCheck() throws IOException {
        requireOpen();
        return embeddedEngine.healthCheck();
    }

    /// Loads one payload into the backend selected by its execution mode.
    ///
    /// @param context exact payload context
    /// @return Host-generated opaque payload ID
    /// @throws IOException if loading or process startup fails
    @Override
    public synchronized String loadPayload(RuntimePayloadContext context) throws IOException {
        requireOpen();
        String payloadId = allocatePayloadId();
        PayloadBackend backend;
        if (context.executionMode() == PluginExecutionMode.EMBEDDED) {
            backend = new EmbeddedPayload(embeddedEngine.loadPayload(context));
        } else if (context.executionMode() == PluginExecutionMode.ISOLATED) {
            backend = new IsolatedPayloadBackend(isolatedFactory.start(processExecutable, context, scheduler));
        } else {
            throw new IOException("Unsupported Rust payload execution mode: " + context.executionMode());
        }
        payloads.put(payloadId, backend);
        return payloadId;
    }

    /// Enables one payload through its selected backend.
    ///
    /// @param payloadId Host-generated opaque payload ID
    /// @throws IOException if the ID is foreign or backend initialization fails
    @Override
    public synchronized void enablePayload(String payloadId) throws IOException {
        PayloadBackend backend = requireBackend(payloadId);
        if (backend instanceof EmbeddedPayload embedded) {
            embeddedEngine.enablePayload(embedded.nativePayloadId());
        } else if (backend instanceof IsolatedPayloadBackend isolated) {
            isolated.payload().enable();
        }
    }

    /// Disables one payload through its selected backend.
    ///
    /// @param payloadId Host-generated opaque payload ID
    /// @throws IOException if the ID is foreign or backend transition fails
    @Override
    public synchronized void disablePayload(String payloadId) throws IOException {
        PayloadBackend backend = requireBackend(payloadId);
        if (backend instanceof EmbeddedPayload embedded) {
            embeddedEngine.disablePayload(embedded.nativePayloadId());
        } else if (backend instanceof IsolatedPayloadBackend isolated) {
            isolated.payload().disable();
        }
    }

    /// Invokes one payload with the default generic isolated deadline.
    ///
    /// @param payloadId Host-generated opaque payload ID
    /// @param operation canonical payload operation
    /// @param input canonical Bridge Value v1 bytes
    /// @param callbackId payload-local callback identifier
    /// @return canonical Bridge Value v1 output
    /// @throws IOException if routing or invocation fails
    @Override
    public synchronized byte[] invokePayload(
            String payloadId,
            String operation,
            byte[] input,
            long callbackId
    ) throws IOException {
        return invokePayload(payloadId, operation, input, callbackId, DEFAULT_INVOCATION_TIMEOUT);
    }

    /// Invokes one payload while forwarding an exact Hook deadline to isolated execution.
    ///
    /// @param payloadId Host-generated opaque payload ID
    /// @param operation canonical payload operation
    /// @param input canonical Bridge Value v1 bytes
    /// @param callbackId payload-local callback identifier
    /// @param timeout positive isolated operation deadline
    /// @return canonical Bridge Value v1 output
    /// @throws IOException if routing or invocation fails
    @Override
    public synchronized byte[] invokePayload(
            String payloadId,
            String operation,
            byte[] input,
            long callbackId,
            Duration timeout
    ) throws IOException {
        if (timeout.isZero() || timeout.isNegative()) {
            throw new IOException("Rust payload invocation timeout must be positive");
        }
        PayloadBackend backend = requireBackend(payloadId);
        if (backend instanceof EmbeddedPayload embedded) {
            return embeddedEngine.invokePayload(
                    embedded.nativePayloadId(), operation, input, callbackId, timeout);
        }
        if (backend instanceof IsolatedPayloadBackend isolated) {
            return isolated.payload().invoke(operation, input, callbackId, timeout);
        }
        throw new AssertionError("Unknown sealed Rust payload backend");
    }

    /// Shuts down and removes one payload backend.
    ///
    /// @param payloadId Host-generated opaque payload ID
    /// @throws IOException if the ID is foreign or shutdown fails
    @Override
    public synchronized void unloadPayload(String payloadId) throws IOException {
        PayloadBackend backend = requireBackend(payloadId);
        if (backend instanceof EmbeddedPayload embedded) {
            embeddedEngine.unloadPayload(embedded.nativePayloadId());
            payloads.remove(payloadId);
        } else if (backend instanceof IsolatedPayloadBackend isolated) {
            try {
                isolated.payload().shutdown();
            } finally {
                payloads.remove(payloadId);
            }
        }
    }

    /// Closes isolated payloads in reverse load order before the shared embedded engine.
    ///
    /// @throws IOException if embedded cleanup fails
    @Override
    public synchronized void close() throws IOException {
        if (closed) {
            return;
        }
        closed = true;
        List<PayloadBackend> backends = new ArrayList<>(payloads.values());
        for (int index = backends.size() - 1; index >= 0; index--) {
            PayloadBackend backend = backends.get(index);
            if (backend instanceof IsolatedPayloadBackend isolated) {
                isolated.payload().close();
            }
        }
        payloads.clear();
        try {
            embeddedEngine.close();
        } finally {
            scheduler.shutdownNow();
        }
    }

    /// Allocates one positive Host-generated public payload identifier.
    ///
    /// @return decimal opaque identifier
    /// @throws IOException if signed-64 identifier space is exhausted
    private String allocatePayloadId() throws IOException {
        long payloadId = nextPayloadId;
        if (payloadId <= 0L) {
            throw new IOException("Rust Runtime Host payload ID space is exhausted");
        }
        nextPayloadId = payloadId == Long.MAX_VALUE ? 0L : payloadId + 1L;
        return Long.toString(payloadId);
    }

    /// Requires one active payload backend owned by this hybrid engine.
    ///
    /// @param payloadId candidate public payload ID
    /// @return selected backend
    /// @throws IOException if closed or unknown
    private PayloadBackend requireBackend(String payloadId) throws IOException {
        requireOpen();
        @Nullable PayloadBackend backend = payloads.get(payloadId);
        if (backend == null) {
            throw new IOException("Unknown Rust Runtime Host payload ID: " + payloadId);
        }
        return backend;
    }

    /// Requires this provider-wide engine to remain open.
    ///
    /// @throws IOException after provider-wide close
    private void requireOpen() throws IOException {
        if (closed) {
            throw new IOException("Rust Runtime Host engine is closed");
        }
    }

    /// Minimal isolated payload lifecycle used by the hybrid router.
    @NotNullByDefault
    interface IsolatedPayload extends AutoCloseable {
        /// Enables the loaded payload.
        void enable() throws IOException;

        /// Invokes one payload operation with an exact deadline.
        byte[] invoke(String operation, byte[] input, long callbackId, Duration timeout) throws IOException;

        /// Disables the enabled payload.
        void disable() throws IOException;

        /// Shuts down the payload and its process.
        void shutdown() throws IOException;

        /// Terminates the payload process idempotently.
        @Override
        void close();
    }

    /// Starts one isolated payload supervisor.
    @FunctionalInterface
    @NotNullByDefault
    interface IsolatedPayloadFactory {
        /// Starts one child for one exact payload context.
        ///
        /// @param executable canonical process Host executable
        /// @param context exact isolated payload context
        /// @param scheduler shared deadline scheduler
        /// @return started and loaded isolated payload
        /// @throws IOException if process startup or loading fails
        IsolatedPayload start(
                Path executable,
                RuntimePayloadContext context,
                ScheduledExecutorService scheduler
        ) throws IOException;
    }

    /// Selects one closed backend representation.
    @NotNullByDefault
    private sealed interface PayloadBackend permits EmbeddedPayload, IsolatedPayloadBackend {
    }

    /// Retains one private embedded JNI payload ID.
    ///
    /// @param nativePayloadId opaque embedded engine payload ID
    @NotNullByDefault
    private record EmbeddedPayload(String nativePayloadId) implements PayloadBackend {
        /// Rejects a null native payload ID from a broken embedded engine.
        private EmbeddedPayload {
            Objects.requireNonNull(nativePayloadId, "nativePayloadId");
        }
    }

    /// Retains one isolated payload supervisor.
    ///
    /// @param payload isolated process owner
    @NotNullByDefault
    private record IsolatedPayloadBackend(IsolatedPayload payload) implements PayloadBackend {
        /// Rejects a null isolated payload from a broken factory.
        private IsolatedPayloadBackend {
            Objects.requireNonNull(payload, "payload");
        }
    }
}
