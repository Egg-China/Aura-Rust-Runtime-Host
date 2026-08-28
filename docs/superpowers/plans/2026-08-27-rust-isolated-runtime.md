# Rust Isolated Runtime Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a process-isolated execution mode to the optional schema-v5 Rust Runtime Host while preserving its existing embedded ABI, Bridge operations, and Hook transport.

**Architecture:** A Java hybrid engine routes embedded payloads to the current JNI engine and gives every isolated payload a dedicated `hmcl-rust-host-process` child. Parent and child exchange length-prefixed canonical Bridge Value v1 envelopes; Bridge callbacks remain launcher-authorized and Java capability tokens never leave the JVM.

**Tech Stack:** Java 17, Gradle 9.6.1, Rust 1.97.1 edition 2024, JNI, canonical Bridge Value v1, PowerShell NPL validator, GitHub Actions.

**Spec:** `docs/superpowers/specs/2026-08-27-rust-isolated-runtime-design.md`

## Global Constraints

- Work only on SDK `schema-v5`; never modify `schema-v4` or old HMCL-CE `main`.
- Keep the Runtime Host optional and separately packaged from Aura Launcher.
- Preserve the existing `hmcl-runtime-abi` ABI and embedded Rust payload compatibility.
- Use one child process per isolated payload and no shell command construction.
- Use Bridge Value v1 for message bodies with a four-byte big-endian length prefix and a 16 MiB frame limit.
- Reject unknown, missing, duplicate, truncated, oversized, wrong-version, and wrong-kind protocol data.
- Never inspect or serialize `PluginCapabilityToken`; resolve launcher authority at each Java Bridge callback.
- Isolated mode must never advertise or accept `raw-jvm`.
- Run Cargo format, Clippy with `-D warnings`, Rust workspace tests, Java Host tests, NPL validation, and `git diff --check` before each delivery commit.

---

## File Map

- `runtime-hosts/rust/crates/hmcl-runtime-protocol/`: strict Rust process protocol and framing only.
- `runtime-hosts/rust/crates/hmcl-rust-host-process/`: stdio executable that owns one current embedded ABI engine.
- `runtime-hosts/rust/host-plugin/src/main/java/dev/hmclce/runtime/rust/RustProcessWireCodec.java`: independent Java protocol encoder/decoder.
- `runtime-hosts/rust/host-plugin/src/main/java/dev/hmclce/runtime/rust/RustIsolatedPayload.java`: one child process, deadlines, callbacks, and cleanup.
- `runtime-hosts/rust/host-plugin/src/main/java/dev/hmclce/runtime/rust/RustRuntimeEngine.java`: mode router over JNI and isolated backends.
- `runtime-hosts/rust/host-plugin/plugin.json` and `build.gradle.kts`: capability and two-artifact packaging.
- `tools/validate-npl.ps1`: reject an isolated Rust Provider package missing its process executable.
- `.github/workflows/rust-runtime-host.yml`: six-platform build and validation matrix.

### Task 1: Freeze The Rust Process Protocol

**Files:**
- Create: `runtime-hosts/rust/crates/hmcl-runtime-protocol/Cargo.toml`
- Create: `runtime-hosts/rust/crates/hmcl-runtime-protocol/src/lib.rs`
- Create: `runtime-hosts/rust/crates/hmcl-runtime-protocol/tests/protocol.rs`
- Modify: `runtime-hosts/rust/Cargo.toml`
- Modify: `runtime-hosts/rust/Cargo.lock`

**Interfaces:**
- Consumes: `hmcl_plugin_sdk::Value::{to_wire, from_wire}`.
- Produces: `PROTOCOL_VERSION`, `MAX_FRAME_BYTES`, `Message`, `MessageBody`, `ProtocolError`, `read_frame`, and `write_frame`.

- [ ] **Step 1: Add failing strict-envelope and framing tests**

Use literal fields rather than the production envelope builder:

```rust
#[test]
fn round_trips_hello_with_a_big_endian_frame_length() {
    let message = Message::new(7, MessageBody::Hello).unwrap();
    let mut bytes = Vec::new();
    write_frame(&mut bytes, &message).unwrap();
    let declared = u32::from_be_bytes(bytes[0..4].try_into().unwrap()) as usize;
    assert_eq!(declared, bytes.len() - 4);
    assert_eq!(read_frame(&mut bytes.as_slice()).unwrap(), Some(message));
}

#[test]
fn rejects_unknown_fields_and_oversized_lengths() {
    let malformed = Value::Map(vec![
        ("protocolVersion".into(), Value::Integer(1)),
        ("requestId".into(), Value::Integer(1)),
        ("kind".into(), Value::String("hello".into())),
        ("payload".into(), Value::Map(vec![])),
        ("unexpected".into(), Value::Bool(true)),
    ]).to_wire().unwrap();
    assert!(Message::from_wire(&malformed).is_err());
    let mut oversized = (MAX_FRAME_BYTES + 1).to_be_bytes().to_vec();
    assert!(read_frame(&mut oversized.as_slice()).is_err());
}
```

- [ ] **Step 2: Run the protocol tests and verify RED**

Run: `cargo test -p hmcl-runtime-protocol --manifest-path runtime-hosts/rust/Cargo.toml`

Expected: FAIL because the crate and protocol types do not exist.

- [ ] **Step 3: Implement the strict protocol model**

Define these exact public shapes:

```rust
pub const PROTOCOL_VERSION: i64 = 1;
pub const MAX_FRAME_BYTES: u32 = 16 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Message {
    request_id: u64,
    body: MessageBody,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MessageBody {
    Hello,
    Load { package_root: String, entrypoint: String, plugin_id: u64, session: u64 },
    Enable,
    Invoke { operation: String, input: Vec<u8>, callback_id: u64 },
    Disable,
    Shutdown,
    Ok,
    Result { output: Vec<u8> },
    Error { code: String, message: String },
    BridgeInvoke { operation: String, input: Vec<u8> },
    RetainHandle { object_id: u64, generation: u64 },
    ReleaseHandle { object_id: u64, generation: u64 },
    CallbackResult { output: Vec<u8> },
    CallbackError { code: String },
}

pub fn read_frame(reader: &mut impl Read) -> Result<Option<Message>, ProtocolError>;
pub fn write_frame(writer: &mut impl Write, message: &Message) -> Result<(), ProtocolError>;
```

Require exact envelope and payload field sets, positive request IDs, signed-64-compatible numeric IDs, canonical kind strings, valid UTF-8 through `Value`, and complete frame consumption. EOF before a new header returns `Ok(None)`; EOF after any header byte is `ProtocolError::Truncated`.

- [ ] **Step 4: Run protocol tests, format, and Clippy**

Run:

```powershell
cargo fmt --manifest-path runtime-hosts/rust/Cargo.toml --check
cargo clippy --manifest-path runtime-hosts/rust/Cargo.toml -p hmcl-runtime-protocol --all-targets -- -D warnings
cargo test --manifest-path runtime-hosts/rust/Cargo.toml -p hmcl-runtime-protocol
```

Expected: all commands exit 0.

- [ ] **Step 5: Commit the frozen protocol**

```powershell
git add runtime-hosts/rust/Cargo.toml runtime-hosts/rust/Cargo.lock runtime-hosts/rust/crates/hmcl-runtime-protocol
git commit -m "Define Rust isolated host protocol"
```

### Task 2: Implement The Rust Stdio Process Host

**Files:**
- Create: `runtime-hosts/rust/crates/hmcl-rust-host-process/Cargo.toml`
- Create: `runtime-hosts/rust/crates/hmcl-rust-host-process/src/main.rs`
- Create: `runtime-hosts/rust/crates/hmcl-rust-host-process/src/server.rs`
- Create: `runtime-hosts/rust/crates/hmcl-rust-host-process/tests/stdio.rs`
- Modify: `runtime-hosts/rust/Cargo.toml`
- Modify: `runtime-hosts/rust/Cargo.lock`

**Interfaces:**
- Consumes: Task 1 messages and `hmcl_rust_host_native::embedded::{BridgeTransport, Engine}`.
- Produces: `Server<R, W>::serve`, `ProcessBridge`, and the `hmcl-rust-host-process` binary with `--stdio` and `--probe`.

- [ ] **Step 1: Add failing stdio lifecycle and callback tests**

Build the existing valid fixture, start the process with piped stdio, then assert this literal sequence:

```rust
send(MessageBody::Hello);
expect(MessageBody::Ok);
send(MessageBody::Load { package_root, entrypoint, plugin_id: 1, session: 1 });
expect(MessageBody::Ok);
send(MessageBody::Enable);
expect_bridge_invoke("initialize");
reply(MessageBody::CallbackResult { output: null_wire() });
expect(MessageBody::Ok);
send(MessageBody::Invoke { operation: "bridge".into(), input: null_wire(), callback_id: 41 });
expect_bridge_invoke("fixture.bridge");
reply(MessageBody::CallbackResult { output: null_wire() });
expect(MessageBody::Result { output: null_wire() });
send(MessageBody::Disable);
expect(MessageBody::Ok);
send(MessageBody::Shutdown);
expect_bridge_invoke("shutdown");
reply(MessageBody::CallbackResult { output: null_wire() });
expect(MessageBody::Ok);
```

Add separate tests for invoke-before-enable, duplicate load, wrong ABI, missing query, callback-error propagation, malformed parent frame, and EOF cleanup.

- [ ] **Step 2: Run process tests and verify RED**

Run: `cargo test --manifest-path runtime-hosts/rust/Cargo.toml -p hmcl-rust-host-process`

Expected: FAIL because the process crate and server do not exist.

- [ ] **Step 3: Export the current embedded engine and implement the server**

Keep `embedded` public from `hmcl-rust-host-native`. Implement a single-payload state machine:

```rust
enum State {
    AwaitHello,
    AwaitLoad,
    Loaded { engine: Engine, payload_id: u64 },
    Closed,
}

pub struct Server<R, W> {
    reader: Arc<Mutex<R>>,
    writer: Arc<Mutex<W>>,
    state: State,
    next_callback_id: AtomicU64,
}
```

The main loop releases the reader mutex immediately after decoding a parent command and before entering the plugin ABI. `ProcessBridge` then writes an even-ID child callback frame and synchronously locks the same reader for its matching `callback-result` or `callback-error`. Parent commands use odd IDs, both sides increment by two, and exhaustion is fatal. Convert `HostError` into stable lower-case codes and messages capped at 4096 UTF-8 bytes. Reject a second payload or any lifecycle transition invalid under the current embedded engine.

`main` accepts only one argument. `--probe` exits 0 without reading stdin. `--stdio` locks stdin/stdout and calls `Server::serve`; all diagnostics use stderr.

- [ ] **Step 4: Run the Rust workspace quality gates**

```powershell
cargo fmt --manifest-path runtime-hosts/rust/Cargo.toml --check
cargo clippy --manifest-path runtime-hosts/rust/Cargo.toml --workspace --all-targets -- -D warnings
cargo test --manifest-path runtime-hosts/rust/Cargo.toml --workspace
```

Expected: all commands exit 0.

- [ ] **Step 5: Commit the process Host**

```powershell
git add runtime-hosts/rust
git commit -m "Run Rust payloads through isolated stdio host"
```

### Task 3: Add The Independent Java Protocol Codec

**Files:**
- Create: `runtime-hosts/rust/host-plugin/src/main/java/dev/hmclce/runtime/rust/RustProcessMessage.java`
- Create: `runtime-hosts/rust/host-plugin/src/main/java/dev/hmclce/runtime/rust/RustProcessWireCodec.java`
- Create: `runtime-hosts/rust/host-plugin/src/test/java/dev/hmclce/runtime/rust/RustProcessWireCodecTest.java`

**Interfaces:**
- Consumes: Aura `BridgeValue` and `RuntimeBridgeWireCodec`.
- Produces: a sealed `RustProcessMessage` model plus `read(InputStream)` and `write(OutputStream, RustProcessMessage)`.

- [ ] **Step 1: Add failing golden compatibility tests**

Use the Rust Task 1 hello and invoke bytes as checked-in literal byte arrays. Assert Java decodes those bytes and Java encoding exactly reproduces them. Add malformed header, oversized length, unknown field, request ID zero, and wrong version cases.

```java
RustProcessMessage message = new RustProcessMessage.Hello(7L);
ByteArrayOutputStream output = new ByteArrayOutputStream();
RustProcessWireCodec.write(output, message);
assertArrayEquals(RUST_HELLO_FRAME, output.toByteArray());
assertEquals(message, RustProcessWireCodec.read(new ByteArrayInputStream(RUST_HELLO_FRAME)));
```

- [ ] **Step 2: Run Java codec tests and verify RED**

Run:

```powershell
$env:HMCL_JAR='C:\Users\ACX\Documents\Aura-Launcher\AuraLauncher\build\libs\Aura-Launcher-26.8.SNAPSHOT-next.jar'
& 'C:\Users\ACX\Documents\Aura-Launcher\gradlew.bat' -p runtime-hosts\rust\host-plugin test --tests dev.hmclce.runtime.rust.RustProcessWireCodecTest --no-daemon
```

Expected: FAIL because the Java codec classes do not exist.

- [ ] **Step 3: Implement exact Java records and codec**

Use `@NotNullByDefault`, `///` documentation on every class, field, method, and record component owner, and `@Nullable` for EOF:

```java
sealed interface RustProcessMessage permits RustProcessMessage.Hello, RustProcessMessage.Load {
    long requestId();

    record Hello(long requestId) implements RustProcessMessage { }
    record Load(long requestId, String packageRoot, String entrypoint, long pluginId, long session)
            implements RustProcessMessage { }
}

static @Nullable RustProcessMessage read(InputStream input) throws IOException;
static void write(OutputStream output, RustProcessMessage message) throws IOException;
```

Define every Task 1 message as a record. Decode exact maps with explicit required-field helpers and reject all unsupported Bridge kinds. Frame length is unsigned big-endian but must be between 1 and `16 * 1024 * 1024`.

- [ ] **Step 4: Run Java codec and existing Rust Host tests**

Run the Task 3 Gradle command without `--tests`, then `git diff --check`.

Expected: all tests pass and diff check emits no errors.

- [ ] **Step 5: Commit the Java codec**

```powershell
git add runtime-hosts/rust/host-plugin/src/main/java/dev/hmclce/runtime/rust/RustProcessMessage.java runtime-hosts/rust/host-plugin/src/main/java/dev/hmclce/runtime/rust/RustProcessWireCodec.java runtime-hosts/rust/host-plugin/src/test/java/dev/hmclce/runtime/rust/RustProcessWireCodecTest.java
git commit -m "Decode Rust isolated host messages"
```

### Task 4: Supervise One Isolated Payload Process

**Files:**
- Create: `runtime-hosts/rust/host-plugin/src/main/java/dev/hmclce/runtime/rust/RustIsolatedPayload.java`
- Create: `runtime-hosts/rust/host-plugin/src/test/java/dev/hmclce/runtime/rust/RustIsolatedPayloadTest.java`

**Interfaces:**
- Consumes: Task 3 codec and one `RuntimePayloadContext`.
- Produces: `start`, `enable`, `invoke`, `disable`, `shutdown`, `close`, and an injectable `ProcessLauncher`.

- [ ] **Step 1: Add failing fake-process boundary tests**

Cover exact `ProcessBuilder` executable/`--stdio`, cleared and allowlisted environment, package-root working directory, hello/load sequence, Bridge invoke/retain/release callbacks, response ID matching, timeout kill, unexpected exit, bounded stderr, idempotent close, and no capability-token supplier access.

```java
RustIsolatedPayload payload = RustIsolatedPayload.start(executable, context, launcher, scheduler);
payload.enable();
assertArrayEquals(WIRE_NULL, payload.invoke("bridge", WIRE_NULL, 41L, Duration.ofSeconds(1)));
payload.disable();
payload.shutdown();
assertFalse(process.isAlive());
assertEquals(0, tokenSupplierCalls.get());
```

- [ ] **Step 2: Run isolated payload tests and verify RED**

Run the Task 3 Gradle command with `--tests dev.hmclce.runtime.rust.RustIsolatedPayloadTest`.

Expected: FAIL because `RustIsolatedPayload` does not exist.

- [ ] **Step 3: Implement process supervision and deadlines**

Use these exact operations:

```java
static RustIsolatedPayload start(
        Path executable,
        RuntimePayloadContext context,
        ProcessLauncher launcher,
        ScheduledExecutorService scheduler
) throws IOException;

byte[] invoke(String operation, byte[] input, long callbackId, Duration timeout) throws IOException;
void enable() throws IOException;
void disable() throws IOException;
void shutdown() throws IOException;
void close();
```

All public operations synchronize on the payload. Schedule a deadline before blocking read; the deadline closes streams, calls `destroy`, waits 250 ms, then calls `destroyForcibly`. Cancel the scheduled deadline after a matching response. A protocol failure or deadline stores one terminal `IOException` and rejects subsequent work. Drain stderr on a daemon thread into a circular 64 KiB UTF-8 byte tail.

Bridge callbacks call `context.bridgeTransport().invoke`, `retainHandle`, and `releaseHandle`. Send `callback-error` on callback failure without serializing exception messages or token objects.

- [ ] **Step 4: Run Java Host tests and mutation-check failure paths**

Run all Rust Host Java tests. Temporarily make the fake child return a mismatched request ID and confirm the mismatch test fails before restoring the production implementation.

- [ ] **Step 5: Commit process supervision**

```powershell
git add runtime-hosts/rust/host-plugin/src/main/java/dev/hmclce/runtime/rust/RustIsolatedPayload.java runtime-hosts/rust/host-plugin/src/test/java/dev/hmclce/runtime/rust/RustIsolatedPayloadTest.java
git commit -m "Supervise isolated Rust payload process"
```

### Task 5: Route Embedded And Isolated Payloads

**Files:**
- Create: `runtime-hosts/rust/host-plugin/src/main/java/dev/hmclce/runtime/rust/RustRuntimeEngine.java`
- Modify: `runtime-hosts/rust/host-plugin/src/main/java/dev/hmclce/runtime/rust/RustRuntimeHostPlugin.java`
- Modify: `runtime-hosts/rust/host-plugin/src/main/java/dev/hmclce/runtime/rust/RustRuntimeProvider.java`
- Modify: `runtime-hosts/rust/host-plugin/src/test/java/dev/hmclce/runtime/rust/RustRuntimeHostPluginTest.java`

**Interfaces:**
- Consumes: `RustNativeEngine`, `RustIsolatedPayload`, and `RuntimePayloadContext.executionMode()`.
- Produces: one Provider engine whose opaque handles route to either backend and whose Hook invocation passes the dispatcher timeout to isolated payloads.

- [ ] **Step 1: Add failing hybrid routing tests**

Assert embedded contexts call only `RustNativeEngine`, isolated contexts start one process, mixed payloads keep distinct opaque IDs, foreign IDs fail, close is reverse load order, and Hook timeouts reach isolated invoke.

```java
String embedded = engine.loadPayload(embeddedContext);
String isolated = engine.loadPayload(isolatedContext);
engine.enablePayload(embedded);
engine.enablePayload(isolated);
assertEquals(List.of("embedded:enable", "isolated:enable"), events);
engine.close();
assertEquals(List.of("isolated:close", "embedded:close"), closeEvents);
```

- [ ] **Step 2: Run hybrid routing tests and verify RED**

Run all Rust Host Java tests.

Expected: FAIL because the production Host still creates `RustNativeEngine` directly and advertises embedded only.

- [ ] **Step 3: Implement `RustRuntimeEngine` and timeout-aware engine invocation**

Extend the internal engine boundary without changing Aura SPI:

```java
default byte[] invokePayload(
        String payloadId,
        String operation,
        byte[] input,
        long callbackId,
        Duration timeout
) throws IOException {
    return invokePayload(payloadId, operation, input, callbackId);
}
```

`RustRuntimeProvider.invokeHook` calls this overload with the dispatcher timeout. Generic `invokePayload` uses `Duration.ofSeconds(30)`. `RustRuntimeEngine` allocates positive monotonically increasing string IDs and stores a sealed backend record; it never prefixes handles with process IDs or native IDs.

Change the production Host factory to `RustRuntimeEngine.load(packageRoot, platform)`. Keep injection seams so existing JNI and plugin lifecycle tests remain deterministic.

- [ ] **Step 4: Run all Java Host and Aura Hook tests**

Run the full Rust Host Gradle test command, then Aura `RuntimeHookWireCodecTest`, `RuntimeHookEndpointTest`, `PluginHookDispatcherTest`, and `RuntimeSupervisorTest`.

Expected: all tests pass.

- [ ] **Step 5: Commit hybrid routing**

```powershell
git add runtime-hosts/rust/host-plugin/src
git commit -m "Route Rust payload execution modes"
```

### Task 6: Advertise And Validate Isolated Packaging

**Files:**
- Modify: `runtime-hosts/rust/host-plugin/plugin.json`
- Modify: `runtime-hosts/rust/host-plugin/build.gradle.kts`
- Modify: `tools/validate-npl.ps1`
- Modify: `tools/test-validate-npl.ps1`
- Modify: `runtime-hosts/rust/host-plugin/src/test/java/dev/hmclce/runtime/rust/RustRuntimeHostPluginTest.java`

**Interfaces:**
- Consumes: built JNI library and process executable paths.
- Produces: an NPL that advertises `embedded` and `isolated` only when it contains both platform artifacts.

- [ ] **Step 1: Add failing manifest, package, and validator tests**

Assert `executionModes` equals both enum values. Add validator fixtures where `providesRuntimes[].runtime == "rust"` advertises isolated mode with and without `native/<platform>/hmcl-rust-host-process[.exe]`; only the complete package is valid.

- [ ] **Step 2: Run Java and validator tests and verify RED**

```powershell
& .\tools\test-validate-npl.ps1
$env:HMCL_JAR='C:\Users\ACX\Documents\Aura-Launcher\AuraLauncher\build\libs\Aura-Launcher-26.8.SNAPSHOT-next.jar'
& 'C:\Users\ACX\Documents\Aura-Launcher\gradlew.bat' -p runtime-hosts\rust\host-plugin test --no-daemon
```

Expected: validator and manifest tests fail because isolated mode and the process artifact are absent.

- [ ] **Step 3: Require and package both platform artifacts**

Add `HMCL_RUST_PROCESS_HOST` beside `HMCL_RUST_NATIVE_LIBRARY`. Package both under the selected `native/<platform>` directory. Use the exact process filename returned by `RustRuntimeEngine.processHostPath(platform)` and require a regular file before Gradle writes the NPL.

Update `plugin.json` description to `Optional embedded and isolated Rust runtime provider for schema-v5 plugins.` and set `executionModes` to `embedded`, `isolated` in that order.

- [ ] **Step 4: Build and validate a real Windows x64 NPL**

```powershell
$env:HMCL_RUST_NATIVE_LIBRARY=(Resolve-Path runtime-hosts\rust\target\debug\hmcl_rust_host_native.dll)
$env:HMCL_RUST_PROCESS_HOST=(Resolve-Path runtime-hosts\rust\target\debug\hmcl-rust-host-process.exe)
$env:HMCL_RUST_PLATFORM='windows-x64'
& 'C:\Users\ACX\Documents\Aura-Launcher\gradlew.bat' -p runtime-hosts\rust\host-plugin packageNpl --no-daemon
& .\tools\validate-npl.ps1 -Package runtime-hosts\rust\host-plugin\build\npl\dev.hmclce.runtime.rust-host-v0.1.0.npl
```

Expected: Gradle and validator exit 0, and archive inspection shows both artifacts.

- [ ] **Step 5: Commit isolated packaging**

```powershell
git add runtime-hosts/rust/host-plugin tools/validate-npl.ps1 tools/test-validate-npl.ps1
git commit -m "Package isolated Rust runtime host"
```

### Task 7: Add Real End-To-End Coverage And Example Documentation

**Files:**
- Modify: `runtime-hosts/rust/host-plugin/src/test/java/dev/hmclce/runtime/rust/RustRuntimeHostPluginTest.java`
- Create: `examples/rust-launch-hook/Cargo.toml`
- Create: `examples/rust-launch-hook/src/lib.rs`
- Create: `examples/rust-launch-hook/plugin.json`
- Create: `examples/rust-launch-hook/README.md`
- Modify: `README.md`
- Modify: `docs/PLUGIN_QUICKSTART.md`

**Interfaces:**
- Consumes: built process Host, existing valid fixture, and schema-v5 Hook wire contract.
- Produces: a real process integration test and a minimal isolated Rust Hook example.

- [ ] **Step 1: Add a failing real-process integration test**

Gate it on `HMCL_RUST_PROCESS_HOST` and `HMCL_RUST_EMBEDDED_FIXTURE`, copy artifacts into canonical test package paths, create an `ISOLATED` context with a recording `RuntimeBridgeTransport`, then assert initialize, `hook.before-game-launch`, retain/release, shutdown, and child exit.

- [ ] **Step 2: Run the integration test and verify RED**

Build Rust workspace artifacts, export both environment variables, and run only `loadsIsolatedPayloadThroughRealProcessBridge`.

Expected: FAIL until Tasks 4-6 are correctly connected.

- [ ] **Step 3: Add the isolated Rust launch-Hook example**

The example manifest uses schema v5, `runtime: "rust"`, ABI 1, `executionMode: "isolated"`, requires runtime features `bridge` and `hooks`, and declares `before-game-launch`. Its Rust callback decodes the Hook event and returns the strict unchanged envelope:

```rust
Value::Map(vec![
    ("contractVersion".into(), Value::Integer(1)),
    ("action".into(), Value::String("unchanged".into())),
])
```

Document build, NPL layout, Runtime Host dependency, and why Java tokens are absent from the process protocol.

- [ ] **Step 4: Run full local verification**

Run Cargo format, full Clippy, Rust workspace tests, full Java Host tests with real integration environment, 150+ validator tests, real NPL validation, Aura Hook tests, and `git diff --check`.

- [ ] **Step 5: Commit integration and example**

```powershell
git add runtime-hosts/rust/host-plugin/src/test examples/rust-launch-hook README.md docs/PLUGIN_QUICKSTART.md
git commit -m "Document isolated Rust launch hooks"
```

### Task 8: Add The Six-Platform Build Matrix

**Files:**
- Create: `.github/workflows/rust-runtime-host.yml`
- Create: `tools/verify-rust-host-artifacts.ps1`
- Create: `tools/test-verify-rust-host-artifacts.ps1`
- Modify: `docs/PLUGIN_STORE_SETUP.md`

**Interfaces:**
- Consumes: Rust workspace, Host Gradle project, private Aura artifact access through `AURA_REPOSITORY_TOKEN`.
- Produces: six target-specific validated NPL artifacts and a merged CI manifest of hashes and sizes.

- [ ] **Step 1: Add failing artifact-verifier tests**

Test exact target-to-filename mappings, missing JNI/process artifacts, executable filename mismatches, duplicate platform outputs, and NPL hashes. The verifier accepts `-Platform`, `-NativeLibrary`, `-ProcessHost`, and `-Package` and exits nonzero on any mismatch.

- [ ] **Step 2: Run verifier tests and verify RED**

Run: `& .\tools\test-verify-rust-host-artifacts.ps1`

Expected: FAIL because the verifier does not exist.

- [ ] **Step 3: Implement the verifier and CI matrix**

Use these six matrix entries:

```yaml
include:
  - { runner: windows-2025, platform: windows-x64, target: x86_64-pc-windows-msvc }
  - { runner: windows-11-arm, platform: windows-arm64, target: aarch64-pc-windows-msvc }
  - { runner: ubuntu-24.04, platform: linux-x64, target: x86_64-unknown-linux-gnu }
  - { runner: ubuntu-24.04-arm, platform: linux-arm64, target: aarch64-unknown-linux-gnu }
  - { runner: macos-15-intel, platform: macos-x64, target: x86_64-apple-darwin }
  - { runner: macos-15, platform: macos-arm64, target: aarch64-apple-darwin }
```

Each job installs Rust 1.97.1 with its exact target, runs format once, runs target Clippy/tests where executable on the runner, builds both release artifacts, downloads the exact verified Aura Next JAR using `AURA_REPOSITORY_TOKEN`, packages one NPL, validates it, runs the artifact verifier, and uploads an immutable artifact named `rust-runtime-host-<platform>`.

- [ ] **Step 4: Run local script tests and inspect workflow syntax**

Run publishing tool tests, validator tests, artifact-verifier tests, and parse the workflow with PowerShell `ConvertFrom-Yaml` when available or Ruby's standard YAML parser on CI.

- [ ] **Step 5: Commit the release matrix**

```powershell
git add .github/workflows/rust-runtime-host.yml tools/verify-rust-host-artifacts.ps1 tools/test-verify-rust-host-artifacts.ps1 docs/PLUGIN_STORE_SETUP.md
git commit -m "Build Rust runtime host for six platforms"
```

## Final Verification And Delivery

- [ ] Run `cargo fmt --manifest-path runtime-hosts/rust/Cargo.toml --check`.
- [ ] Run `cargo clippy --manifest-path runtime-hosts/rust/Cargo.toml --workspace --all-targets -- -D warnings`.
- [ ] Run `cargo test --manifest-path runtime-hosts/rust/Cargo.toml --workspace`.
- [ ] Force-rerun all Rust Host Java tests against `Aura-Launcher-26.8.SNAPSHOT-next.jar`.
- [ ] Run `tools/test-validate-npl.ps1`, publishing tool tests, and artifact-verifier tests.
- [ ] Build and validate a real Windows x64 NPL containing the JNI library and process Host.
- [ ] Run Aura `checkstyle checkTranslations test shadowJar`.
- [ ] Verify Aura JAR filename and `Implementation-Version` contain exactly one `-next`.
- [ ] Run `git diff --check` and confirm only planned files changed.
- [ ] Push SDK `schema-v5` and Aura `main` with ordinary non-force pushes; if GitHub remains unreachable, preserve local commits and record their SHAs.
