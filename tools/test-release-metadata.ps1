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
    if ($content -cnotmatch '(?m)^version = "0\.2\.0-beta\.1"\r?$') {
        throw "Rust crate version is inconsistent: $path"
    }
}

$rootManifest = Get-Content -LiteralPath (Join-Path $root 'manifest.json') -Raw |
    ConvertFrom-Json
Assert-Equal 2 $rootManifest.schemaVersion 'Rust root manifest schema changed'
Assert-Equal 2 $rootManifest.versions.Count 'Rust root manifest must retain 0.1 and add 0.2'

$version01 = $rootManifest.versions | Where-Object version -CEQ '0.1.0-beta.1'
$version02 = $rootManifest.versions | Where-Object version -CEQ '0.2.0-beta.1'
Assert-Equal '>=26.8-0-next' $version01.launcherVersion 'Rust 0.1 Aura compatibility changed'
Assert-Equal '>=27.1-0-next' $version02.launcherVersion 'Rust 0.2 Aura compatibility is inconsistent'
Assert-Equal 6 $version02.artifacts.Count 'Rust 0.2 must publish all six Aura platforms'

$expectedArtifacts = @{
    'linux-arm64' = @('ff248b1904d9f8861ec038778e7eff64806c1c948a5d56f57087ee420beb7806', 536386)
    'linux-x64' = @('9c991e60f6769f7e08c33ac4d91cbd399b5204024c770e996c780078d490394c', 548707)
    'macos-arm64' = @('ce3bc32de53ab71fdeaed1e9fba4514eb62f55d8f24d046434008744f27cfb07', 483994)
    'macos-x64' = @('30dbe500d48ceb978232248e41afc34e96452f51b998d0baf10fb3b0d5b75357', 495808)
    'windows-arm64' = @('cb85988c47c013729302249701f39c0a27e22e1bba67635df38ba7c33500caa9', 287617)
    'windows-x64' = @('2d495681d70e5ce00b9a9e2947add3bcb423974d2b5d499a52eafea4f95a195d', 298827)
}
foreach ($platform in $expectedArtifacts.Keys) {
    $artifact = $version02.artifacts | Where-Object platform -CEQ $platform
    Assert-Equal 1 @($artifact).Count "Rust 0.2 artifact is missing or duplicated: $platform"
    Assert-Equal $expectedArtifacts[$platform][0] $artifact.sha256 "Rust 0.2 hash is inconsistent: $platform"
    Assert-Equal $expectedArtifacts[$platform][1] $artifact.size "Rust 0.2 size is inconsistent: $platform"
    $expectedUrl = "https://github.com/Egg-China/Aura-Rust-Runtime-Host/releases/download/v0.2.0-beta.1/dev.hmclce.runtime.rust-host-v0.2.0-beta.1-$platform.npl"
    Assert-Equal $expectedUrl $artifact.packageUrl "Rust 0.2 URL is inconsistent: $platform"
}

Write-Host 'Rust release metadata tests passed.'
