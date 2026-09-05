# Aura Rust launch-hook example

This source example is a schema-v5 Rust payload for the optional
`dev.hmclce.runtime.rust-host` provider. It observes the `before-game-launch` Hook and the
after Patch for `org.jackhuang.hmcl.util.io.FileUtils.getName(java.nio.file.Path)`. Both handlers
return `unchanged`; the Patch does not modify arguments or the target result.

The payload declares `launcher-hook` and `launcher-patch` in both permission lists. Grant these
permissions only to an exact reviewed NPL. Aura can revoke its callbacks when the payload is
disabled, unloaded, replaced, or loses permission.

## Build and package

Build the library for the current Windows x64 development host and create a reproducible NPL:

```powershell
cargo build --manifest-path examples/launch-hook/Cargo.toml --release --locked
./tools/package-rust-launch-hook.ps1 `
  -Platform windows-x64 `
  -Library target/release/hmcl_rust_launch_hook.dll `
  -Output artifacts/dev.hmclce.example.rust.launch-hook-v1.0.0-windows-x64.npl
./.ci/sdk/tools/validate-npl.ps1 `
  -Package artifacts/dev.hmclce.example.rust.launch-hook-v1.0.0-windows-x64.npl
```

For CI or another target, pass the matching Cargo target output and platform. The generated
manifest contains exactly that platform and its matching entrypoint: `.dll` on Windows, `.so` on
Linux, and `.dylib` on macOS.

## Runtime boundary

The isolated child process receives canonical Hook/Patch envelopes, operation names, and Bridge
Value v1 bytes. JVM `PluginCapabilityToken` values never cross the process boundary. Aura retains
the capability token and reauthorizes every callback against the original payload context.

This is current source, not a claim that existing beta downloads include the Hook/Patch changes.
