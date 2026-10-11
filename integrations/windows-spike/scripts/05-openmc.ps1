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

# Assemble the folder that stands in for the FARIS bundle: executable, DLL and every DLL they load.
$out = "$Dist\$name"
if (Test-Path $out) { Remove-Item -Recurse -Force $out }
New-Item -ItemType Directory -Force $out | Out-Null
Copy-Item "$prefix\bin\openmc.exe", "$prefix\bin\libopenmc.dll" $out
$dirs = @("$Hdf5Root\bin", $EmbreeRoot + '\bin')
if ($Dagmc) { $dirs += @("$Inst\dagmc\bin", "$Inst\moab\bin", "$Inst\dd\bin") }
foreach ($d in $dirs) {
    Get-ChildItem $d -Filter *.dll -ErrorAction SilentlyContinue | ForEach-Object {
        Copy-Item $_.FullName $out -Force
        Add-Content "$Logs\$name-bundle.log" "bundled $($_.Name) from $d"
    }
}
# LLVM OpenMP runtime (libomp140.x86_64.dll) and the C++ runtime come from the Visual Studio redist.
$vs = & "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe" -latest -products * -property installationPath
Get-ChildItem "$vs\VC\Redist\MSVC" -Recurse -Include 'libomp140.x86_64.dll', 'msvcp140.dll', 'vcruntime140.dll', 'vcruntime140_1.dll', 'concrt140.dll' -ErrorAction SilentlyContinue |
    Where-Object { $_.FullName -notmatch 'debug_nonredist|onecore|arm|x86\\' } |
    ForEach-Object { Copy-Item $_.FullName $out -Force; Add-Content "$Logs\$name-bundle.log" "bundled $($_.Name) from $($_.DirectoryName)" }
Get-ChildItem $out | Format-Table Name, Length | Out-String | Tee-Object -FilePath "$Logs\$name-bundle.log" -Append
