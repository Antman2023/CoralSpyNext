#Requires -Version 7.0
<#
Opt-in Windows integration tests. Only fresh instances of the adjacent, source-built
owned fixture are targeted. No target HWND/PID, fixture path, or payload is accepted.
No tests run without -RunOwnedFixture. Desktop-global menu capture additionally
requires BOTH -TestDesktopWideMenu and -ConfirmIsolatedTestDesktop.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)][string]$BrokerDirectory,
    [ValidateSet('x64', 'x86', 'both')][string]$Architecture = 'both',
    [switch]$RunOwnedFixture,
    [switch]$TestDesktopWideMenu,
    [switch]$ConfirmIsolatedTestDesktop
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if (-not $RunOwnedFixture) { throw 'No tests were run. Pass -RunOwnedFixture to launch the public owned fixture.' }
if (-not $IsWindows) { throw 'These integration tests require native Windows. They have not been run here.' }
if (-not [Environment]::Is64BitProcess) { throw 'Use 64-bit PowerShell 7 so both fixture architectures can be inspected.' }
if ($TestDesktopWideMenu -and -not $ConfirmIsolatedTestDesktop) {
    throw 'Desktop-wide capture may touch other same-desktop processes. Use an isolated test desktop and also pass -ConfirmIsolatedTestDesktop.'
}
$brokerDir = (Resolve-Path -LiteralPath $BrokerDirectory).Path
$arches = if ($Architecture -eq 'both') { @('x64', 'x86') } else { @($Architecture) }
foreach ($arch in $arches) {
    foreach ($path in @((Join-Path $PSScriptRoot "bin/owned-fixture-$arch.exe"),
                        (Join-Path $brokerDir "coralspy-hook-broker-$arch.exe"))) {
        if (-not (Test-Path -LiteralPath $path -PathType Leaf)) { throw "Missing required build: $path" }
    }
}
if (-not ('OwnedFixture.Native' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
namespace OwnedFixture {
    public static class Native {
        [DllImport("user32.dll", SetLastError = true)]
        public static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
        [DllImport("user32.dll", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        public static extern bool PostMessageW(IntPtr hwnd, uint message, UIntPtr wparam, IntPtr lparam);
    }
}
'@
}
$results = [System.Collections.Generic.List[object]]::new()
$script:fixture = $null
$script:activeBroker = $null
$script:manifestPath = ''
$script:nonce = ''
$script:currentArch = ''
$runDir = Join-Path $PSScriptRoot ('test-results/' + [DateTime]::UtcNow.ToString('yyyyMMdd-HHmmss') + '-' + [guid]::NewGuid().ToString('N').Substring(0, 8))
[void](New-Item -ItemType Directory -Path $runDir -Force)

function Assert-True([bool]$Condition, [string]$Message) {
    if (-not $Condition) { throw $Message }
}
function Read-OwnManifest {
    Assert-True ($null -ne $script:fixture -and -not $script:fixture.HasExited) 'The fresh fixture is not running.'
    $m = Get-Content -LiteralPath $script:manifestPath -Raw -Encoding utf8 | ConvertFrom-Json
    Assert-True ($m.fixture -ceq 'coralspy-owned-public-fixture-v1') 'Unexpected fixture manifest version.'
    Assert-True ($m.nonce -ceq $script:nonce) 'The manifest is not from this fresh run.'
    Assert-True ([uint32]$m.pid -eq $script:fixture.Id) 'The manifest PID is not the launched fixture.'
    Assert-True ($m.pointer_bits -eq $(if ($script:currentArch -eq 'x64') { 64 } else { 32 })) 'Fixture architecture mismatch.'
    return $m
}
function Wait-Manifest([scriptblock]$Condition, [int]$TimeoutMs = 5000) {
    $watch = [Diagnostics.Stopwatch]::StartNew()
    while ($watch.ElapsedMilliseconds -lt $TimeoutMs) {
        if ($script:fixture.HasExited) { throw "Fixture exited with code $($script:fixture.ExitCode)." }
        if (Test-Path -LiteralPath $script:manifestPath) {
            $m = Read-OwnManifest
            if (& $Condition $m) { return $m }
        }
        Start-Sleep -Milliseconds 25
    }
    throw "Timed out waiting for owned fixture state after $TimeoutMs ms."
}
function Own-Target([string]$Field) {
    $allowed = @('main_hwnd', 'rich_edit_hwnd', 'password_edit_hwnd', 'password_rich_edit_hwnd',
                 'list_view_hwnd', 'tree_view_hwnd', 'wrong_class_hwnd', 'menu_owner_hwnd')
    Assert-True ($Field -cin $allowed) 'Only fixture-declared targets are permitted.'
    $m = Read-OwnManifest
    $window = [uint64]$m.$Field
    [uint32]$owner = 0
    $thread = [OwnedFixture.Native]::GetWindowThreadProcessId([IntPtr]::new([long]$window), [ref]$owner)
    Assert-True ($window -ne 0 -and $owner -eq $script:fixture.Id -and $thread -eq $m.tid) 'The target HWND no longer belongs to the fresh fixture.'
    return [ordered]@{ hwnd = $window; pid = [uint32]$m.pid; tid = [uint32]$m.tid }
}
function Post-OwnMessage([uint32]$Message, [uint64]$Value = 0) {
    $target = Own-Target 'main_hwnd'
    Assert-True ([OwnedFixture.Native]::PostMessageW([IntPtr]::new([long]$target.hwnd), $Message,
        [UIntPtr]::new($Value), [IntPtr]::Zero)) 'Could not post a test message to the owned fixture.'
}
function New-Request([string]$Operation, [string]$Field, [int]$TimeoutMs = 3000) {
    $target = if ($Operation -eq 'menu_desktop_once') { $null } else { Own-Target $Field }
    return [ordered]@{
        language = 'english'; operation = $Operation; architecture = $script:currentArch; target = $target
        limits = [ordered]@{ timeout_ms = $TimeoutMs; max_rows = 512; max_columns = 32;
            max_nodes = 1024; max_depth = 32; max_text_units = 2048; max_result_bytes = 1048576 }
        visible_capture_consent = $true; desktop_menu_consent = ($Operation -eq 'menu_desktop_once')
    }
}
function Start-Broker($Request, [string]$BrokerArch = $script:currentArch) {
    Assert-True ($null -eq $script:activeBroker) 'A previous broker is still active.'
    $path = Join-Path $brokerDir "coralspy-hook-broker-$BrokerArch.exe"
    Assert-True (Test-Path -LiteralPath $path -PathType Leaf) "Missing fixed broker: $path"
    $start = [Diagnostics.ProcessStartInfo]::new($path)
    $start.UseShellExecute = $false
    $start.RedirectStandardInput = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    $start.StandardInputEncoding = [Text.UTF8Encoding]::new($false)
    $start.StandardOutputEncoding = [Text.UTF8Encoding]::new($false)
    $start.StandardErrorEncoding = [Text.UTF8Encoding]::new($false)
    $process = [Diagnostics.Process]::new()
    $process.StartInfo = $start
    Assert-True ($process.Start()) 'Could not launch the fixed broker.'
    $script:activeBroker = $process
    $stdout = $process.StandardOutput.ReadToEndAsync()
    $stderr = $process.StandardError.ReadToEndAsync()
    $process.StandardInput.WriteLine(($Request | ConvertTo-Json -Depth 8 -Compress))
    $process.StandardInput.Flush()
    # Keep the input pipe open: EOF is an explicit broker-cancellation signal.
    return @{ process = $process; stdout = $stdout; stderr = $stderr; request = $Request }
}
function Finish-Broker($Run, [int]$WaitMs = 13000) {
    try {
        Assert-True ($Run.process.WaitForExit($WaitMs)) 'Broker exceeded the independent harness deadline.'
        $text = $Run.stdout.GetAwaiter().GetResult()
        $diagnostics = $Run.stderr.GetAwaiter().GetResult()
        Assert-True ($Run.process.ExitCode -eq 0) "Broker exited $($Run.process.ExitCode): $diagnostics"
        Assert-True ($text.Length -le 8388608) 'Broker output exceeded the client wire cap.'
        $response = $text | ConvertFrom-Json
        Assert-True (@($response.PSObject.Properties).Count -eq 1) 'Expected exactly one Ok/Err result envelope.'
        return $response
    } finally {
        if (-not $Run.process.HasExited) { $Run.process.Kill(); [void]$Run.process.WaitForExit(3000) }
        $Run.process.Dispose()
        $script:activeBroker = $null
    }
}
function Invoke-Capture($Request) { return Finish-Broker (Start-Broker $Request) }
function Expect-Error($Response, [string[]]$Codes) {
    Assert-True ($null -ne $Response.PSObject.Properties['Err']) 'Expected a rejection, received success.'
    Assert-True ($Response.Err.code -cin $Codes) "Expected $($Codes -join '/'), received $($Response.Err.code): $($Response.Err.message)"
}
function Expect-Ok($Response, [string]$Kind, $Target = $null) {
    Assert-True ($null -ne $Response.PSObject.Properties['Ok']) "Expected success: $($Response | ConvertTo-Json -Depth 6 -Compress)"
    Assert-True ($Response.Ok.data.kind -ceq $Kind) 'Unexpected result kind.'
    if ($null -ne $Target) {
        foreach ($field in @('hwnd', 'pid', 'tid')) {
            Assert-True ($Response.Ok.actual_target.$field -eq $Target.$field) "Returned $field differs from the selected fixture target."
        }
    }
    return $Response.Ok
}
function Assert-NoResidentPayload {
    $watch = [Diagnostics.Stopwatch]::StartNew()
    do {
        Post-OwnMessage 0 # Pump only the owned fixture after hook removal.
        $script:fixture.Refresh()
        $modules = @($script:fixture.Modules | Where-Object { $_.ModuleName -match '^cshook-[0-9a-f]{32}\.dll$' })
        if ($modules.Count -eq 0) { return }
        Start-Sleep -Milliseconds 50
    } while ($watch.ElapsedMilliseconds -lt 6000)
    throw 'A nonce-named hook payload is still mapped in the owned fixture after the grace period.'
}
function Test-Case([string]$Name, [scriptblock]$Body) {
    $watch = [Diagnostics.Stopwatch]::StartNew()
    try {
        & $Body
        $results.Add([ordered]@{ architecture = $script:currentArch; test = $Name; result = 'PASS'; elapsed_ms = $watch.ElapsedMilliseconds })
        Write-Host "PASS [$script:currentArch] $Name"
    } catch {
        $results.Add([ordered]@{ architecture = $script:currentArch; test = $Name; result = 'FAIL'; elapsed_ms = $watch.ElapsedMilliseconds; detail = $_.Exception.Message })
        throw
    }
}
function Capture-Menu([bool]$Desktop = $false) {
    $operation = if ($Desktop) { 'menu_desktop_once' } else { 'menu_target' }
    $request = New-Request $operation 'menu_owner_hwnd' 5000
    $run = Start-Broker $request
    # Reopen our own menu periodically so hook installation and menu activation
    # need no fragile one-shot fixed startup delay. No foreign window is clicked.
    $watch = [Diagnostics.Stopwatch]::StartNew()
    while (-not $run.process.HasExited -and $watch.ElapsedMilliseconds -lt 4500) {
        $m = Read-OwnManifest
        Post-OwnMessage ([uint32]$m.messages.close_menu)
        Start-Sleep -Milliseconds 80
        Post-OwnMessage ([uint32]$m.messages.open_menu)
        Start-Sleep -Milliseconds 300
    }
    Post-OwnMessage ([uint32](Read-OwnManifest).messages.close_menu)
    $ok = Expect-Ok (Finish-Broker $run) 'menu' $request.target
    Assert-True ($ok.actual_target.pid -eq $script:fixture.Id) 'A different desktop process won global capture. Discarding this result; use an isolated test desktop.'
    Assert-True (@($ok.data.items | Where-Object { $_.text -match 'Public action' }).Count -gt 0) 'Public fixture menu action missing.'
    Assert-True (@($ok.data.items | Where-Object { $_.text -match 'Disabled public item' -and ($_.flags -band 4) -ne 0 }).Count -gt 0) 'Disabled fixture menu item or disabled metadata missing.'
    Assert-True (@($ok.data.items | Where-Object { ($_.flags -band 16) -ne 0 }).Count -gt 0) 'Menu separator metadata missing.'
}

try {
    foreach ($arch in $arches) {
        $script:currentArch = $arch
        $script:nonce = [guid]::NewGuid().ToString('N')
        $script:manifestPath = Join-Path $runDir "$arch-manifest.json"
        $fixturePath = Join-Path $PSScriptRoot "bin/owned-fixture-$arch.exe"
        $start = [Diagnostics.ProcessStartInfo]::new($fixturePath)
        $start.UseShellExecute = $false
        $start.ArgumentList.Add($script:manifestPath)
        $start.ArgumentList.Add($script:nonce)
        $script:fixture = [Diagnostics.Process]::Start($start)
        try {
            [void](Wait-Manifest { param($m) $m.main_hwnd -ne 0 })
            Test-Case 'raw RTF bytes and Unicode formatting' {
                $r = New-Request 'rich_edit_rtf' 'rich_edit_hwnd'
                $ok = Expect-Ok (Invoke-Capture $r) 'rich_edit_rtf' $r.target
                $rtf = [Text.Encoding]::ASCII.GetString([byte[]]$ok.data.bytes)
                Assert-True ($rtf.StartsWith('{\rtf')) 'Result is not a raw RTF byte stream.'
                Assert-True ($rtf -match '\\colortbl' -and $rtf -match '\\b(?:\s|\\)') 'RichEdit formatting was lost.'
                $golden = [IO.File]::ReadAllBytes($script:manifestPath + '.expected.rtf')
                Assert-True ([Convert]::ToBase64String([byte[]]$ok.data.bytes) -ceq [Convert]::ToBase64String($golden)) 'Broker bytes differ from the fixture own native SF_RTF stream.'
                Assert-True (-not $ok.truncated -and $ok.data.complete) 'Small RTF fixture must be complete.'
                [IO.File]::WriteAllBytes((Join-Path $runDir "$arch-public-sample.rtf"), [byte[]]$ok.data.bytes)
            }
            Test-Case 'Unicode ListView rows and columns' {
                $r = New-Request 'list_view' 'list_view_hwnd'
                $ok = Expect-Ok (Invoke-Capture $r) 'list_view' $r.target
                $cells = @($ok.data.cells)
                Assert-True ($cells.Count -eq 9) 'Expected exactly three rows by three columns.'
                $columns = @($ok.data.columns)
                Assert-True ($columns.Count -eq 3) 'Expected three bounded column-header records.'
                $expectedHeaders = @("Name $([char]0x540D)", '', "Note`t$([char]0x5907)$([char]0x6CE8)")
                for ($i = 0; $i -lt 3; $i++) {
                    Assert-True ($columns[$i].index -eq $i -and $columns[$i].text -ceq $expectedHeaders[$i]) "Column header $i changed blank, tab, or Unicode content."
                }
                Assert-True ($ok.reported_counts.rows -eq 3 -and $ok.reported_counts.columns -eq 3 -and $ok.captured_records -eq 12) 'Incorrect reported ListView counts.'
                $expected = @(@('Alpha', "caf$([char]0xE9)", 'public row 1'),
                    @("$([char]0x65E5)$([char]0x672C)$([char]0x8A9E)", "$([char]0x03B2)eta", 'public row 2'),
                    @(('Emoji ' + [char]::ConvertFromUtf32(0x1F600)), '42', 'public row 3'))
                $seen = [System.Collections.Generic.HashSet[string]]::new()
                foreach ($cell in $cells) {
                    Assert-True ($cell.row -ge 0 -and $cell.row -lt 3 -and $cell.column -ge 0 -and $cell.column -lt 3) 'Cell coordinate is outside the fixture.'
                    Assert-True ($seen.Add("$($cell.row),$($cell.column)")) 'Duplicate ListView cell coordinate.'
                    Assert-True ($cell.text -ceq $expected[$cell.row][$cell.column]) "Unexpected cell at $($cell.row),$($cell.column)."
                }
            }
            Test-Case 'TreeView hierarchy and Unicode' {
                $r = New-Request 'tree_view' 'tree_view_hwnd'
                $ok = Expect-Ok (Invoke-Capture $r) 'tree_view' $r.target
                $nodes = @($ok.data.nodes)
                Assert-True ($nodes.Count -eq 5) 'Expected five fixture tree nodes.'
                Assert-True ($ok.reported_counts.nodes -eq 5 -and $ok.captured_records -eq 5) 'Incorrect reported TreeView count.'
                $labels = @('Public root', "Branch caf$([char]0xE9)",
                    "Leaf $([char]0x65E5)$([char]0x672C)$([char]0x8A9E)",
                    ('Branch ' + [char]::ConvertFromUtf32(0x1F600)), 'Second root')
                $depths = @(0, 1, 2, 1, 0)
                for ($i = 0; $i -lt 5; $i++) {
                    Assert-True ($nodes[$i].text -ceq $labels[$i] -and $nodes[$i].depth -eq $depths[$i]) "Tree node $i has unexpected text or depth."
                }
            }
            Test-Case 'target-owned menu capture' { Capture-Menu }
            Test-Case 'password RichEdit rejects without content' {
                Expect-Error (Invoke-Capture (New-Request 'rich_edit_rtf' 'password_rich_edit_hwnd')) @('password_control')
            }
            Test-Case 'password Edit is rejected' {
                Expect-Error (Invoke-Capture (New-Request 'rich_edit_rtf' 'password_edit_hwnd')) @('password_control', 'class_mismatch')
            }
            Test-Case 'wrong class and cross-control operation reject' {
                Expect-Error (Invoke-Capture (New-Request 'rich_edit_rtf' 'wrong_class_hwnd')) @('class_mismatch')
                Expect-Error (Invoke-Capture (New-Request 'tree_view' 'list_view_hwnd')) @('class_mismatch')
            }
            Test-Case 'mismatched target identity rejects' {
                $r = New-Request 'list_view' 'list_view_hwnd'
                $r.target.pid = [uint32]::MaxValue
                Expect-Error (Invoke-Capture $r) @('target_mismatch')
                $r = New-Request 'list_view' 'list_view_hwnd'
                $r.target.tid = [uint32]::MaxValue
                Expect-Error (Invoke-Capture $r) @('target_mismatch')
            }
            Test-Case 'wrong architecture declaration rejects' {
                $r = New-Request 'list_view' 'list_view_hwnd'
                $r.architecture = if ($arch -eq 'x64') { 'x86' } else { 'x64' }
                Expect-Error (Invoke-Capture $r) @('wrong_architecture')
            }
            $otherArch = if ($arch -eq 'x64') { 'x86' } else { 'x64' }
            if (Test-Path -LiteralPath (Join-Path $brokerDir "coralspy-hook-broker-$otherArch.exe") -PathType Leaf) {
                Test-Case 'opposite broker rejects the fixture architecture' {
                    $r = New-Request 'list_view' 'list_view_hwnd'; $r.architecture = $otherArch
                    Expect-Error (Finish-Broker (Start-Broker $r $otherArch)) @('wrong_architecture')
                }
            } else {
                $results.Add([ordered]@{ architecture = $arch; test = 'opposite broker target bitness'; result = 'SKIP'; detail = "No $otherArch broker installed." })
            }
            Test-Case 'missing consent and oversized limits reject' {
                $r = New-Request 'list_view' 'list_view_hwnd'; $r.visible_capture_consent = $false
                Expect-Error (Invoke-Capture $r) @('invalid_input')
                $r = New-Request 'list_view' 'list_view_hwnd'; $r.limits.max_rows = 513
                Expect-Error (Invoke-Capture $r) @('invalid_input')
            }
            Test-Case 'bounded ListView truncation' {
                $r = New-Request 'list_view' 'list_view_hwnd'; $r.limits.max_rows = 1; $r.limits.max_columns = 2
                $ok = Expect-Ok (Invoke-Capture $r) 'list_view' $r.target
                Assert-True ($ok.truncated -and @($ok.data.cells).Count -eq 2 -and @($ok.data.columns).Count -eq 2 -and $ok.captured_records -eq 4) 'Expected two headers and two cells with an explicit truncation flag.'
            }
            Test-Case 'header text cap preserves explicit truncation and empty labels' {
                $r = New-Request 'list_view' 'list_view_hwnd'; $r.limits.max_text_units = 1
                $ok = Expect-Ok (Invoke-Capture $r) 'list_view' $r.target
                $columns = @($ok.data.columns)
                Assert-True ($ok.truncated -and $columns.Count -eq 3) 'Header text must be capped with an explicit truncation flag.'
                Assert-True ($columns[0].text -ceq 'N' -and ($columns[0].flags -band 1) -eq 1) 'Truncated first header text/flag is wrong.'
                Assert-True ($columns[1].text -ceq '') 'Empty header must remain present and empty.'
            }
            Test-Case 'RTF cap is explicit incomplete diagnostic bytes' {
                $r = New-Request 'rich_edit_rtf' 'rich_edit_hwnd'; $r.limits.max_result_bytes = 40
                $ok = Expect-Ok (Invoke-Capture $r) 'rich_edit_rtf' $r.target
                Assert-True ($ok.truncated -and -not $ok.data.complete -and @($ok.data.bytes).Count -le 40) 'Partial RTF must be capped and marked incomplete.'
            }
            Test-Case 'timeout is bounded and a later capture recovers' {
                Post-OwnMessage ([uint32](Read-OwnManifest).messages.stall) 2500
                [void](Wait-Manifest { param($m) $m.stalling })
                $watch = [Diagnostics.Stopwatch]::StartNew()
                Expect-Error (Invoke-Capture (New-Request 'list_view' 'list_view_hwnd' 150)) @('timed_out')
                Assert-True ($watch.ElapsedMilliseconds -lt 2000) 'The 150 ms timeout did not stop well before the 2.5 s fixture stall.'
                [void](Wait-Manifest { param($m) -not $m.stalling })
                Assert-NoResidentPayload
                [void](Expect-Ok (Invoke-Capture (New-Request 'list_view' 'list_view_hwnd')) 'list_view')
            }
            Test-Case 'stdin EOF cancels a pending owned-menu capture' {
                $run = Start-Broker (New-Request 'menu_target' 'menu_owner_hwnd' 5000)
                Start-Sleep -Milliseconds 250
                $run.process.StandardInput.Close()
                Expect-Error (Finish-Broker $run 3000) @('cancelled')
                Assert-NoResidentPayload
            }
            Test-Case 'in-flight graceful cancellation and payload unload' {
                Post-OwnMessage ([uint32](Read-OwnManifest).messages.slow_rtf) 1
                [void](Wait-Manifest { param($m) $m.slow_rtf })
                $run = Start-Broker (New-Request 'rich_edit_rtf' 'rich_edit_hwnd' 5000)
                [void](Wait-Manifest { param($m) $m.streaming } 4000)
                Assert-True (-not $run.process.HasExited) 'Broker finished before the in-flight cancellation point.'
                $run.process.StandardInput.Write("cancel`n")
                $run.process.StandardInput.Close()
                Expect-Error (Finish-Broker $run 3000) @('cancelled')
                [void](Wait-Manifest { param($m) -not $m.streaming } 5000)
                Post-OwnMessage ([uint32](Read-OwnManifest).messages.slow_rtf) 0
                [void](Wait-Manifest { param($m) -not $m.slow_rtf })
                Assert-NoResidentPayload
                [void](Expect-Ok (Invoke-Capture (New-Request 'rich_edit_rtf' 'rich_edit_hwnd')) 'rich_edit_rtf')
            }
            Test-Case 'in-flight forced broker exit and payload unload' {
                Post-OwnMessage ([uint32](Read-OwnManifest).messages.slow_rtf) 1
                [void](Wait-Manifest { param($m) $m.slow_rtf })
                $run = Start-Broker (New-Request 'rich_edit_rtf' 'rich_edit_hwnd' 5000)
                [void](Wait-Manifest { param($m) $m.streaming } 4000)
                Assert-True (-not $run.process.HasExited) 'Broker finished before the in-flight cancellation point.'
                $run.process.Kill()
                Assert-True ($run.process.WaitForExit(3000)) 'Cancelled broker did not exit.'
                [void]$run.stdout.GetAwaiter().GetResult(); [void]$run.stderr.GetAwaiter().GetResult()
                $run.process.Dispose(); $script:activeBroker = $null
                [void](Wait-Manifest { param($m) -not $m.streaming } 5000)
                Post-OwnMessage ([uint32](Read-OwnManifest).messages.slow_rtf) 0
                [void](Wait-Manifest { param($m) -not $m.slow_rtf })
                Assert-NoResidentPayload
                [void](Expect-Ok (Invoke-Capture (New-Request 'rich_edit_rtf' 'rich_edit_hwnd')) 'rich_edit_rtf')
            }
            Test-Case 'successful captures leave no resident nonce payload' { Assert-NoResidentPayload }
            if ($TestDesktopWideMenu) {
                Test-Case 'explicit isolated-desktop one-shot menu capture' { Capture-Menu $true; Assert-NoResidentPayload }
            } else {
                $results.Add([ordered]@{ architecture = $arch; test = 'desktop-wide menu'; result = 'SKIP'; detail = 'Requires two separate isolated-desktop opt-in switches.' })
            }
        } finally {
            if ($null -ne $script:activeBroker) {
                if (-not $script:activeBroker.HasExited) { $script:activeBroker.Kill(); [void]$script:activeBroker.WaitForExit(3000) }
                $script:activeBroker.Dispose(); $script:activeBroker = $null
            }
            if ($null -ne $script:fixture) {
                if (-not $script:fixture.HasExited) {
                    [void]$script:fixture.CloseMainWindow()
                    if (-not $script:fixture.WaitForExit(6000)) { $script:fixture.Kill(); [void]$script:fixture.WaitForExit(3000) }
                }
                $script:fixture.Dispose(); $script:fixture = $null
            }
        }
    }
} finally {
    $report = Join-Path $runDir 'results.json'
    ConvertTo-Json -InputObject ($results.ToArray()) -Depth 8 | Set-Content -LiteralPath $report -Encoding utf8
    Write-Host "Test report: $report"
}
