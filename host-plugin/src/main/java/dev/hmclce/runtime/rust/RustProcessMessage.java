package dev.hmclce.runtime.rust;

import org.jetbrains.annotations.NotNullByDefault;
import org.jetbrains.annotations.Unmodifiable;

import java.util.Arrays;
import java.util.Objects;

/// Represents one frozen schema-v5 message exchanged with an isolated Rust payload process.
@NotNullByDefault
public sealed interface RustProcessMessage permits RustProcessMessage.Hello, RustProcessMessage.Load,
        RustProcessMessage.Enable, RustProcessMessage.Invoke, RustProcessMessage.Disable,
        RustProcessMessage.Shutdown, RustProcessMessage.Ok, RustProcessMessage.Result,
        RustProcessMessage.Error, RustProcessMessage.BridgeInvoke, RustProcessMessage.RetainHandle,
        RustProcessMessage.ReleaseHandle, RustProcessMessage.CallbackResult, RustProcessMessage.CallbackError {
    /// Returns the positive direction-scoped request identifier.
    ///
    /// @return request identifier
    long requestId();

    /// Negotiates the frozen process protocol.
    ///
    /// @param requestId positive odd parent request identifier
    @NotNullByDefault
    record Hello(long requestId) implements RustProcessMessage {
    }

    /// Requests loading one payload library into the child process.
    ///
    /// @param requestId positive odd parent request identifier
    /// @param packageRoot canonical extracted package root
    /// @param entrypoint package-relative payload library path
    /// @param pluginId positive launcher-generated plugin identifier
    /// @param session positive launcher-generated Bridge session identifier
    @NotNullByDefault
    record Load(long requestId, String packageRoot, String entrypoint, long pluginId, long session)
            implements RustProcessMessage {
        /// Rejects null text before the codec performs protocol validation.
        public Load {
            Objects.requireNonNull(packageRoot, "packageRoot");
            Objects.requireNonNull(entrypoint, "entrypoint");
        }
    }

    /// Enables the loaded payload.
    ///
    /// @param requestId positive odd parent request identifier
    @NotNullByDefault
    record Enable(long requestId) implements RustProcessMessage {
    }

    /// Invokes one enabled payload operation.
    ///
    /// @param requestId positive odd parent request identifier
    /// @param operation nonblank payload operation
    /// @param input opaque canonical Bridge Value v1 bytes
    /// @param callbackId nonnegative payload-local callback identifier
    @NotNullByDefault
    record Invoke(long requestId, String operation, byte @Unmodifiable [] input, long callbackId)
            implements RustProcessMessage {
        /// Copies mutable byte input and rejects null values.
        public Invoke {
            Objects.requireNonNull(operation, "operation");
            input = Objects.requireNonNull(input, "input").clone();
        }

        /// Returns a defensive copy of the opaque input.
        ///
        /// @return copied input bytes
        @Override
        public byte @Unmodifiable [] input() {
            return input.clone();
        }

        /// Compares opaque input by content.
        ///
        /// @param other candidate message
        /// @return whether every component is equal
        @Override
        public boolean equals(Object other) {
            return this == other || other instanceof Invoke that
                    && requestId == that.requestId
                    && callbackId == that.callbackId
                    && operation.equals(that.operation)
                    && Arrays.equals(input, that.input);
        }

        /// Hashes opaque input by content.
        ///
        /// @return content hash
        @Override
        public int hashCode() {
            return 31 * Objects.hash(requestId, operation, callbackId) + Arrays.hashCode(input);
        }
    }

    /// Disables the enabled payload.
    ///
    /// @param requestId positive odd parent request identifier
    @NotNullByDefault
    record Disable(long requestId) implements RustProcessMessage {
    }

    /// Shuts down and unloads the payload before closing the child.
    ///
    /// @param requestId positive odd parent request identifier
    @NotNullByDefault
    record Shutdown(long requestId) implements RustProcessMessage {
    }

    /// Confirms one successful lifecycle operation.
    ///
    /// @param requestId positive odd parent request identifier
    @NotNullByDefault
    record Ok(long requestId) implements RustProcessMessage {
    }

    /// Returns one successful payload invocation result.
    ///
    /// @param requestId positive odd parent request identifier
    /// @param output opaque canonical Bridge Value v1 bytes
    @NotNullByDefault
    record Result(long requestId, byte @Unmodifiable [] output) implements RustProcessMessage {
        /// Copies mutable byte output and rejects null values.
        public Result {
            output = Objects.requireNonNull(output, "output").clone();
        }

        /// Returns a defensive copy of the opaque output.
        ///
        /// @return copied output bytes
        @Override
        public byte @Unmodifiable [] output() {
            return output.clone();
        }

        /// Compares opaque output by content.
        ///
        /// @param other candidate message
        /// @return whether every component is equal
        @Override
        public boolean equals(Object other) {
            return this == other || other instanceof Result that
                    && requestId == that.requestId
                    && Arrays.equals(output, that.output);
        }

        /// Hashes opaque output by content.
        ///
        /// @return content hash
        @Override
        public int hashCode() {
            return 31 * Long.hashCode(requestId) + Arrays.hashCode(output);
        }
    }

    /// Reports one stable child-side operation failure.
    ///
    /// @param requestId positive odd parent request identifier
    /// @param code stable lower-case kebab error code
    /// @param message bounded diagnostic text
    @NotNullByDefault
    record Error(long requestId, String code, String message) implements RustProcessMessage {
        /// Rejects null error fields before protocol validation.
        public Error {
            Objects.requireNonNull(code, "code");
            Objects.requireNonNull(message, "message");
        }
    }

    /// Requests one launcher-owned Runtime Bridge invocation from the child.
    ///
    /// @param requestId positive even child callback identifier
    /// @param operation nonblank Runtime Bridge operation
    /// @param input opaque canonical Bridge Value v1 bytes
    @NotNullByDefault
    record BridgeInvoke(long requestId, String operation, byte @Unmodifiable [] input)
            implements RustProcessMessage {
        /// Copies mutable byte input and rejects null values.
        public BridgeInvoke {
            Objects.requireNonNull(operation, "operation");
            input = Objects.requireNonNull(input, "input").clone();
        }

        /// Returns a defensive copy of the opaque input.
        ///
        /// @return copied input bytes
        @Override
        public byte @Unmodifiable [] input() {
            return input.clone();
        }

        /// Compares opaque input by content.
        ///
        /// @param other candidate message
        /// @return whether every component is equal
        @Override
        public boolean equals(Object other) {
            return this == other || other instanceof BridgeInvoke that
                    && requestId == that.requestId
                    && operation.equals(that.operation)
                    && Arrays.equals(input, that.input);
        }

        /// Hashes opaque input by content.
        ///
        /// @return content hash
        @Override
        public int hashCode() {
            return 31 * Objects.hash(requestId, operation) + Arrays.hashCode(input);
        }
    }

    /// Requests retaining one generation-safe launcher handle.
    ///
    /// @param requestId positive even child callback identifier
    /// @param objectId positive launcher object identifier
    /// @param generation positive handle generation
    @NotNullByDefault
    record RetainHandle(long requestId, long objectId, long generation) implements RustProcessMessage {
    }

    /// Requests releasing one generation-safe launcher handle.
    ///
    /// @param requestId positive even child callback identifier
    /// @param objectId positive launcher object identifier
    /// @param generation positive handle generation
    @NotNullByDefault
    record ReleaseHandle(long requestId, long objectId, long generation) implements RustProcessMessage {
    }

    /// Returns one successful parent-side Bridge callback result.
    ///
    /// @param requestId positive even child callback identifier
    /// @param output opaque canonical Bridge Value v1 bytes
    @NotNullByDefault
    record CallbackResult(long requestId, byte @Unmodifiable [] output) implements RustProcessMessage {
        /// Copies mutable byte output and rejects null values.
        public CallbackResult {
            output = Objects.requireNonNull(output, "output").clone();
        }

        /// Returns a defensive copy of the opaque output.
        ///
        /// @return copied output bytes
        @Override
        public byte @Unmodifiable [] output() {
            return output.clone();
        }

        /// Compares opaque output by content.
        ///
        /// @param other candidate message
        /// @return whether every component is equal
        @Override
        public boolean equals(Object other) {
            return this == other || other instanceof CallbackResult that
                    && requestId == that.requestId
                    && Arrays.equals(output, that.output);
        }

        /// Hashes opaque output by content.
        ///
        /// @return content hash
        @Override
        public int hashCode() {
            return 31 * Long.hashCode(requestId) + Arrays.hashCode(output);
        }
    }

    /// Reports one redacted parent-side Bridge callback failure.
    ///
    /// @param requestId positive even child callback identifier
    /// @param code stable lower-case kebab error code
    @NotNullByDefault
    record CallbackError(long requestId, String code) implements RustProcessMessage {
        /// Rejects a null error code before protocol validation.
        public CallbackError {
            Objects.requireNonNull(code, "code");
        }
    }
}
