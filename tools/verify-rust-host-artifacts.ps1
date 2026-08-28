param(
    [Parameter(Mandatory = $true)]
    [string]$Platform,
    [Parameter(Mandatory = $true)]
    [string]$NativeLibrary,
    [Parameter(Mandatory = $true)]
    [string]$ProcessHost,
    [Parameter(Mandatory = $true)]
    [string]$Package,
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

function Get-StreamSha256([System.IO.Stream]$Stream) {
    $algorithm = [System.Security.Cryptography.SHA256]::Create()
    try {
        return ([System.BitConverter]::ToString($algorithm.ComputeHash($Stream))).Replace('-', '').ToLowerInvariant()
    } finally {
        $algorithm.Dispose()
    }
}

$artifactsByPlatform = @{
    'windows-x64' = @('hmcl_rust_host_native.dll', 'hmcl-rust-host-process.exe')
    'windows-arm64' = @('hmcl_rust_host_native.dll', 'hmcl-rust-host-process.exe')
    'linux-x64' = @('libhmcl_rust_host_native.so', 'hmcl-rust-host-process')
    'linux-arm64' = @('libhmcl_rust_host_native.so', 'hmcl-rust-host-process')
    'macos-x64' = @('libhmcl_rust_host_native.dylib', 'hmcl-rust-host-process')
    'macos-arm64' = @('libhmcl_rust_host_native.dylib', 'hmcl-rust-host-process')
}
Assert-Condition ($artifactsByPlatform.ContainsKey($Platform)) "Unsupported Rust Host platform: $Platform"
Assert-Condition (Test-Path -LiteralPath $NativeLibrary -PathType Leaf) `
    "Native library does not exist: $NativeLibrary"
Assert-Condition (Test-Path -LiteralPath $ProcessHost -PathType Leaf) `
    "Process Host does not exist: $ProcessHost"
Assert-Condition (Test-Path -LiteralPath $Package -PathType Leaf) "NPL package does not exist: $Package"

$resolvedNative = (Resolve-Path -LiteralPath $NativeLibrary).Path
$resolvedProcess = (Resolve-Path -LiteralPath $ProcessHost).Path
$resolvedPackage = (Resolve-Path -LiteralPath $Package).Path
$expectedNativeName = $artifactsByPlatform[$Platform][0]
$expectedProcessName = $artifactsByPlatform[$Platform][1]
$nativeName = Split-Path -Leaf $resolvedNative
$processName = Split-Path -Leaf $resolvedProcess
Assert-Condition ($nativeName -ceq $expectedNativeName) `
    "Native library for $Platform must be named $expectedNativeName"
Assert-Condition ($processName -ceq $expectedProcessName) `
    "Process Host for $Platform must be named $expectedProcessName"
Assert-Condition ($resolvedPackage.EndsWith('.npl', [System.StringComparison]::OrdinalIgnoreCase)) `
    'Rust Host package must use the .npl extension'

$archive = [System.IO.Compression.ZipFile]::OpenRead($resolvedPackage)
try {
    $nativePlatforms = [System.Collections.Generic.HashSet[string]]::new(
        [System.StringComparer]::Ordinal
    )
    foreach ($entry in $archive.Entries) {
        Assert-Condition (-not $entry.FullName.Contains('\')) "Unsafe NPL entry path: $($entry.FullName)"
        if ($entry.FullName -cmatch '^native/([^/]+)/' -and -not $entry.FullName.EndsWith('/')) {
            [void]$nativePlatforms.Add($Matches[1])
        }
    }
    Assert-Condition ($nativePlatforms.Count -eq 1 -and $nativePlatforms.Contains($Platform)) `
        'NPL must contain exactly one native platform output matching -Platform'

    $nativeEntryPath = "native/$Platform/$expectedNativeName"
    $processEntryPath = "native/$Platform/$expectedProcessName"
    $nativeEntry = $archive.GetEntry($nativeEntryPath)
    $processEntry = $archive.GetEntry($processEntryPath)
    Assert-Condition ($null -ne $nativeEntry -and -not $nativeEntry.FullName.EndsWith('/')) `
        "NPL is missing native library entry: $nativeEntryPath"
    Assert-Condition ($null -ne $processEntry -and -not $processEntry.FullName.EndsWith('/')) `
        "NPL is missing process Host entry: $processEntryPath"

    $nativeStream = $nativeEntry.Open()
    try {
        $packagedNativeHash = Get-StreamSha256 $nativeStream
    } finally {
        $nativeStream.Dispose()
    }
    $processStream = $processEntry.Open()
    try {
        $packagedProcessHash = Get-StreamSha256 $processStream
    } finally {
        $processStream.Dispose()
    }
} finally {
    $archive.Dispose()
}

$nativeHash = (Get-FileHash -LiteralPath $resolvedNative -Algorithm SHA256).Hash.ToLowerInvariant()
$processHash = (Get-FileHash -LiteralPath $resolvedProcess -Algorithm SHA256).Hash.ToLowerInvariant()
Assert-Condition ($packagedNativeHash -ceq $nativeHash) `
    'NPL native library bytes do not match the input artifact'
Assert-Condition ($packagedProcessHash -ceq $processHash) `
    'NPL process Host bytes do not match the input artifact'

$record = [pscustomobject][ordered]@{
    platform = $Platform
    package = Split-Path -Leaf $resolvedPackage
    sha256 = (Get-FileHash -LiteralPath $resolvedPackage -Algorithm SHA256).Hash.ToLowerInvariant()
    size = (Get-Item -LiteralPath $resolvedPackage).Length
    nativeLibrary = $expectedNativeName
    processHost = $expectedProcessName
}
$json = $record | ConvertTo-Json -Depth 5
if (-not [string]::IsNullOrWhiteSpace($Output)) {
    $outputPath = [System.IO.Path]::GetFullPath($Output)
    $outputParent = Split-Path -Parent $outputPath
    if (-not [string]::IsNullOrWhiteSpace($outputParent)) {
        [void](New-Item -ItemType Directory -Path $outputParent -Force)
    }
    [System.IO.File]::WriteAllText(
        $outputPath,
        $json + "`n",
        [System.Text.UTF8Encoding]::new($false)
    )
}
Write-Output $json
