[CmdletBinding()]
param(
    [ValidateSet('windows-x64', 'windows-arm64', 'linux-x64', 'linux-arm64', 'macos-x64', 'macos-arm64')]
    [string]$ActualPlatform = 'windows-x64',
    [string]$ActualPackage = 'host-plugin/build/npl/dev.hmclce.runtime.rust-host-v0.2.0-beta.1.npl',
    [string]$ActualNative = 'target/release/hmcl_rust_host_native.dll',
    [string]$ActualProcess = 'target/release/hmcl-rust-host-process.exe'
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem

function Assert-Condition([bool]$Condition, [string]$Message) {
    if (-not $Condition) {
        throw $Message
    }
}

function Assert-Fails([scriptblock]$Action, [string]$ExpectedMessage) {
    try {
        & $Action
    } catch {
        Assert-Condition ($_.Exception.Message -like "*$ExpectedMessage*") `
            "Expected '$ExpectedMessage', got '$($_.Exception.Message)'"
        return
    }
    throw "Expected failure containing '$ExpectedMessage'"
}

function New-Artifact([string]$Root, [string]$Name, [byte]$Marker) {
    $path = Join-Path $Root $Name
    [System.IO.File]::WriteAllBytes($path, [byte[]]($Marker, 0x4d, 0x43, 0x4c))
    return $path
}

function New-HostManifest([string]$Root, [string]$Platform, [string[]]$Features) {
    $manifest = [ordered]@{
        schemaVersion = 5
        id = 'dev.hmclce.runtime.rust-host'
        runtime = 'java'
        platforms = @($Platform)
        entrypoint = 'dev.hmclce.runtime.rust.RustRuntimeHostPlugin'
        permissions = @('native-code')
        requiredPermissions = @('native-code')
        providesRuntimes = @([ordered]@{
            runtime = 'rust'
            abis = @(1)
            bridgeAbi = 1
            executionModes = @('embedded', 'isolated')
            features = $Features
        })
    }
    $path = Join-Path $Root 'plugin.json'
    [System.IO.File]::WriteAllText($path, ($manifest | ConvertTo-Json -Depth 6), [System.Text.UTF8Encoding]::new($false))
    return $path
}

function New-Package([string]$Root, [string]$Name, [hashtable]$Entries) {
    $package = Join-Path $Root "$Name.npl"
    $archive = [System.IO.Compression.ZipFile]::Open(
        $package,
        [System.IO.Compression.ZipArchiveMode]::Create
    )
    try {
        foreach ($entryName in @($Entries.Keys | Sort-Object)) {
            [System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile(
                $archive,
                $Entries[$entryName],
                $entryName
            ) | Out-Null
        }
    } finally {
        $archive.Dispose()
    }
    return $package
}

$targets = @(
    @('windows-x64', 'hmcl_rust_host_native.dll', 'hmcl-rust-host-process.exe'),
    @('windows-arm64', 'hmcl_rust_host_native.dll', 'hmcl-rust-host-process.exe'),
    @('linux-x64', 'libhmcl_rust_host_native.so', 'hmcl-rust-host-process'),
    @('linux-arm64', 'libhmcl_rust_host_native.so', 'hmcl-rust-host-process'),
    @('macos-x64', 'libhmcl_rust_host_native.dylib', 'hmcl-rust-host-process'),
    @('macos-arm64', 'libhmcl_rust_host_native.dylib', 'hmcl-rust-host-process')
)
$temporary = Join-Path ([System.IO.Path]::GetTempPath()) `
    ('hmclce-rust-host-artifacts-test-' + [guid]::NewGuid().ToString('N'))
[void](New-Item -ItemType Directory -Path $temporary)
$verifier = Join-Path $PSScriptRoot 'verify-rust-host-artifacts.ps1'

try {
    foreach ($target in $targets) {
        $platform = $target[0]
        $fixture = Join-Path $temporary $platform
        [void](New-Item -ItemType Directory -Path $fixture)
        $native = New-Artifact $fixture $target[1] 0x4e
        $process = New-Artifact $fixture $target[2] 0x50
        $manifest = New-HostManifest $fixture $platform @('bridge', 'hooks', 'patches', 'native')
        $entries = @{
            'plugin.json' = $manifest
            "native/$platform/$($target[1])" = $native
            "native/$platform/$($target[2])" = $process
        }
        $package = New-Package $fixture "rust-host-$platform" $entries
        $recordPath = Join-Path $fixture 'artifact.json'

        $output = & $verifier `
            -Platform $platform `
            -NativeLibrary $native `
            -ProcessHost $process `
            -Package $package `
            -Output $recordPath
        $record = ($output -join "`n") | ConvertFrom-Json
        $writtenRecord = Get-Content -LiteralPath $recordPath -Raw | ConvertFrom-Json
        $expectedHash = (Get-FileHash -LiteralPath $package -Algorithm SHA256).Hash.ToLowerInvariant()
        $expectedSize = (Get-Item -LiteralPath $package).Length
        Assert-Condition ([string]$record.platform -ceq $platform) "$platform output platform mismatch"
        Assert-Condition ([string]$record.nativeLibrary -ceq $target[1]) `
            "$platform output native filename mismatch"
        Assert-Condition ([string]$record.processHost -ceq $target[2]) `
            "$platform output process filename mismatch"
        Assert-Condition ([string]$record.sha256 -ceq $expectedHash) "$platform output SHA-256 mismatch"
        Assert-Condition ([int64]$record.size -eq $expectedSize) "$platform output size mismatch"
        Assert-Condition ([string]$writtenRecord.sha256 -ceq $expectedHash) `
            "$platform written record SHA-256 mismatch"
    }

    $windows = Join-Path $temporary 'negative-windows'
    [void](New-Item -ItemType Directory -Path $windows)
    $native = New-Artifact $windows 'hmcl_rust_host_native.dll' 0x61
    $process = New-Artifact $windows 'hmcl-rust-host-process.exe' 0x62
    $manifest = New-HostManifest $windows 'windows-x64' @('bridge', 'hooks', 'patches', 'native')
    $validEntries = @{
        'plugin.json' = $manifest
        'native/windows-x64/hmcl_rust_host_native.dll' = $native
        'native/windows-x64/hmcl-rust-host-process.exe' = $process
    }
    $validPackage = New-Package $windows 'valid' $validEntries

    Assert-Fails {
        & $verifier -Platform windows-x64 -NativeLibrary (Join-Path $windows 'missing.dll') `
            -ProcessHost $process -Package $validPackage
    } 'Native library does not exist'

    $wrongNative = New-Artifact $windows 'wrong-native.dll' 0x63
    Assert-Fails {
        & $verifier -Platform windows-x64 -NativeLibrary $wrongNative `
            -ProcessHost $process -Package $validPackage
    } 'Native library for windows-x64 must be named hmcl_rust_host_native.dll'

    $wrongProcess = New-Artifact $windows 'wrong-process.exe' 0x64
    Assert-Fails {
        & $verifier -Platform windows-x64 -NativeLibrary $native `
            -ProcessHost $wrongProcess -Package $validPackage
    } 'Process Host for windows-x64 must be named hmcl-rust-host-process.exe'

    $missingNativePackage = New-Package $windows 'missing-native' @{
        'plugin.json' = $manifest
        'native/windows-x64/hmcl-rust-host-process.exe' = $process
    }
    Assert-Fails {
        & $verifier -Platform windows-x64 -NativeLibrary $native `
            -ProcessHost $process -Package $missingNativePackage
    } 'NPL is missing native library entry'

    $missingProcessPackage = New-Package $windows 'missing-process' @{
        'plugin.json' = $manifest
        'native/windows-x64/hmcl_rust_host_native.dll' = $native
    }
    Assert-Fails {
        & $verifier -Platform windows-x64 -NativeLibrary $native `
            -ProcessHost $process -Package $missingProcessPackage
    } 'NPL is missing process Host entry'

    $differentProcess = New-Artifact $windows 'different-process.exe' 0x65
    $mismatchedPackage = New-Package $windows 'mismatched-bytes' @{
        'plugin.json' = $manifest
        'native/windows-x64/hmcl_rust_host_native.dll' = $native
        'native/windows-x64/hmcl-rust-host-process.exe' = $differentProcess
    }
    Assert-Fails {
        & $verifier -Platform windows-x64 -NativeLibrary $native `
            -ProcessHost $process -Package $mismatchedPackage
    } 'NPL process Host bytes do not match the input artifact'

    $duplicatePlatformPackage = New-Package $windows 'duplicate-platform' @{
        'plugin.json' = $manifest
        'native/windows-x64/hmcl_rust_host_native.dll' = $native
        'native/windows-x64/hmcl-rust-host-process.exe' = $process
        'native/linux-x64/unexpected.bin' = $native
    }
    Assert-Fails {
        & $verifier -Platform windows-x64 -NativeLibrary $native `
            -ProcessHost $process -Package $duplicatePlatformPackage
    } 'NPL must contain exactly one native platform output'

    foreach ($requiredFeature in @('bridge', 'hooks', 'patches', 'native')) {
        $features = @('bridge', 'hooks', 'patches', 'native') | Where-Object { $_ -cne $requiredFeature }
        $missingFeatureManifest = New-HostManifest $windows 'windows-x64' $features
        $missingFeaturePackage = New-Package $windows "missing-feature-$requiredFeature" @{
            'plugin.json' = $missingFeatureManifest
            'native/windows-x64/hmcl_rust_host_native.dll' = $native
            'native/windows-x64/hmcl-rust-host-process.exe' = $process
        }
        Assert-Fails {
            & $verifier -Platform windows-x64 -NativeLibrary $native `
                -ProcessHost $process -Package $missingFeaturePackage
        } "Rust Host manifest is missing required feature: $requiredFeature"
    }

    $wrongSchemaManifest = New-HostManifest $windows 'windows-x64' @('bridge', 'hooks', 'patches', 'native')
    $wrongSchema = Get-Content -LiteralPath $wrongSchemaManifest -Raw | ConvertFrom-Json
    $wrongSchema.schemaVersion = 4
    $wrongSchema | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $wrongSchemaManifest -NoNewline
    $wrongSchemaPackage = New-Package $windows 'wrong-schema' @{
        'plugin.json' = $wrongSchemaManifest
        'native/windows-x64/hmcl_rust_host_native.dll' = $native
        'native/windows-x64/hmcl-rust-host-process.exe' = $process
    }
    Assert-Fails {
        & $verifier -Platform windows-x64 -NativeLibrary $native `
            -ProcessHost $process -Package $wrongSchemaPackage
    } 'Rust Host manifest must use schemaVersion 5'

    $wrongEntrypointManifest = New-HostManifest $windows 'windows-x64' @('bridge', 'hooks', 'patches', 'native')
    (Get-Content -LiteralPath $wrongEntrypointManifest -Raw).Replace(
        'dev.hmclce.runtime.rust.RustRuntimeHostPlugin', 'wrong.Entrypoint') |
        Set-Content -LiteralPath $wrongEntrypointManifest -NoNewline
    $wrongEntrypointPackage = New-Package $windows 'wrong-entrypoint' @{
        'plugin.json' = $wrongEntrypointManifest
        'native/windows-x64/hmcl_rust_host_native.dll' = $native
        'native/windows-x64/hmcl-rust-host-process.exe' = $process
    }
    Assert-Fails {
        & $verifier -Platform windows-x64 -NativeLibrary $native `
            -ProcessHost $process -Package $wrongEntrypointPackage
    } 'Rust Host manifest entrypoint is invalid'

    $corruptPackage = Join-Path $windows 'corrupt.npl'
    [System.IO.File]::WriteAllBytes($corruptPackage, [byte[]](0x00, 0x01, 0x02))
    Assert-Fails {
        & $verifier -Platform windows-x64 -NativeLibrary $native `
            -ProcessHost $process -Package $corruptPackage
    } 'NPL archive cannot be opened'

    Assert-Condition (Test-Path -LiteralPath $ActualPackage -PathType Leaf) `
        "Build the actual Host NPL before verifier tests: $ActualPackage"
    Assert-Condition (Test-Path -LiteralPath $ActualNative -PathType Leaf) `
        "Build the actual Host native library before verifier tests: $ActualNative"
    Assert-Condition (Test-Path -LiteralPath $ActualProcess -PathType Leaf) `
        "Build the actual Host process before verifier tests: $ActualProcess"
    $actualRoot = Join-Path $temporary 'actual-host'
    [void](New-Item -ItemType Directory -Path $actualRoot)

    function New-ActualManifestMutation([string]$Name, [scriptblock]$Mutation) {
        $copy = Join-Path $actualRoot "$Name.npl"
        Copy-Item -LiteralPath $ActualPackage -Destination $copy
        $updatedArchive = [System.IO.Compression.ZipFile]::Open(
            $copy, [System.IO.Compression.ZipArchiveMode]::Update)
        try {
            $entry = $updatedArchive.GetEntry('plugin.json')
            $reader = [System.IO.StreamReader]::new($entry.Open())
            try { $actualManifest = $reader.ReadToEnd() | ConvertFrom-Json } finally { $reader.Dispose() }
            & $Mutation $actualManifest
            $entry.Delete()
            $replacement = $updatedArchive.CreateEntry('plugin.json')
            $writer = [System.IO.StreamWriter]::new($replacement.Open(), [System.Text.UTF8Encoding]::new($false))
            $actualManifestJson = $actualManifest | ConvertTo-Json -Depth 8
            try { $writer.Write($actualManifestJson) } finally { $writer.Dispose() }
        } finally {
            $updatedArchive.Dispose()
        }
        return $copy
    }

    foreach ($requiredFeature in @('bridge', 'hooks', 'patches', 'native')) {
        $actualFeature = New-ActualManifestMutation "actual-missing-$requiredFeature" {
            param($manifest)
            $manifest.providesRuntimes[0].features = @($manifest.providesRuntimes[0].features | Where-Object { $_ -cne $requiredFeature })
        }
        Assert-Fails {
            & $verifier -Platform $ActualPlatform -NativeLibrary $ActualNative `
                -ProcessHost $ActualProcess -Package $actualFeature
        } "Rust Host manifest is missing required feature: $requiredFeature"
    }

    $actualSchema = New-ActualManifestMutation 'actual-wrong-schema' { param($manifest) $manifest.schemaVersion = 4 }
    Assert-Fails {
        & $verifier -Platform $ActualPlatform -NativeLibrary $ActualNative `
            -ProcessHost $ActualProcess -Package $actualSchema
    } 'Rust Host manifest must use schemaVersion 5'

    $actualEntrypoint = New-ActualManifestMutation 'actual-wrong-entrypoint' { param($manifest) $manifest.entrypoint = 'wrong.Entrypoint' }
    Assert-Fails {
        & $verifier -Platform $ActualPlatform -NativeLibrary $ActualNative `
            -ProcessHost $ActualProcess -Package $actualEntrypoint
    } 'Rust Host manifest entrypoint is invalid'

    $actualCorrupt = Join-Path $actualRoot 'actual-corrupt.npl'
    Copy-Item -LiteralPath $ActualPackage -Destination $actualCorrupt
    [System.IO.File]::WriteAllBytes($actualCorrupt, [byte[]](0x00, 0x01, 0x02))
    Assert-Fails {
        & $verifier -Platform $ActualPlatform -NativeLibrary $ActualNative `
            -ProcessHost $ActualProcess -Package $actualCorrupt
    } 'NPL archive cannot be opened'

    Write-Host 'Rust Host artifact verifier tests passed.'
} finally {
    Remove-Item -LiteralPath $temporary -Recurse -Force -ErrorAction SilentlyContinue
}
