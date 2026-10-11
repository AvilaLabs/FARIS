# Stage 4: DAGMC 3.2.4, shared library with double-down. No Geant4, FLUKA, PyNE, UWUW, tally,
# tools or tests.
. "$PSScriptRoot\common.ps1"
Get-Source DAGMC 'https://github.com/svalinn/DAGMC.git' 'v3.2.4'
Set-Patches DAGMC 'dagmc'
Set-CommonFlags
$cm = (Get-CmakeCommon "$Inst\dagmc") + @(
    "-DMOAB_DIR=$(Fwd "$Inst\moab")",
    "-DDOUBLE_DOWN=ON", "-DDOUBLE_DOWN_DIR=$(Fwd "$Inst\dd")",
    '-DBUILD_SHARED_LIBS=ON', '-DBUILD_STATIC_LIBS=OFF', '-DBUILD_EXE=OFF', '-DBUILD_TESTS=OFF',
    '-DBUILD_UWUW=OFF', '-DBUILD_TALLY=OFF', '-DBUILD_BUILD_OBB=OFF', '-DBUILD_MAKE_WATERTIGHT=OFF',
    '-DBUILD_OVERLAP_CHECK=OFF', '-DBUILD_GEANT4=OFF', '-DBUILD_FLUKA=OFF', '-DGIT_SUBMODULE=OFF'
)
Build-Cmake DAGMC $cm
foreach ($f in 'bin\dagmc.dll', 'lib\dagmc.lib', 'lib\cmake\dagmc\DAGMCConfig.cmake') {
    if (-not (Test-Path "$Inst\dagmc\$f")) { throw "DAGMC install is missing $f" }
}
