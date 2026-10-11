# Stage 4: DAGMC 3.2.4, shared library. No Geant4, FLUKA, PyNE, UWUW, tally, tools or tests.
# Default: with double-down (Embree). -NoDoubleDown builds the stock MOAB ray tracer variant into
# its own prefix; it has the same exports, so its dagmc.dll can replace the other one for A/B runs.
param([switch]$NoDoubleDown)
. "$PSScriptRoot\common.ps1"
Get-Source DAGMC 'https://github.com/svalinn/DAGMC.git' 'v3.2.4'
Set-Patches DAGMC 'dagmc'
Set-CommonFlags
$tag = if ($NoDoubleDown) { 'dagmc-nodd' } else { 'dagmc' }
$cm = (Get-CmakeCommon "$Inst\$tag") + @(
    "-DMOAB_DIR=$(Fwd "$Inst\moab")",
    '-DBUILD_SHARED_LIBS=ON', '-DBUILD_STATIC_LIBS=OFF', '-DBUILD_EXE=OFF', '-DBUILD_TESTS=OFF',
    '-DBUILD_UWUW=OFF', '-DBUILD_TALLY=OFF', '-DBUILD_BUILD_OBB=OFF', '-DBUILD_MAKE_WATERTIGHT=OFF',
    '-DBUILD_OVERLAP_CHECK=OFF', '-DBUILD_GEANT4=OFF', '-DBUILD_FLUKA=OFF', '-DGIT_SUBMODULE=OFF'
)
if (-not $NoDoubleDown) {
    $cm += @('-DDOUBLE_DOWN=ON', "-DDOUBLE_DOWN_DIR=$(Fwd "$Inst\dd")")
}
Build-Cmake DAGMC $cm -Tag $tag
foreach ($f in 'bin\dagmc.dll', 'lib\dagmc.lib', 'lib\cmake\dagmc\DAGMCConfig.cmake') {
    if (-not (Test-Path "$Inst\$tag\$f")) { throw "DAGMC install is missing $f" }
}
