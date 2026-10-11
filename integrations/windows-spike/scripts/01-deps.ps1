# Stage 1: HDF5 (official HDF Group Windows build), Embree 4 (official zip, ships TBB).
# Eigen3 is not fetched: MOAB 5.6.0 needs it only with ENABLE_EIGEN3=ON and DAGMC only for dagtally,
# both off here.
. "$PSScriptRoot\common.ps1"
$log = "$Logs\deps.log"
$sw = [System.Diagnostics.Stopwatch]::StartNew()

$tmp = Join-Path $env:RUNNER_TEMP 'spike-dl'
New-Item -ItemType Directory -Force $tmp | Out-Null

Invoke-Logged $log curl.exe @('-L', '--fail', '--retry', '3', '-o', "$tmp\hdf5.zip", $Hdf5Url)
Expand-Archive "$tmp\hdf5.zip" -DestinationPath "$tmp\hdf5" -Force
$inner = Get-ChildItem "$tmp\hdf5" -Recurse -Filter 'HDF5-*-win64.zip' | Select-Object -First 1
New-Item -ItemType Directory -Force "$Inst\hdf5" | Out-Null
Expand-Archive $inner.FullName -DestinationPath "$Inst\hdf5" -Force
if (-not (Test-Path "$Hdf5Root\cmake\hdf5-config.cmake")) { throw "HDF5 package layout not as expected under $Hdf5Root" }
Get-ChildItem "$Hdf5Root\lib" -Filter '*hdf5*.lib' | ForEach-Object { Add-Content $log "hdf5 lib: $($_.Name)" }
# keep the install small for the cache
Remove-Item -Recurse -Force "$Hdf5Root\html", "$Hdf5Root\HDF5Examples" -ErrorAction SilentlyContinue

Invoke-Logged $log curl.exe @('-L', '--fail', '--retry', '3', '-o', "$tmp\embree.zip", $EmbreeUrl)
New-Item -ItemType Directory -Force $EmbreeRoot | Out-Null
Expand-Archive "$tmp\embree.zip" -DestinationPath $EmbreeRoot -Force
Remove-Item -Recurse -Force "$EmbreeRoot\doc", "$EmbreeRoot\src", "$EmbreeRoot\bin\models" -ErrorAction SilentlyContinue
Remove-Item -Force "$EmbreeRoot\bin\*.exe" -ErrorAction SilentlyContinue
if (-not (Test-Path "$EmbreeRoot\lib\cmake\embree-$EmbreeVersion\embree-config.cmake")) { throw 'Embree layout not as expected' }
Add-Timing 'deps' $sw.Elapsed.TotalSeconds
