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
        $entries = @{
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
    $validEntries = @{
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
        'native/windows-x64/hmcl-rust-host-process.exe' = $process
    }
    Assert-Fails {
        & $verifier -Platform windows-x64 -NativeLibrary $native `
            -ProcessHost $process -Package $missingNativePackage
    } 'NPL is missing native library entry'

    $missingProcessPackage = New-Package $windows 'missing-process' @{
        'native/windows-x64/hmcl_rust_host_native.dll' = $native
    }
    Assert-Fails {
        & $verifier -Platform windows-x64 -NativeLibrary $native `
            -ProcessHost $process -Package $missingProcessPackage
    } 'NPL is missing process Host entry'

    $differentProcess = New-Artifact $windows 'different-process.exe' 0x65
    $mismatchedPackage = New-Package $windows 'mismatched-bytes' @{
        'native/windows-x64/hmcl_rust_host_native.dll' = $native
        'native/windows-x64/hmcl-rust-host-process.exe' = $differentProcess
    }
    Assert-Fails {
        & $verifier -Platform windows-x64 -NativeLibrary $native `
            -ProcessHost $process -Package $mismatchedPackage
    } 'NPL process Host bytes do not match the input artifact'

    $duplicatePlatformPackage = New-Package $windows 'duplicate-platform' @{
        'native/windows-x64/hmcl_rust_host_native.dll' = $native
        'native/windows-x64/hmcl-rust-host-process.exe' = $process
        'native/linux-x64/unexpected.bin' = $native
    }
    Assert-Fails {
        & $verifier -Platform windows-x64 -NativeLibrary $native `
            -ProcessHost $process -Package $duplicatePlatformPackage
    } 'NPL must contain exactly one native platform output'

    Write-Host 'Rust Host artifact verifier tests passed.'
} finally {
    Remove-Item -LiteralPath $temporary -Recurse -Force -ErrorAction SilentlyContinue
}
