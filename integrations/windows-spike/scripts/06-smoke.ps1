# Stage 6: run one model with one bundle. The PATH during the run holds only the bundle folder
# and the Windows system directories, so a missing DLL fails the run instead of being found
# somewhere in the runner image.
param([string]$Bundle, [string]$Model, [string]$Tag)
. "$PSScriptRoot\common.ps1"
$bundleDir = "$Dist\$Bundle"
$run = "$Spike\runs\$Tag"
if (Test-Path $run) { Remove-Item -Recurse -Force $run }
New-Item -ItemType Directory -Force $run | Out-Null
Copy-Item "$SpikeDir\models\$Model\*.xml" $run
Copy-Item "$SpikeDir\data\cross_sections.xml", "$SpikeDir\data\H1.h5" $run
if ($Model -eq 'dagmc') { Copy-Item (Join-Path $SpikeDir '..\cad\fixtures\three-boxes.h5m') $run }
$log = "$Logs\run-$Tag.log"

$oldPath = $env:PATH
$env:PATH = "$bundleDir;$env:SystemRoot\System32;$env:SystemRoot"
$env:OPENMC_CROSS_SECTIONS = "$run\cross_sections.xml"
$env:OMP_NUM_THREADS = '4'
try {
    Push-Location $run
    Invoke-Logged $log "$bundleDir\openmc.exe" @('--version')
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    Invoke-Logged $log "$bundleDir\openmc.exe" @()
    Add-Timing "run-$Tag" $sw.Elapsed.TotalSeconds
} finally {
    Pop-Location
    $env:PATH = $oldPath
}
$sp = Get-ChildItem $run -Filter 'statepoint.*.h5' | Select-Object -First 1
if (-not $sp) { throw 'no statepoint written' }
python "$SpikeDir\scripts\summarize_statepoint.py" $sp.FullName "$run\summary.json"
if ($LASTEXITCODE -ne 0) { throw 'summarize_statepoint.py failed' }

# DLL dependencies of the library, for the bundling record
& dumpbin /dependents "$bundleDir\libopenmc.dll" 2>&1 | Out-File "$Logs\dependents-$Bundle.txt"
