package dev.hmclce.runtime.rust;

import org.jackhuang.hmcl.plugin.PluginArtifactIdentity;
import org.jackhuang.hmcl.plugin.runtime.PluginExecutionMode;
import org.jackhuang.hmcl.plugin.runtime.RuntimeBridgeTransport;
import org.jackhuang.hmcl.plugin.runtime.RuntimePayloadContext;
import org.jetbrains.annotations.NotNullByDefault;
import org.jetbrains.annotations.Nullable;
import org.jetbrains.annotations.Unmodifiable;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.api.io.TempDir;

import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.time.Duration;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.ScheduledThreadPoolExecutor;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertInstanceOf;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

/// Verifies supervision of one isolated Rust payload through a real duplex stdio boundary.
@NotNullByDefault
final class RustIsolatedPayloadTest {
    /// Canonical Bridge Value v1 null used as opaque callback data.
    private static final byte @Unmodifiable [] WIRE_NULL = new byte[]{(byte) 0x92, 0, (byte) 0xc0};

    /// Test-owned package root and executable location.
    @TempDir
    Path temporaryDirectory;

    /// Starts an exact child process and completes all lifecycle and Bridge callback operations.
    @Test
    void startsExactProcessAndCompletesBridgeLifecycleWithoutResolvingTokens() throws Exception {
        List<RustProcessMessage> childMessages = new CopyOnWriteArrayList<>();
        List<RustProcessMessage> callbackResponses = new CopyOnWriteArrayList<>();
        ScriptedProcess process = new ScriptedProcess(endpoint -> {
            completeHandshake(endpoint, childMessages);

            RustProcessMessage.Enable enable = expect(endpoint, RustProcessMessage.Enable.class);
            childMessages.add(enable);
            endpoint.write(new RustProcessMessage.BridgeInvoke(2L, "initialize", WIRE_NULL));
            callbackResponses.add(endpoint.readRequired());
            endpoint.write(new RustProcessMessage.Ok(enable.requestId()));

            RustProcessMessage.Invoke invoke = expect(endpoint, RustProcessMessage.Invoke.class);
            childMessages.add(invoke);
            endpoint.write(new RustProcessMessage.RetainHandle(4L, 71L, 3L));
            callbackResponses.add(endpoint.readRequired());
            endpoint.write(new RustProcessMessage.BridgeInvoke(6L, "fixture.bridge", invoke.input()));
            RustProcessMessage.CallbackResult callback = expect(
                    endpoint, RustProcessMessage.CallbackResult.class);
            callbackResponses.add(callback);
            endpoint.write(new RustProcessMessage.ReleaseHandle(8L, 71L, 3L));
            callbackResponses.add(endpoint.readRequired());
            endpoint.write(new RustProcessMessage.Result(invoke.requestId(), callback.output()));

            RustProcessMessage.Disable disable = expect(endpoint, RustProcessMessage.Disable.class);
            childMessages.add(disable);
            endpoint.write(new RustProcessMessage.Ok(disable.requestId()));

            RustProcessMessage.Shutdown shutdown = expect(endpoint, RustProcessMessage.Shutdown.class);
            childMessages.add(shutdown);
            endpoint.write(new RustProcessMessage.BridgeInvoke(10L, "shutdown", WIRE_NULL));
            callbackResponses.add(endpoint.readRequired());
            endpoint.write(new RustProcessMessage.Ok(shutdown.requestId()));
            return 0;
        }, true);
        RecordingLauncher launcher = new RecordingLauncher(process);
        RecordingBridgeTransport bridge = new RecordingBridgeTransport();
        AtomicInteger tokenCalls = new AtomicInteger();
        RuntimePayloadContext context = context(tokenCalls, bridge);
        Path executable = executable();
        ScheduledExecutorService scheduler = scheduler();

        try {
            RustIsolatedPayload payload = RustIsolatedPayload.start(executable, context, launcher, scheduler);
            payload.enable();
            assertArrayEquals(WIRE_NULL,
                    payload.invoke("bridge", WIRE_NULL, 41L, Duration.ofSeconds(1)));
            payload.disable();
            payload.shutdown();

            process.awaitScript();
            assertFalse(process.isAlive());
            assertEquals(0, tokenCalls.get());
            assertEquals(List.of(executable.toString(), "--stdio"), launcher.command);
            assertEquals(temporaryDirectory.toRealPath(), launcher.directory.toRealPath());
            assertEquals(expectedChildEnvironment(), launcher.environment);
            assertEquals(List.of(
                    "invoke:initialize", "retain:71:3", "invoke:fixture.bridge",
                    "release:71:3", "invoke:shutdown"), bridge.events);
            assertEquals(List.of(1L, 3L, 5L, 7L, 9L, 11L),
                    childMessages.stream().map(RustProcessMessage::requestId).toList());
            RustProcessMessage.Load load = assertInstanceOf(RustProcessMessage.Load.class, childMessages.get(1));
            assertEquals(temporaryDirectory.toAbsolutePath().normalize().toString(), load.packageRoot());
            assertEquals("payload/plugin.dll", load.entrypoint());
            assertEquals(1L, load.pluginId());
            assertEquals(1L, load.session());
            assertEquals(List.of(2L, 4L, 6L, 8L, 10L),
                    callbackResponses.stream().map(RustProcessMessage::requestId).toList());
        } finally {
            scheduler.shutdownNow();
            process.destroyForcibly();
        }
    }

    /// A mismatched response ID poisons the payload and terminates its child exactly once.
    @Test
    void poisonsAndKillsChildOnResponseIdMismatch() throws Exception {
        ScriptedProcess process = new ScriptedProcess(endpoint -> {
            completeHandshake(endpoint, new ArrayList<>());
            RustProcessMessage.Enable enable = expect(endpoint, RustProcessMessage.Enable.class);
            endpoint.write(new RustProcessMessage.Ok(enable.requestId() + 2L));
            endpoint.read();
            return 0;
        }, true);
        ScheduledExecutorService scheduler = scheduler();
        try {
            RustIsolatedPayload payload = RustIsolatedPayload.start(
                    executable(), context(new AtomicInteger(), new RecordingBridgeTransport()),
                    new RecordingLauncher(process), scheduler);

            IOException mismatch = assertThrows(IOException.class, payload::enable);
            IOException terminal = assertThrows(IOException.class, payload::disable);

            assertTrue(mismatch.getMessage().contains("request ID"));
            assertEquals(mismatch.getMessage(), terminal.getMessage());
            assertFalse(process.isAlive());
            assertEquals(1, process.destroyCalls.get() + process.forceDestroyCalls.get());
        } finally {
            scheduler.shutdownNow();
            process.destroyForcibly();
        }
    }

    /// An invocation deadline closes protocol streams, tries graceful destroy, then forces a stuck child.
    @Test
    void timeoutPoisonsAndForciblyTerminatesStuckChild() throws Exception {
        CountDownLatch stuck = new CountDownLatch(1);
        ScriptedProcess process = new ScriptedProcess(endpoint -> {
            completeHandshake(endpoint, new ArrayList<>());
            RustProcessMessage.Enable enable = expect(endpoint, RustProcessMessage.Enable.class);
            endpoint.write(new RustProcessMessage.Ok(enable.requestId()));
            expect(endpoint, RustProcessMessage.Invoke.class);
            stuck.await();
            return 0;
        }, false);
        ScheduledExecutorService scheduler = scheduler();
        try {
            RustIsolatedPayload payload = RustIsolatedPayload.start(
                    executable(), context(new AtomicInteger(), new RecordingBridgeTransport()),
                    new RecordingLauncher(process), scheduler);
            payload.enable();

            IOException timeout = assertThrows(IOException.class,
                    () -> payload.invoke("hang", WIRE_NULL, 41L, Duration.ofMillis(20)));

            assertTrue(timeout.getMessage().contains("timed out"));
            assertFalse(process.isAlive());
            assertEquals(1, process.destroyCalls.get());
            assertEquals(1, process.forceDestroyCalls.get());
        } finally {
            stuck.countDown();
            scheduler.shutdownNow();
            process.destroyForcibly();
        }
    }

    /// Unexpected child exit reports only the bounded stderr tail and remains terminal.
    @Test
    void unexpectedExitIncludesOnlyBoundedStderrTail() throws Exception {
        ScriptedProcess process = new ScriptedProcess(endpoint -> {
            completeHandshake(endpoint, new ArrayList<>());
            expect(endpoint, RustProcessMessage.Enable.class);
            endpoint.error().write("x".repeat(70_000).getBytes(StandardCharsets.UTF_8));
            endpoint.error().write("TAIL-MARKER".getBytes(StandardCharsets.UTF_8));
            endpoint.error().flush();
            return 23;
        }, true);
        ScheduledExecutorService scheduler = scheduler();
        try {
            RustIsolatedPayload payload = RustIsolatedPayload.start(
                    executable(), context(new AtomicInteger(), new RecordingBridgeTransport()),
                    new RecordingLauncher(process), scheduler);

            IOException failure = assertThrows(IOException.class, payload::enable);

            assertTrue(failure.getMessage().contains("TAIL-MARKER"));
            assertTrue(failure.getMessage().length() < 66_000);
            assertThrows(IOException.class, payload::disable);
        } finally {
            scheduler.shutdownNow();
            process.destroyForcibly();
        }
    }

    /// Bridge exceptions become one stable callback code without exposing exception text.
    @Test
    void bridgeFailuresReturnRedactedCallbackErrors() throws Exception {
        List<RustProcessMessage> callbackResponses = new CopyOnWriteArrayList<>();
        ScriptedProcess process = new ScriptedProcess(endpoint -> {
            completeHandshake(endpoint, new ArrayList<>());
            RustProcessMessage.Enable enable = expect(endpoint, RustProcessMessage.Enable.class);
            endpoint.write(new RustProcessMessage.BridgeInvoke(2L, "denied", WIRE_NULL));
            RustProcessMessage response = endpoint.readRequired();
            callbackResponses.add(response);
            endpoint.write(new RustProcessMessage.Error(
                    enable.requestId(), "plugin-status", "Plugin callback failed"));
            RustProcessMessage.Shutdown shutdown = expect(endpoint, RustProcessMessage.Shutdown.class);
            endpoint.write(new RustProcessMessage.Ok(shutdown.requestId()));
            return 0;
        }, true);
        RecordingBridgeTransport bridge = new RecordingBridgeTransport();
        bridge.failInvocations = true;
        ScheduledExecutorService scheduler = scheduler();
        try {
            RustIsolatedPayload payload = RustIsolatedPayload.start(
                    executable(), context(new AtomicInteger(), bridge), new RecordingLauncher(process), scheduler);

            IOException pluginFailure = assertThrows(IOException.class, payload::enable);
            RustProcessMessage.CallbackError callback = assertInstanceOf(
                    RustProcessMessage.CallbackError.class, callbackResponses.get(0));

            assertEquals("bridge-callback", callback.code());
            assertFalse(callback.toString().contains("SECRET-DIAGNOSTIC"));
            assertTrue(pluginFailure.getMessage().contains("plugin-status"));
            payload.shutdown();
        } finally {
            scheduler.shutdownNow();
            process.destroyForcibly();
        }
    }

    /// Closing an active payload repeatedly terminates its process only once.
    @Test
    void closeIsIdempotent() throws Exception {
        ScriptedProcess process = new ScriptedProcess(endpoint -> {
            completeHandshake(endpoint, new ArrayList<>());
            endpoint.read();
            return 0;
        }, true);
        ScheduledExecutorService scheduler = scheduler();
        try {
            RustIsolatedPayload payload = RustIsolatedPayload.start(
                    executable(), context(new AtomicInteger(), new RecordingBridgeTransport()),
                    new RecordingLauncher(process), scheduler);

            payload.close();
            payload.close();

            assertFalse(process.isAlive());
            assertEquals(1, process.destroyCalls.get());
            assertEquals(0, process.forceDestroyCalls.get());
        } finally {
            scheduler.shutdownNow();
            process.destroyForcibly();
        }
    }

    /// Creates one executable fixture at an absolute canonical location.
    ///
    /// @return executable fixture
    private Path executable() throws IOException {
        Path executable = temporaryDirectory.resolve("hmcl-rust-host-process.exe");
        Files.write(executable, new byte[]{0x48, 0x4d, 0x43, 0x4c});
        return executable.toRealPath();
    }

    /// Creates one isolated payload context with a token supplier that must remain untouched.
    ///
    /// @param tokenCalls supplier call counter
    /// @param bridge recording Bridge transport
    /// @return isolated payload context
    private RuntimePayloadContext context(AtomicInteger tokenCalls, RuntimeBridgeTransport bridge) throws IOException {
        Files.createDirectories(temporaryDirectory.resolve("data"));
        return new RuntimePayloadContext(
                new PluginArtifactIdentity("dev.hmclce.test.rust-isolated", "1.0.0", "a".repeat(64)),
                temporaryDirectory,
                "payload/plugin.dll",
                PluginExecutionMode.ISOLATED,
                temporaryDirectory.resolve("data"),
                () -> {
                    tokenCalls.incrementAndGet();
                    throw new AssertionError("Capability token supplier must not cross the process boundary");
                },
                bridge
        );
    }

    /// Creates one scheduler whose worker cannot keep the test JVM alive.
    ///
    /// @return single-thread deadline scheduler
    private static ScheduledExecutorService scheduler() {
        ScheduledThreadPoolExecutor scheduler = new ScheduledThreadPoolExecutor(1, runnable -> {
            Thread thread = new Thread(runnable, "rust-isolated-test-deadline");
            thread.setDaemon(true);
            return thread;
        });
        scheduler.setRemoveOnCancelPolicy(true);
        return scheduler;
    }

    /// Completes the required hello and one-payload load sequence.
    ///
    /// @param endpoint child-side protocol endpoint
    /// @param messages observed parent messages
    private static void completeHandshake(ChildEndpoint endpoint, List<RustProcessMessage> messages)
            throws IOException {
        RustProcessMessage.Hello hello = expect(endpoint, RustProcessMessage.Hello.class);
        messages.add(hello);
        endpoint.write(new RustProcessMessage.Ok(hello.requestId()));
        RustProcessMessage.Load load = expect(endpoint, RustProcessMessage.Load.class);
        messages.add(load);
        endpoint.write(new RustProcessMessage.Ok(load.requestId()));
    }

    /// Reads one message and requires its exact model type.
    ///
    /// @param endpoint child-side endpoint
    /// @param type expected model class
    /// @param <T> exact message type
    /// @return typed message
    private static <T extends RustProcessMessage> T expect(ChildEndpoint endpoint, Class<T> type)
            throws IOException {
        return type.cast(endpoint.readRequired());
    }

    /// Computes the exact environment allowlist expected by the child process.
    ///
    /// @return ordered allowlisted environment
    private static Map<String, String> expectedChildEnvironment() {
        Map<String, String> expected = new LinkedHashMap<>();
        for (Map.Entry<String, String> entry : System.getenv().entrySet()) {
            if (allowedEnvironmentKey(entry.getKey())) {
                expected.put(entry.getKey(), entry.getValue());
            }
        }
        return expected;
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

    /// Starts one prepared process while retaining its exact builder configuration.
    @NotNullByDefault
    private static final class RecordingLauncher implements RustIsolatedPayload.ProcessLauncher {
        /// Process returned for the single launch.
        private final Process process;

        /// Exact command snapshot.
        private List<String> command = List.of();

        /// Exact child working directory.
        private Path directory = Path.of(".");

        /// Exact child environment snapshot.
        private Map<String, String> environment = Map.of();

        /// Creates one single-process launcher.
        ///
        /// @param process process to return
        private RecordingLauncher(Process process) {
            this.process = process;
        }

        /// Records the builder and returns the scripted process.
        ///
        /// @param builder exact prepared process builder
        /// @return scripted process
        @Override
        public Process start(ProcessBuilder builder) {
            command = List.copyOf(builder.command());
            directory = builder.directory().toPath();
            environment = Map.copyOf(builder.environment());
            return process;
        }
    }

    /// Records launcher Bridge calls and optionally rejects invocation.
    @NotNullByDefault
    private static final class RecordingBridgeTransport implements RuntimeBridgeTransport {
        /// Ordered Bridge side effects.
        private final List<String> events = new CopyOnWriteArrayList<>();

        /// Whether invocation returns a redacted failure.
        private boolean failInvocations;

        /// Records and echoes one opaque Bridge invocation.
        @Override
        public byte[] invoke(RuntimePayloadContext context, String operation, byte[] input) throws IOException {
            events.add("invoke:" + operation);
            if (failInvocations) {
                throw new IOException("SECRET-DIAGNOSTIC");
            }
            return input.clone();
        }

        /// Records one handle retain.
        @Override
        public void retainHandle(RuntimePayloadContext context, long objectId, long generation) {
            events.add("retain:" + objectId + ":" + generation);
        }

        /// Records one handle release.
        @Override
        public void releaseHandle(RuntimePayloadContext context, long objectId, long generation) {
            events.add("release:" + objectId + ":" + generation);
        }
    }

    /// Runs one child-side protocol script over connected in-memory pipes.
    @FunctionalInterface
    @NotNullByDefault
    private interface ChildBehavior {
        /// Runs the child and returns its exit code.
        ///
        /// @param endpoint child-side protocol and stderr streams
        /// @return process exit code
        int run(ChildEndpoint endpoint) throws Exception;
    }

    /// Exposes the child's connected protocol and diagnostic streams.
    @NotNullByDefault
    private static final class ChildEndpoint {
        /// Parent-to-child protocol stream.
        private final InputStream input;

        /// Child-to-parent protocol stream.
        private final OutputStream output;

        /// Child diagnostic stream.
        private final OutputStream error;

        /// Creates one connected child endpoint.
        private ChildEndpoint(InputStream input, OutputStream output, OutputStream error) {
            this.input = input;
            this.output = output;
            this.error = error;
        }

        /// Reads one message or clean EOF.
        ///
        /// @return message or null
        private @Nullable RustProcessMessage read() throws IOException {
            return RustProcessWireCodec.read(input);
        }

        /// Reads one required parent message.
        ///
        /// @return message
        private RustProcessMessage readRequired() throws IOException {
            RustProcessMessage message = read();
            if (message == null) {
                throw new IOException("Unexpected parent EOF");
            }
            return message;
        }

        /// Writes one child message immediately.
        ///
        /// @param message source message
        private void write(RustProcessMessage message) throws IOException {
            RustProcessWireCodec.write(output, message);
            output.flush();
        }

        /// Returns the child diagnostic stream.
        ///
        /// @return stderr stream
        private OutputStream error() {
            return error;
        }
    }

    /// Implements one controllable `Process` with real connected pipes.
    @NotNullByDefault
    private static final class ScriptedProcess extends Process {
        /// Parent-to-child writable stream.
        private final java.io.PipedOutputStream parentInput;

        /// Child-to-parent readable stream.
        private final java.io.PipedInputStream parentOutput;

        /// Child-to-parent diagnostic stream.
        private final java.io.PipedInputStream parentError;

        /// Child-side input closed on termination.
        private final java.io.PipedInputStream childInput;

        /// Child-side output closed on termination.
        private final java.io.PipedOutputStream childOutput;

        /// Child-side diagnostic output closed on termination.
        private final java.io.PipedOutputStream childError;

        /// Process completion signal.
        private final CountDownLatch exited = new CountDownLatch(1);

        /// Whether the fake process is still live.
        private final AtomicBoolean alive = new AtomicBoolean(true);

        /// Whether ordinary destroy terminates the child.
        private final boolean exitOnDestroy;

        /// Ordinary destroy call count.
        private final AtomicInteger destroyCalls = new AtomicInteger();

        /// Force-destroy call count.
        private final AtomicInteger forceDestroyCalls = new AtomicInteger();

        /// Child script thread.
        private final Thread scriptThread;

        /// Child exit code after completion.
        private volatile int exitCode;

        /// Unexpected child script failure.
        private volatile @Nullable Throwable scriptFailure;

        /// Creates and starts one connected child script.
        private ScriptedProcess(ChildBehavior behavior, boolean exitOnDestroy) throws IOException {
            childInput = new java.io.PipedInputStream(128 * 1024);
            parentInput = new java.io.PipedOutputStream(childInput);
            parentOutput = new java.io.PipedInputStream(128 * 1024);
            childOutput = new java.io.PipedOutputStream(parentOutput);
            parentError = new java.io.PipedInputStream(128 * 1024);
            childError = new java.io.PipedOutputStream(parentError);
            this.exitOnDestroy = exitOnDestroy;
            scriptThread = new Thread(() -> runScript(behavior), "rust-isolated-test-child");
            scriptThread.setDaemon(true);
            scriptThread.start();
        }

        /// Runs the child behavior and records unexpected failures only while still live.
        private void runScript(ChildBehavior behavior) {
            try {
                int result = behavior.run(new ChildEndpoint(childInput, childOutput, childError));
                complete(result);
            } catch (Throwable throwable) {
                if (alive.get()) {
                    scriptFailure = throwable;
                    complete(91);
                }
            }
        }

        /// Marks process completion and closes every child-side pipe.
        private void complete(int result) {
            if (!alive.compareAndSet(true, false)) {
                return;
            }
            exitCode = result;
            closeQuietly(childInput);
            closeQuietly(childOutput);
            closeQuietly(childError);
            exited.countDown();
        }

        /// Waits for the child script and rethrows unexpected script failures.
        private void awaitScript() throws Exception {
            assertTrue(exited.await(2, TimeUnit.SECONDS));
            if (scriptFailure != null) {
                throw new AssertionError("Child script failed", scriptFailure);
            }
        }

        /// Closes one stream without changing the test outcome.
        private static void closeQuietly(java.io.Closeable closeable) {
            try {
                closeable.close();
            } catch (IOException ignored) {
                // Termination already owns the observable outcome.
            }
        }

        /// Returns the parent's writable stdin endpoint.
        @Override
        public OutputStream getOutputStream() {
            return parentInput;
        }

        /// Returns the parent's readable stdout endpoint.
        @Override
        public InputStream getInputStream() {
            return parentOutput;
        }

        /// Returns the parent's readable stderr endpoint.
        @Override
        public InputStream getErrorStream() {
            return parentError;
        }

        /// Waits indefinitely for completion.
        @Override
        public int waitFor() throws InterruptedException {
            exited.await();
            return exitCode;
        }

        /// Waits up to one timeout for completion.
        @Override
        public boolean waitFor(long timeout, TimeUnit unit) throws InterruptedException {
            return exited.await(timeout, unit);
        }

        /// Returns the exit code only after completion.
        @Override
        public int exitValue() {
            if (alive.get()) {
                throw new IllegalThreadStateException("process is alive");
            }
            return exitCode;
        }

        /// Requests ordinary process termination.
        @Override
        public void destroy() {
            destroyCalls.incrementAndGet();
            if (exitOnDestroy) {
                complete(143);
            }
        }

        /// Returns whether the scripted process remains live.
        @Override
        public boolean isAlive() {
            return alive.get();
        }

        /// Forces process termination.
        @Override
        public Process destroyForcibly() {
            forceDestroyCalls.incrementAndGet();
            complete(137);
            return this;
        }
    }
}
