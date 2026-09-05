[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('windows-x64', 'windows-arm64', 'linux-x64', 'linux-arm64', 'macos-x64', 'macos-arm64')]
    [string]$Platform,
    [Parameter(Mandatory = $true)]
    [string]$Library
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem

function Assert-Condition([bool]$Condition, [string]$Message) {
    if (-not $Condition) {
        throw $Message
    }
}

$repositoryRoot = Split-Path -Parent $PSScriptRoot
$expectedLibraryNames = [ordered]@{
    'windows-x64' = 'hmcl_rust_launch_hook.dll'
    'windows-arm64' = 'hmcl_rust_launch_hook.dll'
    'linux-x64' = 'libhmcl_rust_launch_hook.so'
    'linux-arm64' = 'libhmcl_rust_launch_hook.so'
    'macos-x64' = 'libhmcl_rust_launch_hook.dylib'
    'macos-arm64' = 'libhmcl_rust_launch_hook.dylib'
}

function Assert-PackageLayout([string]$Package, [string]$ExpectedPlatform) {
    $expectedName = $expectedLibraryNames[$ExpectedPlatform]
    $archive = [System.IO.Compression.ZipFile]::OpenRead($Package)
    try {
        $manifestEntry = $archive.GetEntry('plugin.json')
        Assert-Condition ($null -ne $manifestEntry) "Rust payload package is missing plugin.json: $Package"
        $reader = [System.IO.StreamReader]::new($manifestEntry.Open())
        try {
            $manifest = $reader.ReadToEnd() | ConvertFrom-Json
        } finally {
            $reader.Dispose()
        }
        Assert-Condition (($archive.Entries.FullName -join ',') -ceq "payload/$expectedName,plugin.json") `
            "Rust payload package has unexpected entries: $Package"
        Assert-Condition ($manifest.schemaVersion -eq 5) "Rust payload package has the wrong schema: $Package"
        Assert-Condition (($manifest.platforms -join ',') -ceq $ExpectedPlatform) `
            "Rust payload package platform is not exact: $Package"
        Assert-Condition ($manifest.entrypoint -ceq "payload/$expectedName") `
            "Rust payload entrypoint is not exact: $Package"
    } finally {
        $archive.Dispose()
    }
}

if (-not (Test-Path -LiteralPath $Library -PathType Leaf)) {
    throw "Build the Rust launch-hook sample before running packaging tests: $Library"
}

$temporary = Join-Path ([System.IO.Path]::GetTempPath()) ('aura-rust-launch-hook-package-test-' + [guid]::NewGuid().ToString('N'))
try {
    [void](New-Item -ItemType Directory -Path $temporary)
    $packager = Join-Path $PSScriptRoot 'package-rust-launch-hook.ps1'
    $actualFirst = Join-Path $temporary 'actual-first.npl'
    $actualSecond = Join-Path $temporary 'actual-second.npl'
    & $packager -Platform $Platform -Library $Library -Output $actualFirst
    & $packager -Platform $Platform -Library $Library -Output $actualSecond
    Assert-Condition ((Get-FileHash -LiteralPath $actualFirst -Algorithm SHA256).Hash -ceq
        (Get-FileHash -LiteralPath $actualSecond -Algorithm SHA256).Hash) `
        "Actual $Platform Rust payload packages were not deterministic"
    Assert-PackageLayout -Package $actualFirst -ExpectedPlatform $Platform

    foreach ($layoutPlatform in $expectedLibraryNames.Keys) {
        $layoutName = $expectedLibraryNames[$layoutPlatform]
        $layoutDirectory = Join-Path $temporary "layout-only-$layoutPlatform"
        [void](New-Item -ItemType Directory -Path $layoutDirectory -Force)
        $layoutFixture = Join-Path $layoutDirectory $layoutName
        [System.IO.File]::WriteAllBytes($layoutFixture, [System.Text.Encoding]::UTF8.GetBytes("layout-only-$layoutPlatform"))
        $layoutFirst = Join-Path $layoutDirectory 'first.npl'
        $layoutSecond = Join-Path $layoutDirectory 'second.npl'
        & $packager -Platform $layoutPlatform -Library $layoutFixture -Output $layoutFirst
        & $packager -Platform $layoutPlatform -Library $layoutFixture -Output $layoutSecond
        Assert-Condition ((Get-FileHash -LiteralPath $layoutFirst -Algorithm SHA256).Hash -ceq
            (Get-FileHash -LiteralPath $layoutSecond -Algorithm SHA256).Hash) `
            "Layout-only $layoutPlatform Rust payload packages were not deterministic"
        Assert-PackageLayout -Package $layoutFirst -ExpectedPlatform $layoutPlatform
    }
} finally {
    Remove-Item -LiteralPath $temporary -Recurse -Force -ErrorAction SilentlyContinue
}

Write-Host 'Rust launch-hook payload packaging layout tests passed for the actual library and all six layout-only fixtures.'
