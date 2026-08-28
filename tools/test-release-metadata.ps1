$ErrorActionPreference = 'Stop'

function Assert-Equal([object]$Expected, [object]$Actual, [string]$Message) {
    if ($Expected -cne $Actual) {
        throw "$Message. Expected '$Expected', found '$Actual'."
    }
}

$root = Split-Path -Parent $PSScriptRoot
$manifest = Get-Content -LiteralPath (Join-Path $root 'host-plugin/plugin.json') -Raw |
    ConvertFrom-Json

Assert-Equal 5 $manifest.schemaVersion 'Rust Host must use plugin manifest schema v5'
Assert-Equal 'dev.hmclce.runtime.rust-host' $manifest.id 'Rust Host compatibility ID changed'
Assert-Equal 'Aura Rust Runtime Host' $manifest.name 'Rust Host display identity is not Aura'
Assert-Equal '0.2.0-beta.1' $manifest.version 'Rust Host release version is inconsistent'
Assert-Equal '>=27.1-0-next' $manifest.launcherVersion 'Rust Host launcher constraint is inconsistent'

$build = Get-Content -LiteralPath (Join-Path $root 'host-plugin/build.gradle.kts') -Raw
if ($build -cnotmatch 'dev\.hmclce\.runtime\.rust-host-v0\.2\.0-beta\.1\.npl') {
    throw 'Rust Host NPL filename does not contain version 0.2.0-beta.1'
}

$crateManifests = @(
    'crates/hmcl-runtime-abi/Cargo.toml',
    'crates/hmcl-runtime-protocol/Cargo.toml',
    'crates/hmcl-plugin-sdk/Cargo.toml',
    'crates/hmcl-rust-host-native/Cargo.toml',
    'crates/hmcl-rust-host-process/Cargo.toml'
)
foreach ($crateManifest in $crateManifests) {
    $path = Join-Path $root $crateManifest
    $content = Get-Content -LiteralPath $path -Raw
    if ($content -cnotmatch '(?m)^version = "0\.2\.0-beta\.1"$') {
        throw "Rust crate version is inconsistent: $path"
    }
}

Write-Host 'Rust release metadata tests passed.'
