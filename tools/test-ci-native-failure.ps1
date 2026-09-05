# Exercises every guarded native CI command in a child PowerShell process.
[CmdletBinding()]
param()

$ErrorActionPreference = 'Stop'

function Assert-Condition([bool]$Condition, [string]$Message) {
    if (-not $Condition) {
        throw $Message
    }
}

function Get-StepBody([string]$Workflow, [string]$StepName) {
    $start = $Workflow.IndexOf("      - name: $StepName", [System.StringComparison]::Ordinal)
    Assert-Condition ($start -ge 0) "CI native step is missing: $StepName"
    $end = $Workflow.IndexOf("`n      - name:", $start + 1, [System.StringComparison]::Ordinal)
    if ($end -lt 0) {
        $end = $Workflow.Length
    }
    $step = $Workflow.Substring($start, $end - $start)
    $match = [regex]::Match($step, '(?ms)^        run: \|\r?\n(?<body>.*)$')
    Assert-Condition $match.Success "CI native step has no PowerShell block: $StepName"
    return [regex]::Replace($match.Groups['body'].Value, '(?m)^          ', '')
}

function Get-GuardedCommands([string]$Body, [string]$StepName) {
    $nativeCommands = [regex]::Matches(
        $Body,
        '(?m)^\s*(?:\$[A-Za-z0-9_]+\s*=\s*)?(?<command>(?:cargo|rustup|rustc|gh|gradle|pwsh) .+|& \./(?:tools|\.ci/sdk)/.+)'
    )
    $matches = [regex]::Matches(
        $Body,
        "(?m)^\s*(?:\$[A-Za-z0-9_]+\s*=\s*)?(?<command>(?:cargo|rustup|rustc|gh|gradle|pwsh) .+|& \./(?:tools|\.ci/sdk)/.+)`r?`n\s*(?<guard>if \(\`$LASTEXITCODE -ne 0\) \{ throw .+\})"
    )
    Assert-Condition ($matches.Count -gt 0) "CI native step has no guarded commands: $StepName"
    foreach ($nativeCommand in $nativeCommands) {
        $guarded = @($matches | Where-Object { $_.Groups['command'].Index -eq $nativeCommand.Groups['command'].Index })
        Assert-Condition ($guarded.Count -eq 1) "CI native command lacks an immediate failure guard: ${StepName}: $($nativeCommand.Value.Trim())"
    }
    return @($matches)
}

$root = Split-Path -Parent $PSScriptRoot
$workflow = Get-Content -LiteralPath (Join-Path $root '.github/workflows/ci.yml') -Raw
$steps = @(
    'Verify successful private Aura run',
    'Set up Rust toolchain',
    'Check Rust and packaging gates',
    'Build native Host and launch-hook sample',
    'Download and verify exact Aura Next JAR',
    'Test deterministic launch-hook package',
    'Run Java native Hook and Patch integration tests',
    'Package and validate native Host and sample NPLs',
    'Download every platform artifact'
)

if ($env:OS -eq 'Windows_NT' -and $env:AURA_CI_FAILURE_NO_COMSPEC -ne '1') {
    $shell = (Get-Process -Id $PID).Path
    $savedComSpec = $env:ComSpec
    $savedNoComSpec = $env:AURA_CI_FAILURE_NO_COMSPEC
    try {
        $env:ComSpec = $null
        $env:AURA_CI_FAILURE_NO_COMSPEC = '1'
        & $shell -NoProfile -NonInteractive -File $PSCommandPath
        $childExitCode = $LASTEXITCODE
        Assert-Condition ($childExitCode -eq 0) 'Native CI failure harness requires no ComSpec on Windows'
    } finally {
        $env:ComSpec = $savedComSpec
        $env:AURA_CI_FAILURE_NO_COMSPEC = $savedNoComSpec
    }
    Write-Host 'Native CI failure harness passed with ComSpec absent from the child process.'
    return
}
$cases = foreach ($stepName in $steps) {
    $body = Get-StepBody $workflow $stepName
    $guards = Get-GuardedCommands $body $stepName
    for ($failureIndex = 1; $failureIndex -le $guards.Count; $failureIndex++) {
        [pscustomobject]@{ step = $stepName; guards = $guards; failureIndex = $failureIndex }
    }
}

foreach ($case in $cases) {
    $commands = [System.Text.StringBuilder]::new()
    foreach ($guard in $case.guards) {
        [void]$commands.AppendLine("Invoke-InjectedNative '$($guard.Groups['command'].Value.Replace("'", "''"))'")
        [void]$commands.AppendLine($guard.Groups['guard'].Value)
    }
    $child = @"
`$ErrorActionPreference = 'Stop'
`$script:invocations = 0
function Invoke-InjectedNative([string]`$Command) {
    `$script:invocations++
    if (`$script:invocations -eq $($case.failureIndex)) {
        Write-Output "INJECTED:`$Command"
        `$shell = (Get-Process -Id `$PID).Path
        & `$shell -NoProfile -NonInteractive -Command 'exit 23'
        return
    }
    if (`$script:invocations -gt $($case.failureIndex)) { Write-Output "LATER:`$Command" }
    `$shell = (Get-Process -Id `$PID).Path
    & `$shell -NoProfile -NonInteractive -Command 'exit 0'
}
$($commands.ToString())
Write-Output 'COMPLETED'
"@
    $encoded = [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($child))
    $savedErrorActionPreference = $ErrorActionPreference
    try {
        $ErrorActionPreference = 'Continue'
        $output = & (Get-Process -Id $PID).Path -NoProfile -NonInteractive -EncodedCommand $encoded 2>&1
        $exitCode = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $savedErrorActionPreference
    }
    $label = "$($case.step) command $($case.failureIndex)"
    Assert-Condition ($exitCode -eq 1) "$label did not fail through its immediate guard"
    Assert-Condition (($output -join "`n") -like 'INJECTED:*') "$label did not inject a native failure"
    Assert-Condition (($output -join "`n") -notlike '*LATER:*') "$label ran a later native command"
    Assert-Condition (($output -join "`n") -notlike '*COMPLETED*') "$label continued after native failure"
}

Write-Host "Native CI failure propagation passed for $($cases.Count) guarded command cases."
