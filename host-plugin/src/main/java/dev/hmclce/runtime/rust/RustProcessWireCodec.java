package dev.hmclce.runtime.rust;

import org.jackhuang.hmcl.plugin.bridge.BridgeValue;
import org.jackhuang.hmcl.plugin.bridge.RuntimeBridgeWireCodec;
import org.jetbrains.annotations.NotNullByDefault;
import org.jetbrains.annotations.Nullable;

import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.nio.charset.StandardCharsets;
import java.util.LinkedHashMap;
import java.util.Map;

/// Encodes and decodes strict length-prefixed messages for one isolated Rust payload process.
@NotNullByDefault
public final class RustProcessWireCodec {
    /// Frozen isolated process protocol generation.
    static final long PROTOCOL_VERSION = 1L;

    /// Maximum accepted Bridge Value frame body.
    static final int MAX_FRAME_BYTES = 16 * 1024 * 1024;

    /// Prevents construction of this stateless codec.
    private RustProcessWireCodec() {
    }

    /// Reads one complete message, returning null only for clean EOF before a new header.
    ///
    /// @param input untrusted process stream
    /// @return decoded message or null for clean EOF
    /// @throws IOException if the frame or message is malformed or input fails
    public static @Nullable RustProcessMessage read(InputStream input) throws IOException {
        int first = input.read();
        if (first < 0) {
            return null;
        }
        byte[] remainder = readExact(input, Integer.BYTES - 1, "truncated frame header");
        int length = (first << 24)
                | (Byte.toUnsignedInt(remainder[0]) << 16)
                | (Byte.toUnsignedInt(remainder[1]) << 8)
                | Byte.toUnsignedInt(remainder[2]);
        if (length <= 0 || length > MAX_FRAME_BYTES) {
            throw invalid("frame length is outside bounds");
        }
        byte[] body = readExact(input, length, "truncated frame body");
        BridgeValue decoded = RuntimeBridgeWireCodec.decode(body);
        return decodeMessage(decoded);
    }

    /// Writes one canonical message with a four-byte unsigned big-endian frame length.
    ///
    /// @param output process stream
    /// @param message validated message
    /// @throws IOException if the message is invalid, too large, or output fails
    public static void write(OutputStream output, RustProcessMessage message) throws IOException {
        BridgeValue envelope;
        try {
            envelope = encodeMessage(message);
        } catch (IllegalArgumentException exception) {
            throw invalid("message exceeds Bridge Value bounds", exception);
        }
        byte[] body = RuntimeBridgeWireCodec.encode(envelope);
        if (body.length == 0 || body.length > MAX_FRAME_BYTES) {
            throw invalid("frame length is outside bounds");
        }
        output.write((body.length >>> 24) & 0xff);
        output.write((body.length >>> 16) & 0xff);
        output.write((body.length >>> 8) & 0xff);
        output.write(body.length & 0xff);
        output.write(body);
    }

    /// Decodes and validates one strict protocol envelope.
    ///
    /// @param value decoded Bridge root
    /// @return exact message model
    /// @throws IOException if any envelope or payload field is invalid
    private static RustProcessMessage decodeMessage(BridgeValue value) throws IOException {
        Map<String, BridgeValue> envelope = exactMap(
                value, "protocolVersion", "requestId", "kind", "payload");
        long version = integer(field(envelope, "protocolVersion"));
        if (version != PROTOCOL_VERSION) {
            throw invalid("unsupported protocol version");
        }
        long requestId = positive(field(envelope, "requestId"), "request ID must be positive");
        String kind = string(field(envelope, "kind"));
        Map<String, BridgeValue> payload;
        RustProcessMessage message;
        switch (kind) {
            case "hello":
                payload = exactMap(field(envelope, "payload"));
                message = new RustProcessMessage.Hello(requestId);
                break;
            case "load":
                payload = exactMap(field(envelope, "payload"),
                        "packageRoot", "entrypoint", "pluginId", "session");
                message = new RustProcessMessage.Load(
                        requestId,
                        nonblank(field(payload, "packageRoot"), "package root must not be blank"),
                        nonblank(field(payload, "entrypoint"), "entrypoint must not be blank"),
                        positive(field(payload, "pluginId"), "plugin ID must be positive"),
                        positive(field(payload, "session"), "session ID must be positive")
                );
                break;
            case "enable":
                payload = exactMap(field(envelope, "payload"));
                message = new RustProcessMessage.Enable(requestId);
                break;
            case "invoke":
                payload = exactMap(field(envelope, "payload"), "operation", "input", "callbackId");
                message = new RustProcessMessage.Invoke(
                        requestId,
                        nonblank(field(payload, "operation"), "operation must not be blank"),
                        bytes(field(payload, "input")),
                        nonnegative(field(payload, "callbackId"), "callback ID must be nonnegative")
                );
                break;
            case "disable":
                payload = exactMap(field(envelope, "payload"));
                message = new RustProcessMessage.Disable(requestId);
                break;
            case "shutdown":
                payload = exactMap(field(envelope, "payload"));
                message = new RustProcessMessage.Shutdown(requestId);
                break;
            case "ok":
                payload = exactMap(field(envelope, "payload"));
                message = new RustProcessMessage.Ok(requestId);
                break;
            case "result":
                payload = exactMap(field(envelope, "payload"), "output");
                message = new RustProcessMessage.Result(requestId, bytes(field(payload, "output")));
                break;
            case "error":
                payload = exactMap(field(envelope, "payload"), "code", "message");
                message = new RustProcessMessage.Error(
                        requestId,
                        code(field(payload, "code")),
                        errorMessage(field(payload, "message"))
                );
                break;
            case "bridge-invoke":
                payload = exactMap(field(envelope, "payload"), "operation", "input");
                message = new RustProcessMessage.BridgeInvoke(
                        requestId,
                        nonblank(field(payload, "operation"), "Bridge operation must not be blank"),
                        bytes(field(payload, "input"))
                );
                break;
            case "retain-handle":
                payload = exactMap(field(envelope, "payload"), "objectId", "generation");
                message = new RustProcessMessage.RetainHandle(
                        requestId,
                        positive(field(payload, "objectId"), "object ID must be positive"),
                        positive(field(payload, "generation"), "handle generation must be positive")
                );
                break;
            case "release-handle":
                payload = exactMap(field(envelope, "payload"), "objectId", "generation");
                message = new RustProcessMessage.ReleaseHandle(
                        requestId,
                        positive(field(payload, "objectId"), "object ID must be positive"),
                        positive(field(payload, "generation"), "handle generation must be positive")
                );
                break;
            case "callback-result":
                payload = exactMap(field(envelope, "payload"), "output");
                message = new RustProcessMessage.CallbackResult(requestId, bytes(field(payload, "output")));
                break;
            case "callback-error":
                payload = exactMap(field(envelope, "payload"), "code");
                message = new RustProcessMessage.CallbackError(requestId, code(field(payload, "code")));
                break;
            default:
                throw invalid("unsupported message kind");
        }
        validateDirection(message);
        return message;
    }

    /// Encodes one validated message into its canonical ordered envelope.
    ///
    /// @param message source model
    /// @return canonical Bridge map
    /// @throws IOException if scalar constraints or request direction are invalid
    private static BridgeValue encodeMessage(RustProcessMessage message) throws IOException {
        validateDirection(message);
        EncodedBody encoded;
        if (message instanceof RustProcessMessage.Hello) {
            encoded = new EncodedBody("hello", map());
        } else if (message instanceof RustProcessMessage.Load load) {
            encoded = new EncodedBody("load", map(
                    "packageRoot", BridgeValue.string(requireNonblank(load.packageRoot(), "package root")),
                    "entrypoint", BridgeValue.string(requireNonblank(load.entrypoint(), "entrypoint")),
                    "pluginId", BridgeValue.integer(requirePositive(load.pluginId(), "plugin ID")),
                    "session", BridgeValue.integer(requirePositive(load.session(), "session ID"))
            ));
        } else if (message instanceof RustProcessMessage.Enable) {
            encoded = new EncodedBody("enable", map());
        } else if (message instanceof RustProcessMessage.Invoke invoke) {
            encoded = new EncodedBody("invoke", map(
                    "operation", BridgeValue.string(requireNonblank(invoke.operation(), "operation")),
                    "input", BridgeValue.bytes(invoke.input()),
                    "callbackId", BridgeValue.integer(requireNonnegative(invoke.callbackId(), "callback ID"))
            ));
        } else if (message instanceof RustProcessMessage.Disable) {
            encoded = new EncodedBody("disable", map());
        } else if (message instanceof RustProcessMessage.Shutdown) {
            encoded = new EncodedBody("shutdown", map());
        } else if (message instanceof RustProcessMessage.Ok) {
            encoded = new EncodedBody("ok", map());
        } else if (message instanceof RustProcessMessage.Result result) {
            encoded = new EncodedBody("result", map("output", BridgeValue.bytes(result.output())));
        } else if (message instanceof RustProcessMessage.Error error) {
            encoded = new EncodedBody("error", map(
                    "code", BridgeValue.string(requireCode(error.code())),
                    "message", BridgeValue.string(requireErrorMessage(error.message()))
            ));
        } else if (message instanceof RustProcessMessage.BridgeInvoke invoke) {
            encoded = new EncodedBody("bridge-invoke", map(
                    "operation", BridgeValue.string(requireNonblank(invoke.operation(), "Bridge operation")),
                    "input", BridgeValue.bytes(invoke.input())
            ));
        } else if (message instanceof RustProcessMessage.RetainHandle retain) {
            encoded = new EncodedBody("retain-handle", handlePayload(retain.objectId(), retain.generation()));
        } else if (message instanceof RustProcessMessage.ReleaseHandle release) {
            encoded = new EncodedBody("release-handle", handlePayload(release.objectId(), release.generation()));
        } else if (message instanceof RustProcessMessage.CallbackResult result) {
            encoded = new EncodedBody("callback-result", map("output", BridgeValue.bytes(result.output())));
        } else if (message instanceof RustProcessMessage.CallbackError error) {
            encoded = new EncodedBody("callback-error", map("code", BridgeValue.string(requireCode(error.code()))));
        } else {
            throw invalid("unsupported message model");
        }
        return BridgeValue.map(map(
                "protocolVersion", BridgeValue.integer(PROTOCOL_VERSION),
                "requestId", BridgeValue.integer(message.requestId()),
                "kind", BridgeValue.string(encoded.kind()),
                "payload", BridgeValue.map(encoded.payload())
        ));
    }

    /// Validates the request identifier and its protocol direction.
    ///
    /// @param message candidate message
    /// @throws IOException if the ID is zero, negative, or belongs to the opposite direction
    private static void validateDirection(RustProcessMessage message) throws IOException {
        long requestId = message.requestId();
        if (requestId <= 0L) {
            throw invalid("request ID must be positive");
        }
        boolean evenKind = message instanceof RustProcessMessage.BridgeInvoke
                || message instanceof RustProcessMessage.RetainHandle
                || message instanceof RustProcessMessage.ReleaseHandle
                || message instanceof RustProcessMessage.CallbackResult
                || message instanceof RustProcessMessage.CallbackError;
        if ((requestId & 1L) == 0L != evenKind) {
            throw invalid("request ID belongs to the other protocol direction");
        }
    }

    /// Builds the exact shared handle payload.
    ///
    /// @param objectId positive object identifier
    /// @param generation positive handle generation
    /// @return ordered payload map
    /// @throws IOException if either identifier is invalid
    private static Map<String, BridgeValue> handlePayload(long objectId, long generation) throws IOException {
        return map(
                "objectId", BridgeValue.integer(requirePositive(objectId, "object ID")),
                "generation", BridgeValue.integer(requirePositive(generation, "handle generation"))
        );
    }

    /// Requires one Bridge map with exactly the named keys.
    ///
    /// @param value candidate Bridge value
    /// @param fields required field names
    /// @return immutable decoded map
    /// @throws IOException if type or field membership differs
    private static Map<String, BridgeValue> exactMap(BridgeValue value, String... fields) throws IOException {
        if (!(value instanceof BridgeValue.MapValue map)) {
            throw invalid("message value must be a map");
        }
        Map<String, BridgeValue> values = map.values();
        if (values.size() != fields.length) {
            throw invalid("message map has unknown or missing fields");
        }
        for (String field : fields) {
            if (!values.containsKey(field)) {
                throw invalid("message map has unknown or missing fields");
            }
        }
        return values;
    }

    /// Returns one required field from an already exact map.
    ///
    /// @param fields exact map
    /// @param name field name
    /// @return field value
    /// @throws IOException if the field is unexpectedly absent
    private static BridgeValue field(Map<String, BridgeValue> fields, String name) throws IOException {
        BridgeValue value = fields.get(name);
        if (value == null) {
            throw invalid("message map is missing a required field");
        }
        return value;
    }

    /// Requires one signed 64-bit integer.
    ///
    /// @param value candidate field
    /// @return integer value
    /// @throws IOException if the field has another type
    private static long integer(BridgeValue value) throws IOException {
        if (value instanceof BridgeValue.IntegerValue integer) {
            return integer.value();
        }
        throw invalid("message field must be an integer");
    }

    /// Requires one positive integer field.
    ///
    /// @param value candidate field
    /// @param failure stable validation detail
    /// @return positive value
    /// @throws IOException if the value is not a positive integer
    private static long positive(BridgeValue value, String failure) throws IOException {
        long identifier = integer(value);
        if (identifier <= 0L) {
            throw invalid(failure);
        }
        return identifier;
    }

    /// Requires one nonnegative integer field.
    ///
    /// @param value candidate field
    /// @param failure stable validation detail
    /// @return nonnegative value
    /// @throws IOException if the value is negative or not an integer
    private static long nonnegative(BridgeValue value, String failure) throws IOException {
        long identifier = integer(value);
        if (identifier < 0L) {
            throw invalid(failure);
        }
        return identifier;
    }

    /// Requires one string field.
    ///
    /// @param value candidate field
    /// @return string value
    /// @throws IOException if the field has another type
    private static String string(BridgeValue value) throws IOException {
        if (value instanceof BridgeValue.StringValue string) {
            return string.value();
        }
        throw invalid("message field must be a string");
    }

    /// Requires one nonblank string field.
    ///
    /// @param value candidate field
    /// @param failure stable validation detail
    /// @return nonblank text
    /// @throws IOException if the field is not a string or is blank
    private static String nonblank(BridgeValue value, String failure) throws IOException {
        String text = string(value);
        if (text.trim().isEmpty()) {
            throw invalid(failure);
        }
        return text;
    }

    /// Requires one byte-array field.
    ///
    /// @param value candidate field
    /// @return copied bytes
    /// @throws IOException if the field has another type
    private static byte[] bytes(BridgeValue value) throws IOException {
        if (value instanceof BridgeValue.BytesValue bytes) {
            return bytes.value();
        }
        throw invalid("message field must be bytes");
    }

    /// Requires one stable lower-case kebab error code field.
    ///
    /// @param value candidate field
    /// @return validated code
    /// @throws IOException if the field is malformed
    private static String code(BridgeValue value) throws IOException {
        return requireCode(string(value));
    }

    /// Requires one bounded nonblank error message field.
    ///
    /// @param value candidate field
    /// @return validated diagnostic message
    /// @throws IOException if the field is malformed
    private static String errorMessage(BridgeValue value) throws IOException {
        return requireErrorMessage(string(value));
    }

    /// Requires one positive model identifier.
    ///
    /// @param value candidate value
    /// @param field field label
    /// @return positive value
    /// @throws IOException if the value is not positive
    private static long requirePositive(long value, String field) throws IOException {
        if (value <= 0L) {
            throw invalid(field + " must be positive");
        }
        return value;
    }

    /// Requires one nonnegative model identifier.
    ///
    /// @param value candidate value
    /// @param field field label
    /// @return nonnegative value
    /// @throws IOException if the value is negative
    private static long requireNonnegative(long value, String field) throws IOException {
        if (value < 0L) {
            throw invalid(field + " must be nonnegative");
        }
        return value;
    }

    /// Requires one nonblank model string.
    ///
    /// @param value candidate text
    /// @param field field label
    /// @return nonblank text
    /// @throws IOException if the value is blank
    private static String requireNonblank(String value, String field) throws IOException {
        if (value.trim().isEmpty()) {
            throw invalid(field + " must not be blank");
        }
        return value;
    }

    /// Requires one stable lower-case kebab error code.
    ///
    /// @param value candidate code
    /// @return validated code
    /// @throws IOException if the code violates the frozen grammar
    private static String requireCode(String value) throws IOException {
        if (value.length() > 128 || !value.matches("[a-z][a-z0-9]*(?:-[a-z0-9]+)*")) {
            throw invalid("error code must be lower-case kebab text");
        }
        return value;
    }

    /// Requires one nonblank diagnostic of at most 4096 UTF-8 bytes.
    ///
    /// @param value candidate message
    /// @return validated message
    /// @throws IOException if the message is blank or too large
    private static String requireErrorMessage(String value) throws IOException {
        if (value.trim().isEmpty()) {
            throw invalid("error message must not be blank");
        }
        if (value.getBytes(StandardCharsets.UTF_8).length > 4096) {
            throw invalid("error message exceeds 4096 UTF-8 bytes");
        }
        return value;
    }

    /// Builds one insertion-ordered map from alternating string keys and Bridge values.
    ///
    /// @param entries alternating key and value entries
    /// @return ordered mutable map consumed immediately by `BridgeValue.map`
    private static Map<String, BridgeValue> map(Object... entries) {
        if ((entries.length & 1) != 0) {
            throw new IllegalArgumentException("Protocol map entries must contain key-value pairs");
        }
        Map<String, BridgeValue> result = new LinkedHashMap<>(entries.length / 2);
        for (int index = 0; index < entries.length; index += 2) {
            result.put((String) entries[index], (BridgeValue) entries[index + 1]);
        }
        return result;
    }

    /// Reads exactly one bounded byte sequence.
    ///
    /// @param input source stream
    /// @param length required length
    /// @param failure truncation detail
    /// @return exact bytes
    /// @throws IOException if EOF arrives early or input fails
    private static byte[] readExact(InputStream input, int length, String failure) throws IOException {
        byte[] result = input.readNBytes(length);
        if (result.length != length) {
            throw invalid(failure);
        }
        return result;
    }

    /// Creates one protocol validation failure.
    ///
    /// @param detail stable failure detail
    /// @return checked protocol failure
    private static IOException invalid(String detail) {
        return new IOException("Invalid Rust isolated Host protocol: " + detail);
    }

    /// Creates one protocol validation failure with a local cause.
    ///
    /// @param detail stable failure detail
    /// @param cause local validation cause
    /// @return checked protocol failure
    private static IOException invalid(String detail, Throwable cause) {
        return new IOException("Invalid Rust isolated Host protocol: " + detail, cause);
    }

    /// Carries one validated kind and ordered payload during encoding.
    ///
    /// @param kind canonical kind string
    /// @param payload exact ordered payload map
    @NotNullByDefault
    private record EncodedBody(String kind, Map<String, BridgeValue> payload) {
    }
}
