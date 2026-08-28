# Isolated Rust Launch Hook

此示例是 schema-v5 Rust payload。它由单独安装的
`dev.hmclce.runtime.rust-host` 插件启动，每个 payload 使用一个
`hmcl-rust-host-process` 子进程。示例接收 `before-game-launch` Hook，验证
Bridge Value v1 事件字段，并返回严格的 `unchanged` 结果。

## 构建 Windows x64 NPL

```powershell
cargo build --manifest-path examples/rust-launch-hook/Cargo.toml --release
$stage = Join-Path $env:TEMP ('hmcl-rust-launch-hook-npl-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force "$stage/payload" | Out-Null
Copy-Item examples/rust-launch-hook/plugin.json "$stage/plugin.json"
Copy-Item examples/rust-launch-hook/target/release/hmcl_rust_launch_hook.dll `
    "$stage/payload/hmcl_rust_launch_hook.dll"
$package = "$stage/dev.hmclce.example.rust.launch-hook-v1.0.0.npl"
Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem
$archive = [System.IO.Compression.ZipFile]::Open(
    $package,
    [System.IO.Compression.ZipArchiveMode]::Create
)
try {
    [System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile(
        $archive, "$stage/plugin.json", 'plugin.json') | Out-Null
    [System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile(
        $archive,
        "$stage/payload/hmcl_rust_launch_hook.dll",
        'payload/hmcl_rust_launch_hook.dll'
    ) | Out-Null
} finally {
    $archive.Dispose()
}
./tools/validate-npl.ps1 -Package $package
```

先安装与当前平台匹配的 Rust Runtime Host NPL，再安装示例 NPL。Host 是可选插件，
不会被打进 Aura Launcher，也不会被打进此 payload 包。

Schema v5 当前保留 legacy `type` 字段，所以清单仍写 `type: "java"`；它不加载 Java 类，
也不把 Rust 类型绑定到 JVM。`runtime: "rust"`、ABI 1、`executionMode: "isolated"` 和
`runtimeProvider` 才决定实际 Host、ABI 与进程边界。固定 Provider 会参与 Launcher 的运行时
依赖规划，因此 `dependencies` 不需要重复列出 Host 插件 ID。

## 其他平台

每个平台发布独立 NPL，并同步修改 `platforms` 与 `entrypoint`：

| 平台 | Cargo payload 文件 | NPL entrypoint |
| --- | --- | --- |
| Windows x64/ARM64 | `hmcl_rust_launch_hook.dll` | `payload/hmcl_rust_launch_hook.dll` |
| Linux x64/ARM64 | `libhmcl_rust_launch_hook.so` | `payload/libhmcl_rust_launch_hook.so` |
| macOS x64/ARM64 | `libhmcl_rust_launch_hook.dylib` | `payload/libhmcl_rust_launch_hook.dylib` |

Java 的 `PluginCapabilityToken` 不会序列化到子进程。子进程只收到协议 request ID、
payload 路径、操作名和 Bridge Value v1 字节；每次 Bridge 回调都由 Launcher 依据原始
payload context 重新执行权限检查。
