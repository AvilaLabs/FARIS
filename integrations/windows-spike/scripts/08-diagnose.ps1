# Runs only after a failed smoke test: repeats the run with one thread and under cdb to get a stack.
param([string]$Bundle, [string]$Model, [string]$Tag)
. "$PSScriptRoot\common.ps1"
$ErrorActionPreference = 'Continue'
$bundleDir = "$Dist\$Bundle"
$run = "$Spike\runs\$Tag"
$log = "$Logs\diagnose-$Tag.log"
$env:PATH = "$bundleDir;$env:SystemRoot\System32;$env:SystemRoot;$env:PATH"
$env:OPENMC_CROSS_SECTIONS = "$run\cross_sections.xml"

$cdb = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits" -Recurse -Filter cdb.exe -ErrorAction SilentlyContinue | Where-Object { $_.FullName -match 'x64' } | Select-Object -First 1
Add-Content $log "cdb: $($cdb.FullName)"

foreach ($threads in 1, 4) {
    $env:OMP_NUM_THREADS = "$threads"
    Push-Location $run
    Add-Content $log "=== plain run, $threads thread(s)"
    & "$bundleDir\openmc.exe" *>> $log
    Add-Content $log "exit code $LASTEXITCODE"
    if ($cdb) {
        Add-Content $log "=== cdb run, $threads thread(s)"
        & $cdb.FullName -g -lines -y "$bundleDir;srv*C:\sym*https://msdl.microsoft.com/download/symbols" -c "g;.lines;~*kb 25;q" "$bundleDir\openmc.exe" *>> $log
    }
    Pop-Location
}
Get-Content $log | Select-Object -Last 150
