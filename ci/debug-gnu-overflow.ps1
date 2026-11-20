<#
Stress rustc to reproduce the random STATUS_STACK_OVERFLOW on windows-gnu
(rust-lang/rust#139070).

Runs the two kinds of tiny rustc invocations that crashed in cargo's CI,
alternating between them:
* "probe": the target-info probe cargo runs (`rustc - --print=cfg ...`)
* "check": a metadata-only build of an empty crate

With -DumpDir, every run is a probe started under cdb,
which writes a full dump on the first-chance stack overflow.
WER does not help here: it skips crashes during process shutdown.
#>
param(
    [Parameter(Mandatory)] [string] $Toolchain,
    [string] $Threads = 'default',
    [int] $Iterations = 3000,
    [int] $Parallel = 4,
    [string] $OutDir = 'stress-out',
    [string] $DumpDir = '',
    [int] $MaxDumps = 3
)

$ErrorActionPreference = 'Stop'
$STATUS_STACK_OVERFLOW = -1073741571 # 0xC00000FD as Int32

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$rustc = (rustup which --toolchain $Toolchain rustc).Trim()
& $rustc -vV

$work = Join-Path ([IO.Path]::GetTempPath()) "gnu-overflow-$([guid]::NewGuid())"
New-Item -ItemType Directory -Path $work | Out-Null
$src = Join-Path $work 'main.rs'
Set-Content -Path $src -Value 'fn main() {}'

$zflags = @()
if ($Threads -ne 'default') {
    $zflags = @("-Zthreads=$Threads")
    # Lets `-Z` work on non-nightly toolchains. No effect on nightly.
    $env:RUSTC_BOOTSTRAP = '1'
}

$probe = @(
    '-', '--crate-name', '___', '--print=file-names',
    '--crate-type', 'bin', '--crate-type', 'rlib', '--crate-type', 'dylib',
    '--crate-type', 'cdylib', '--crate-type', 'staticlib', '--crate-type', 'proc-macro',
    '--print=sysroot', '--print=split-debuginfo', '--print=crate-name', '--print=cfg',
    '-Wwarnings'
)

$cdb = 'C:\Program Files (x86)\Windows Kits\10\Debuggers\x64\cdb.exe'
if ($DumpDir) {
    New-Item -ItemType Directory -Force -Path $DumpDir | Out-Null
    # Processes started under a debugger get the NT debug heap, which changes timing.
    $env:_NO_DEBUG_HEAP = '1'
    # Smoke test: show that cdb starts rustc and exits by itself.
    $smoke = Join-Path $work 'cdb-smoke.txt'
    $null = '' | & $cdb -G -c 'g; q' $rustc --version 2>&1 | Out-File $smoke
    Write-Host "cdb smoke test (exit $LASTEXITCODE):"
    Get-Content $smoke | Select-Object -Last 15 | Write-Host
}

$sw = [Diagnostics.Stopwatch]::StartNew()
$results = 1..$Iterations | ForEach-Object -ThrottleLimit $Parallel -Parallel {
    $i = $_
    $rustc = $using:rustc
    $work = $using:work
    $z = @($using:zflags)
    $errFile = Join-Path $work "err-$i.txt"
    $dumpDir = $using:DumpDir
    if ($dumpDir) {
        $kind = 'probe'
        if (@(Get-ChildItem $dumpDir -Filter *.dmp).Count -ge $using:MaxDumps) {
            return [pscustomobject]@{ I = $i; Kind = 'skipped'; Code = 0; Stderr = '' }
        }
        $dmp = Join-Path $dumpDir "sov-$i.dmp"
        $log = Join-Path $dumpDir "sov-$i.txt"
        $script = Join-Path $work "cdb-$i.txt"
        # Break on the first-chance overflow, before Rust's handler runs.
        Set-Content -Path $script -Value @(
            "sxe -c `".dump /ma $dmp; .echo SOV-HIT; r; kn 0x2000; ~*kn 80; q`" sov"
            'g'
            'q'
        )
        $a = @('-G', '-cf', $script, '-logo', $log, $rustc) + $z + @($using:probe)
        $cdbExe = $using:cdb
        $null = '' | & $cdbExe @a 2> $errFile
        $err = if (Test-Path $errFile) { Get-Content -Raw $errFile } else { '' }
        # Second check in case cdb missed the event and Rust's handler ran.
        $hit = ((Test-Path $log) -and (Select-String -Quiet -Path $log -Pattern 'SOV-HIT')) -or
            ("$err" -match 'overflowed its stack')
        if (-not $hit) { Remove-Item -Force -ErrorAction SilentlyContinue $log }
        Remove-Item -Force -ErrorAction SilentlyContinue $errFile, $script
        $code = if ($hit) { $using:STATUS_STACK_OVERFLOW } else { 0 }
        return [pscustomobject]@{ I = $i; Kind = $kind; Code = $code; Stderr = "$err".Trim() }
    }
    if ($i % 2) {
        $kind = 'probe'
        $a = $z + @($using:probe)
        $null = '' | & $rustc @a 2> $errFile
    } else {
        $kind = 'check'
        $od = Join-Path $work "out-$i"
        # Like cargo, create the output dir up front; rustc won't for dep-info.
        New-Item -ItemType Directory -Path $od | Out-Null
        $a = $z + @(
            '--crate-name', "m$i", '--edition=2021', $using:src,
            '--emit=dep-info,metadata', '--crate-type', 'bin', '--out-dir', $od
        )
        $null = & $rustc @a 2> $errFile
        Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $od
    }
    $code = $LASTEXITCODE
    $err = if (Test-Path $errFile) { Get-Content -Raw $errFile } else { '' }
    Remove-Item -Force -ErrorAction SilentlyContinue $errFile
    [pscustomobject]@{ I = $i; Kind = $kind; Code = $code; Stderr = "$err".Trim() }
}
$sw.Stop()
Remove-Item -Recurse -Force -ErrorAction SilentlyContinue $work

$results | Export-Csv -NoTypeInformation -Path (Join-Path $OutDir 'results.csv')
$ran = @($results | Where-Object Kind -ne 'skipped').Count
$fails = @($results | Where-Object Code -ne 0)
$overflows = @($fails | Where-Object {
    $_.Code -eq $STATUS_STACK_OVERFLOW -or $_.Stderr -match 'overflowed its stack'
})
$probeOverflows = @($overflows | Where-Object Kind -eq 'probe').Count
$checkOverflows = @($overflows | Where-Object Kind -eq 'check').Count
$others = @($fails | Where-Object { $overflows -notcontains $_ })

$summary = @"
| toolchain | threads | runs | stack overflows (probe / check) | other failures | time |
|---|---|---|---|---|---|
| ``$Toolchain``$(if ($DumpDir) { ' (cdb)' }) | $Threads | $ran | $($overflows.Count) ($probeOverflows / $checkOverflows) | $($others.Count) | $([int]$sw.Elapsed.TotalSeconds)s |
"@
Write-Host $summary
if ($env:GITHUB_STEP_SUMMARY) {
    Add-Content -Path $env:GITHUB_STEP_SUMMARY -Value $summary
}

foreach ($f in $others | Select-Object -First 5) {
    Write-Host "--- other failure: run $($f.I) ($($f.Kind)) exit=$($f.Code)"
    Write-Host $f.Stderr
}
foreach ($f in $overflows | Select-Object -First 3) {
    Write-Host "--- overflow: run $($f.I) ($($f.Kind)) exit=$($f.Code)"
    Write-Host $f.Stderr
}
