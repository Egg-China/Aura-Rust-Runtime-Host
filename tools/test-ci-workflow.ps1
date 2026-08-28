$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent $PSScriptRoot
$workflow = Get-Content -LiteralPath (Join-Path $root '.github/workflows/ci.yml') -Raw

$required = @(
    "- '.github/workflows/ci.yml'",
    'AURA_HEAD_SHA: 2ba6c34437fef289b11c40f45c87221578e65ef0',
    'AURA_RUN_ID: 33098318102',
    'SDK_SHA: 05d687fb8bd8a9683113c5e4317e9aca2b6d7112',
    'HOST_VERSION: 0.1.0-beta.1',
    './.ci/sdk/tools/validate-npl.ps1'
)
foreach ($text in $required) {
    if (-not $workflow.Contains($text)) {
        throw "Rust CI is missing required pinned content: $text"
    }
}
if ($workflow.Contains('runtime-hosts/rust/')) {
    throw 'Rust CI still contains pre-migration SDK paths'
}

Write-Host 'Rust CI workflow tests passed.'
