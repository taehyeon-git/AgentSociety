#Requires -Version 7.2
<#
Verify the existing release CLI sequentially without building or benchmarking.
The supplied branch scenario starts after tick 500, so the workload is fixed at
1000 ticks (500 + 500 for resume/branch). Relative output paths use the repo root.
Existing output directories are refused; every invocation preserves its evidence.
#>
param(
    [ValidateRange(1, 100000)][int]$Agents = 100,
    [UInt64]$Seed = 42,
    [string]$OutputDirectory = ('runs\release-verification-{0}' -f (Get-Date -Format 'yyyyMMdd-HHmmss-fff'))
)

$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$executableName = if ($env:OS -eq 'Windows_NT') { 'agentsociety-cli.exe' } else { 'agentsociety-cli' }
$executable = Join-Path $projectRoot "target/release/$executableName"
$config = Join-Path $projectRoot 'examples/complete.json'
$branchScenario = Join-Path $projectRoot 'examples/scenarios/branch-after-500.json'
$crisisScenario = Join-Path $projectRoot 'examples/scenarios/crisis.json'
$redistributionScenario = Join-Path $projectRoot 'examples/scenarios/redistribution.json'
$inputs = @($executable, $config, $branchScenario, $crisisScenario, $redistributionScenario)
foreach ($inputPath in $inputs) {
    if (-not (Test-Path -LiteralPath $inputPath -PathType Leaf)) {
        throw "Required input missing: $inputPath. Build the release CLI before running this script."
    }
}
if (-not [System.IO.Path]::IsPathRooted($OutputDirectory)) {
    $OutputDirectory = Join-Path $projectRoot $OutputDirectory
}
$OutputDirectory = [System.IO.Path]::GetFullPath($OutputDirectory)
if (Test-Path -LiteralPath $OutputDirectory) {
    throw "Output directory already exists; choose a fresh directory: $OutputDirectory"
}
New-Item -ItemType Directory -Path $OutputDirectory | Out-Null
$commands = [System.Collections.Generic.List[object]]::new()

function Output-Path([string]$Name) { Join-Path $OutputDirectory $Name }
function Read-Json([string]$Name) { Get-Content -LiteralPath (Output-Path $Name) -Raw | ConvertFrom-Json }
function Write-Json([object]$Value, [string]$Name) {
    ConvertTo-Json -InputObject $Value -Depth 100 | Set-Content -LiteralPath (Output-Path $Name) -Encoding utf8
}
function File-Hash([string]$Path) { (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant() }
function Assert-Condition([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw $Message }
}
function Invoke-Cli([string]$Name, [string[]]$CliArguments) {
    # Capture native failures ourselves even when the caller enables this preference.
    $PSNativeCommandUseErrorActionPreference = $false
    Write-Host "Verifying $Name..."
    $entry = [pscustomobject]@{
        name = $Name
        executable = $executable
        arguments = $CliArguments
        started_utc = [DateTime]::UtcNow.ToString('o')
        exit_code = $null
    }
    $commands.Add($entry)
    Write-Json @($commands.ToArray()) 'commands.json'
    & $executable @CliArguments 1> (Output-Path "$Name.stdout.log") 2> (Output-Path "$Name.stderr.log")
    $entry.exit_code = $LASTEXITCODE
    Write-Json @($commands.ToArray()) 'commands.json'
    if ($entry.exit_code -ne 0) {
        throw "CLI stage '$Name' failed with exit code $($entry.exit_code). See $Name.stderr.log."
    }
}

try {
    $inputHashes = @($inputs | ForEach-Object { [pscustomobject]@{ path = $_; sha256 = File-Hash $_ } })
    Write-Json $inputHashes 'input-hashes.json'
    $newRun = @('--config', $config, '--agents', "$Agents", '--seed', "$Seed", '--report-every', '0', '--json')
    $continueRun = @('--ticks', '500', '--report-every', '0', '--json')

    Invoke-Cli 'continuous' (@('run') + $newRun + @('--ticks', '1000',
        '--events', (Output-Path 'continuous.jsonl'), '--state-out', (Output-Path 'continuous-state.json'),
        '--summary-out', (Output-Path 'continuous-summary.json'), '--csv', (Output-Path 'continuous-metrics.csv')))
    Invoke-Cli 'half' (@('run') + $newRun + @('--ticks', '500',
        '--checkpoint-out', (Output-Path 'half-checkpoint.json'), '--database', (Output-Path 'half-source.sqlite'),
        '--state-out', (Output-Path 'half-state.json'), '--summary-out', (Output-Path 'half-summary.json')))
    $sourceHashes = [ordered]@{
        checkpoint = File-Hash (Output-Path 'half-checkpoint.json')
        sqlite = File-Hash (Output-Path 'half-source.sqlite')
    }
    # The CLI has exited, closing its WAL connection before this independent copy.
    Assert-Condition (-not (Test-Path -LiteralPath (Output-Path 'half-source.sqlite-wal'))) 'Source SQLite WAL is still present.'
    Copy-Item -LiteralPath (Output-Path 'half-source.sqlite') -Destination (Output-Path 'resumed.sqlite')
    Invoke-Cli 'resume-file' (@('resume', (Output-Path 'half-checkpoint.json')) + $continueRun + @(
        '--state-out', (Output-Path 'resume-file-state.json'), '--summary-out', (Output-Path 'resume-file-summary.json')))
    Invoke-Cli 'resume-sqlite' (@('resume', (Output-Path 'resumed.sqlite')) + $continueRun + @(
        '--state-out', (Output-Path 'resume-sqlite-state.json'), '--summary-out', (Output-Path 'resume-sqlite-summary.json')))
    Invoke-Cli 'replay-jsonl' @('replay', (Output-Path 'continuous.jsonl'), '--json',
        '--state-out', (Output-Path 'replay-jsonl-state.json'), '--summary-out', (Output-Path 'replay-jsonl-summary.json'))
    Invoke-Cli 'replay-sqlite' @('replay', (Output-Path 'resumed.sqlite'), '--json',
        '--state-out', (Output-Path 'replay-sqlite-state.json'), '--summary-out', (Output-Path 'replay-sqlite-summary.json'))

    $stateFiles = @('continuous-state.json', 'resume-file-state.json', 'resume-sqlite-state.json', 'replay-jsonl-state.json', 'replay-sqlite-state.json')
    $stateHashes = @($stateFiles | ForEach-Object { [pscustomobject]@{ file = $_; sha256 = File-Hash (Output-Path $_) } })
    Assert-Condition (@($stateHashes.sha256 | Select-Object -Unique).Count -eq 1) 'Continuous/resume/replay state file SHA-256 mismatch.'
    $jsonlReplay = Read-Json 'replay-jsonl-summary.json'
    $sqliteReplay = Read-Json 'replay-sqlite-summary.json'
    Assert-Condition ($jsonlReplay.state_sha256 -eq $sqliteReplay.state_sha256) 'Replay compact state SHA-256 mismatch.'
    foreach ($replay in @($jsonlReplay, $sqliteReplay)) {
        Assert-Condition ($replay.final_tick -eq 1000 -and $replay.replayed_ticks -eq 1000 -and $replay.verified_events -gt 0) 'Replay did not verify the full 1000 ticks.'
    }

    Invoke-Cli 'branch' (@('branch', (Output-Path 'half-checkpoint.json'), '--scenario', $branchScenario) + $continueRun + @(
        '--checkpoint-out', (Output-Path 'branch-checkpoint.json'), '--database', (Output-Path 'branch.sqlite'),
        '--events', (Output-Path 'branch.jsonl'), '--state-out', (Output-Path 'branch-state.json'),
        '--summary-out', (Output-Path 'branch-summary.json')))
    Invoke-Cli 'branch-replay-jsonl' @('replay', (Output-Path 'branch.jsonl'), '--json',
        '--state-out', (Output-Path 'branch-replay-jsonl-state.json'), '--summary-out', (Output-Path 'branch-replay-jsonl-summary.json'))
    Invoke-Cli 'branch-replay-sqlite' @('replay', (Output-Path 'branch.sqlite'), '--json',
        '--state-out', (Output-Path 'branch-replay-sqlite-state.json'), '--summary-out', (Output-Path 'branch-replay-sqlite-summary.json'))
    $branchFiles = @('branch-state.json', 'branch-replay-jsonl-state.json', 'branch-replay-sqlite-state.json')
    $branchHashes = @($branchFiles | ForEach-Object { [pscustomobject]@{ file = $_; sha256 = File-Hash (Output-Path $_) } })
    Assert-Condition (@($branchHashes.sha256 | Select-Object -Unique).Count -eq 1) 'Branch/replay state file SHA-256 mismatch.'
    Assert-Condition ($branchHashes[0].sha256 -ne $stateHashes[0].sha256) 'Branch unexpectedly matches the unchanged schedule.'
    $branchJsonl = Read-Json 'branch-replay-jsonl-summary.json'
    $branchSqlite = Read-Json 'branch-replay-sqlite-summary.json'
    Assert-Condition ($branchJsonl.state_sha256 -eq $branchSqlite.state_sha256) 'Branch replay compact state SHA-256 mismatch.'
    foreach ($replay in @($branchJsonl, $branchSqlite)) {
        Assert-Condition ($replay.final_tick -eq 1000 -and $replay.replayed_ticks -eq 500 -and $replay.verified_events -gt 0) 'Branch replay did not verify exactly 500 new ticks.'
    }
    Assert-Condition ($sourceHashes.checkpoint -eq (File-Hash (Output-Path 'half-checkpoint.json'))) 'Source checkpoint changed.'
    Assert-Condition ($sourceHashes.sqlite -eq (File-Hash (Output-Path 'half-source.sqlite'))) 'Source SQLite changed.'

    Invoke-Cli 'matrix' @('experiment', '--config', $config, '--scenarios', "$crisisScenario,$redistributionScenario",
        '--include-baseline', '--seeds', '42,43,44', '--populations', "$Agents", '--policies', 'rule-based',
        '--ticks', '1000', '--out-dir', (Output-Path 'matrix'), '--json')
    $matrix = Read-Json 'matrix/report.json'
    Assert-Condition ($matrix.completed -and $matrix.requested_runs -eq 9 -and @($matrix.results).Count -eq 9) 'The nine-run matrix is incomplete.'
    $expectedScenarios = @('baseline', (Get-Content -LiteralPath $crisisScenario -Raw | ConvertFrom-Json).name,
        (Get-Content -LiteralPath $redistributionScenario -Raw | ConvertFrom-Json).name)
    foreach ($scenario in $expectedScenarios) {
        foreach ($matrixSeed in @(42, 43, 44)) {
            $matches = @($matrix.results | Where-Object { $_.config.scenario.name -eq $scenario -and $_.config.seed -eq $matrixSeed })
            Assert-Condition ($matches.Count -eq 1) "Missing or duplicate matrix condition: $scenario, seed $matrixSeed."
            $result = $matches[0]
            Assert-Condition ($result.stats.tick -eq 1000 -and $result.stats.agent_count -eq $Agents -and $result.config.policy -eq 'rule_based') 'Unexpected matrix workload.'
            Assert-Condition ($result.state_sha256 -match '^[0-9a-f]{64}$') 'Missing matrix state SHA-256.'
            Assert-Condition (Test-Path -LiteralPath (Output-Path "matrix/$($result.checkpoint_file)") -PathType Leaf) 'Missing matrix checkpoint.'
        }
    }
    Assert-Condition (@(Import-Csv -LiteralPath (Output-Path 'matrix/results.csv')).Count -eq 9) 'Matrix CSV must contain nine rows.'
    foreach ($inputHash in $inputHashes) {
        Assert-Condition ($inputHash.sha256 -eq (File-Hash $inputHash.path)) "Verification input changed: $($inputHash.path)"
    }
    $continuous = Read-Json 'continuous-summary.json'
    $branch = Read-Json 'branch-summary.json'
    $evidence = [ordered]@{
        passed = $true
        verified_utc = [DateTime]::UtcNow.ToString('o')
        engine_version = $continuous.engine_version
        format_version = $continuous.format_version
        agents = $Agents
        ticks = 1000
        seed = $Seed
        script_sha256 = File-Hash $PSCommandPath
        state_file_hashes = $stateHashes
        compact_state_sha256 = $jsonlReplay.state_sha256
        replay = @($jsonlReplay, $sqliteReplay)
        branch_state_file_hashes = $branchHashes
        branch_compact_state_sha256 = $branchJsonl.state_sha256
        branch_replay = @($branchJsonl, $branchSqlite)
        source_unchanged = $true
        source_sha256 = $sourceHashes
        continuous_metrics = @{ stats = $continuous.stats; cumulative = $continuous.cumulative; total_events = $continuous.total_events }
        branch_metrics = @{ stats = $branch.stats; cumulative = $branch.cumulative; total_events = $branch.total_events }
        matrix_completed = $matrix.completed
        matrix_runs = @($matrix.results).Count
        matrix_comparisons = $matrix.comparisons
        commands = @($commands.ToArray())
        note = 'Correctness validation only. Runtime metrics in raw outputs are not benchmark measurements.'
    }
    Write-Json $evidence 'verification.json'
    Write-Host "Release verification passed: $(Output-Path 'verification.json')"
} catch {
    Write-Json ([ordered]@{ passed = $false; error = $_.Exception.Message; commands = @($commands.ToArray()) }) 'failure.json'
    throw
}
