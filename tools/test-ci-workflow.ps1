$ErrorActionPreference = 'Stop'

function Assert-Condition([bool]$Condition, [string]$Message) {
    if (-not $Condition) {
        throw $Message
    }
}

function Assert-WorkflowPolicy([string]$Workflow) {
    $required = @(
        "- 'examples/launch-hook/**'",
        'AURA_COMMIT: 636b06aad03c5d21946369c836280c891c13054d',
        'AURA_RUN_ID: "33931508945"',
        'AURA_JAR_SHA256: 674f717f5f97a5b7e8f7f20e4d60aa2e25451d71a96ab475f4595d0482f99d4b',
        'SDK_COMMIT: a36719a890ed69d5db6dc126e0ebbb107a9d073b',
        'cargo build --manifest-path examples/launch-hook/Cargo.toml --release',
        './tools/test-package-rust-launch-hook.ps1',
        './tools/test-package-rust-launch-hook.ps1 -Platform ''${{ matrix.platform }}'' -Library $env:AURA_RUST_LAUNCH_HOOK',
        './tools/test-verify-rust-host-artifacts.ps1',
        './tools/test-ci-native-failure.ps1',
        './tools/test-ci-workflow.ps1',
        'go run github.com/rhysd/actionlint/cmd/actionlint@v1.7.12',
        'go run github.com/zricethezav/gitleaks/v8@v8.30.1 detect --source . --no-banner --redact --no-git',
        './tools/verify-rust-host-artifacts.ps1',
        'Rust Host package byte verification failed',
        'HMCL_RUST_EMBEDDED_FIXTURE',
        'crates/hmcl-rust-host-native/tests/fixtures/valid/Cargo.toml',
        'Downloaded Host manifest is invalid',
        'Downloaded Host native entries are invalid',
        'pwsh -NoProfile -NonInteractive -File ./tools/test-ci-workflow.ps1',
        'pwsh -NoProfile -NonInteractive -File ./tools/test-ci-native-failure.ps1',
        '-ActualPlatform ''${{ matrix.platform }}'''
    )
    foreach ($requiredText in $required) {
        Assert-Condition ($Workflow.Contains($requiredText)) "Rust CI is missing required gate: $requiredText"
    }
    Assert-Condition ($Workflow -match '(?m)(?<![A-Z0-9_])AURA_RUST_LAUNCH_HOOK(?![A-Z0-9_])') `
        'Rust CI is missing the mandatory launch-hook input'
    foreach ($nativeInput in @('HMCL_RUST_NATIVE_LIBRARY', 'HMCL_RUST_PROCESS_HOST', 'HMCL_RUST_PLATFORM', 'HMCL_RUST_EMBEDDED_FIXTURE')) {
        Assert-Condition ($Workflow -match "(?m)(?<![A-Z0-9_])$nativeInput(?![A-Z0-9_])") `
            "Rust CI is missing the mandatory native input: $nativeInput"
    }
    Assert-Condition (-not $Workflow.Contains('continue-on-error')) 'Rust CI must not tolerate failed gates'
    Assert-Condition (-not $Workflow.Contains('AURA_HEAD_SHA:')) 'Rust CI still uses obsolete Aura provenance names'
    Assert-Condition (-not $Workflow.Contains('SDK_SHA:')) 'Rust CI still uses obsolete SDK provenance names'
    Assert-Condition ($Workflow -match 'if \(\$LASTEXITCODE -ne 0\) \{ throw') `
        'Rust CI native commands must check their exit codes'
}

$root = Split-Path -Parent $PSScriptRoot
$workflowPath = Join-Path $root '.github/workflows/ci.yml'
$workflow = Get-Content -LiteralPath $workflowPath -Raw
Assert-WorkflowPolicy $workflow

$mutations = [ordered]@{
    'Aura commit' = @('636b06aad03c5d21946369c836280c891c13054d', '0' * 40)
    'Aura JAR hash' = @('674f717f5f97a5b7e8f7f20e4d60aa2e25451d71a96ab475f4595d0482f99d4b', '0' * 64)
    'SDK commit' = @('a36719a890ed69d5db6dc126e0ebbb107a9d073b', '0' * 40)
    'launch-hook input' = @('AURA_RUST_LAUNCH_HOOK', 'AURA_RUST_LAUNCH_HOOK_REMOVED')
    'native library input' = @('HMCL_RUST_NATIVE_LIBRARY', 'HMCL_RUST_NATIVE_LIBRARY_REMOVED')
    'process Host input' = @('HMCL_RUST_PROCESS_HOST', 'HMCL_RUST_PROCESS_HOST_REMOVED')
    'platform input' = @('HMCL_RUST_PLATFORM', 'HMCL_RUST_PLATFORM_REMOVED')
    'sample build' = @('cargo build --manifest-path examples/launch-hook/Cargo.toml --release', 'cargo build --release')
    'launch-hook package platform propagation removed' = @(
        './tools/test-package-rust-launch-hook.ps1 -Platform ''${{ matrix.platform }}'' -Library',
        './tools/test-package-rust-launch-hook.ps1 -Library')
    'launch-hook package platform propagation substituted' = @(
        './tools/test-package-rust-launch-hook.ps1 -Platform ''${{ matrix.platform }}'' -Library',
        './tools/test-package-rust-launch-hook.ps1 -Platform ''windows-x64'' -Library')
    'artifact byte verification' = @('./tools/verify-rust-host-artifacts.ps1', './tools/verify-rust-host-artifacts-removed.ps1')
    'manifest feature verification' = @('./tools/test-verify-rust-host-artifacts.ps1', './tools/test-verify-rust-host-artifacts-removed.ps1')
    'embedded fixture input' = @('HMCL_RUST_EMBEDDED_FIXTURE', 'HMCL_RUST_EMBEDDED_FIXTURE_REMOVED')
    'downloaded Host manifest verification' = @('Downloaded Host manifest is invalid', 'Downloaded Host manifest unchecked')
    'native failure handling' = @('if ($LASTEXITCODE -ne 0) { throw', 'if ($LASTEXITCODE -ne 0) { Write-Warning')
}
foreach ($name in $mutations.Keys) {
    $mutation = $mutations[$name]
    $mutated = $workflow.Replace($mutation[0], $mutation[1])
    $rejected = $false
    try {
        Assert-WorkflowPolicy $mutated
    } catch {
        $rejected = $true
    }
    Assert-Condition $rejected "Workflow mutation was accepted: $name"
}

Write-Host 'Rust CI workflow mutation tests passed.'
