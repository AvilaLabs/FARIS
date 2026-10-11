# Stage 6c: build the Rust probe against libopenmc.lib and run a model through the C API, with the
# same restricted PATH as the smoke tests.
param([string]$Bundle, [string]$Model, [string]$Tag, [string]$LibBundle = '')
. "$PSScriptRoot\common.ps1"
$bundleDir = "$Dist\$Bundle"
$log = "$Logs\capi-$Tag.log"
$probe = "$SpikeDir\capi-probe"

$env:RUSTUP_TOOLCHAIN = 'stable'   # the repository's rust-toolchain.toml is not wanted here
if (-not $LibBundle) { $LibBundle = $Bundle }
$env:OPENMC_LIB_DIR = "$Inst\$LibBundle\lib"
$env:CARGO_TARGET_DIR = "$Bld\capi-probe"
$sw = [System.Diagnostics.Stopwatch]::StartNew()
Invoke-Logged $log cargo @('--version')
Invoke-Logged $log cargo @('build', '--release', '--manifest-path', "$probe\Cargo.toml")
Add-Timing 'capi-probe-build' $sw.Elapsed.TotalSeconds
Copy-Item "$Bld\capi-probe\release\capi-probe.exe" $bundleDir

$run = "$Spike\runs\capi-$Tag"
if (Test-Path $run) { Remove-Item -Recurse -Force $run }
New-Item -ItemType Directory -Force $run | Out-Null
Copy-Item "$SpikeDir\models\$Model\*.xml" $run
Copy-Item "$SpikeDir\data\cross_sections.xml", "$SpikeDir\data\H1.h5" $run
if ($Model -eq 'dagmc') { Copy-Item (Join-Path $SpikeDir '..\cad\fixtures\three-boxes.h5m') $run }

$oldPath = $env:PATH
$env:PATH = "$bundleDir;$env:SystemRoot\System32;$env:SystemRoot"
$env:OPENMC_CROSS_SECTIONS = "$run\cross_sections.xml"
$env:OMP_NUM_THREADS = '4'
try {
    Invoke-Logged $log "$bundleDir\capi-probe.exe" @($run, "$run\summary-capi.json")
} finally {
    $env:PATH = $oldPath
}
# same run through the statepoint file, for a bin-by-bin check that the C API returns the same numbers
$sp = Get-ChildItem $run -Filter 'statepoint.*.h5' | Select-Object -First 1
python "$SpikeDir\scripts\summarize_statepoint.py" $sp.FullName "$run\summary.json"
python "$SpikeDir\scripts\compare_results.py" "$run\summary-capi.json" "$run\summary.json" --label-a capi --label-b statepoint --out "$run\capi-vs-statepoint.md"
