# Runs only after a failed smoke test: repeats the run under cdb (one thread) and prints the
# exception, registers, the frames above it and the disassembly around the faulting instruction.
param([string]$Bundle, [string]$Model, [string]$Tag)
. "$PSScriptRoot\common.ps1"
$ErrorActionPreference = 'Continue'
$bundleDir = "$Dist\$Bundle"
$run = "$Spike\runs\$Tag"
$log = "$Logs\diagnose-$Tag.log"
$env:PATH = "$bundleDir;$env:SystemRoot\System32;$env:SystemRoot;$env:PATH"
$env:OPENMC_CROSS_SECTIONS = "$run\cross_sections.xml"
$env:OMP_NUM_THREADS = '1'

$cdb = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits" -Recurse -Filter cdb.exe -ErrorAction SilentlyContinue | Where-Object { $_.FullName -match 'x64' } | Select-Object -First 1
Add-Content $log "cdb: $($cdb.FullName)"
if (-not $cdb) { return }
Push-Location $run
$cmds = 'g;.lines;.ecxr;r;kv 8;.frame 1;dv /V;.frame 2;dv /V;.frame 3;dv /V;.frame 0;u @rip-0x20 L14;!address @rcx;lm;q'
& $cdb.FullName -g -lines -y "$bundleDir;srv*C:\sym*https://msdl.microsoft.com/download/symbols" -c $cmds "$bundleDir\openmc.exe" *>> $log
Pop-Location
Get-Content $log | Where-Object { $_ -notmatch '^\s*[%#]+\s*$' } | Select-Object -Last 200
