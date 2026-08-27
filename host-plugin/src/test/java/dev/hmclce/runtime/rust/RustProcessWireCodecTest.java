package dev.hmclce.runtime.rust;

import org.jackhuang.hmcl.plugin.bridge.BridgeValue;
import org.jackhuang.hmcl.plugin.bridge.RuntimeBridgeWireCodec;
import org.jetbrains.annotations.NotNullByDefault;
import org.jetbrains.annotations.Unmodifiable;
import org.junit.jupiter.api.Test;

import java.io.ByteArrayInputStream;
import java.io.ByteArrayOutputStream;
import java.io.DataOutputStream;
import java.io.IOException;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertNull;
import static org.junit.jupiter.api.Assertions.assertThrows;

/// Verifies exact wire compatibility with the frozen Rust isolated Host protocol.
@NotNullByDefault
final class RustProcessWireCodecTest {
    /// Rust Task 1's canonical hello frame for request ID 7.
    private static final byte @Unmodifiable [] RUST_HELLO_FRAME = new byte[]{
            0, 0, 0, 107, (byte) 146, 7, (byte) 221, 0, 0, 0, 4, (byte) 146, (byte) 219, 0, 0,
            0, 15, 112, 114, 111, 116, 111, 99, 111, 108, 86, 101, 114, 115, 105, 111, 110,
            (byte) 146, 2, (byte) 211, 0, 0, 0, 0, 0, 0, 0, 1, (byte) 146, (byte) 219, 0, 0, 0,
            9, 114, 101, 113, 117, 101, 115, 116, 73, 100, (byte) 146, 2, (byte) 211, 0, 0, 0,
            0, 0, 0, 0, 7, (byte) 146, (byte) 219, 0, 0, 0, 4, 107, 105, 110, 100, (byte) 146,
            4, (byte) 219, 0, 0, 0, 5, 104, 101, 108, 108, 111, (byte) 146, (byte) 219, 0, 0,
            0, 7, 112, 97, 121, 108, 111, 97, 100, (byte) 146, 7, (byte) 221, 0, 0, 0, 0
    };

    /// Rust Task 1's canonical invoke frame for a launch Hook call.
    private static final byte @Unmodifiable [] RUST_INVOKE_FRAME = new byte[]{
            0, 0, 0, (byte) 201, (byte) 146, 7, (byte) 221, 0, 0, 0, 4, (byte) 146, (byte) 219,
            0, 0, 0, 15, 112, 114, 111, 116, 111, 99, 111, 108, 86, 101, 114, 115, 105, 111,
            110, (byte) 146, 2, (byte) 211, 0, 0, 0, 0, 0, 0, 0, 1, (byte) 146, (byte) 219,
            0, 0, 0, 9, 114, 101, 113, 117, 101, 115, 116, 73, 100, (byte) 146, 2, (byte) 211,
            0, 0, 0, 0, 0, 0, 0, 7, (byte) 146, (byte) 219, 0, 0, 0, 4, 107, 105, 110, 100,
            (byte) 146, 4, (byte) 219, 0, 0, 0, 6, 105, 110, 118, 111, 107, 101, (byte) 146,
            (byte) 219, 0, 0, 0, 7, 112, 97, 121, 108, 111, 97, 100, (byte) 146, 7, (byte) 221,
            0, 0, 0, 3, (byte) 146, (byte) 219, 0, 0, 0, 9, 111, 112, 101, 114, 97, 116, 105,
            111, 110, (byte) 146, 4, (byte) 219, 0, 0, 0, 23, 104, 111, 111, 107, 46, 98, 101,
            102, 111, 114, 101, 45, 103, 97, 109, 101, 45, 108, 97, 117, 110, 99, 104,
            (byte) 146, (byte) 219, 0, 0, 0, 5, 105, 110, 112, 117, 116, (byte) 146, 5,
            (byte) 198, 0, 0, 0, 3, 1, 2, 3, (byte) 146, (byte) 219, 0, 0, 0, 10, 99, 97,
            108, 108, 98, 97, 99, 107, 73, 100, (byte) 146, 2, (byte) 211, 0, 0, 0, 0, 0, 0,
            0, 0
    };

    /// Java must decode and reproduce the exact Rust hello frame.
    @Test
    void matchesRustHelloGoldenFrame() throws IOException {
        RustProcessMessage.Hello hello = new RustProcessMessage.Hello(7L);

        assertEquals(hello, RustProcessWireCodec.read(new ByteArrayInputStream(RUST_HELLO_FRAME)));
        assertArrayEquals(RUST_HELLO_FRAME, write(hello));
    }

    /// Java must decode and reproduce the exact Rust invoke frame.
    @Test
    void matchesRustInvokeGoldenFrame() throws IOException {
        RustProcessMessage.Invoke invoke = new RustProcessMessage.Invoke(
                7L, "hook.before-game-launch", new byte[]{1, 2, 3}, 0L);

        assertEquals(invoke, RustProcessWireCodec.read(new ByteArrayInputStream(RUST_INVOKE_FRAME)));
        assertArrayEquals(RUST_INVOKE_FRAME, write(invoke));
    }

    /// Every frozen message kind must retain its exact model through the Java codec.
    @Test
    void roundTripsEveryFrozenMessageKind() throws IOException {
        List<RustProcessMessage> messages = List.of(
                new RustProcessMessage.Hello(1L),
                new RustProcessMessage.Load(3L, "C:/plugins/example", "payload/plugin.dll", 11L, 13L),
                new RustProcessMessage.Enable(5L),
                new RustProcessMessage.Invoke(7L, "hook.before-game-launch", new byte[]{1, 2, 3}, 0L),
                new RustProcessMessage.Disable(9L),
                new RustProcessMessage.Shutdown(11L),
                new RustProcessMessage.Ok(13L),
                new RustProcessMessage.Result(15L, new byte[]{4, 5}),
                new RustProcessMessage.Error(17L, "plugin-status", "Plugin callback failed"),
                new RustProcessMessage.BridgeInvoke(2L, "core.launcher-version", new byte[]{6}),
                new RustProcessMessage.RetainHandle(4L, 19L, 23L),
                new RustProcessMessage.ReleaseHandle(6L, 29L, 31L),
                new RustProcessMessage.CallbackResult(8L, new byte[]{7, 8}),
                new RustProcessMessage.CallbackError(10L, "permission-denied")
        );

        for (RustProcessMessage message : messages) {
            assertEquals(message, RustProcessWireCodec.read(new ByteArrayInputStream(write(message))));
        }
    }

    /// Clean EOF before a frame returns null while partial or invalid frame headers fail.
    @Test
    void distinguishesCleanEofFromMalformedFrameHeaders() {
        assertNull(readUnchecked(new byte[]{}));
        assertThrows(IOException.class, () -> RustProcessWireCodec.read(new ByteArrayInputStream(new byte[]{0})));
        assertThrows(IOException.class,
                () -> RustProcessWireCodec.read(new ByteArrayInputStream(new byte[]{0, 0, 0, 0})));
        assertThrows(IOException.class,
                () -> RustProcessWireCodec.read(new ByteArrayInputStream(new byte[]{1, 0, 0, 1})));
    }

    /// Envelope decoding must reject unknown fields, zero request IDs, and other protocol versions.
    @Test
    void rejectsInvalidStrictEnvelopes() throws IOException {
        Map<String, BridgeValue> unknown = envelope(1L, 1L, "hello", Map.of());
        unknown.put("unexpected", BridgeValue.bool(true));

        assertThrows(IOException.class,
                () -> RustProcessWireCodec.read(new ByteArrayInputStream(frame(unknown))));
        assertThrows(IOException.class,
                () -> RustProcessWireCodec.read(new ByteArrayInputStream(
                        frame(envelope(1L, 0L, "hello", Map.of())))));
        assertThrows(IOException.class,
                () -> RustProcessWireCodec.read(new ByteArrayInputStream(
                        frame(envelope(2L, 1L, "hello", Map.of())))));
    }

    /// Serializes one framed message through the production Java writer.
    ///
    /// @param message source message
    /// @return complete length-prefixed frame
    private static byte[] write(RustProcessMessage message) throws IOException {
        ByteArrayOutputStream output = new ByteArrayOutputStream();
        RustProcessWireCodec.write(output, message);
        return output.toByteArray();
    }

    /// Reads one frame and converts checked I/O into a test failure.
    ///
    /// @param frame source bytes
    /// @return decoded message or null for clean EOF
    private static RustProcessMessage readUnchecked(byte[] frame) {
        try {
            return RustProcessWireCodec.read(new ByteArrayInputStream(frame));
        } catch (IOException exception) {
            throw new AssertionError(exception);
        }
    }

    /// Creates one strict envelope with insertion order matching the Rust protocol.
    ///
    /// @param version protocol generation
    /// @param requestId direction-scoped request ID
    /// @param kind canonical kind
    /// @param payload kind-specific payload
    /// @return mutable ordered envelope for malformed-field fixtures
    private static Map<String, BridgeValue> envelope(
            long version,
            long requestId,
            String kind,
            Map<String, BridgeValue> payload
    ) {
        Map<String, BridgeValue> envelope = new LinkedHashMap<>();
        envelope.put("protocolVersion", BridgeValue.integer(version));
        envelope.put("requestId", BridgeValue.integer(requestId));
        envelope.put("kind", BridgeValue.string(kind));
        envelope.put("payload", BridgeValue.map(payload));
        return envelope;
    }

    /// Encodes one Bridge map and adds the protocol frame header.
    ///
    /// @param envelope source envelope
    /// @return complete frame
    private static byte[] frame(Map<String, BridgeValue> envelope) throws IOException {
        byte[] body = RuntimeBridgeWireCodec.encode(BridgeValue.map(envelope));
        ByteArrayOutputStream framed = new ByteArrayOutputStream(Integer.BYTES + body.length);
        try (DataOutputStream output = new DataOutputStream(framed)) {
            output.writeInt(body.length);
            output.write(body);
        }
        return framed.toByteArray();
    }
}
