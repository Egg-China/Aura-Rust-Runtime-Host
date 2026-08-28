# Rust Isolated Runtime Design

## Status

Approved continuation of the schema-v5 Rust Runtime Host plan. This design adds an optional isolated execution mode without changing the existing embedded ABI or putting a Rust runtime in Aura Launcher itself.

## Goals

- Let a schema-v5 Rust payload select either `embedded` or `isolated` execution.
- Keep the Rust Runtime Host a separately installed runtime-provider plugin.
- Run each isolated payload in its own supervised process so one native plugin crash cannot terminate Aura Launcher or another payload.
- Preserve the existing Rust plugin ABI and canonical Bridge Value v1 encoding.
- Preserve launcher-owned permission checks. Java capability tokens and Java objects never cross the process boundary.
- Carry generic Bridge calls and schema-v5 Hook dispatch in both modes.
- Fail closed on malformed frames, protocol mismatches, timeouts, and unexpected process exits.

## Non-Goals

- Isolated payloads do not receive `raw-jvm`; schema v5 already rejects that combination.
- This phase does not implement Patch transport or direct JVM object access.
- This phase does not add a permanent process shared by unrelated plugins.
- This phase does not change the stable `schema-v4` branch.

## Architecture

The optional Host package contains two platform artifacts:

1. `hmcl_rust_host_native`, the existing JNI library for embedded payloads.
2. `hmcl-rust-host-process`, a new executable used only for isolated payloads.

The Java Host creates a hybrid Provider engine. It initializes the embedded JNI engine once and resolves the isolated executable from the same verified Host package. `RuntimePayloadContext.executionMode()` selects the backend during payload load.

Embedded payloads keep their current provider-wide native engine. Each isolated payload owns exactly one child process, one protocol stream, and one launcher Bridge context. Provider handles remain opaque and map to the selected backend inside the Java Host.

The child process loads the same Rust `cdylib` entrypoint through the current `hmcl-runtime-abi` tables. The payload therefore does not need separate embedded and isolated builds.

## Components

### Java Hybrid Engine

`RustRuntimeEngine` implements `RustRuntimeProvider.Engine` and owns:

- the existing `RustNativeEngine` for embedded payloads;
- the canonical isolated executable path;
- an insertion-ordered map of Provider payload IDs to backend payloads;
- reverse-order close and cleanup.

Its public payload IDs are Host-generated and never expose a native pointer, operating-system process ID, or Java capability token.

### Java Isolated Process

`RustIsolatedPayload` owns one `Process`, buffered stdin/stdout, a bounded stderr collector, protocol request IDs, lifecycle state, and the exact `RuntimePayloadContext` used for Bridge callbacks.

Calls are serialized. A caller writes one command frame and reads frames until the matching response arrives. While waiting, it services child-originated Bridge callback frames synchronously and writes their responses. This preserves the current callback ordering and avoids a second Java callback executor.

A shared daemon scheduler enforces deadlines by closing the protocol streams and terminating the child. A timeout poisons the payload permanently; later ordinary work fails immediately and unload performs cleanup.

### Rust Process Host

The `hmcl-rust-host-process` crate is a small stdio server. It owns one existing `embedded::Engine` and accepts exactly one payload. It has no network listener, inherited console protocol, plugin discovery, or package manager.

The process accepts `--stdio` for normal operation and `--probe` for package health checks. Plugin metadata and paths are sent only inside the framed protocol, not command-line arguments.

### Shared Protocol Model

The Rust protocol implementation lives in a small reusable crate. Java implements the same frozen contract independently. Golden vectors prove both sides emit identical bytes.

## Wire Protocol

Every message is a four-byte unsigned big-endian length followed by one canonical Bridge Value v1 map. The encoded map must not exceed 16 MiB. Zero-length, oversized, truncated, non-map, duplicate-field, unknown-field, wrong-kind, and wrong-version frames are fatal protocol errors.

Every envelope has exactly these fields:

- `protocolVersion`: integer `1`;
- `requestId`: positive signed-64-compatible integer;
- `kind`: canonical lower-case kebab string;
- `payload`: kind-specific map.

Parent command request IDs are positive odd integers. Child callback request IDs are positive even integers. Each side increments its own ID by two and treats exhaustion as a terminal protocol error, so simultaneous directions cannot collide.

Parent request kinds:

- `hello`: empty payload; negotiates protocol version before any plugin path is accepted.
- `load`: package root, entrypoint, and Host-generated numeric plugin/session identifiers.
- `enable`: empty payload.
- `invoke`: operation string, opaque Bridge input bytes, and nonnegative callback ID.
- `disable`: empty payload.
- `shutdown`: empty payload; invokes plugin shutdown and unloads the library.

Child response kinds:

- `ok`: empty payload for successful lifecycle operations.
- `result`: opaque Bridge result bytes for `invoke`.
- `error`: stable error code and bounded diagnostic message.

Child callback kinds:

- `bridge-invoke`: canonical operation and opaque Bridge input bytes.
- `retain-handle`: object ID and generation.
- `release-handle`: object ID and generation.

The parent answers callbacks with `callback-result` or `callback-error` using the same request ID. Bridge input and result bytes remain opaque to the process protocol.

The numeric session value visible to the Rust ABI is a Host-generated routing identifier. It is not the Java `PluginCapabilityToken`. The Java context resolves current capability authority at each Bridge call exactly as embedded mode does.

## Lifecycle And Failure Handling

Default deadlines are five seconds for handshake, ten seconds for lifecycle calls, and thirty seconds for generic invocations. Hook dispatch uses the shorter positive deadline supplied by the launcher dispatcher.

Any of the following poisons and terminates the isolated payload:

- handshake or command timeout;
- unexpected child exit;
- malformed or oversized frame;
- response request-ID mismatch;
- callback kind or field mismatch;
- write or read failure.

Plugin-reported lifecycle and invocation errors remain ordinary `IOException` failures and do not automatically corrupt the protocol. A shutdown failure is reported after the process is terminated so native resources cannot linger.

Stderr is never part of the protocol. Java drains it continuously into a bounded tail used only in diagnostics. Stdout is protocol-only; arbitrary payload output on stdout makes the protocol fail closed.

Provider close terminates isolated payloads in reverse load order, then closes the embedded engine. Cleanup is idempotent. A child crash cannot directly crash Aura Launcher; repeated plugin failures continue through the launcher's existing runtime supervisor and startup protector policy.

## Path And Process Safety

- The Host executable is resolved with the same canonical-root and symlink-escape checks as the JNI library.
- The payload entrypoint remains confined beneath the verified extracted package root.
- The child working directory is the payload package root.
- The child environment is cleared, then repopulated only from `SystemRoot`, `WINDIR`, `PATH`, `PATHEXT`, `TEMP`, `TMP`, `HOME`, `USERPROFILE`, `LANG`, and `LC_*` keys present in the launcher environment. Plugin data access continues through the permission-checked Bridge instead of an inherited authority token.
- No shell is used. Java starts the canonical executable directly with `ProcessBuilder`.

## Packaging

The Host manifest advertises both `embedded` and `isolated` only when both artifacts are present in every published platform package. Platform paths are:

- Windows: `native/<platform>/hmcl_rust_host_native.dll` and `native/<platform>/hmcl-rust-host-process.exe`.
- Linux: `native/<platform>/libhmcl_rust_host_native.so` and `native/<platform>/hmcl-rust-host-process`.
- macOS: `native/<platform>/libhmcl_rust_host_native.dylib` and `native/<platform>/hmcl-rust-host-process`.

The NPL task requires both artifacts and the six-platform release matrix publishes one package per target. The validator must reject a Rust Host package that advertises isolated mode without the process artifact.

## Testing

1. Rust protocol unit tests cover golden frames, every kind, strict fields, size limits, truncation, invalid UTF-8, request IDs, and version rejection.
2. Rust process tests run the existing valid, missing-query, and wrong-ABI fixtures through stdio lifecycle commands.
3. Java unit tests use an injected process boundary to cover mode routing, exact frames, Bridge callbacks, timeouts, process exit, stderr bounds, cleanup, and foreign handles.
4. A real Windows integration test launches the built process Host and exercises load, enable, Bridge invoke, disable, and shutdown against the existing fixture.
5. Cargo format, Clippy with warnings denied, Rust workspace tests, Java Host tests, NPL validation, and Aura Hook tests remain required.
6. CI builds and validates Windows x64/arm64, Linux x64/arm64, and macOS x64/arm64 artifacts before the Host advertises isolated mode in a release package.

## Delivery Order

1. Freeze the process protocol with independent golden tests.
2. Implement and test the Rust stdio process Host.
3. Implement Java isolated transport and hybrid mode routing.
4. Advertise isolated mode and require both package artifacts.
5. Add real integration coverage, example payload documentation, and the six-platform release matrix.
6. Add Patch and `raw-jvm` capabilities as separate later changes; isolated mode remains unable to request `raw-jvm`.
