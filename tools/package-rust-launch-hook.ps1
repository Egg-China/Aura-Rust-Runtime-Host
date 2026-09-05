[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('windows-x64', 'windows-arm64', 'linux-x64', 'linux-arm64', 'macos-x64', 'macos-arm64')]
    [string]$Platform,
    [Parameter(Mandatory = $true)]
    [string]$Library,
    [Parameter(Mandatory = $true)]
    [string]$Output
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
$sourceManifest = Join-Path $repositoryRoot 'examples/launch-hook/plugin.json'
Assert-Condition (Test-Path -LiteralPath $sourceManifest -PathType Leaf) "Launch-hook manifest does not exist: $sourceManifest"
Assert-Condition (Test-Path -LiteralPath $Library -PathType Leaf) "Launch-hook library does not exist: $Library"

$expectedName = if ($Platform.StartsWith('windows-')) {
    'hmcl_rust_launch_hook.dll'
} elseif ($Platform.StartsWith('linux-')) {
    'libhmcl_rust_launch_hook.so'
} else {
    'libhmcl_rust_launch_hook.dylib'
}
$resolvedLibrary = (Resolve-Path -LiteralPath $Library).Path
Assert-Condition ((Split-Path -Leaf $resolvedLibrary) -ceq $expectedName) `
    "Launch-hook library for $Platform must be named $expectedName"

$manifest = Get-Content -LiteralPath $sourceManifest -Raw | ConvertFrom-Json
Assert-Condition ($manifest.schemaVersion -eq 5 -and $manifest.runtime -ceq 'rust' -and $manifest.abi -eq 1) `
    'Launch-hook source manifest must declare schema-v5 Rust ABI 1'
$manifest.platforms = @($Platform)
$manifest.entrypoint = "payload/$expectedName"
$manifestJson = $manifest | ConvertTo-Json -Depth 8

$outputPath = [System.IO.Path]::GetFullPath($Output)
$outputParent = Split-Path -Parent $outputPath
[void](New-Item -ItemType Directory -Path $outputParent -Force)
if (Test-Path -LiteralPath $outputPath) {
    Remove-Item -LiteralPath $outputPath -Force
}
$archive = [System.IO.Compression.ZipFile]::Open($outputPath, [System.IO.Compression.ZipArchiveMode]::Create)
try {
    $timestamp = [System.DateTimeOffset]::new(1980, 1, 1, 0, 0, 0, [System.TimeSpan]::Zero)
    $entries = [ordered]@{
        "payload/$expectedName" = $resolvedLibrary
        'plugin.json' = $null
    }
    foreach ($entryName in @($entries.Keys | Sort-Object)) {
        $entry = $archive.CreateEntry($entryName, [System.IO.Compression.CompressionLevel]::Optimal)
        $entry.LastWriteTime = $timestamp
        $stream = $entry.Open()
        try {
            if ($entryName -ceq 'plugin.json') {
                $writer = [System.IO.StreamWriter]::new($stream, [System.Text.UTF8Encoding]::new($false), 1024, $true)
                try {
                    $writer.Write($manifestJson + "`n")
                } finally {
                    $writer.Dispose()
                }
            } else {
                $input = [System.IO.File]::OpenRead($entries[$entryName])
                try {
                    $input.CopyTo($stream)
                } finally {
                    $input.Dispose()
                }
            }
        } finally {
            $stream.Dispose()
        }
    }
} finally {
    $archive.Dispose()
}

Write-Output $outputPath
