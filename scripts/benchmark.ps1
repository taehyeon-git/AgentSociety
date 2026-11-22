<#
Runs the release executable repeatedly, recording process CPU time and working set.
Stage timings come from the Rust engine; working set is an external sampled measure.
Relative output directories are resolved from the workspace root.
#>
param(
    [int[]]$Population = @(10, 100, 1000),
    [ValidateRange(1, 1000000)][int]$Ticks = 1000,
    [UInt64]$Seed = 42,
    [ValidateRange(1, 30)][int]$Repetitions = 1,
    [string]$Config,
    [switch]$WithEvents,
    [string]$OutputDirectory = 'runs\benchmarks'
)
$ErrorActionPreference = 'Stop'
if ($env:OS -ne 'Windows_NT') { throw 'This memory sampling script requires Windows. The CLI benchmarks run on all supported Rust platforms.' }
$projectRoot = Split-Path -Parent $PSScriptRoot
$executable = Join-Path $projectRoot 'target\release\agentsociety-cli.exe'
if (-not (Test-Path -LiteralPath $executable)) { throw 'Build first: cargo build --workspace --release --locked' }
$configPath = $null
if ($Config) {
    $configPath = if ([System.IO.Path]::IsPathRooted($Config)) { $Config } else { Join-Path $projectRoot $Config }
    if (-not (Test-Path -LiteralPath $configPath -PathType Leaf)) { throw "Configuration file not found: $configPath" }
    $configPath = (Resolve-Path -LiteralPath $configPath).Path
}
if (-not [System.IO.Path]::IsPathRooted($OutputDirectory)) {
    $OutputDirectory = Join-Path $projectRoot $OutputDirectory
}
New-Item -ItemType Directory -Path $outputDirectory -Force | Out-Null
$mode = if ($WithEvents) { 'logged' } else { 'baseline' }
$measurements = foreach ($agents in $Population) {
    if ($agents -lt 0 -or $agents -gt 100000) { throw 'Population must be between 0 and 100000.' }
    for ($repetition = 1; $repetition -le $Repetitions; $repetition++) {
        $runSuffix = if ($Repetitions -eq 1) { "$agents" } else { "$agents-r$repetition" }
        $summaryPath = Join-Path $outputDirectory "$mode-$runSuffix.json"
        $stderrPath = Join-Path $outputDirectory "$mode-$runSuffix.stderr.log"
        $eventPath = Join-Path $outputDirectory "$mode-$runSuffix.events.jsonl"
        $cliArguments = @('--agents', "$agents", '--ticks', "$Ticks", '--seed', "$Seed", '--report-every', '0', '--json')
        if ($configPath) { $cliArguments += @('--config', ('"{0}"' -f $configPath)) }
        if ($WithEvents) { $cliArguments += @('--events', ('"{0}"' -f $eventPath), '--overwrite') }
        $process = Start-Process -FilePath $executable -ArgumentList $cliArguments -WorkingDirectory $projectRoot -WindowStyle Hidden -PassThru -RedirectStandardOutput $summaryPath -RedirectStandardError $stderrPath
        $null = $process.Handle
        $peakBytes = 0L
        $samples = 0
        while (-not $process.HasExited) {
            try {
                $process.Refresh()
                $peakBytes = [Math]::Max($peakBytes, $process.PeakWorkingSet64)
                $samples += 1
            } catch [System.InvalidOperationException] { break }
            Start-Sleep -Milliseconds 10
        }
        $process.WaitForExit()
        if ($process.ExitCode -ne 0) { throw "Benchmark failed for $agents agents: $(Get-Content -LiteralPath $stderrPath -Raw)" }
        $cpuMilliseconds = $null
        try { $cpuMilliseconds = $process.TotalProcessorTime.TotalMilliseconds } catch [System.InvalidOperationException] { }
        $summary = Get-Content -LiteralPath $summaryPath -Raw | ConvertFrom-Json
        [pscustomobject]@{
            agents = $agents
            ticks = $Ticks
            seed = $Seed
            mode = $mode
            repetition = $repetition
            engine_version = $summary.engine_version
            input_config = $configPath
            interactions = $summary.config.interactions
            organizations_config = $summary.config.organizations
            scenario = $summary.config.scenario
            process_cpu_ms = $cpuMilliseconds
            elapsed_ms = $summary.performance.elapsed_ms
            mean_tick_ms = $summary.performance.mean_tick_ms
            max_tick_ms = $summary.performance.max_tick_ms
            mean_market_ms = $summary.performance.mean_market_ms
            event_write_ms = $summary.performance.event_write_ms
            stages_mean_ms = $summary.performance.stages_mean_ms
            agent_ticks_per_second = $summary.performance.agent_ticks_per_second
            sampled_peak_working_set_mib = if ($samples -gt 0) { [Math]::Round($peakBytes / 1MB, 3) } else { $null }
            memory_samples = $samples
            trades = $summary.cumulative.trade_count
            negotiations = $summary.cumulative.negotiations
            negotiated_trades = $summary.cumulative.negotiated_trades
            cooperations = $summary.cumulative.cooperations
            goal_changes = $summary.cumulative.goal_changes
            organizations = $summary.stats.organizations
            group_productions = $summary.cumulative.group_productions
            contract_deliveries = $summary.cumulative.contract_deliveries
            contract_defaults = $summary.cumulative.contract_defaults
            taxes = $summary.cumulative.taxes
            redistributed = $summary.cumulative.redistributed
            treasury_money = $summary.stats.treasury_money
            total_money = $summary.stats.total_money
            events = $summary.total_events
            event_file_bytes = if ($WithEvents) { (Get-Item -LiteralPath $eventPath).Length } else { 0 }
        }
        $process.Dispose()
    }
}
$resultPath = Join-Path $outputDirectory "$mode-measurements.json"
[System.IO.File]::WriteAllText($resultPath, (ConvertTo-Json -InputObject @($measurements) -Depth 10))
$measurements | Format-Table agents, repetition, ticks, elapsed_ms, mean_tick_ms, mean_market_ms, process_cpu_ms, sampled_peak_working_set_mib, trades -AutoSize
Write-Output "Measurements written to $resultPath"
