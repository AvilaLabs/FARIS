# Stage 5: OpenMC 0.15.3 with the Windows patches. -Dagmc builds against the DAGMC from stage 4.
param([switch]$Dagmc)
. "$PSScriptRoot\common.ps1"
$name = if ($Dagmc) { 'openmc-dagmc' } else { 'openmc-csg' }
Get-Source $name 'https://github.com/openmc-dev/openmc.git' 'v0.15.3' -Submodules
Set-Patches $name 'openmc'
Set-CommonFlags
$prefix = "$Inst\$name"
$cm = (Get-CmakeCommon $prefix) + @(
    '-DOPENMC_USE_OPENMP=ON', '-DOpenMP_RUNTIME_MSVC=llvm',
    '-DOPENMC_USE_MPI=OFF', '-DOPENMC_USE_MCPL=OFF', '-DOPENMC_USE_NCRYSTAL=OFF',
    '-DOPENMC_USE_LIBMESH=OFF', '-DOPENMC_BUILD_TESTS=OFF'
)
if ($Dagmc) {
    $cm += @(
        '-DOPENMC_USE_DAGMC=ON',
        "-DDAGMC_ROOT=$(Fwd "$Inst\dagmc")",
        "-DCMAKE_PREFIX_PATH=$(Fwd "$Inst\dagmc");$(Fwd "$Inst\moab");$(Fwd "$Inst\dd");$(Fwd $EmbreeRoot)",
        "-DMOAB_DIR=$(Fwd "$Inst\moab")",
        "-DDOUBLE_DOWN_DIR=$(Fwd "$Inst\dd")"
    )
}
Build-Cmake $name $cm
foreach ($f in 'bin\openmc.exe', 'bin\libopenmc.dll') {
    if (-not (Test-Path "$prefix\$f")) { throw "OpenMC install is missing $f" }
}

# Assemble the folder that stands in for the FARIS bundle: executable, DLL and every DLL they
# load, found by walking `dumpbin /dependents`. Windows system libraries (kernel32, the UCRT API
# sets) are left to the OS; the Visual C++ runtime and the LLVM OpenMP runtime are bundled.
$out = "$Dist\$name"
if (Test-Path $out) { Remove-Item -Recurse -Force $out }
New-Item -ItemType Directory -Force $out | Out-Null
Copy-Item "$prefix\bin\openmc.exe", "$prefix\bin\libopenmc.dll" $out
$bl = "$Logs\$name-bundle.log"
$searchDirs = @("$Hdf5Root\bin", "$EmbreeRoot\bin")
if ($Dagmc) { $searchDirs += @("$Inst\dagmc\bin", "$Inst\moab\bin", "$Inst\dd\bin") }
$osOnly = '^(api-ms-win-|ext-ms-win-|kernel32|user32|advapi32|shell32|ole32|oleaut32|shlwapi|ws2_32|ntdll|ucrtbase|bcrypt|crypt32|gdi32|rpcrt4|sechost|version|msvcrt|combase|setupapi|cfgmgr32|imm32|win32u|gdi32full|msvcp_win)'
$queue = [System.Collections.Generic.Queue[string]]::new()
$queue.Enqueue("$out\openmc.exe"); $queue.Enqueue("$out\libopenmc.dll")
while ($queue.Count -gt 0) {
    $f = $queue.Dequeue()
    $deps = & dumpbin /dependents $f | ForEach-Object { if ($_ -match '^\s+(\S+\.dll)\s*$') { $Matches[1] } }
    foreach ($d in $deps) {
        if (Test-Path "$out\$d") { continue }
        if ($d -match $osOnly) { Add-Content $bl "os    $d (needed by $(Split-Path $f -Leaf))"; continue }
        $src = $null
        foreach ($sd in $searchDirs) { if (Test-Path "$sd\$d") { $src = "$sd\$d"; break } }
        if (-not $src) {
            $found = Get-ChildItem "$env:SystemRoot\System32" -Filter $d -ErrorAction SilentlyContinue | Select-Object -First 1
            if ($found) { $src = $found.FullName }
        }
        if (-not $src) { Add-Content $bl "MISSING $d (needed by $(Split-Path $f -Leaf))"; Write-Warning "no source for $d"; continue }
        Copy-Item $src $out
        Add-Content $bl "bundle $d from $src (needed by $(Split-Path $f -Leaf))"
        $queue.Enqueue("$out\$d")
    }
}
# debug symbols next to the binaries, for crash analysis
Get-ChildItem $Bld -Recurse -Filter *.pdb -ErrorAction SilentlyContinue | Where-Object { $_.FullName -match "\\(openmc|DAGMC|double-down)[^\\]*\\" } | ForEach-Object { Copy-Item $_.FullName $out -Force -ErrorAction SilentlyContinue }
Get-ChildItem $out | Format-Table Name, Length | Out-String | Tee-Object -FilePath $bl -Append
