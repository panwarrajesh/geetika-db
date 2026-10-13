# Runs every check in order and writes verification-report.txt (paste its contents when asking for help).
#   powershell -ExecutionPolicy Bypass -File scripts\verify-all.ps1            full run
#   powershell -ExecutionPolicy Bypass -File scripts\verify-all.ps1 -Quick     no downloads: Rust tests (if cargo exists) + JS/Python logic tests
#   powershell -ExecutionPolicy Bypass -File scripts\verify-all.ps1 -Bench     also run the small engine benchmark
# Missing tools are reported as SKIPPED, never as success.
param([switch]$Quick, [switch]$Bench)

$ErrorActionPreference = 'Continue'
$Root = Resolve-Path (Join-Path $PSScriptRoot '..')
$Report = Join-Path $Root 'verification-report.txt'
$LogDir = Join-Path ([System.IO.Path]::GetTempPath()) ("rajeshdb-verify-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $LogDir | Out-Null
$Results = New-Object System.Collections.ArrayList

function Test-Tool([string]$Name) { return [bool](Get-Command $Name -ErrorAction SilentlyContinue) }

function Invoke-Step([string]$Name, [string]$Dir, [string]$Tool, [string[]]$CommandLine) {
    $log = Join-Path $LogDir ("{0}.log" -f $Results.Count)
    if (-not (Test-Tool $Tool)) {
        [void]$Results.Add([pscustomobject]@{ Name = $Name; Result = "SKIPPED (no $Tool)"; Log = $log })
        Write-Host "-- ${Name}: SKIPPED ($Tool not installed)"
        return
    }
    Write-Host "-- $Name ..."
    Push-Location (Join-Path $Root $Dir)
    try {
        $exe = $CommandLine[0]
        $rest = @()
        if ($CommandLine.Length -gt 1) { $rest = $CommandLine[1..($CommandLine.Length - 1)] }
        & $exe @rest *> $log
        $code = $LASTEXITCODE
    } finally { Pop-Location }
    $res = if ($code -eq 0) { 'PASS' } else { "FAIL (exit $code)" }
    [void]$Results.Add([pscustomobject]@{ Name = $Name; Result = $res; Log = $log })
    Write-Host "   $res"
}
function Skip-Step([string]$Name, [string]$Why) {
    [void]$Results.Add([pscustomobject]@{ Name = $Name; Result = "SKIPPED ($Why)"; Log = $null })
    Write-Host "-- ${Name}: SKIPPED ($Why)"
}

$py = 'python'
if (Test-Tool 'py') { $py = 'py' } elseif (Test-Tool 'python3') { $py = 'python3' }

Invoke-Step 'rust: core tests (engine, crash-kill)' 'engine' 'cargo' @('cargo', 'test', '-p', 'rajeshdb-core')
Invoke-Step 'rust: docs layer tests'                'engine' 'cargo' @('cargo', 'test', '-p', 'rajeshdb-docs')
Invoke-Step 'rust: server (phase 4) tests'          'engine' 'cargo' @('cargo', 'test', '-p', 'rajeshdb-server')
Invoke-Step 'rust: platform tests'                  'engine' 'cargo' @('cargo', 'test', '-p', 'rajeshdb-platform')
if (-not $Quick) { Invoke-Step 'rust: release build of all crates' 'engine' 'cargo' @('cargo', 'build', '--release', '--workspace') } else { Skip-Step 'rust: release build' '-Quick' }

Invoke-Step 'dashboard: pure-logic tests' 'dashboard' 'node' @('node', '--test', 'src/lib.test.ts')
Invoke-Step 'loadtest: stats tests'       'loadtest'  'node' @('node', '--test', 'loadtest.test.mjs')
Invoke-Step 'sdk js: tests'               'sdk\js'    'node' @('node', '--test', 'test/sdk.test.ts')
Invoke-Step 'sdk python: tests'           'sdk\python' $py   @($py, '-m', 'unittest', 'discover', '-s', 'tests', '-v')

if (-not $Quick) {
    Invoke-Step 'dashboard: npm install'          'dashboard' 'npm' @('npm', 'install')
    Invoke-Step 'dashboard: typecheck'            'dashboard' 'npm' @('npm', 'run', 'typecheck')
    Invoke-Step 'dashboard: production build'     'dashboard' 'npm' @('npm', 'run', 'build')
    $envFile = Join-Path $Root 'deploy\.env'
    $madeEnv = $false
    if (-not (Test-Path $envFile)) { Copy-Item (Join-Path $Root 'deploy\.env.example') $envFile; $madeEnv = $true }
    Invoke-Step 'deploy: docker compose config'   'deploy' 'docker' @('docker', 'compose', 'config', '-q')
    if ($madeEnv) { Remove-Item $envFile -Force }
} else {
    Skip-Step 'dashboard: npm install/typecheck/build' '-Quick'
    Skip-Step 'deploy: docker compose config' '-Quick'
}
if ($Bench) { Invoke-Step 'engine benchmark (small)' 'engine' 'cargo' @('cargo', 'run', '--release', '-p', 'rajeshdb-bench', '--', '--size', 'small') }

# ---- report
$lines = New-Object System.Collections.ArrayList
[void]$lines.Add('RajeshDB verification report')
[void]$lines.Add(("date: {0}   host: {1}" -f (Get-Date -Format 'yyyy-MM-ddTHH:mm:ssK'), [System.Environment]::OSVersion.VersionString))
foreach ($t in @(@('rustc', '--version'), @('cargo', '--version'), @('node', '--version'), @('npm', '--version'), @($py, '--version'), @('docker', '--version'))) {
    if (Test-Tool $t[0]) { [void]$lines.Add(("{0} {1}: {2}" -f $t[0], $t[1], ((& $t[0] $t[1] 2>&1 | Select-Object -First 1) -join ' '))) }
    else { [void]$lines.Add(("{0}: not installed" -f $t[0])) }
}
[void]$lines.Add('')
[void]$lines.Add('RESULTS')
foreach ($r in $Results) { [void]$lines.Add(("  {0,-44} {1}" -f $r.Name, $r.Result)) }
[void]$lines.Add('')
foreach ($r in $Results) {
    if ($r.Result -like 'FAIL*' -and $r.Log -and (Test-Path $r.Log)) {
        [void]$lines.Add("===== $($r.Name): last 60 lines =====")
        foreach ($l in (Get-Content $r.Log -Tail 60)) { [void]$lines.Add($l) }
        [void]$lines.Add('')
    }
    if ($r.Name -eq 'engine benchmark (small)' -and $r.Result -eq 'PASS') {
        [void]$lines.Add('===== benchmark output =====')
        foreach ($l in (Get-Content $r.Log)) { [void]$lines.Add($l) }
    }
}
$lines | Set-Content -Path $Report -Encoding UTF8
Write-Host ''
Write-Host "Report written to: $Report"
foreach ($r in $Results) { Write-Host ("  {0,-44} {1}" -f $r.Name, $r.Result) }
if ($Results | Where-Object { $_.Result -like 'FAIL*' }) { Write-Host 'RESULT: at least one step FAILED'; exit 1 }
if ($Results | Where-Object { $_.Result -like 'SKIPPED (no *' }) { Write-Host 'RESULT: no failures, but some tools are missing: SKIPPED is NOT a pass'; exit 3 }
Write-Host 'RESULT: all executed steps passed'
exit 0
