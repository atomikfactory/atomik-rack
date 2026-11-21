<#
.SYNOPSIS
  Measures the running Atomik Rack app: CPU, memory, threads, handles, child processes, GPU and
  how quickly the window answers, one row per scenario.

.DESCRIPTION
  Each scenario starts the release app with a fresh temporary settings folder and library folder
  (a reused settings folder keeps the saved window layout and skews the numbers), waits for the
  first window, lets the app settle, then samples for a window of time:

    CpuPctOneCore  process CPU time over the window, as a percentage of ONE core (100 = one core
                   fully busy; the app can exceed 100 when it runs several threads)
    WS_MB          working set at the end of the window
    Private_MB     private bytes at the end of the window
    Threads        thread count of the app process
    Handles        handle count of the app process
    Children       child processes (name and private MB), for example the plugin host processes
    GpuUtilPct     summed GPU engine utilisation of the app (empty without the counters)
    GpuDedicMB     dedicated GPU memory of the app (empty without the counters)
    UiP50ms, UiP95ms, UiMaxms, UiTimeouts
                   UI responsiveness: a WM_NULL message is sent to the main window every 100 ms
                   and the round trip is timed; a probe that gets no answer in 2 s is a timeout
    FirstFrameMs, Adapter
                   read from the app's own log of the run (the "startup ... first frame
                   presented" line and the "graphics adapter" line), when they are there

  The output is one CSV file (a row per scenario) and a text table. The exit code is 0 unless the
  app crashed in a scenario (it ended with an exit code other than 0 before it was asked to quit).

  THIS SCRIPT MUST NOT MAKE SOUND. It never passes --play, so the transport stays stopped and
  nothing sounds in any scenario. Do not add --play to a scenario or to -ExtraArgs.

  Windows PowerShell 5.1 is enough. The script is for Windows (it uses user32 for the window
  probe and the Windows performance counters for the GPU).

.PARAMETER Exe
  The app. Default: target\release\atomik.exe of the repository this script is in. Build it with
  `cargo build --release -p atomik-app`.

.PARAMETER Scenario
  A scenario name, a comma separated list of names, or `all` (the default). Names:
    empty-noaudio     empty project, no audio stream (--no-audio)
    empty             empty project, audio on
    empty-workers0    empty project, audio on, no render workers (--workers 0)
    stress            stress rack of 100 devices, audio on (--stress)
    stress-workers0   stress rack, no render workers
    stress-workers2   stress rack, two render workers
    stress-audio-fx   audio tracks with effects, audio on (--stress-audio --stress-fx)
    stress-detached   stress rack, no audio, rack and mixer in windows of their own
                      (--stress --no-audio --detach rack,console)
    stress-plugins    the plugin stress project (--stress-plugins); skipped when the test plugins
                      and the plugin scanner are not next to the app

.PARAMETER Settle
  Seconds to wait after the first window appeared before sampling (default 15).

.PARAMETER Window
  Seconds to sample (default 20).

.PARAMETER Affinity
  A processor mask (for example 0xF for the first four logical processors, that is two cores and
  four threads on a machine with hyper-threading); 0 (the default) leaves the app unpinned. The
  app chooses its number of render workers when it starts, before the mask applies, so for a
  low-power run pass the worker count yourself with -ExtraArgs '--workers','3' and a scenario
  that does not set one.

.PARAMETER OutDir
  Where the CSV goes. Default: the folder atomik-perf in the temporary folder of the user.

.PARAMETER ExtraArgs
  More arguments for the app, added to every scenario (for example --workers 3).

.EXAMPLE
  .\scripts\perf\sample-app.ps1 -Scenario all

.EXAMPLE
  .\scripts\perf\sample-app.ps1 -Scenario stress -Affinity 0xF -ExtraArgs '--workers','3'
#>
param(
    [string]$Exe = '',
    [string[]]$Scenario = @('all'),
    [int]$Settle = 15,
    [int]$Window = 20,
    [int64]$Affinity = 0,
    [string]$OutDir = '',
    [string[]]$ExtraArgs = @()
)
$ErrorActionPreference = 'Stop'

$repo = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
if (-not $Exe) { $Exe = Join-Path $repo 'target\release\atomik.exe' }
if (-not $OutDir) { $OutDir = Join-Path $env:TEMP 'atomik-perf' }
if ($ExtraArgs -contains '--play') { throw 'This script never plays audio: remove --play.' }
if (-not (Test-Path -LiteralPath $Exe)) {
    throw "The app was not found at $Exe. Build it with: cargo build --release -p atomik-app (or pass -Exe)."
}
$Exe = (Resolve-Path -LiteralPath $Exe).Path
$exeDir = Split-Path -Parent $Exe
New-Item -ItemType Directory -Force $OutDir | Out-Null

# name -> arguments (no --play anywhere: the transport stays stopped).
$scenarios = [ordered]@{
    'empty-noaudio'   = @('--no-audio')
    'empty'           = @()
    'empty-workers0'  = @('--workers', '0')
    'stress'          = @('--stress')
    'stress-workers0' = @('--stress', '--workers', '0')
    'stress-workers2' = @('--stress', '--workers', '2')
    'stress-audio-fx' = @('--stress-audio', '--stress-fx')
    'stress-detached' = @('--stress', '--no-audio', '--detach', 'rack,console')
    'stress-plugins'  = @('--stress-plugins')
}
$names = @()
foreach ($s in $Scenario) { $names += ($s -split ',') }
if ($names -contains 'all') { $names = @($scenarios.Keys) }
foreach ($n in $names) {
    if (-not $scenarios.Contains($n)) { throw "Unknown scenario '$n'. Known: $(@($scenarios.Keys) -join ', '), all." }
}

Add-Type -Namespace W -Name U -MemberDefinition @'
[DllImport("user32.dll")] public static extern System.IntPtr SendMessageTimeoutW(System.IntPtr h, uint msg, System.IntPtr w, System.IntPtr l, uint flags, uint timeoutMs, out System.IntPtr result);
public delegate bool EnumProc(System.IntPtr h, System.IntPtr l);
[DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, System.IntPtr l);
[DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(System.IntPtr h, out uint pid);
[DllImport("user32.dll")] public static extern bool IsWindowVisible(System.IntPtr h);
[DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetClassNameW(System.IntPtr h, System.Text.StringBuilder s, int n);
[DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern int GetWindowTextLengthW(System.IntPtr h);
// The first visible, titled top-level window of the process that is not its console. A build
// with a console has the console as the process main window: probing that one would measure
// the console host, not the app.
public static System.IntPtr AppWindow(uint pid) {
    System.IntPtr best = System.IntPtr.Zero;
    EnumWindows((h, l) => { uint p; GetWindowThreadProcessId(h, out p);
        if (p == pid && best == System.IntPtr.Zero && IsWindowVisible(h) && GetWindowTextLengthW(h) > 0) {
            var c = new System.Text.StringBuilder(256); GetClassNameW(h, c, 256);
            if (c.ToString() != "ConsoleWindowClass") best = h; }
        return true; }, System.IntPtr.Zero);
    return best; }
'@

function Test-PluginStressReady {
    $plugin = @('atomik_test_clap.dll', 'libatomik_test_clap.so', 'libatomik_test_clap.dylib') |
        Where-Object { Test-Path -LiteralPath (Join-Path $exeDir $_) }
    $scanner = @('atomik-plugin-scan.exe', 'atomik-plugin-scan') |
        Where-Object { Test-Path -LiteralPath (Join-Path $exeDir $_) }
    return [bool]($plugin -and $scanner)
}

function Get-LogFacts {
    param([string]$Config)
    $facts = @{ FirstFrameMs = $null; Adapter = $null }
    $log = Join-Path (Join-Path $Config 'logs') 'atomik.log'
    if (-not (Test-Path -LiteralPath $log)) { return $facts }
    try {
        $text = Get-Content -LiteralPath $log -Encoding UTF8 -ErrorAction Stop
        foreach ($line in $text) {
            if ($line -match 'startup (\d+) ms: first frame presented') { $facts.FirstFrameMs = [int]$Matches[1] }
            elseif ($line -match 'graphics adapter: (.+)$') { $facts.Adapter = $Matches[1].Trim() }
        }
    } catch {}
    return $facts
}

function Measure-Scenario {
    param([string]$Name, [string[]]$ScenarioArgs)
    $tmp = Join-Path $env:TEMP ('atomik-perf-' + [guid]::NewGuid().ToString('N'))
    $lib = Join-Path $tmp 'lib'
    New-Item -ItemType Directory -Force $lib | Out-Null
    $settings = Join-Path $tmp 'settings.json'
    $quitAfter = $Settle + $Window + 20
    $all = @('--settings', "`"$settings`"", '--library', "`"$lib`"", '--no-midi', '--quit-after', $quitAfter) +
        $ScenarioArgs + $ExtraArgs
    $row = [ordered]@{
        Scenario = $Name; CpuPctOneCore = $null; WS_MB = $null; Private_MB = $null; Threads = $null
        Handles = $null; Children = ''; GpuUtilPct = $null; GpuDedicMB = $null
        UiP50ms = $null; UiP95ms = $null; UiMaxms = $null; UiTimeouts = $null; Probes = $null
        FirstWindowMs = $null; FirstFrameMs = $null; Adapter = $null; Note = ''
    }
    $crashed = $false
    $job = $null
    $p = $null
    try {
        $sw = [Diagnostics.Stopwatch]::StartNew()
        $p = Start-Process -FilePath $Exe -ArgumentList $all -PassThru
        if ($Affinity -ne 0) { try { $p.ProcessorAffinity = [IntPtr]$Affinity } catch { $row.Note = 'affinity not applied' } }
        while ($sw.ElapsedMilliseconds -lt 60000) {
            $p.Refresh()
            if ($p.HasExited) { break }
            $h = [W.U]::AppWindow([uint32]$p.Id)
            if ($h -ne [IntPtr]::Zero) { $row.FirstWindowMs = $sw.ElapsedMilliseconds; break }
            Start-Sleep -Milliseconds 25
        }
        if ($p.HasExited) {
            $row.Note = "exited early, code $($p.ExitCode)"
            $crashed = $true
            return [pscustomobject]$row, $crashed
        }
        Start-Sleep -Seconds $Settle
        if ($p.HasExited) {
            $row.Note = "exited during settle, code $($p.ExitCode)"
            $crashed = ($p.ExitCode -ne 0)
            return [pscustomobject]$row, $crashed
        }
        # GPU utilisation of the app over the window, collected beside the probes (the counters
        # may be missing; the job then returns nothing).
        $procId = $p.Id
        $job = Start-Job -ScriptBlock {
            param($id, $seconds)
            try {
                $c = Get-Counter -Counter "\GPU Engine(pid_${id}_*)\Utilization Percentage" -SampleInterval 1 -MaxSamples $seconds -ErrorAction Stop
                $sums = $c | ForEach-Object { ($_.CounterSamples | Measure-Object CookedValue -Sum).Sum }
                [math]::Round(($sums | Measure-Object -Average).Average, 2)
            } catch { $null }
        } -ArgumentList $procId, $Window
        $p.Refresh()
        $cpu0 = $p.TotalProcessorTime.TotalMilliseconds
        $lat = New-Object System.Collections.Generic.List[double]
        $timeouts = 0
        $t = [Diagnostics.Stopwatch]::StartNew()
        $one = New-Object Diagnostics.Stopwatch
        while ($t.ElapsedMilliseconds -lt ($Window * 1000)) {
            if ($p.HasExited) { break }
            $res = [IntPtr]::Zero
            $one.Restart()
            $ok = [W.U]::SendMessageTimeoutW($h, 0, [IntPtr]::Zero, [IntPtr]::Zero, 2, 2000, [ref]$res)
            $ms = $one.Elapsed.TotalMilliseconds
            if ($ok -eq [IntPtr]::Zero) { $timeouts++ } else { $lat.Add($ms) }
            Start-Sleep -Milliseconds 100
        }
        $elapsed = $t.Elapsed.TotalSeconds
        if ($p.HasExited) {
            $row.Note = "exited during the window, code $($p.ExitCode)"
            $crashed = ($p.ExitCode -ne 0)
            return [pscustomobject]$row, $crashed
        }
        $p.Refresh()
        $cpu1 = $p.TotalProcessorTime.TotalMilliseconds
        $row.CpuPctOneCore = [math]::Round(($cpu1 - $cpu0) / ($elapsed * 10.0), 2)
        $row.WS_MB = [math]::Round($p.WorkingSet64 / 1MB, 1)
        $row.Private_MB = [math]::Round($p.PrivateMemorySize64 / 1MB, 1)
        $row.Threads = $p.Threads.Count
        $row.Handles = $p.HandleCount
        $kids = @()
        try {
            foreach ($k in @(Get-CimInstance Win32_Process -Filter "ParentProcessId=$procId" -ErrorAction Stop)) {
                $mb = ''
                try { $mb = [math]::Round((Get-Process -Id $k.ProcessId -ErrorAction Stop).PrivateMemorySize64 / 1MB, 1) } catch {}
                $kids += ('{0} {1}' -f $k.Name, $mb)
            }
        } catch {}
        $row.Children = $kids -join '; '
        try {
            $m = Get-Counter -Counter "\GPU Process Memory(pid_${procId}_*)\Dedicated Usage" -ErrorAction Stop
            $row.GpuDedicMB = [math]::Round((($m.CounterSamples | Measure-Object CookedValue -Sum).Sum) / 1MB, 1)
        } catch {}
        if ($job) {
            if (Wait-Job $job -Timeout ($Window + 15)) { $row.GpuUtilPct = Receive-Job $job -ErrorAction SilentlyContinue }
        }
        $sorted = @($lat | Sort-Object)
        if ($sorted.Count) {
            $q = { param($f) [math]::Round($sorted[[math]::Min($sorted.Count - 1, [int]($f * $sorted.Count))], 1) }
            $row.UiP50ms = & $q 0.5
            $row.UiP95ms = & $q 0.95
            $row.UiMaxms = [math]::Round($sorted[$sorted.Count - 1], 1)
        }
        $row.UiTimeouts = $timeouts
        $row.Probes = $lat.Count + $timeouts
        # The app quits by itself (--quit-after); ask it nicely, then end it, if it does not.
        if (-not $p.WaitForExit(($quitAfter + 30) * 1000)) {
            $p.CloseMainWindow() | Out-Null
            if (-not $p.WaitForExit(8000)) { $p.Kill() }
            $row.Note = 'did not quit by itself'
        } elseif ($p.ExitCode -ne 0) {
            $row.Note = "exit $($p.ExitCode)"
            $crashed = $true
        }
    } finally {
        if ($job) { Remove-Job $job -Force -ErrorAction SilentlyContinue }
        if ($p -and -not $p.HasExited) { try { $p.Kill() } catch {} }
        $facts = Get-LogFacts -Config $tmp
        $row.FirstFrameMs = $facts.FirstFrameMs
        $row.Adapter = $facts.Adapter
        Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
    }
    return [pscustomobject]$row, $crashed
}

$rows = @()
$anyCrash = $false
foreach ($name in $names) {
    if ($name -eq 'stress-plugins' -and -not (Test-PluginStressReady)) {
        Write-Host "skipping stress-plugins: the test plugins (cargo build -p atomik-test-clap) and the plugin scanner are not next to $Exe"
        $rows += [pscustomobject][ordered]@{ Scenario = $name; Note = 'skipped: test plugins not built' }
        continue
    }
    Write-Host ("scenario {0}: settle {1} s, window {2} s" -f $name, $Settle, $Window)
    $result = Measure-Scenario -Name $name -ScenarioArgs $scenarios[$name]
    $rows += $result[-2]
    if ($result[-1]) { $anyCrash = $true }
}

$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$csv = Join-Path $OutDir "perf-$stamp.csv"
$cols = 'Scenario', 'CpuPctOneCore', 'WS_MB', 'Private_MB', 'Threads', 'Handles', 'GpuUtilPct', 'GpuDedicMB',
    'UiP50ms', 'UiP95ms', 'UiMaxms', 'UiTimeouts', 'Probes', 'FirstWindowMs', 'FirstFrameMs', 'Children', 'Adapter', 'Note'
$rows | Select-Object $cols | Export-Csv -Path $csv -NoTypeInformation -Encoding UTF8
$rows | Format-Table Scenario, CpuPctOneCore, WS_MB, Private_MB, Threads, Handles, GpuUtilPct, GpuDedicMB,
    UiP50ms, UiP95ms, UiMaxms, UiTimeouts, FirstFrameMs, Note -AutoSize | Out-String -Width 250 | Write-Host
Write-Host "CSV written to $csv"
if ($anyCrash) { Write-Host 'The app crashed in at least one scenario.'; exit 1 }
exit 0
