# Aura Rust Runtime Host

Aura Rust Runtime Host is an optional schema-v5 runtime provider for Aura Launcher. It supports the existing embedded JNI ABI and one isolated child process per Rust payload.

The compatibility identifiers `dev.hmclce.runtime.rust-host` and `dev.hmclce` remain unchanged for installed-plugin and protocol compatibility.

## Requirements

- Aura Launcher `27.1` Next or newer
- Java 17 or newer
- Gradle `9.6.1` on `PATH`, or `AURA_GRADLE` set to an Aura checkout's `gradlew.ps1`
- A platform package matching Windows, Linux, or macOS on x64 or ARM64

## Development

```powershell
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
./tools/test-verify-rust-host-artifacts.ps1
./tools/test-ci-workflow.ps1
```

Build the native Host and launch-hook sample on Windows x64, then run the real Java Hook/Patch
integration tests against the pinned Aura Next Shadow JAR:

```powershell
$env:HMCL_JAR = (Resolve-Path '.ci/aura/Aura-Launcher-27.1.dev-636b06a-next.jar').Path
cargo build --release -p hmcl-rust-host-native -p hmcl-rust-host-process
cargo build --manifest-path examples/launch-hook/Cargo.toml --release --locked
$env:HMCL_RUST_PLATFORM = 'windows-x64'
$env:HMCL_RUST_NATIVE_LIBRARY = (Resolve-Path 'target/release/hmcl_rust_host_native.dll').Path
$env:HMCL_RUST_PROCESS_HOST = (Resolve-Path 'target/release/hmcl-rust-host-process.exe').Path
$env:AURA_RUST_LAUNCH_HOOK = (Resolve-Path 'target/release/hmcl_rust_launch_hook.dll').Path
$gradle = if ([string]::IsNullOrWhiteSpace($env:AURA_GRADLE)) { 'gradle' } else { $env:AURA_GRADLE }
& $gradle -p host-plugin test packageNpl --no-daemon
./tools/package-rust-launch-hook.ps1 `
  -Platform windows-x64 `
  -Library $env:AURA_RUST_LAUNCH_HOOK `
  -Output artifacts/dev.hmclce.example.rust.launch-hook-v1.0.0-windows-x64.npl
./.ci/sdk/tools/validate-npl.ps1 `
  -Package host-plugin/build/npl/dev.hmclce.runtime.rust-host-v0.2.0-beta.1.npl
./.ci/sdk/tools/validate-npl.ps1 `
  -Package artifacts/dev.hmclce.example.rust.launch-hook-v1.0.0-windows-x64.npl
```

The source is `0.2.0-beta.1`; existing published beta metadata and assets are unchanged. JVM
capability tokens remain inside Aura and never cross into the Rust process protocol.

## License

Aura Rust Runtime Host is licensed under GPL-3.0-or-later. Upstream HMCL copyrights and compatibility identifiers are retained where required.
