# Aura Rust Runtime Host

Aura Rust Runtime Host is an optional schema-v5 runtime provider for Aura Launcher. It supports the existing embedded JNI ABI and one isolated child process per Rust payload.

The compatibility identifiers `dev.hmclce.runtime.rust-host` and `dev.hmclce` remain unchanged for installed-plugin and protocol compatibility.

## Requirements

- Aura Launcher `26.8` Next or newer
- Java 17 or newer
- A platform package matching Windows, Linux, or macOS on x64 or ARM64

## Development

```powershell
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Java Provider tests compile against an exact Aura Launcher Shadow JAR:

```powershell
$env:HMCL_JAR = 'C:\path\to\Aura-Launcher-26.8.SNAPSHOT-next.jar'
& C:\path\to\Aura-Launcher\gradlew.bat -p host-plugin test --no-daemon
```

Release packages and the Store manifest are published from immutable GitHub Releases. Temporary Actions artifact URLs are never used as Store download URLs.

## License

Aura Rust Runtime Host is licensed under GPL-3.0-or-later. Upstream HMCL copyrights and compatibility identifiers are retained where required.
