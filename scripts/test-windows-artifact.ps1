#Requires -Version 7.0
<# Verify extracted source-built release and CI bundles on a native Windows
   test runner. Only owned fixtures are targeted. No desktop-global hook option
   is accepted. The owned Hook suite requires the explicit -RunOwnedFixture. #>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$ReleaseDirectory,
    [Parameter(Mandatory = $true)][string]$TestBundleDirectory,
    [switch]$RunOwnedFixture,
    [ValidateRange(30, 900)][int]$TestTimeoutSeconds = 240,
    [ValidateRange(60, 1800)][int]$HookTimeoutSeconds = 600
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if (-not $IsWindows -or -not [Environment]::Is64BitProcess) {
    throw 'Use native Windows and 64-bit PowerShell 7. No test was run.'
}
if (-not $RunOwnedFixture) {
    throw 'This verification includes targeted Hook tests against newly launched owned fixtures. Pass -RunOwnedFixture explicitly.'
}
$release = (Resolve-Path -LiteralPath $ReleaseDirectory).Path
$bundle = (Resolve-Path -LiteralPath $TestBundleDirectory).Path
$resultsDirectory = Join-Path $bundle 'results'
[void](New-Item -ItemType Directory -Force -Path $resultsDirectory)
$records = [System.Collections.Generic.List[object]]::new()
$manifest = $null
$application = ''
$passed = $false
$failure = $null

function Bundle-Path([string]$Base, [string]$Relative) {
    if ([IO.Path]::IsPathRooted($Relative)) { throw "Expected a relative bundle path: $Relative" }
    $path = [IO.Path]::GetFullPath((Join-Path $Base $Relative))
    $prefix = $Base.TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
    if (-not $path.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Path escapes the extracted bundle: $Relative"
    }
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw "Missing bundle file: $Relative" }
    if ((Get-Item -LiteralPath $path).Attributes -band [IO.FileAttributes]::ReparsePoint) {
        throw "Reparse points are not eligible bundle files: $Relative"
    }
    return $path
}

function Verify-Hash([string]$Base, $Entry) {
    $path = Bundle-Path $Base ([string]$Entry.path)
    if ((Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash -ine [string]$Entry.sha256) {
        throw "SHA256 mismatch: $($Entry.path)"
    }
    return $path
}

function Run-OwnedProcess([string]$Name, [string]$File, [string[]]$Arguments, [int]$TimeoutSeconds) {
    $start = [Diagnostics.ProcessStartInfo]::new($File)
    $start.UseShellExecute = $false
    $start.WorkingDirectory = $release
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $start.StandardOutputEncoding = [Text.UTF8Encoding]::new($false)
    $start.StandardErrorEncoding = [Text.UTF8Encoding]::new($false)
    $start.Environment['CORALSPY_TEST_APP'] = $application
    $start.Environment['CORALSPYNEXT_LANG'] = 'zh-CN'
    foreach ($argument in $Arguments) { $start.ArgumentList.Add($argument) }
    $process = [Diagnostics.Process]::new()
    $process.StartInfo = $start
    $clock = [Diagnostics.Stopwatch]::StartNew()
    $exitCode = $null
    $timedOut = $false
    $stdout = ''
    $stderr = ''
    $started = $false
    $processFailure = $null
    try {
        $started = $process.Start()
        if (-not $started) { throw "Could not start $Name" }
        $stdoutTask = $process.StandardOutput.ReadToEndAsync()
        $stderrTask = $process.StandardError.ReadToEndAsync()
        if (-not $process.WaitForExit($TimeoutSeconds * 1000)) {
            $timedOut = $true
            $process.Kill($true)
            [void]$process.WaitForExit(10000)
        }
        if ($process.HasExited) { $exitCode = $process.ExitCode }
        # Bound output cleanup as well as the process itself. Descendants cannot
        # keep an inherited output pipe open indefinitely after a timeout.
        if ($stdoutTask.Wait(10000)) { $stdout = $stdoutTask.GetAwaiter().GetResult() }
        else { $timedOut = $true; $stdout = '[Output collection exceeded its deadline.]' }
        if ($stderrTask.Wait(10000)) { $stderr = $stderrTask.GetAwaiter().GetResult() }
        else { $timedOut = $true; $stderr = '[Error collection exceeded its deadline.]' }
        if ($timedOut -or $exitCode -ne 0) { throw "$Name failed: exit=$exitCode, timeout=$timedOut" }
    } catch {
        $processFailure = $_.Exception.Message
        throw
    } finally {
        if ($started -and -not $process.HasExited) {
            try {
                $process.Kill($true)
                [void]$process.WaitForExit(10000)
            } catch {
                $cleanupFailure = $_.Exception.Message
                if ($null -eq $processFailure) { $processFailure = "Process cleanup failed: $cleanupFailure" }
                $stderr += "`nProcess cleanup failed: $cleanupFailure"
            }
        }
        $clock.Stop()
        $process.Dispose()
        $safeName = $Name -replace '[^A-Za-z0-9_.-]', '_'
        $stdout | Set-Content -LiteralPath (Join-Path $resultsDirectory "$safeName.stdout.log") -Encoding utf8
        $stderr | Set-Content -LiteralPath (Join-Path $resultsDirectory "$safeName.stderr.log") -Encoding utf8
        if ($stdout) { Write-Host $stdout }
        if ($stderr) { Write-Host $stderr }
        $records.Add([ordered]@{
            name = $Name; exit_code = $exitCode; timed_out = $timedOut
            duration_seconds = [Math]::Round($clock.Elapsed.TotalSeconds, 3)
            passed = ($null -eq $processFailure -and -not $timedOut -and $exitCode -eq 0)
            error = $processFailure
        })
    }
}

try {
    $manifest = Get-Content -LiteralPath (Join-Path $bundle 'manifest.json') -Raw -Encoding utf8 | ConvertFrom-Json
    if ($manifest.schema -ne 1 -or $manifest.target -ne 'x86_64-pc-windows-gnu' -or $manifest.desktop_global_hook -ne $false) {
        throw 'Unexpected CI manifest schema, target, or hook scope.'
    }
    if (@($manifest.test_executables).Count -eq 0) { throw 'No Rust test executables were collected.' }
    foreach ($entry in $manifest.files) { [void](Verify-Hash $bundle $entry) }
    foreach ($entry in $manifest.expected_release_files) { [void](Verify-Hash $release $entry) }
    $buildInfo = Get-Content -LiteralPath (Bundle-Path $release 'BUILD-INFO.json') -Raw -Encoding utf8 | ConvertFrom-Json
    if ($buildInfo.commit -ne $manifest.commit) { throw 'Release and verification bundles come from different commits.' }
    $application = Bundle-Path $release 'coralspynext.exe'
    # Preflight hashes must all pass before any executable starts. Individual
    # fixture failures are then collected, so one defect does not hide unrelated
    # tests; any failure still prevents publishing.
    foreach ($test in $manifest.test_executables) { [void](Verify-Hash $bundle $test) }
    $testFailures = [System.Collections.Generic.List[string]]::new()
    foreach ($test in $manifest.test_executables) {
        $executable = Bundle-Path $bundle ([string]$test.path)
        try {
            Run-OwnedProcess ([IO.Path]::GetFileNameWithoutExtension($executable)) $executable @('--test-threads=1', '--nocapture') $TestTimeoutSeconds
        } catch {
            $testFailures.Add($_.Exception.Message)
            Write-Warning $_.Exception.Message
        }
    }
    $harness = Bundle-Path $bundle 'hook-fixtures/Test-OwnedFixture.ps1'
    $reportRoot = Join-Path (Split-Path -Parent $harness) 'test-results'
    $priorReports = @(if (Test-Path -LiteralPath $reportRoot) { Get-ChildItem -LiteralPath $reportRoot -Recurse -Filter results.json | ForEach-Object FullName })
    $pwsh = Join-Path $PSHOME 'pwsh.exe'
    Run-OwnedProcess 'owned-hook-fixtures' $pwsh @('-NoProfile', '-NonInteractive', '-File', $harness, '-BrokerDirectory', $release, '-Architecture', 'both', '-RunOwnedFixture') $HookTimeoutSeconds
    $reports = @(Get-ChildItem -LiteralPath $reportRoot -Recurse -Filter results.json | Where-Object { $_.FullName -notin $priorReports })
    if ($reports.Count -ne 1) { throw 'The Hook harness did not produce exactly one fresh result report.' }
    $hookResults = @(Get-Content -LiteralPath $reports[0].FullName -Raw -Encoding utf8 | ConvertFrom-Json)
    foreach ($architecture in 'x64', 'x86') {
        if (@($hookResults | Where-Object { $_.architecture -eq $architecture -and $_.result -eq 'PASS' }).Count -eq 0) {
            throw "No passing owned Hook cases were recorded for $architecture."
        }
    }
    foreach ($case in $hookResults) {
        if ($case.result -ne 'PASS' -and -not ($case.test -eq 'desktop-wide menu' -and $case.result -eq 'SKIP')) {
            throw "Unexpected Hook test result: $($case.test) = $($case.result)"
        }
    }
    Copy-Item -LiteralPath $reports[0].FullName -Destination (Join-Path $resultsDirectory 'owned-hook-results.json')
    if ($testFailures.Count -gt 0) { throw ('Native Rust test failures: ' + ($testFailures -join '; ')) }
    $passed = $true
    Write-Host 'The exact packaged GUI, owned root Windows tests, and targeted x86/x64 Hook fixtures passed. Desktop-global Hook tests were not enabled.'
} catch {
    $failure = $_.Exception.Message
    throw
} finally {
    # Preserve partial owned-fixture reports on failure as well as full passes.
    # The workflow uploads this directory even when verification terminates.
    $hookReportRoot = Join-Path $bundle 'hook-fixtures/test-results'
    if (Test-Path -LiteralPath $hookReportRoot) {
        foreach ($report in Get-ChildItem -LiteralPath $hookReportRoot -Recurse -Filter results.json) {
            $name = 'owned-hook-' + $report.Directory.Name + '.json'
            Copy-Item -LiteralPath $report.FullName -Destination (Join-Path $resultsDirectory $name) -ErrorAction Continue
        }
    }
    [ordered]@{
        schema = 1; commit = $(if ($null -ne $manifest) { $manifest.commit } else { $null })
        finished_at_utc = [DateTime]::UtcNow.ToString('o'); passed = $passed; error = $failure
        desktop_global_hook = $false; processes = $records.ToArray()
    } | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath (Join-Path $resultsDirectory 'windows-test-results.json') -Encoding utf8
}
