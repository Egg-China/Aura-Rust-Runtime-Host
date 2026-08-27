package dev.hmclce.runtime.rust;

import org.jackhuang.hmcl.plugin.runtime.PluginExecutionMode;
import org.jackhuang.hmcl.plugin.runtime.RuntimePayloadContext;
import org.jetbrains.annotations.NotNullByDefault;
import org.jetbrains.annotations.Nullable;

import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.time.Duration;
import java.util.Map;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.RejectedExecutionException;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.ScheduledFuture;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicReference;

/// Supervises one process-isolated Rust payload and services its synchronous Bridge callbacks.
@NotNullByDefault
public final class RustIsolatedPayload implements RustRuntimeEngine.IsolatedPayload {
    /// Deadline for protocol negotiation and payload loading.
    private static final Duration HANDSHAKE_TIMEOUT = Duration.ofSeconds(5L);

    /// Deadline for ordinary lifecycle transitions.
    private static final Duration LIFECYCLE_TIMEOUT = Duration.ofSeconds(10L);

    /// Grace period between ordinary and forced child termination.
    private static final long TERMINATION_GRACE_MILLIS = 250L;

    /// Maximum retained stderr tail in bytes.
    private static final int STDERR_LIMIT_BYTES = 64 * 1024;

    /// Stable redacted Bridge callback error code.
    private static final String BRIDGE_CALLBACK_ERROR = "bridge-callback";

    /// Exact child process owned by this payload.
    private final Process process;

    /// Parent-to-child protocol stream.
    private final OutputStream writer;

    /// Child-to-parent protocol stream.
    private final InputStream reader;

    /// Child diagnostic stream drained independently from protocol output.
    private final InputStream errorReader;

    /// Exact Java payload context retained for launcher-authorized Bridge callbacks.
    private final RuntimePayloadContext context;

    /// Shared daemon scheduler used only for operation deadlines.
    private final ScheduledExecutorService scheduler;

    /// Bounded process diagnostic tail.
    private final StderrTail stderrTail = new StderrTail(STDERR_LIMIT_BYTES);

    /// Daemon thread continuously draining process diagnostics.
    private final Thread stderrThread;

    /// First terminal protocol, I/O, exit, or deadline failure.
    private final AtomicReference<@Nullable IOException> terminalFailure = new AtomicReference<>();

    /// Ensures stream closure and process termination execute once.
    private final AtomicBoolean terminationStarted = new AtomicBoolean();

    /// Signals completion of stream closure and any forced process escalation.
    private final CountDownLatch terminationComplete = new CountDownLatch(1);

    /// Next positive odd parent request identifier.
    private long nextRequestId = 1L;

    /// Current Java-side lifecycle state.
    private State state = State.LOADED;

    /// Whether explicit shutdown or close has selected this payload.
    private boolean closed;

    /// Creates one supervisor around an already started child.
    ///
    /// @param process exact child process
    /// @param context exact payload context
    /// @param scheduler shared deadline scheduler
    private RustIsolatedPayload(
            Process process,
            RuntimePayloadContext context,
            ScheduledExecutorService scheduler
    ) {
        this.process = process;
        this.writer = process.getOutputStream();
        this.reader = process.getInputStream();
        this.errorReader = process.getErrorStream();
        this.context = context;
        this.scheduler = scheduler;
        this.stderrThread = new Thread(
                this::drainStderr,
                "rust-isolated-stderr-" + Integer.toUnsignedString(System.identityHashCode(process))
        );
        this.stderrThread.setDaemon(true);
        this.stderrThread.start();
    }

    /// Starts one isolated process, negotiates the protocol, and loads exactly one payload.
    ///
    /// @param executable canonical Rust process Host executable
    /// @param context exact isolated payload context
    /// @param launcher injectable process boundary
    /// @param scheduler shared deadline scheduler
    /// @return loaded isolated payload supervisor
    /// @throws IOException if paths, process launch, negotiation, or loading fail
    static RustIsolatedPayload start(
            Path executable,
            RuntimePayloadContext context,
            ProcessLauncher launcher,
            ScheduledExecutorService scheduler
    ) throws IOException {
        if (context.executionMode() != PluginExecutionMode.ISOLATED) {
            throw new IOException("Rust isolated process cannot load execution mode: " + context.executionMode());
        }
        Path canonicalExecutable = executable.toRealPath();
        if (!Files.isRegularFile(canonicalExecutable)) {
            throw new IOException("Rust isolated Host executable is not a regular file: " + executable);
        }
        Path packageRoot = context.packagePath().toRealPath();
        if (!Files.isDirectory(packageRoot)) {
            throw new IOException("Rust isolated payload root is not a directory: " + packageRoot);
        }
        ProcessBuilder builder = new ProcessBuilder(canonicalExecutable.toString(), "--stdio");
        builder.directory(packageRoot.toFile());
        Map<String, String> environment = builder.environment();
        environment.clear();
        for (Map.Entry<String, String> entry : System.getenv().entrySet()) {
            if (allowedEnvironmentKey(entry.getKey())) {
                environment.put(entry.getKey(), entry.getValue());
            }
        }

        Process process = launcher.start(builder);
        RustIsolatedPayload payload = new RustIsolatedPayload(process, context, scheduler);
        try {
            payload.expectOk(new RustProcessMessage.Hello(payload.allocateRequestId()), HANDSHAKE_TIMEOUT);
            payload.expectOk(new RustProcessMessage.Load(
                    payload.allocateRequestId(),
                    packageRoot.toString(),
                    context.entrypoint(),
                    1L,
                    1L
            ), HANDSHAKE_TIMEOUT);
            return payload;
        } catch (IOException | RuntimeException | Error exception) {
            payload.close();
            throw exception;
        }
    }

    /// Enables the loaded isolated payload.
    ///
    /// @throws IOException if the lifecycle state, child, or plugin rejects the transition
    public synchronized void enable() throws IOException {
        requireState(State.LOADED, "enable");
        expectOk(new RustProcessMessage.Enable(allocateRequestId()), LIFECYCLE_TIMEOUT);
        state = State.ENABLED;
    }

    /// Invokes one enabled isolated payload operation with the caller-supplied deadline.
    ///
    /// @param operation canonical payload operation
    /// @param input opaque canonical Bridge Value v1 input
    /// @param callbackId nonnegative payload-local callback identifier
    /// @param timeout positive operation deadline
    /// @return opaque canonical Bridge Value v1 output
    /// @throws IOException if state, transport, process, deadline, or plugin invocation fails
    public synchronized byte[] invoke(
            String operation,
            byte[] input,
            long callbackId,
            Duration timeout
    ) throws IOException {
        requireState(State.ENABLED, "invoke");
        RustProcessMessage response = exchange(
                new RustProcessMessage.Invoke(allocateRequestId(), operation, input, callbackId), timeout);
        if (response instanceof RustProcessMessage.Result result) {
            return result.output();
        }
        throwResponse(response, "invoke result");
        throw new AssertionError("throwResponse must not return");
    }

    /// Disables the enabled isolated payload.
    ///
    /// @throws IOException if the lifecycle state, child, or plugin rejects the transition
    public synchronized void disable() throws IOException {
        requireState(State.ENABLED, "disable");
        expectOk(new RustProcessMessage.Disable(allocateRequestId()), LIFECYCLE_TIMEOUT);
        state = State.DISABLED;
    }

    /// Shuts down the payload and guarantees process termination even when the plugin fails.
    ///
    /// @throws IOException if the lifecycle state, child, or plugin rejects shutdown
    public synchronized void shutdown() throws IOException {
        requireActive("shutdown");
        try {
            expectOk(new RustProcessMessage.Shutdown(allocateRequestId()), LIFECYCLE_TIMEOUT);
            state = State.CLOSED;
            closed = true;
            terminateProcess();
        } catch (IOException exception) {
            state = State.CLOSED;
            closed = true;
            terminateProcess();
            throw exception;
        }
    }

    /// Terminates this child exactly once without resolving Java capability authority.
    @Override
    public synchronized void close() {
        if (closed) {
            return;
        }
        closed = true;
        state = State.CLOSED;
        terminateProcess();
    }

    /// Exchanges one parent command while synchronously servicing child callbacks.
    ///
    /// @param request parent command
    /// @param timeout positive command deadline
    /// @return matching child response
    /// @throws IOException if transport, protocol, process, or deadline fails
    private RustProcessMessage exchange(RustProcessMessage request, Duration timeout) throws IOException {
        requireActive("command");
        if (timeout.isZero() || timeout.isNegative()) {
            throw new IOException("Rust isolated payload timeout must be positive");
        }
        AtomicBoolean deadlineOpen = new AtomicBoolean(true);
        ScheduledFuture<?> deadline;
        try {
            deadline = scheduler.schedule(
                    () -> expireDeadline(deadlineOpen, request.requestId()),
                    timeout.toNanos(),
                    TimeUnit.NANOSECONDS
            );
        } catch (ArithmeticException | RejectedExecutionException exception) {
            throw poison(failure("Cannot schedule Rust isolated payload deadline", exception));
        }

        try {
            RustProcessWireCodec.write(writer, request);
            writer.flush();
            while (true) {
                @Nullable RustProcessMessage response = RustProcessWireCodec.read(reader);
                if (response == null) {
                    throw new IOException("Rust isolated payload exited before responding");
                }
                if (response.requestId() == request.requestId()) {
                    if (!deadlineOpen.compareAndSet(true, false)) {
                        throw terminalOr(failure("Rust isolated payload deadline completed without a failure"));
                    }
                    return response;
                }
                if ((response.requestId() & 1L) == 0L && isChildCallback(response)) {
                    answerCallback(response);
                    continue;
                }
                throw new IOException("Rust isolated payload returned a mismatched request ID");
            }
        } catch (IOException exception) {
            @Nullable IOException terminal = terminalFailure.get();
            if (terminal != null) {
                awaitTerminationComplete();
                throw terminal;
            }
            throw poison(failure("Rust isolated payload protocol failed", exception));
        } finally {
            deadlineOpen.set(false);
            deadline.cancel(false);
        }
    }

    /// Requires one successful lifecycle response or propagates a plugin-reported error.
    ///
    /// @param request lifecycle request
    /// @param timeout lifecycle deadline
    /// @throws IOException if the response is an error or has an unexpected kind
    private void expectOk(RustProcessMessage request, Duration timeout) throws IOException {
        RustProcessMessage response = exchange(request, timeout);
        if (response instanceof RustProcessMessage.Ok) {
            return;
        }
        throwResponse(response, "ok response");
    }

    /// Converts one remote error to an ordinary failure and poisons unexpected response kinds.
    ///
    /// @param response matching response
    /// @param expected expected response description
    /// @throws IOException always
    private void throwResponse(RustProcessMessage response, String expected) throws IOException {
        if (response instanceof RustProcessMessage.Error error) {
            throw new IOException("Rust isolated Host error [" + error.code() + "]: " + error.message());
        }
        throw poison(failure("Rust isolated payload returned an unexpected " + expected));
    }

    /// Services one child-originated Bridge callback and writes a same-ID response.
    ///
    /// @param callback child callback message
    /// @throws IOException if response encoding or transport fails
    private void answerCallback(RustProcessMessage callback) throws IOException {
        RustProcessMessage response;
        try {
            if (callback instanceof RustProcessMessage.BridgeInvoke invoke) {
                byte[] output = context.bridgeTransport().invoke(context, invoke.operation(), invoke.input());
                response = new RustProcessMessage.CallbackResult(invoke.requestId(), output);
            } else if (callback instanceof RustProcessMessage.RetainHandle retain) {
                context.bridgeTransport().retainHandle(context, retain.objectId(), retain.generation());
                response = new RustProcessMessage.CallbackResult(retain.requestId(), new byte[0]);
            } else if (callback instanceof RustProcessMessage.ReleaseHandle release) {
                context.bridgeTransport().releaseHandle(context, release.objectId(), release.generation());
                response = new RustProcessMessage.CallbackResult(release.requestId(), new byte[0]);
            } else {
                throw failure("Rust isolated payload returned an unexpected callback kind");
            }
        } catch (IOException | RuntimeException exception) {
            response = new RustProcessMessage.CallbackError(callback.requestId(), BRIDGE_CALLBACK_ERROR);
        }
        RustProcessWireCodec.write(writer, response);
        writer.flush();
    }

    /// Returns whether one even-ID message is a child callback request.
    ///
    /// @param message candidate child message
    /// @return whether the parent must service it
    private static boolean isChildCallback(RustProcessMessage message) {
        return message instanceof RustProcessMessage.BridgeInvoke
                || message instanceof RustProcessMessage.RetainHandle
                || message instanceof RustProcessMessage.ReleaseHandle;
    }

    /// Expires one still-active command and terminates its child.
    ///
    /// @param deadlineOpen command deadline gate
    /// @param requestId timed-out request identifier
    private void expireDeadline(AtomicBoolean deadlineOpen, long requestId) {
        if (!deadlineOpen.compareAndSet(true, false)) {
            return;
        }
        IOException timeout = failure("Rust isolated payload request " + requestId + " timed out");
        terminalFailure.compareAndSet(null, timeout);
        terminateProcess();
    }

    /// Stores the first terminal failure and terminates the child.
    ///
    /// @param failure new terminal failure
    /// @return first retained terminal failure
    private IOException poison(IOException failure) {
        terminalFailure.compareAndSet(null, failure);
        terminateProcess();
        return terminalOr(failure);
    }

    /// Returns the retained terminal failure or one supplied fallback.
    ///
    /// @param fallback failure used before terminal publication
    /// @return retained or fallback failure
    private IOException terminalOr(IOException fallback) {
        @Nullable IOException terminal = terminalFailure.get();
        return terminal == null ? fallback : terminal;
    }

    /// Allocates the next positive odd parent request identifier.
    ///
    /// @return allocated request identifier
    /// @throws IOException if signed-64 identifier space is exhausted
    private long allocateRequestId() throws IOException {
        long requestId = nextRequestId;
        if (requestId <= 0L) {
            throw poison(failure("Rust isolated payload request ID space is exhausted"));
        }
        nextRequestId = requestId == Long.MAX_VALUE ? 0L : requestId + 2L;
        return requestId;
    }

    /// Requires one exact local lifecycle state.
    ///
    /// @param expected required state
    /// @param operation attempted operation
    /// @throws IOException if terminal, closed, or in another state
    private void requireState(State expected, String operation) throws IOException {
        requireActive(operation);
        if (state != expected) {
            throw new IOException("Rust isolated payload cannot " + operation + " from state " + state);
        }
    }

    /// Requires an active nonterminal payload.
    ///
    /// @param operation attempted operation
    /// @throws IOException if closed or poisoned
    private void requireActive(String operation) throws IOException {
        @Nullable IOException terminal = terminalFailure.get();
        if (terminal != null) {
            throw terminal;
        }
        if (closed) {
            throw new IOException("Rust isolated payload is closed and cannot " + operation);
        }
    }

    /// Drains stderr continuously into the bounded diagnostic tail.
    private void drainStderr() {
        byte[] buffer = new byte[4096];
        try {
            int read;
            while ((read = errorReader.read(buffer)) >= 0) {
                if (read > 0) {
                    stderrTail.append(buffer, read);
                }
            }
        } catch (IOException ignored) {
            // stderr is diagnostic-only and is closed during every terminal path.
        }
    }

    /// Terminates the child once, escalating after the fixed grace period.
    private void terminateProcess() {
        if (!terminationStarted.compareAndSet(false, true)) {
            return;
        }
        try {
            closeQuietly(writer);
            closeQuietly(reader);
            if (process.isAlive()) {
                process.destroy();
                try {
                    if (!process.waitFor(TERMINATION_GRACE_MILLIS, TimeUnit.MILLISECONDS)) {
                        process.destroyForcibly();
                    }
                } catch (InterruptedException exception) {
                    Thread.currentThread().interrupt();
                    process.destroyForcibly();
                }
            }
            closeQuietly(errorReader);
        } finally {
            terminationComplete.countDown();
        }
    }

    /// Waits for an already selected terminal path to finish process escalation.
    private void awaitTerminationComplete() {
        try {
            terminationComplete.await();
        } catch (InterruptedException exception) {
            Thread.currentThread().interrupt();
        }
    }

    /// Closes one process stream without replacing the primary failure.
    ///
    /// @param closeable owned process stream
    private static void closeQuietly(java.io.Closeable closeable) {
        try {
            closeable.close();
        } catch (IOException ignored) {
            // The protocol or deadline failure remains the observable cause.
        }
    }

    /// Creates one diagnostic failure and includes the bounded stderr tail.
    ///
    /// @param detail primary failure detail
    /// @return diagnostic failure
    private IOException failure(String detail) {
        awaitExitedStderr();
        StringBuilder message = new StringBuilder(detail);
        if (!process.isAlive()) {
            try {
                message.append(" (exit ").append(process.exitValue()).append(')');
            } catch (IllegalThreadStateException ignored) {
                // The process raced back to the live state, which real Process implementations do not do.
            }
        }
        String stderr = stderrTail.snapshot();
        if (!stderr.isEmpty()) {
            message.append("; stderr tail: ").append(stderr);
        }
        return new IOException(message.toString());
    }

    /// Creates one diagnostic failure with its local transport cause.
    ///
    /// @param detail primary failure detail
    /// @param cause local failure cause
    /// @return diagnostic failure
    private IOException failure(String detail, Throwable cause) {
        @Nullable String causeMessage = cause.getMessage();
        IOException failure = failure(causeMessage == null || causeMessage.isBlank()
                ? detail
                : detail + ": " + causeMessage);
        failure.initCause(cause);
        return failure;
    }

    /// Lets a completed child finish draining its already closed diagnostic stream.
    private void awaitExitedStderr() {
        if (process.isAlive() || Thread.currentThread() == stderrThread) {
            return;
        }
        try {
            stderrThread.join(TERMINATION_GRACE_MILLIS);
        } catch (InterruptedException exception) {
            Thread.currentThread().interrupt();
        }
    }

    /// Returns whether one launcher environment key is safe to inherit.
    ///
    /// @param key environment key
    /// @return allowlist membership
    private static boolean allowedEnvironmentKey(String key) {
        return key.equalsIgnoreCase("SystemRoot")
                || key.equalsIgnoreCase("WINDIR")
                || key.equalsIgnoreCase("PATH")
                || key.equalsIgnoreCase("PATHEXT")
                || key.equalsIgnoreCase("TEMP")
                || key.equalsIgnoreCase("TMP")
                || key.equalsIgnoreCase("HOME")
                || key.equalsIgnoreCase("USERPROFILE")
                || key.equalsIgnoreCase("LANG")
                || key.regionMatches(true, 0, "LC_", 0, 3);
    }

    /// Starts one exact no-shell child process from a prepared builder.
    @FunctionalInterface
    @NotNullByDefault
    interface ProcessLauncher {
        /// Starts the prepared process directly.
        ///
        /// @param builder exact command, working directory, and environment
        /// @return started child process
        /// @throws IOException if the operating system rejects process creation
        Process start(ProcessBuilder builder) throws IOException;
    }

    /// Enumerates Java-side payload lifecycle states.
    @NotNullByDefault
    private enum State {
        /// Payload library is loaded but not initialized.
        LOADED,

        /// Payload is initialized and accepts invocation.
        ENABLED,

        /// Payload is disabled and ready for shutdown.
        DISABLED,

        /// Payload process has been selected for termination.
        CLOSED
    }

    /// Stores only the newest bounded bytes written to stderr.
    @NotNullByDefault
    private static final class StderrTail {
        /// Fixed circular byte storage.
        private final byte[] buffer;

        /// Index where the next byte is written.
        private int cursor;

        /// Number of currently retained bytes.
        private int size;

        /// Creates one fixed-capacity diagnostic ring.
        ///
        /// @param capacity maximum retained byte count
        private StderrTail(int capacity) {
            this.buffer = new byte[capacity];
        }

        /// Appends bytes while discarding the oldest excess content.
        ///
        /// @param source source buffer
        /// @param length valid source prefix length
        private synchronized void append(byte[] source, int length) {
            for (int index = 0; index < length; index++) {
                buffer[cursor] = source[index];
                cursor = (cursor + 1) % buffer.length;
                if (size < buffer.length) {
                    size++;
                }
            }
        }

        /// Decodes a stable copy of the retained byte tail as UTF-8.
        ///
        /// @return diagnostic tail with malformed boundaries replaced
        private synchronized String snapshot() {
            byte[] ordered = new byte[size];
            int start = (cursor - size + buffer.length) % buffer.length;
            int first = Math.min(size, buffer.length - start);
            System.arraycopy(buffer, start, ordered, 0, first);
            if (first < size) {
                System.arraycopy(buffer, 0, ordered, first, size - first);
            }
            return new String(ordered, StandardCharsets.UTF_8);
        }
    }
}
